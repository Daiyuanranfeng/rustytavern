use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    card::Card,
    phases::{
        EditMode, EnrichmentOutput, HookInput, HookPolicy, PromptHook, SectionEdit, TransformInput,
    },
    plugin::{
        Contribution, PluginContext, PluginManifest, PluginRegistry, Retention, RuntimePlugin,
    },
    prompt::PromptOptions,
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderStream,
    },
    runtime::Runtime,
    session::{Generation, GenerationStatus, Persona, Role, Session},
    store::{SqliteStore, StoreBackend, now_ms},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

struct LocalProvider(Arc<Mutex<Vec<Value>>>);
#[async_trait]
impl ProviderAdapter for LocalProvider {
    fn prepare(
        &self,
        plan: &rustytavern::prompt::PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        HttpProvider.prepare(plan, model, config)
    }
    async fn open_stream(
        &self,
        request: PreparedRequest,
        _: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        self.0.lock().unwrap().push(request.body);
        Ok(Box::pin(futures_util::stream::iter([
            Ok(ProviderEvent::Text {
                text: "A reply.".into(),
            }),
            Ok(ProviderEvent::Complete),
        ])))
    }
}
#[derive(Default)]
struct TestHook {
    policy: HookPolicy,
    output: EnrichmentOutput,
    edits: Vec<SectionEdit>,
    fail_enrichment: bool,
    fail_transform: bool,
    pause: bool,
    changing: bool,
    calls: AtomicUsize,
    transforms: AtomicUsize,
    seen: Mutex<Vec<Value>>,
    store: Option<Arc<SqliteStore>>,
    namespace: String,
    blocking: Option<Mutex<std::sync::mpsc::Receiver<()>>>,
}
#[async_trait]
impl PromptHook for TestHook {
    fn policy(&self) -> HookPolicy {
        self.policy.clone()
    }
    async fn enrich(
        &self,
        input: &HookInput<'_>,
        _: CancellationToken,
    ) -> Result<EnrichmentOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(input.snapshot["plugin_state"].is_null());
        assert!(input.snapshot["provider_config"].is_null());
        assert!(input.snapshot["session"]["composition"].is_null());
        self.seen.lock().unwrap().push(input.state.clone());
        if let Some(store) = &self.store {
            let session = input.snapshot["session"]["id"].as_str().unwrap();
            let saved = store
                .export_session(session, false)?
                .generations
                .last()
                .unwrap()
                .enrichment
                .clone();
            self.seen.lock().unwrap().push(serde_json::to_value(saved)?);
            if !self.namespace.is_empty() {
                store.state_write(&self.namespace, "memo", &json!("changed-after-snapshot"))?;
            }
        }
        if let Some(gate) = &self.blocking {
            let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(3));
        }
        if self.pause {
            futures_util::future::pending::<()>().await;
        }
        if self.fail_enrichment {
            return Err(Error::new("fixture_private", "PRIVATE_ERROR_SENTINEL"));
        }
        Ok(self.output.clone())
    }
    fn transform(&self, input: &TransformInput<'_>) -> Result<Vec<SectionEdit>> {
        let round = self.transforms.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push(json!({"state":input.input.state,"enrichment":input.enrichment}));
        if self.fail_transform {
            return Err(Error::new("fixture_private", "PRIVATE_ERROR_SENTINEL"));
        }
        let mut edits = self.edits.clone();
        if self.changing {
            for edit in &mut edits {
                edit.text.push_str(&round.to_string());
            }
        }
        Ok(edits)
    }
}
struct HookPlugin {
    manifest: PluginManifest,
    hook: Arc<TestHook>,
}
#[async_trait]
impl RuntimePlugin for HookPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let hook: Arc<dyn PromptHook> = self.hook.clone();
        context.register(&self.manifest.provides[0], Arc::new(hook))
    }
}
struct Fixture {
    runtime: Runtime,
    store: Arc<SqliteStore>,
    registry: Arc<tokio::sync::Mutex<PluginRegistry>>,
    session: String,
    requests: Arc<Mutex<Vec<Value>>>,
}
impl Fixture {
    fn new() -> Self {
        let store = Arc::new(SqliteStore::memory().unwrap());
        let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
        let persona = Persona::new("Alex".into(), "Visitor.".into()).unwrap();
        store.save_card(&card).unwrap();
        store.save_persona(&persona).unwrap();
        let session = store
            .create_session(
                &Session {
                    id: String::new(),
                    character_id: card.id,
                    persona_id: persona.id,
                    worldbook_ids: Vec::new(),
                    provider: "local".into(),
                    model: "fixture".into(),
                    head: None,
                    version: 0,
                    accept_reduced: false,
                    options: PromptOptions::default(),
                    composition: json!({}),
                },
                None,
            )
            .unwrap()
            .id;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
        let runtime = Runtime::new(store.clone())
            .with_provider(Arc::new(LocalProvider(requests.clone())))
            .with_plugins(registry.clone());
        Self {
            runtime,
            store,
            registry,
            session,
            requests,
        }
    }
    async fn add(&self, id: &str, hook: Arc<TestHook>, capabilities: &[&str], grant: bool) {
        let requested = capabilities
            .iter()
            .map(|c| c.to_string())
            .collect::<BTreeSet<_>>();
        let plugin = HookPlugin {
            manifest: PluginManifest {
                id: id.into(),
                provides: vec![format!("prompt.hook.{id}/1")],
                requested_capabilities: requested.clone(),
                allowed_slots: ["before_character".into()].into(),
                artifact_hash: "fixture-artifact".into(),
                configuration_hash: "fixture-configuration".into(),
                ..Default::default()
            },
            hook,
        };
        self.registry
            .lock()
            .await
            .inject(
                vec![Arc::new(plugin)],
                id,
                &BTreeMap::from([(id.into(), if grant { requested } else { BTreeSet::new() })]),
            )
            .await
            .unwrap();
        self.runtime
            .configure_session(
                &self.store.session(&self.session).unwrap(),
                ProviderConfig::default(),
            )
            .await
            .unwrap();
    }
    async fn run(&self, config: ProviderConfig) -> Generation {
        self.runtime
            .configure_session(&self.store.session(&self.session).unwrap(), config.clone())
            .await
            .unwrap();
        self.runtime
            .start(&self.session, "Hello.", None, config)
            .await
            .unwrap()
            .wait()
            .await
            .unwrap()
    }
}
fn edit(target: &str, mode: EditMode, text: &str) -> SectionEdit {
    SectionEdit {
        section_id: target.into(),
        mode,
        text: text.into(),
    }
}
fn contribution(text: &str, retention: Retention) -> Contribution {
    Contribution {
        plugin_id: String::new(),
        section_id: "memory".into(),
        slot: "before_character".into(),
        priority: 0,
        role: Role::System,
        text: text.into(),
        provenance: json!({"source":"fixture-memory"}),
        retention,
    }
}

