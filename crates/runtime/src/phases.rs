use std::{collections::BTreeSet, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use futures_util::FutureExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result, digest,
    plugin::{CompositionLease, PluginInfo, normalize_contribution},
    prompt::PromptPlan,
};

pub use rustytavern_plugin::prompt::*;

#[derive(Clone)]
pub(crate) struct HookBinding {
    pub service: String,
    pub info: PluginInfo,
    pub policy: HookPolicy,
    pub hook: Arc<dyn PromptHook>,
}
impl HookBinding {
    fn authorize(&self, capability: &str) -> Result<()> {
        if self.info.grants.contains(capability) {
            Ok(())
        } else {
            Err(Error::new(
                "capability_denied",
                "The prompt hook has no grant for this operation.",
            ))
        }
    }
    pub fn record(&self, result: Result<EnrichmentOutput>) -> EnrichmentRecord {
        let (output, error) = match result {
            Ok(output) => (Some(output), None),
            Err(error) => (None, Some(error)),
        };
        EnrichmentRecord {
            service: self.service.clone(),
            plugin_id: self.info.manifest.id.clone(),
            policy: self.policy.clone(),
            output,
            error,
        }
    }
    pub fn offline_record(&self) -> Result<EnrichmentRecord> {
        self.authorize("session.read:current")?;
        if self
            .info
            .manifest
            .requested_capabilities
            .contains("prompt.enrich")
        {
            self.authorize("prompt.enrich")?;
            return Err(Error::new(
                "enrichment_required",
                "This prompt requires saved enrichment. The replay command uses a saved generation.",
            ));
        }
        Ok(self.record(Ok(EnrichmentOutput::default())))
    }
    pub async fn enrich(
        &self,
        snapshot: &Value,
        cancellation: CancellationToken,
        lease: Option<Arc<CompositionLease>>,
    ) -> Result<EnrichmentOutput> {
        self.authorize("session.read:current")?;
        let mut output = if self
            .info
            .manifest
            .requested_capabilities
            .contains("prompt.enrich")
        {
            self.authorize("prompt.enrich")?;
            let cancellation = cancellation.child_token();
            let _guard = cancellation.clone().drop_guard();
            let scoped = conversation_snapshot(snapshot);
            let state = snapshot["plugin_state"][&self.info.manifest.id].clone();
            let hook = self.hook.clone();
            let worker_token = cancellation.clone();
            let runtime = tokio::runtime::Handle::current();
            let deadline = Duration::from_millis(self.policy.timeout_ms);
            let started = tokio::time::Instant::now();
            let work = tokio::task::spawn_blocking(move || {
                let _lease = lease;
                let input = HookInput {
                    snapshot: &scoped,
                    state: &state,
                };
                runtime.block_on(async {
                    tokio::select! { biased;
                        _ = worker_token.cancelled() => Err(Error::new("cancelled", "The caller cancelled prompt enrichment.")),
                        result = AssertUnwindSafe(hook.enrich(&input, worker_token.clone())).catch_unwind() => result
                            .map_err(|_| Error::new("prompt_hook_failed", "The prompt hook panics."))?,
                    }
                })
            });
            let result = tokio::select! { biased;
                _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled prompt enrichment.")),
                result = tokio::time::timeout(deadline, work) => result
                    .map_err(|_| Error::new("prompt_hook_timeout", "The prompt hook exceeds its deadline."))?
                    .map_err(Error::context("prompt_hook_failed", "The prompt hook worker fails."))?,
            };
            if started.elapsed() > deadline {
                return Err(Error::new(
                    "prompt_hook_timeout",
                    "The prompt hook exceeds its deadline.",
                ));
            }
            result.map_err(|error| {
                Error::new(
                    "prompt_hook_failed",
                    "The prompt hook fails during enrichment.",
                )
                .with_source(error)
            })?
        } else {
            EnrichmentOutput::default()
        };
        if serde_json::to_vec(&output)?.len() > 1024 * 1024 {
            return Err(Error::new(
                "invalid_enrichment_size",
                "The enrichment result exceeds its byte limit.",
            ));
        }
        output.contributions = output
            .contributions
            .into_iter()
            .map(|c| normalize_contribution(&self.info.manifest, &self.info.grants, c))
            .collect::<Result<_>>()?;
        Ok(output)
    }
}

pub(crate) fn bindings(composition: Option<&CompositionLease>) -> Result<Vec<HookBinding>> {
    let Some(composition) = composition else {
        return Ok(Vec::new());
    };
    let mut hooks = Vec::new();
    for service in composition.service_names("prompt.hook.") {
        if !service.ends_with("/1") {
            return Err(Error::new(
                "incompatible_plugin_protocol",
                "The prompt hook requires another service protocol.",
            ));
        }
        let info = composition.service_owner(&service)?.clone();
        let hook = composition
            .service::<Arc<dyn PromptHook>>(&service)?
            .as_ref()
            .clone();
        let policy = std::panic::catch_unwind(AssertUnwindSafe(|| hook.policy()))
            .map_err(|_| Error::new("prompt_hook_failed", "The prompt hook policy panics."))?;
        if policy.timeout_ms == 0 || policy.timeout_ms > 120_000 {
            return Err(Error::new(
                "invalid_prompt_hook",
                "The prompt hook deadline is invalid.",
            ));
        }
        hooks.push(HookBinding {
            service,
            info,
            policy,
            hook,
        });
    }
    hooks.sort_by(|a, b| {
        (a.policy.priority, &a.info.manifest.id, &a.service).cmp(&(
            b.policy.priority,
            &b.info.manifest.id,
            &b.service,
        ))
    });
    Ok(hooks)
}

