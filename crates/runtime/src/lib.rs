use rustytavern_adapters::provider;
use rustytavern_card as card;
use rustytavern_conversation as session;
use rustytavern_conversation::prompt;
use rustytavern_core::{Error, Result, digest};
use rustytavern_plugin as plugin;
use rustytavern_store as store;

pub mod driver;
pub mod phases;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use card::{Card, CardDecoder, CcDecoder};
use driver::{ConversationDriver, DRIVER_CAPABILITIES, DriverContext, SingleReplyDriver};
use phases::{HookBinding, PhaseTrace, SavedEnrichment};
use prompt::{ConservativeCounter, CoreProfile, PromptInput, PromptPlan, PromptProfile};
use provider::{
    CredentialResolver, EnvironmentCredentials, HttpProvider, PreparedRequest, ProviderAdapter,
    ProviderConfig, ProviderStream,
};
use session::{Generation, GenerationStatus, Role, Session, SessionConfiguration};
use store::{Preparation, StoreBackend, Submit, now_ms};

const LEASE_MS: i64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledRequest {
    pub plan: PromptPlan,
    pub prepared: PreparedRequest,
    #[serde(default)]
    pub phase_trace: PhaseTrace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub r#type: String,
    pub generation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missed_events: Option<usize>,
}

impl RuntimeEvent {
    fn new(kind: &str, generation: &str) -> Self {
        Self {
            r#type: kind.to_owned(),
            generation_id: generation.to_owned(),
            text: None,
            usage: None,
            error: None,
            missed_events: None,
        }
    }
}

#[derive(Clone)]
struct EventSink(Arc<Mutex<EventState>>);
struct EventState {
    sender: Option<mpsc::Sender<RuntimeEvent>>,
    missed: usize,
}
impl EventSink {
    fn new(sender: mpsc::Sender<RuntimeEvent>) -> Self {
        Self(Arc::new(Mutex::new(EventState {
            sender: Some(sender),
            missed: 0,
        })))
    }
    fn emit(&self, mut event: RuntimeEvent) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        let Some(sender) = &state.sender else {
            return;
        };
        let terminal = matches!(
            event.r#type.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        );
        if state.missed > 0 {
            event.missed_events = Some(state.missed);
        }
        let result = sender.try_send(event);
        match result {
            Ok(()) => state.missed = 0,
            Err(_) => state.missed = state.missed.saturating_add(1),
        }
        if terminal {
            state.sender.take();
        }
    }
}

/// The handle cancels unfinished work when its owner drops it.
pub struct GenerationHandle {
    pub generation_id: String,
    pub events: mpsc::Receiver<RuntimeEvent>,
    cancellation: CancellationToken,
    task: Option<JoinHandle<Result<Generation>>>,
}
impl GenerationHandle {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub async fn wait(&mut self) -> Result<Generation> {
        let task = self.task.as_mut().ok_or_else(|| {
            Error::new(
                "invalid_generation_handle",
                "The generation result already has an owner.",
            )
        })?;
        let result = task.await;
        self.task = None;
        result.map_err(Error::context("runtime", "Task failed."))?
    }
}
impl Drop for GenerationHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
pub struct Runtime {
    pub store: Arc<dyn StoreBackend>,
    provider: Arc<dyn ProviderAdapter>,
    profile: Arc<dyn PromptProfile>,
    decoder: Arc<dyn CardDecoder>,
    driver: Arc<dyn ConversationDriver>,
    credentials: Arc<dyn CredentialResolver>,
    registry: Option<Arc<tokio::sync::Mutex<crate::plugin::PluginRegistry>>>,
    composition: Option<Arc<crate::plugin::CompositionLease>>,
    hooks: Vec<HookBinding>,
    product_composition: Option<Value>,
    effective_lock: Value,
}