#[tokio::test]
async fn replay_uses_saved_enrichment_and_state_without_external_calls() {
    let fixture = Fixture::new();
    fixture
        .store
        .state_write("memory", "memo", &json!("original-state"))
        .unwrap();
    fixture
        .store
        .state_write("other", "private", &json!("OTHER_NAMESPACE_SENTINEL"))
        .unwrap();
    fixture
        .add(
            "other",
            Arc::new(TestHook::default()),
            &["session.read:current", "state.read:other"],
            true,
        )
        .await;
    let hook = Arc::new(TestHook {
        output: EnrichmentOutput {
            data: json!({"retrieved":"The lamp shines."}),
            contributions: vec![contribution("A saved memory.", Retention::Required)],
        },
        edits: vec![edit("description", EditMode::Append, " The lamp shines.")],
        store: Some(fixture.store.clone()),
        namespace: "memory".into(),
        ..Default::default()
    });
    fixture
        .add(
            "memory",
            hook.clone(),
            &[
                "session.read:current",
                "prompt.enrich",
                "state.read:memory",
                "prompt.contribute:before_character",
                "prompt.transform:description",
            ],
            true,
        )
        .await;
    assert_eq!(
        fixture
            .runtime
            .inspect(&fixture.session, "Hello.")
            .await
            .unwrap_err()
            .code,
        "enrichment_required"
    );
    assert_eq!(hook.calls.load(Ordering::SeqCst), 0);
    let generation = fixture.run(ProviderConfig::default()).await;
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(
        generation.snapshot["plugin_state"]["memory"]["memo"]["revision"],
        1
    );
    assert_eq!(
        fixture
            .store
            .state_record("memory", "memo")
            .unwrap()
            .unwrap()
            .0,
        2
    );
    assert_eq!(
        generation.snapshot["plugin_state"]["other"]["private"]["value"],
        "OTHER_NAMESPACE_SENTINEL"
    );
    assert!(
        !serde_json::to_string(&*hook.seen.lock().unwrap())
            .unwrap()
            .contains("OTHER_NAMESPACE_SENTINEL")
    );
    assert!(generation.enrichment.as_ref().unwrap().complete);
    let replay = fixture.runtime.replay(&generation.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        generation.request.unwrap()
    );
    assert_eq!(hook.calls.load(Ordering::SeqCst), 1);
    assert!(replay.plan.profile.starts_with("core-v1+"));
    assert_eq!(replay.phase_trace.edits.len(), 1);
    assert_eq!(
        replay.phase_trace.contributions[0].provenance["source"],
        "fixture-memory"
    );
    assert_eq!(fixture.requests.lock().unwrap()[0], replay.prepared.body);
    assert!(
        hook.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|v| !v["state"].is_null())
            .all(|v| v["state"]["memo"]["revision"] == 1)
    );
    assert_eq!(
        fixture
            .store
            .card(
                &fixture
                    .store
                    .session(&fixture.session)
                    .unwrap()
                    .character_id
            )
            .unwrap()
            .text("description"),
        "{{char}} keeps a lighthouse."
    );
}