pub(crate) fn apply(
    hooks: &[HookBinding],
    snapshot: &Value,
    saved: &SavedEnrichment,
    composition: Option<&CompositionLease>,
    identity: &str,
    plan: &mut PromptPlan,
) -> Result<PhaseTrace> {
    saved.validate(snapshot, identity)?;
    if hooks.len() != saved.records.len() {
        return Err(Error::new(
            "invalid_enrichment_snapshot",
            "The saved hook set differs from the current hook set.",
        ));
    }
    let mut trace = PhaseTrace {
        base_profile: plan.profile.clone(),
        composition_id: identity.into(),
        enrichment_hash: digest(&serde_json::to_vec(saved)?),
        contributions: composition.map_or_else(Vec::new, |c| c.contributions.clone()),
        edits: Vec::new(),
    };
    for (hook, record) in hooks.iter().zip(&saved.records) {
        if hook.service != record.service
            || hook.info.manifest.id != record.plugin_id
            || hook.policy != record.policy
        {
            return Err(Error::new(
                "invalid_enrichment_snapshot",
                "The saved hook identity or policy differs.",
            ));
        }
        if let Some(error) = &record.error {
            if hook.policy.required {
                return Err(error.clone());
            }
            plan.warnings
                .push(format!("optional_hook:{}:{}", record.service, error.code));
        } else if let Some(output) = &record.output {
            for contribution in &output.contributions {
                trace.contributions.push(normalize_contribution(
                    &hook.info.manifest,
                    &hook.info.grants,
                    contribution.clone(),
                )?);
            }
        } else {
            return Err(Error::new(
                "invalid_enrichment_snapshot",
                "The saved hook record has no result.",
            ));
        }
    }
    crate::plugin::apply_contributions(plan, &trace.contributions)?;
    let mut replacements = BTreeSet::new();
    for (hook, record) in hooks
        .iter()
        .zip(&saved.records)
        .filter(|(_, r)| r.error.is_none())
    {
        let Some(output) = &record.output else {
            continue;
        };
        let mut candidate = plan.clone();
        let mut claimed = replacements.clone();
        let result = (|| {
            hook.authorize("session.read:current")?;
            let scoped = conversation_snapshot(snapshot);
            let input = TransformInput {
                input: HookInput {
                    snapshot: &scoped,
                    state: &snapshot["plugin_state"][&hook.info.manifest.id],
                },
                enrichment: &output.data,
                plan,
            };
            let edits = std::panic::catch_unwind(AssertUnwindSafe(|| hook.hook.transform(&input)))
                .map_err(|_| {
                    Error::new(
                        "prompt_hook_failed",
                        "The prompt hook panics during a transform.",
                    )
                })?
                .map_err(|error| {
                    Error::new(
                        "prompt_hook_failed",
                        "The prompt hook fails during a transform.",
                    )
                    .with_source(error)
                })?;
            if edits.len() > 128 {
                return Err(Error::new(
                    "invalid_prompt_edit",
                    "The prompt hook returns too many edits.",
                ));
            }
            let mut applied = Vec::new();
            for edit in edits {
                hook.authorize(&format!("prompt.transform:{}", edit.section_id))?;
                let section = candidate
                    .sections
                    .iter_mut()
                    .find(|s| s.id == edit.section_id)
                    .ok_or_else(|| {
                        Error::new(
                            "invalid_prompt_edit",
                            "The transform target does not exist.",
                        )
                    })?;
                if matches!(section.source.as_str(), "history" | "current_user") {
                    return Err(Error::new(
                        "invalid_prompt_edit",
                        "The transform cannot change transcript sections.",
                    ));
                }
                if edit.mode == EditMode::Replace && !claimed.insert(edit.section_id.clone()) {
                    return Err(Error::new(
                        "conflicting_prompt_edit",
                        "Two edits replace the same prompt section.",
                    ));
                }
                let before_hash = digest(section.text.as_bytes());
                section.text = match edit.mode {
                    EditMode::Prepend => format!("{}{}", edit.text, section.text),
                    EditMode::Append => format!("{}{}", section.text, edit.text),
                    EditMode::Replace => edit.text.clone(),
                };
                if section.text.len() > 1024 * 1024 {
                    return Err(Error::new(
                        "invalid_prompt_edit",
                        "The edited section exceeds its byte limit.",
                    ));
                }
                section.units = section.text.len().saturating_add(4);
                applied.push(AppliedEdit {
                    plugin_id: record.plugin_id.clone(),
                    service: hook.service.clone(),
                    edit,
                    before_hash,
                    after_hash: digest(section.text.as_bytes()),
                });
            }
            candidate.refresh()?;
            Ok(applied)
        })();
        match result {
            Ok(edits) => {
                *plan = candidate;
                replacements = claimed;
                trace.edits.extend(edits);
            }
            Err(error) if !hook.policy.required && error.code != "conflicting_prompt_edit" => plan
                .warnings
                .push(format!("optional_hook:{}:{}", hook.service, error.code)),
            Err(error) => return Err(error),
        }
    }
    if !trace.contributions.is_empty() || !hooks.is_empty() {
        let hash = digest(&serde_json::to_vec(&trace)?);
        plan.profile = format!("{}+{}", trace.base_profile, &hash[..12]);
    }
    plan.refresh()?;
    Ok(trace)
}

pub(crate) fn conversation_snapshot(snapshot: &Value) -> Value {
    let mut scoped = snapshot.clone();
    let Some(fields) = scoped.as_object_mut() else {
        return Value::Null;
    };
    for key in ["plugin_state", "provider_config", "plugin_composition"] {
        fields.remove(key);
    }
    if let Some(session) = fields.get_mut("session").and_then(Value::as_object_mut) {
        session.remove("composition");
    }
    scoped
}