impl Runtime {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self {
            store,
            provider: Arc::new(HttpProvider),
            profile: Arc::new(CoreProfile),
            decoder: Arc::new(CcDecoder),
            driver: Arc::new(SingleReplyDriver),
            credentials: Arc::new(EnvironmentCredentials),
            registry: None,
            composition: None,
            hooks: Vec::new(),
            product_composition: None,
            effective_lock: Value::Null,
        }
    }
    pub fn with_provider(mut self, provider: Arc<dyn ProviderAdapter>) -> Self {
        self.provider = provider;
        self
    }
    pub fn with_profile(mut self, profile: Arc<dyn PromptProfile>) -> Self {
        self.profile = profile;
        self
    }
    pub fn with_decoder(mut self, decoder: Arc<dyn CardDecoder>) -> Self {
        self.decoder = decoder;
        self
    }
    pub fn with_driver(mut self, driver: Arc<dyn ConversationDriver>) -> Self {
        self.driver = driver;
        self
    }
    pub fn with_credentials(mut self, credentials: Arc<dyn CredentialResolver>) -> Self {
        self.credentials = credentials;
        self
    }

    pub fn with_plugins(
        mut self,
        registry: Arc<tokio::sync::Mutex<crate::plugin::PluginRegistry>>,
    ) -> Self {
        self.registry = Some(registry);
        self
    }
    pub fn with_product_composition(mut self, composition: Value) -> Self {
        self.product_composition = Some(composition);
        self
    }

    fn composition_record(&self) -> Result<Value> {
        if [
            self.provider.id(),
            self.profile.id(),
            self.decoder.id(),
            self.driver.id(),
        ]
        .iter()
        .any(|id| id.is_empty())
        {
            return Err(Error::new(
                "invalid_runtime_service",
                "The selected runtime services require nonempty implementation IDs.",
            ));
        }
        let mut record = self
            .product_composition
            .clone()
            .unwrap_or_else(|| json!({"preset":"embedded","extensions":[]}));
        let fields = record.as_object_mut().ok_or_else(|| {
            Error::new(
                "invalid_composition",
                "The product composition requires an object.",
            )
        })?;
        if fields.contains_key("session_configurations")
            || fields.contains_key("configuration_schema")
        {
            return Err(Error::new(
                "invalid_composition",
                "The product composition contains reserved session fields.",
            ));
        }
        let services = self
            .composition
            .as_ref()
            .map(|c| c.snapshot())
            .unwrap_or_else(
                || json!({"identity":"","plugins":[],"bindings":{},"contributions":[]}),
            );
        fields.extend(services.as_object().unwrap().clone());
        let service_identity = fields.remove("identity").unwrap();
        fields.insert("service_identity".into(), service_identity);
        fields.insert("native".into(), json!({"artifact_hash":rustytavern_core::native_artifact_hash()?,"provider":self.provider.id(),"profile":self.profile.id(),"decoder":self.decoder.id(),"driver":self.driver.id()}));
        let identity = crate::digest(&serde_json::to_vec(&record)?);
        record["identity"] = json!(identity);
        Ok(record)
    }

    async fn composed(&self) -> Result<Self> {
        let mut runtime = self.clone();
        let Some(registry) = &self.registry else {
            runtime.effective_lock = runtime.composition_record()?;
            return Ok(runtime);
        };
        let lease = registry.lock().await.freeze()?;
        if self
            .product_composition
            .as_ref()
            .is_some_and(|p| p["identity"] != lease.identity)
        {
            return Err(Error::new(
                "composition_changed",
                "The application must resolve its changed product composition.",
            ));
        }
        if let Some(profile) = lease.optional_service::<Arc<dyn PromptProfile>>("profile/1")? {
            runtime.profile = profile.as_ref().clone();
        }
        if let Some(provider) = lease.optional_service::<Arc<dyn ProviderAdapter>>("provider/1")? {
            runtime.provider = provider.as_ref().clone();
        }
        if let Some(decoder) = lease.optional_service::<Arc<dyn CardDecoder>>("card.decoder/1")? {
            runtime.decoder = decoder.as_ref().clone();
        }
        if let Some(driver) = lease.optional_service::<Arc<dyn ConversationDriver>>("driver/1")? {
            let owner = lease.service_owner("driver/1")?;
            for capability in DRIVER_CAPABILITIES {
                if !owner.grants.contains(capability) {
                    return Err(Error::new(
                        "capability_denied",
                        format!("The selected driver requires {capability}."),
                    ));
                }
            }
            runtime.driver = driver.as_ref().clone();
        }
        if self
            .product_composition
            .as_ref()
            .is_some_and(|p| p["preset"] == "tavern")
        {
            for service in [
                "profile/1",
                "provider/1",
                "store/1",
                "card.decoder/1",
                "driver/1",
            ] {
                if !lease.contains_service(service) {
                    return Err(Error::new(
                        "missing_service",
                        format!("The tavern preset requires {service}."),
                    ));
                }
            }
        }
        runtime.hooks = phases::bindings(Some(&lease))?;
        runtime.composition = Some(Arc::new(lease));
        runtime.effective_lock = runtime.composition_record()?;
        Ok(runtime)
    }

    pub async fn import_card(&self, bytes: Vec<u8>) -> Result<Card> {
        self.import(bytes, None).await
    }
    pub async fn import_artifact(&self, artifact: rustytavern_core::Artifact) -> Result<Card> {
        self.import(artifact.bytes, Some(artifact.provenance)).await
    }
    async fn import(
        &self,
        bytes: Vec<u8>,
        provenance: Option<rustytavern_core::Provenance>,
    ) -> Result<Card> {
        let runtime = self.composed().await?;
        let worker = runtime.clone();
        runtime
            .store_call(move |store| {
                let mut card = Card::import_with(&bytes, worker.decoder.as_ref())?;
                if let Some(provenance) = provenance {
                    card = card.with_provenance(&provenance)?;
                }
                store.save_card(&card)?;
                Ok(card)
            })
            .await
    }

    pub async fn create_session(
        &self,
        session: &Session,
        greeting: Option<&str>,
        config: ProviderConfig,
    ) -> Result<Session> {
        let runtime = self.composed().await?;
        let mut session = session.clone();
        let greeting = greeting.map(str::to_owned);
        let worker = runtime.clone();
        runtime
            .store_call(move |store| {
                worker.provider.validate_config(&session.model, &config)?;
                session.composition = json!({});
                let record = SessionConfiguration::new(
                    &session,
                    worker.profile.id().into(),
                    config,
                    worker.effective_lock.clone(),
                );
                session.append_configuration(record)?;
                store.create_session(&session, greeting.as_deref())
            })
            .await
    }

    pub async fn configure_session(
        &self,
        session: &Session,
        config: ProviderConfig,
    ) -> Result<Session> {
        let runtime = self.composed().await?;
        let session = session.clone();
        let worker = runtime.clone();
        runtime
            .store_call(move |store| {
                worker.provider.validate_config(&session.model, &config)?;
                let record = SessionConfiguration::new(
                    &session,
                    worker.profile.id().into(),
                    config,
                    worker.effective_lock.clone(),
                );
                store.configure_session(&session.id, session.version, record)
            })
            .await
    }

    pub async fn provider_config(&self, session_id: &str) -> Result<ProviderConfig> {
        let id = session_id.to_owned();
        self.store_call(move |store| Ok(store.session(&id)?.configuration()?.provider_config))
            .await
    }

    pub async fn inspect(&self, session_id: &str, message: &str) -> Result<PromptPlan> {
        Ok(self
            .inspect_request(session_id, message, self.provider_config(session_id).await?)
            .await?
            .plan)
    }

    pub async fn inspect_request(
        &self,
        session_id: &str,
        message: &str,
        config: ProviderConfig,
    ) -> Result<CompiledRequest> {
        let runtime = self.composed().await?;
        let id = session_id.to_owned();
        let message = message.to_owned();
        let worker = runtime.clone();
        runtime
            .store_call(move |store| {
                let session = store.session(&id)?;
                let snapshot = worker.input_snapshot(store, &session, &message, &config, None)?;
                let mut saved = SavedEnrichment::new(&snapshot, worker.composition_id().into())?;
                for hook in &worker.hooks {
                    let record = match hook.offline_record() {
                        Ok(record) => record,
                        Err(error)
                            if !hook.policy.required && error.code != "enrichment_required" =>
                        {
                            hook.record(Err(error))
                        }
                        Err(error) => return Err(error),
                    };
                    saved.records.push(record);
                }
                saved.complete = true;
                worker.compile_snapshot(&snapshot, &config, &saved)
            })
            .await
    }

    pub async fn replay(&self, generation_id: &str) -> Result<CompiledRequest> {
        let runtime = self.composed().await?;
        let id = generation_id.to_owned();
        let worker = runtime.clone();
        runtime
            .store_call(move |store| {
                let generation = store.generation(&id)?;
                let enrichment = generation.enrichment.as_ref().ok_or_else(|| {
                    Error::new(
                        "missing_enrichment",
                        "The generation has no saved enrichment phase.",
                    )
                })?;
                let expected = generation.request.as_ref().ok_or_else(|| {
                    Error::new(
                        "invalid_request_snapshot",
                        "The generation has no saved request.",
                    )
                })?;
                let config =
                    serde_json::from_value(generation.snapshot["provider_config"].clone())?;
                let compiled =
                    worker.compile_snapshot(&generation.snapshot, &config, enrichment)?;
                if serde_json::to_value(&compiled)? != *expected {
                    return Err(Error::new(
                        "nondeterministic_prompt",
                        "The replay differs from the saved request.",
                    ));
                }
                Ok(compiled)
            })
            .await
    }

    fn composition_id(&self) -> &str {
        self.effective_lock["identity"].as_str().unwrap_or_default()
    }

    fn input_snapshot(
        &self,
        store: &dyn StoreBackend,
        session: &crate::session::Session,
        message: &str,
        config: &ProviderConfig,
        retry_user: Option<&str>,
    ) -> Result<Value> {
        let configuration = session.configuration()?;
        if configuration.composition != self.effective_lock
            || configuration.profile != self.profile.id()
        {
            return Err(Error::new(
                "pinned_composition_unavailable",
                "The pinned composition is unavailable. The session requires an explicit configuration change.",
            ));
        }
        if serde_json::to_value(&configuration.provider_config)? != serde_json::to_value(config)? {
            return Err(Error::new(
                "session_configuration_changed",
                "The provider settings differ from the saved session configuration.",
            ));
        }
        let card = store.card(&session.character_id)?;
        card.ensure_compatible(session.accept_reduced)?;
        let worldbooks = store.worldbooks(&session.worldbook_ids)?;
        let persona = store.persona(&session.persona_id)?;
        let mut history = store.history(&session.id)?;
        if let Some(position) = history
            .iter()
            .position(|m| Some(m.id.as_str()) == retry_user)
        {
            history.truncate(position);
        }
        let mut state = serde_json::Map::new();
        if let Some(composition) = &self.composition {
            for plugin in &composition.plugins {
                if plugin
                    .grants
                    .contains(&format!("state.read:{}", plugin.manifest.id))
                {
                    let snapshot = store.plugin_state_snapshot(&plugin.manifest.id)?;
                    snapshot.require_schema(plugin.manifest.state_schema)?;
                    state.insert(plugin.manifest.id.clone(), snapshot.entries);
                }
            }
        }
        let composition = &self.effective_lock;
        let mut snapshot = json!({"schema_version":2,"character_id":card.id,"character_document":card.document,"persona":persona,"session":session,"history":history,"submitted_text":message,"profile":self.profile.id(),"plugin_composition":composition,"plugin_state":state,"provider_config":config});
        if !worldbooks.is_empty() {
            snapshot["worldbooks"] = json!(
                worldbooks
                    .iter()
                    .map(|book| json!({"id":book.id(),"document":book.document()}))
                    .collect::<Vec<_>>()
            );
        }
        Ok(snapshot)
    }

    pub async fn start(
        &self,
        session_id: &str,
        message: &str,
        idempotency_key: Option<&str>,
        config: ProviderConfig,
    ) -> Result<GenerationHandle> {
        self.start_inner(session_id, message, idempotency_key, None, config)
            .await
    }

    pub async fn regenerate(
        &self,
        session_id: &str,
        config: ProviderConfig,
    ) -> Result<GenerationHandle> {
        let id = session_id.to_owned();
        let (node, text) = self
            .store_call(move |store| {
                let history = store.history(&id)?;
                let user = history
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::User)
                    .ok_or_else(|| {
                        Error::new("invalid_retry", "The selected branch has no user message.")
                    })?;
                Ok((user.id.clone(), user.text.clone()))
            })
            .await?;
        self.start_inner(session_id, &text, None, Some(node), config)
            .await
    }

    async fn start_inner(
        &self,
        session_id: &str,
        message: &str,
        key: Option<&str>,
        retry_user: Option<String>,
        config: ProviderConfig,
    ) -> Result<GenerationHandle> {
        let runtime = self.composed().await?;
        let id = session_id.to_owned();
        let message = message.to_owned();
        let key = key.map(str::to_owned);
        let worker = runtime.clone();
        let profile_config = config.clone();
        let preparation = runtime
            .store_call(move |store| {
                store.recover(&id, now_ms())?;
                let session = store.session(&id)?;
                let snapshot = worker.input_snapshot(
                    store,
                    &session,
                    &message,
                    &profile_config,
                    retry_user.as_deref(),
                )?;
                store.prepare(Submit {
                    session_id: &id,
                    expected_version: session.version,
                    text: &message,
                    idempotency_key: key.as_deref(),
                    retry_user: retry_user.as_deref(),
                    snapshot,
                    now: now_ms(),
                    lease_ms: LEASE_MS,
                })
            })
            .await?;
        let cancellation = CancellationToken::new();
        let (sender, receiver) = mpsc::channel(64);
        let generation_id = preparation.generation.id.clone();
        let token = cancellation.clone();
        let task = tokio::spawn(async move {
            if !preparation.created {
                if !preparation.generation.status.terminal() {
                    return Err(Error::new(
                        "session_conflict",
                        "The original submission still owns an active generation.",
                    ));
                }
                let kind = status_name(&preparation.generation.status);
                let _ = sender.try_send(RuntimeEvent::new(kind, &preparation.generation.id));
                return Ok(preparation.generation);
            }
            runtime.drive(preparation, config, token, sender).await
        });
        Ok(GenerationHandle {
            generation_id,
            events: receiver,
            cancellation,
            task: Some(task),
        })
    }

    async fn drive(
        &self,
        preparation: Preparation,
        config: ProviderConfig,
        cancellation: CancellationToken,
        sender: mpsc::Sender<RuntimeEvent>,
    ) -> Result<Generation> {
        let generation = preparation.generation;
        let sink = EventSink::new(sender);
        sink.emit(RuntimeEvent::new("prepared", &generation.id));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(8));
        heartbeat.tick().await;
        let outcome = {
            let work = self.generate(&generation, &config, &cancellation, &sink);
            tokio::pin!(work);
            loop {
                tokio::select! { biased;
                    _ = cancellation.cancelled() => break Err(Error::new("cancelled", "The caller cancelled the generation.")),
                    _ = heartbeat.tick() => if let Err(error) = self.renew(&generation).await { cancellation.cancel(); break Err(error); },
                    result = &mut work => break result,
                }
            }
        };
        let (status, error) = match outcome {
            Ok(()) if !cancellation.is_cancelled() => (GenerationStatus::Completed, None),
            Ok(()) => (
                GenerationStatus::Cancelled,
                Some(Error::new(
                    "cancelled",
                    "The caller cancelled the generation.",
                )),
            ),
            Err(error) if error.code == "cancelled" => (GenerationStatus::Cancelled, Some(error)),
            Err(error) => (GenerationStatus::Failed, Some(error)),
        };
        let id = generation.id.clone();
        let token = generation.fencing_token;
        let settled = self
            .store_call(move |store| store.settle(&id, token, status, None, error, now_ms()))
            .await?;
        let mut event = RuntimeEvent::new(status_name(&settled.status), &settled.id);
        event.error = settled.error.clone();
        event.usage = settled.usage.clone();
        event.text = Some(settled.draft.clone());
        sink.emit(event);
        Ok(settled)
    }

    fn compile_snapshot(
        &self,
        snapshot: &Value,
        config: &ProviderConfig,
        enrichment: &SavedEnrichment,
    ) -> Result<CompiledRequest> {
        let session: crate::session::Session = serde_json::from_value(snapshot["session"].clone())?;
        let card = self.store.card(&session.character_id)?;
        card.validate()?;
        let persona = serde_json::from_value(snapshot["persona"].clone())?;
        let history: Vec<crate::session::Message> =
            serde_json::from_value(snapshot["history"].clone())?;
        let mut session = session;
        session.options.context_limit = session.options.context_limit.min(config.context_limit);
        session.options.output_limit = session.options.output_limit.min(config.output_limit);
        let message = snapshot["submitted_text"]
            .as_str()
            .ok_or_else(|| Error::new("storage", "The input snapshot has no submitted text."))?;
        if snapshot["profile"] != self.profile.id()
            || snapshot["character_id"] != card.id
            || snapshot["character_document"] != card.document
        {
            return Err(Error::new(
                "composition_mismatch",
                "The active card or profile differs from the saved input.",
            ));
        }
        let worldbooks = self.store.worldbooks(&session.worldbook_ids)?;
        let saved_worldbooks = snapshot.get("worldbooks").cloned().unwrap_or(json!([]));
        let expected_worldbooks = json!(
            worldbooks
                .iter()
                .map(|book| json!({"id":book.id(),"document":book.document()}))
                .collect::<Vec<_>>()
        );
        if saved_worldbooks != expected_worldbooks {
            return Err(Error::new(
                "composition_mismatch",
                "The worldbooks differ from the saved input.",
            ));
        }
        let mut plan = self.profile.draft_with_worldbooks(
            &PromptInput {
                card: &card,
                persona: &persona,
                history: &history,
                message,
                options: &session.options,
                accept_reduced: session.accept_reduced,
                counter: &ConservativeCounter,
            },
            &worldbooks,
        )?;
        plan.profile = self.profile.id().into();
        plan.output_reserve = session.options.output_limit;
        plan.protocol_margin = session.options.protocol_margin;
        let phase_trace = phases::apply(
            &self.hooks,
            snapshot,
            enrichment,
            self.composition.as_deref(),
            self.composition_id(),
            &mut plan,
        )?;
        self.profile.select_context(&mut plan, &session.options)?;
        loop {
            let prepared = self.provider.prepare(&plan, &session.model, config)?;
            let units = serde_json::to_vec(&prepared.body)?.len();
            if units <= 8 * 1024 * 1024
                && units
                    .saturating_add(plan.output_reserve)
                    .saturating_add(plan.protocol_margin)
                    <= session.options.context_limit
            {
                plan.serialized_input_units = Some(units);
                return Ok(CompiledRequest {
                    plan,
                    prepared,
                    phase_trace,
                });
            }
            plan.trim_optional("serialized_request_budget")?;
        }
    }

    async fn generate(
        &self,
        generation: &Generation,
        config: &ProviderConfig,
        cancellation: &CancellationToken,
        sink: &EventSink,
    ) -> Result<()> {
        let timeout_seconds = config.timeout_seconds;
        let mut saved = SavedEnrichment::new(&generation.snapshot, self.composition_id().into())?;
        for hook in &self.hooks {
            let record = hook.record(
                hook.enrich(
                    &generation.snapshot,
                    cancellation.clone(),
                    self.composition.clone(),
                )
                .await,
            );
            let failure = record.error.clone().filter(|_| record.policy.required);
            saved.records.push(record);
            self.save_enrichment(generation, saved.clone()).await?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        saved.complete = true;
        self.save_enrichment(generation, saved.clone()).await?;
        let runtime = self.clone();
        let snapshot = generation.snapshot.clone();
        let config = config.clone();
        let compiled = tokio::task::spawn_blocking(move || {
            runtime.compile_snapshot(&snapshot, &config, &saved)
        })
        .await
        .map_err(Error::context("runtime", "Prompt compilation failed."))??;
        let record = serde_json::to_value(&compiled)?;
        let id = generation.id.clone();
        let token = generation.fencing_token;
        self.store_call(move |store| store.save_request(&id, token, &record, now_ms()))
            .await?;
        let (mut context, _control) = DriverContext::new(
            self.clone(),
            generation.clone(),
            compiled.prepared,
            cancellation.clone(),
            sink.clone(),
        );
        let driver = self.driver.clone();
        let handle = tokio::runtime::Handle::current();
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_secs(timeout_seconds))
            .ok_or_else(|| {
                Error::new(
                    "invalid_driver_deadline",
                    "The driver deadline exceeds the supported clock range.",
                )
            })?;
        let job = tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                let result = driver.run(&mut context).await;
                context.finish(result)
            })
        });
        tokio::time::timeout_at(deadline, job)
            .await
            .map_err(Error::context(
                "driver_timeout",
                "The driver phase exceeded its deadline.",
            ))?
            .map_err(Error::context(
                "driver_failure",
                "The driver worker failed.",
            ))?
    }

    async fn dispatch(
        &self,
        generation: &Generation,
        request: PreparedRequest,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream> {
        self.renew(generation).await?;
        self.provider
            .open_stream(request, self.credentials.as_ref(), cancellation)
            .await
    }

    async fn save_enrichment(
        &self,
        generation: &Generation,
        enrichment: SavedEnrichment,
    ) -> Result<()> {
        let id = generation.id.clone();
        let token = generation.fencing_token;
        self.store_call(move |store| store.record_enrichment(&id, token, &enrichment, now_ms()))
            .await
    }

    async fn renew(&self, generation: &Generation) -> Result<()> {
        let id = generation.id.clone();
        let token = generation.fencing_token;
        self.store_call(move |store| store.renew(&id, token, now_ms(), LEASE_MS))
            .await
    }

    async fn store_call<T: Send + 'static>(
        &self,
        action: impl FnOnce(&dyn StoreBackend) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || action(store.as_ref()))
            .await
            .map_err(Error::context("storage", "Store failed."))?
    }
}

fn status_name(status: &GenerationStatus) -> &'static str {
    match status {
        GenerationStatus::Prepared => "prepared",
        GenerationStatus::Streaming => "text_delta",
        GenerationStatus::Completed => "completed",
        GenerationStatus::Failed => "failed",
        GenerationStatus::Cancelled => "cancelled",
        GenerationStatus::Interrupted => "interrupted",
    }
}