#[tokio::test]
async fn enrichment_results_commit_before_the_next_hook_and_survive_cancellation() {
    let fixture = Fixture::new();
    let first = Arc::new(TestHook {
        output: EnrichmentOutput {
            data: json!("first-result"),
            ..Default::default()
        },
        ..Default::default()
    });
    let second = Arc::new(TestHook {
        policy: HookPolicy {
            priority: 1,
            ..Default::default()
        },
        pause: true,
        store: Some(fixture.store.clone()),
        ..Default::default()
    });
    fixture
        .add(
            "first",
            first.clone(),
            &["session.read:current", "prompt.enrich"],
            true,
        )
        .await;
    fixture
        .add(
            "second",
            second.clone(),
            &["session.read:current", "prompt.enrich"],
            true,
        )
        .await;
    let mut handle = fixture
        .runtime
        .start(&fixture.session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while second.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let partial = fixture
        .store
        .generation(&handle.generation_id)
        .unwrap()
        .enrichment
        .unwrap();
    assert!(!partial.complete);
    assert_eq!(partial.records.len(), 1);
    assert_eq!(
        partial.records[0].output.as_ref().unwrap().data,
        "first-result"
    );
    let mut changed = partial.clone();
    changed.records[0].output.as_mut().unwrap().data = json!("rewritten");
    let active = fixture.store.generation(&handle.generation_id).unwrap();
    assert_eq!(
        fixture
            .store
            .record_enrichment(&active.id, active.fencing_token, &changed, now_ms())
            .unwrap_err()
            .code,
        "invalid_enrichment_snapshot"
    );
    handle.cancel();
    let result = handle.wait().await.unwrap();
    assert_eq!(result.status, GenerationStatus::Cancelled);
    assert_eq!(result.enrichment.unwrap().records.len(), 1);
    assert!(result.request.is_none() && fixture.requests.lock().unwrap().is_empty());
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        second.seen.lock().unwrap()[1]["records"][0]["output"]["data"],
        "first-result"
    );
}

#[tokio::test]
async fn required_and_optional_hook_errors_have_distinct_results() {
    for (required, fail_enrichment) in [(true, true), (false, true), (true, false), (false, false)]
    {
        let fixture = Fixture::new();
        let hook = Arc::new(TestHook {
            policy: HookPolicy {
                required,
                ..Default::default()
            },
            fail_enrichment,
            fail_transform: !fail_enrichment,
            ..Default::default()
        });
        fixture
            .add(
                "errors",
                hook,
                &["session.read:current", "prompt.enrich"],
                true,
            )
            .await;
        assert_eq!(
            fixture
                .runtime
                .inspect(&fixture.session, "Hello.")
                .await
                .unwrap_err()
                .code,
            "enrichment_required"
        );
        let result = fixture.run(ProviderConfig::default()).await;
        assert_eq!(
            result.status,
            if required {
                GenerationStatus::Failed
            } else {
                GenerationStatus::Completed
            }
        );
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("PRIVATE_ERROR_SENTINEL")
        );
        assert_eq!(
            fixture.requests.lock().unwrap().len(),
            usize::from(!required)
        );
        if required {
            assert_eq!(result.error.unwrap().code, "prompt_hook_failed");
        } else {
            assert!(
                result.request.unwrap()["plan"]["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v.as_str().unwrap().starts_with("optional_hook:"))
            );
        }
    }
}

#[tokio::test]
async fn missing_grants_and_enrichment_deadlines_fail_before_dispatch() {
    for denied in [true, false] {
        let fixture = Fixture::new();
        let hook = Arc::new(TestHook {
            policy: HookPolicy {
                timeout_ms: 10,
                ..Default::default()
            },
            pause: !denied,
            ..Default::default()
        });
        fixture
            .add(
                "limited",
                hook.clone(),
                &["session.read:current", "prompt.enrich"],
                !denied,
            )
            .await;
        let result = fixture.run(ProviderConfig::default()).await;
        assert_eq!(result.status, GenerationStatus::Failed);
        assert_eq!(
            result.error.unwrap().code,
            if denied {
                "capability_denied"
            } else {
                "prompt_hook_timeout"
            }
        );
        assert_eq!(hook.calls.load(Ordering::SeqCst), usize::from(!denied));
        assert!(fixture.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn transforms_run_before_context_selection_and_preserve_card_revisions() {
    let fixture = Fixture::new();
    let original = fixture
        .store
        .card(
            &fixture
                .store
                .session(&fixture.session)
                .unwrap()
                .character_id,
        )
        .unwrap();
    let mut document = original.document.clone();
    document["data"]["description"] = json!("x".repeat(5000));
    document["data"]["character_book"] =
        json!({"entries":[{"id":"large","keys":[],"constant":true,"content":"y".repeat(5000)}]});
    let card = Card::import(&serde_json::to_vec(&document).unwrap()).unwrap();
    fixture.store.save_card(&card).unwrap();
    fixture
        .store
        .rebind(&fixture.session, Some(&card.id), None, false)
        .unwrap();
    let config = ProviderConfig {
        context_limit: 2048,
        ..Default::default()
    };
    fixture
        .runtime
        .configure_session(
            &fixture.store.session(&fixture.session).unwrap(),
            config.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture
            .runtime
            .inspect_request(&fixture.session, "Hello.", config.clone())
            .await
            .unwrap_err()
            .code,
        "mandatory_context_overflow"
    );
    let hook = Arc::new(TestHook {
        edits: vec![
            edit("description", EditMode::Replace, "A keeper."),
            edit("lore:large", EditMode::Replace, "A small lore entry."),
        ],
        ..Default::default()
    });
    fixture
        .add(
            "edits",
            hook,
            &[
                "session.read:current",
                "prompt.transform:description",
                "prompt.transform:lore:large",
            ],
            true,
        )
        .await;
    fixture
        .runtime
        .configure_session(
            &fixture.store.session(&fixture.session).unwrap(),
            config.clone(),
        )
        .await
        .unwrap();
    let inspected = fixture
        .runtime
        .inspect_request(&fixture.session, "Hello.", config.clone())
        .await
        .unwrap();
    assert_eq!(inspected.plan.lore_ids, ["large"]);
    assert_eq!(inspected.phase_trace.edits.len(), 2);
    let result = fixture.run(config).await;
    assert_eq!(result.status, GenerationStatus::Completed);
    assert_eq!(
        result.request.unwrap()["prepared"]["body"],
        inspected.prepared.body
    );
    assert_eq!(fixture.store.card(&card.id).unwrap().document, document);
}

#[tokio::test]
async fn transcript_edits_and_competing_replacements_are_rejected() {
    for target in ["current_user", "history"] {
        let fixture = Fixture::new();
        let first = fixture.run(ProviderConfig::default()).await;
        let history = fixture.store.history(&fixture.session).unwrap();
        let target = if target == "history" {
            format!("history:{}", history[0].id)
        } else {
            target.into()
        };
        fixture
            .add(
                "forbidden",
                Arc::new(TestHook {
                    edits: vec![edit(&target, EditMode::Replace, "Changed.")],
                    ..Default::default()
                }),
                &[
                    "session.read:current",
                    &format!("prompt.transform:{target}"),
                ],
                true,
            )
            .await;
        let result = fixture.run(ProviderConfig::default()).await;
        assert_eq!(result.status, GenerationStatus::Failed);
        assert_eq!(result.error.unwrap().code, "invalid_prompt_edit");
        assert_eq!(
            fixture.store.history(&fixture.session).unwrap()[..2],
            history
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert_eq!(
            fixture.store.generation(&first.id).unwrap().draft,
            "A reply."
        );
    }
    let fixture = Fixture::new();
    for id in ["a", "b"] {
        fixture
            .add(
                id,
                Arc::new(TestHook {
                    edits: vec![edit("description", EditMode::Replace, id)],
                    ..Default::default()
                }),
                &["session.read:current", "prompt.transform:description"],
                true,
            )
            .await;
    }
    let result = fixture.run(ProviderConfig::default()).await;
    assert_eq!(result.error.unwrap().code, "conflicting_prompt_edit");
    assert!(fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn optional_retention_has_a_diagnostic_and_required_retention_stops_overflow() {
    for retention in [Retention::Optional, Retention::Required] {
        let fixture = Fixture::new();
        let hook = Arc::new(TestHook {
            output: EnrichmentOutput {
                contributions: vec![contribution(&"x".repeat(4096), retention.clone())],
                ..Default::default()
            },
            ..Default::default()
        });
        fixture
            .add(
                "memory",
                hook,
                &[
                    "session.read:current",
                    "prompt.enrich",
                    "prompt.contribute:before_character",
                ],
                true,
            )
            .await;
        let result = fixture
            .run(ProviderConfig {
                context_limit: 2048,
                ..Default::default()
            })
            .await;
        if retention == Retention::Required {
            assert_eq!(result.error.unwrap().code, "mandatory_context_overflow");
        } else {
            assert_eq!(result.status, GenerationStatus::Completed);
            assert!(
                result.request.unwrap()["plan"]["omissions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|o| o["id"] == "plugin:memory:memory"
                        && o["reason"] == "contribution_budget")
            );
        }
    }
}

#[tokio::test]
async fn replay_rejects_nondeterministic_transforms_and_changed_compositions() {
    let fixture = Fixture::new();
    let hook = Arc::new(TestHook {
        changing: true,
        edits: vec![edit("description", EditMode::Append, " Round ")],
        ..Default::default()
    });
    fixture
        .add(
            "changing",
            hook.clone(),
            &["session.read:current", "prompt.transform:description"],
            true,
        )
        .await;
    let result = fixture.run(ProviderConfig::default()).await;
    assert_eq!(result.status, GenerationStatus::Completed);
    assert_eq!(
        fixture.runtime.replay(&result.id).await.unwrap_err().code,
        "nondeterministic_prompt"
    );
    assert_eq!(hook.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    fixture
        .registry
        .lock()
        .await
        .eject_owner("changing")
        .await
        .unwrap();
    assert_eq!(
        fixture.runtime.replay(&result.id).await.unwrap_err().code,
        "invalid_enrichment_snapshot"
    );
}

#[tokio::test]
async fn hook_priority_and_plugin_ids_determine_edit_order() {
    let fixture = Fixture::new();
    for (id, priority) in [("z", -1), ("b", 0), ("a", 0)] {
        fixture
            .add(
                id,
                Arc::new(TestHook {
                    policy: HookPolicy {
                        priority,
                        ..Default::default()
                    },
                    edits: vec![edit("description", EditMode::Append, id)],
                    ..Default::default()
                }),
                &["session.read:current", "prompt.transform:description"],
                true,
            )
            .await;
    }
    let result = fixture.run(ProviderConfig::default()).await;
    assert_eq!(result.status, GenerationStatus::Completed);
    let request = result.request.unwrap();
    assert_eq!(
        request["phase_trace"]["edits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["plugin_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["z", "a", "b"]
    );
    assert_eq!(
        result
            .enrichment
            .unwrap()
            .records
            .iter()
            .map(|r| r.plugin_id.as_str())
            .collect::<Vec<_>>(),
        ["z", "a", "b"]
    );
}

#[tokio::test]
async fn dynamic_contributions_reject_invalid_slots_denied_grants_and_duplicate_ids() {
    for (slot, capability, duplicate, expected) in [
        (
            "after_history",
            "prompt.contribute:after_history",
            false,
            "invalid_prompt_slot",
        ),
        (
            "before_character",
            "prompt.transform:description",
            false,
            "capability_denied",
        ),
        (
            "before_character",
            "prompt.contribute:before_character",
            true,
            "invalid_section_id",
        ),
    ] {
        let fixture = Fixture::new();
        let mut value = contribution("A memory.", Retention::Required);
        value.slot = slot.into();
        let values = if duplicate {
            vec![value.clone(), value]
        } else {
            vec![value]
        };
        fixture
            .add(
                "invalid",
                Arc::new(TestHook {
                    output: EnrichmentOutput {
                        contributions: values,
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                &["session.read:current", "prompt.enrich", capability],
                true,
            )
            .await;
        let result = fixture.run(ProviderConfig::default()).await;
        assert_eq!(result.status, GenerationStatus::Failed);
        assert_eq!(result.error.unwrap().code, expected);
        assert!(fixture.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn transforms_reject_missing_targets_and_ungranted_edits() {
    for (target, capability, expected) in [
        ("missing", "prompt.transform:missing", "invalid_prompt_edit"),
        (
            "description",
            "prompt.transform:scenario",
            "capability_denied",
        ),
    ] {
        let fixture = Fixture::new();
        fixture
            .add(
                "invalid",
                Arc::new(TestHook {
                    edits: vec![edit(target, EditMode::Replace, "Changed.")],
                    ..Default::default()
                }),
                &["session.read:current", capability],
                true,
            )
            .await;
        let result = fixture.run(ProviderConfig::default()).await;
        assert_eq!(result.error.unwrap().code, expected);
        assert!(fixture.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_blocking_enricher_cannot_block_its_deadline_or_release_its_plugin_early() {
    let fixture = Fixture::new();
    let (release, receiver) = std::sync::mpsc::channel();
    let hook = Arc::new(TestHook {
        policy: HookPolicy {
            timeout_ms: 250,
            ..Default::default()
        },
        blocking: Some(Mutex::new(receiver)),
        ..Default::default()
    });
    fixture
        .add(
            "blocking",
            hook,
            &["session.read:current", "prompt.enrich"],
            true,
        )
        .await;
    let generation = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.run(ProviderConfig::default()),
    )
    .await
    .unwrap();
    assert_eq!(generation.status, GenerationStatus::Failed);
    assert_eq!(generation.error.unwrap().code, "prompt_hook_timeout");
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .registry
            .lock()
            .await
            .eject_owner("blocking")
            .await
            .unwrap_err()
            .code,
        "plugin_change_deferred"
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match fixture.registry.lock().await.eject_owner("blocking").await {
                Ok(_) => break,
                Err(error) => assert_eq!(error.code, "plugin_change_deferred"),
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
