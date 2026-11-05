use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use rustytavern::{
    Error, Result,
    app::default_registry,
    card::{Card, CardDecoder, CcDecoder, DecodedCard, MAX_INPUT_BYTES},
    driver::{ConversationDriver, DRIVER_CAPABILITIES, DriverContext, MAX_DRIVER_EVENTS},
    plugin::{PluginContext, PluginManifest, PluginRegistry, RuntimePlugin},
    prompt::{PromptOptions, PromptPlan},
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderStream,
    },
    runtime::Runtime,
    session::{GenerationStatus, Persona, Session, SessionExport},
    store::{SqliteStore, StoreBackend},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct PrefixDecoder {
    id: &'static str,
    calls: Arc<AtomicUsize>,
    document: Option<Value>,
}
impl CardDecoder for PrefixDecoder {
    fn id(&self) -> &str {
        self.id
    }
    fn decode(&self, bytes: &[u8]) -> Result<DecodedCard> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let document = if let Some(document) = &self.document {
            document.clone()
        } else {
            let json = bytes
                .strip_prefix(b"RT:\n")
                .ok_or_else(|| Error::new("invalid_prefix", "The card prefix is missing."))?;
            serde_json::from_slice(json)?
        };
        Ok(DecodedCard {
            document,
            format: "prefixed-json".into(),
            selected_envelope: None,
            conflicting_envelopes: false,
        })
    }
}
fn prefix() -> Vec<u8> {
    [
        b"RT:\n".as_slice(),
        include_bytes!("fixtures/synthetic/card-v3.json"),
    ]
    .concat()
}
fn decoder(id: &'static str, document: Option<Value>) -> Arc<PrefixDecoder> {
    Arc::new(PrefixDecoder {
        id,
        calls: Arc::new(AtomicUsize::new(0)),
        document,
    })
}

struct ProbeStream {
    inner: ProviderStream,
    closed: Arc<AtomicBool>,
}
impl Stream for ProbeStream {
    type Item = Result<ProviderEvent>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
impl Drop for ProbeStream {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}
struct LocalProvider {
    events: Vec<ProviderEvent>,
    hold: bool,
    count: Option<usize>,
    calls: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}
impl Default for LocalProvider {
    fn default() -> Self {
        Self {
            events: vec![
                ProviderEvent::Text {
                    text: "hello".into(),
                },
                ProviderEvent::Usage {
                    usage: json!({"total_tokens":9}),
                },
                ProviderEvent::Complete,
            ],
            hold: false,
            count: None,
            calls: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
}
#[async_trait]
impl ProviderAdapter for LocalProvider {
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        HttpProvider.prepare(plan, model, config)
    }
    async fn open_stream(
        &self,
        _: PreparedRequest,
        _: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let inner: ProviderStream = if let Some(count) = self.count {
            Box::pin(
                futures_util::stream::repeat(Ok(ProviderEvent::Text {
                    text: String::new(),
                }))
                .take(count)
                .chain(futures_util::stream::once(std::future::ready(Ok(
                    ProviderEvent::Complete,
                )))),
            )
        } else {
            let events = futures_util::stream::iter(self.events.clone().into_iter().map(Ok));
            if self.hold {
                Box::pin(events.chain(futures_util::stream::pending()))
            } else {
                Box::pin(events)
            }
        };
        Ok(Box::pin(ProbeStream {
            inner,
            closed: self.closed.clone(),
        }))
    }
}

enum Mode {
    Uppercase,
    Early,
    Swallow,
    BeforeDispatch,
    Oversize,
}
struct TestDriver(Mode);
#[async_trait]
impl ConversationDriver for TestDriver {
    fn id(&self) -> &str {
        "test-reply/1"
    }
    async fn run(&self, context: &mut DriverContext) -> Result<()> {
        if matches!(self.0, Mode::Early) {
            return Ok(());
        }
        if matches!(self.0, Mode::BeforeDispatch) {
            return context.append_text("illegal").await;
        }
        loop {
            match context.next_event().await {
                Ok(Some(ProviderEvent::Text { text })) if matches!(self.0, Mode::Uppercase) => {
                    context.append_text(&text.to_uppercase()).await?
                }
                Ok(Some(ProviderEvent::Text { .. })) if matches!(self.0, Mode::Oversize) => {
                    let _ = context.append_text(&"x".repeat(8 * 1024 * 1024 + 1)).await;
                    return Ok(());
                }
                Ok(Some(ProviderEvent::Complete)) | Ok(None) | Err(_) => return Ok(()),
                _ => {}
            }
        }
    }
}
struct FeaturePlugin {
    manifest: PluginManifest,
    decoder: Arc<dyn CardDecoder>,
    driver: Arc<dyn ConversationDriver>,
    provider: Option<Arc<dyn ProviderAdapter>>,
}
#[async_trait]
impl RuntimePlugin for FeaturePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        if let Some(provider) = &self.provider {
            context.register("provider/1", Arc::new(provider.clone()))?;
        }
        context.register("card.decoder/1", Arc::new(self.decoder.clone()))?;
        context.register("driver/1", Arc::new(self.driver.clone()))
    }
}
fn target(store: &dyn StoreBackend, card: &Card) -> Session {
    let persona = Persona::new("Alex".into(), "A visitor.".into()).unwrap();
    store.save_persona(&persona).unwrap();
    Session {
        id: String::new(),
        character_id: card.id.clone(),
        persona_id: persona.id,
        worldbook_ids: Vec::new(),
        provider: "fixture".into(),
        model: "fixture".into(),
        head: None,
        version: 0,
        accept_reduced: false,
        options: PromptOptions::default(),
        composition: json!({}),
    }
}
async fn setup(
    driver: Arc<dyn ConversationDriver>,
    provider: Arc<LocalProvider>,
) -> (
    Runtime,
    Arc<SqliteStore>,
    Arc<tokio::sync::Mutex<PluginRegistry>>,
    Session,
) {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let mut registry = PluginRegistry::default();
    registry
        .inject(
            vec![Arc::new(FeaturePlugin {
                manifest: PluginManifest {
                    id: "features".into(),
                    provides: vec!["card.decoder/1".into(), "driver/1".into()],
                    artifact_hash: "test-artifact".into(),
                    configuration_hash: "test-configuration".into(),
                    requested_capabilities: DRIVER_CAPABILITIES
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    ..Default::default()
                },
                decoder: decoder("prefix-json/1", None),
                driver,
                provider: None,
            })],
            "test",
            &BTreeMap::from([(
                "features".into(),
                DRIVER_CAPABILITIES.into_iter().map(str::to_owned).collect(),
            )]),
        )
        .await
        .unwrap();
    let registry = Arc::new(tokio::sync::Mutex::new(registry));
    let runtime = Runtime::new(store.clone())
        .with_provider(provider)
        .with_plugins(registry.clone());
    let card = runtime.import_card(prefix()).await.unwrap();
    let session = runtime
        .create_session(
            &target(store.as_ref(), &card),
            None,
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    (runtime, store, registry, session)
}

#[tokio::test]
async fn the_tavern_preset_resolves_all_five_services_and_rejects_missing_parts() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let registry = default_registry(store.clone()).await.unwrap();
    let lease = registry.lock().await.freeze().unwrap().snapshot();
    for service in [
        "card.decoder/1",
        "driver/1",
        "profile/1",
        "provider/1",
        "store/1",
    ] {
        assert!(lease["resolved_bindings"][service].as_str().is_some());
    }
    assert_eq!(lease["plugins"].as_array().unwrap().len(), 5);
    let empty = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
    let lock = empty.lock().await.freeze().unwrap().snapshot();
    let runtime = Runtime::new(store.clone())
        .with_plugins(empty)
        .with_product_composition(json!({"preset":"tavern","identity":lock["identity"]}));
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    store.save_card(&card).unwrap();
    assert_eq!(
        runtime
            .create_session(
                &target(store.as_ref(), &card),
                None,
                ProviderConfig::default()
            )
            .await
            .unwrap_err()
            .code,
        "missing_service"
    );
    assert!(store.list("session").unwrap().is_empty());
}

#[tokio::test]
async fn selected_decoder_and_driver_change_the_actual_runtime_and_keep_exports_readable() {
    let provider = Arc::new(LocalProvider::default());
    let (runtime, store, _, session) =
        setup(Arc::new(TestDriver(Mode::Uppercase)), provider.clone()).await;
    let card = store.card(&session.character_id).unwrap();
    assert_eq!(card.export_original(), prefix());
    assert_eq!(card.decoder.as_ref().unwrap().id, "prefix-json/1");
    let mut handle = runtime
        .start(&session.id, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    let generation = handle.wait().await.unwrap();
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(generation.draft, "HELLO");
    assert_eq!(generation.usage.unwrap()["total_tokens"], 9);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        generation.snapshot["plugin_composition"]["resolved_bindings"]["driver/1"],
        "features"
    );
    assert_eq!(
        generation.snapshot["plugin_composition"]["native"]["decoder"],
        "prefix-json/1"
    );
    let export = store.export_session(&session.id, true).unwrap();
    assert_eq!(export.schema_version, 4);
    let bytes = export.encode().unwrap();
    drop(runtime);
    let decoded = SessionExport::decode(&bytes).unwrap();
    assert_eq!(decoded.characters[0].original, prefix());
    assert_eq!(decoded.generations[0].draft, "HELLO");
}

#[tokio::test]
async fn native_bindings_replace_bundled_services_and_missing_driver_grants_block_use() {
    for granted in [false, true] {
        let store = Arc::new(SqliteStore::memory().unwrap());
        let registry = default_registry(store.clone()).await.unwrap();
        let requested = DRIVER_CAPABILITIES.into_iter().map(str::to_owned).collect();
        let plugin = FeaturePlugin {
            manifest: PluginManifest {
                id: "replacement".into(),
                provides: vec![
                    "card.decoder/1".into(),
                    "driver/1".into(),
                    "provider/1".into(),
                ],
                requested_capabilities: requested,
                ..Default::default()
            },
            decoder: decoder("prefix/1", None),
            driver: Arc::new(TestDriver(Mode::Uppercase)),
            provider: Some(Arc::new(LocalProvider::default())),
        };
        let grants = if granted {
            DRIVER_CAPABILITIES.into_iter().map(str::to_owned).collect()
        } else {
            Default::default()
        };
        registry
            .lock()
            .await
            .inject_bound(
                vec![Arc::new(plugin)],
                "replacement",
                &BTreeMap::from([("replacement".into(), grants)]),
                ["card.decoder/1", "driver/1", "provider/1"]
                    .into_iter()
                    .map(|name| (name.into(), "replacement".into()))
                    .collect(),
            )
            .await
            .unwrap();
        let mut product = registry.lock().await.freeze().unwrap().snapshot();
        product["preset"] = json!("tavern");
        let runtime = Runtime::new(store.clone())
            .with_plugins(registry)
            .with_product_composition(product);
        if !granted {
            assert_eq!(
                runtime.import_card(prefix()).await.unwrap_err().code,
                "capability_denied"
            );
            assert!(store.list("card").unwrap().is_empty());
            continue;
        }
        let card = runtime.import_card(prefix()).await.unwrap();
        let session = runtime
            .create_session(
                &target(store.as_ref(), &card),
                None,
                ProviderConfig::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            runtime
                .start(&session.id, "Hello.", None, ProviderConfig::default())
                .await
                .unwrap()
                .wait()
                .await
                .unwrap()
                .draft,
            "HELLO"
        );
    }
}

#[test]
fn decoder_policies_have_distinct_revisions_and_cannot_forge_normalized_records() {
    let first = Card::import_with(&prefix(), decoder("prefix/1", None).as_ref()).unwrap();
    let second = Card::import_with(&prefix(), decoder("prefix/2", None).as_ref()).unwrap();
    assert_ne!(first.id, second.id);
    for field in ["id", "document", "format", "hash", "decoder"] {
        let mut value = serde_json::to_value(&first).unwrap();
        match field {
            "id" => value["id"] = json!("wrong"),
            "document" => value["document"]["data"]["name"] = json!("Wrong"),
            "format" => value["format"] = json!("wrong"),
            "hash" => value["decoder"]["normalized_hash"] = json!("wrong"),
            _ => value["decoder"]["id"] = json!("wrong"),
        }
        assert!(
            serde_json::from_value::<Card>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    let mut invalid = first.document.clone();
    invalid["data"]["name"] = json!(42);
    assert_eq!(
        Card::import_with(&prefix(), decoder("invalid/1", Some(invalid)).as_ref())
            .unwrap_err()
            .code,
        "invalid_card_field"
    );
    assert!(Card::import_with(&prefix(), decoder(CcDecoder.id(), None).as_ref()).is_err());
}

#[test]
fn decoder_input_and_normalized_document_limits_apply_before_storage() {
    let decoder = decoder("prefix/1", None);
    assert_eq!(
        Card::import_with(&vec![0; MAX_INPUT_BYTES + 1], decoder.as_ref())
            .unwrap_err()
            .code,
        "invalid_card_size"
    );
    assert_eq!(decoder.calls.load(Ordering::SeqCst), 0);
    let mut document: Value =
        serde_json::from_slice(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    for part in 0..9 {
        document["data"]["extensions"][format!("part-{part}")] = json!("x".repeat(1024 * 1024 - 1));
    }
    let large = PrefixDecoder {
        id: "large/1",
        calls: Arc::new(AtomicUsize::new(0)),
        document: Some(document),
    };
    assert_eq!(
        Card::import_with(&prefix(), &large).unwrap_err().code,
        "invalid_card_size"
    );
}

#[tokio::test]
async fn a_driver_cannot_forge_success_hide_failure_or_ignore_output_limits() {
    for (mode, events, expected, dispatches) in [
        (
            Mode::Early,
            vec![ProviderEvent::Complete],
            "invalid_driver_completion",
            0,
        ),
        (
            Mode::BeforeDispatch,
            vec![ProviderEvent::Complete],
            "invalid_driver_command",
            0,
        ),
        (
            Mode::Swallow,
            vec![
                ProviderEvent::Usage {
                    usage: json!({"total_tokens":9}),
                },
                ProviderEvent::Failure {
                    error: Error::new("provider_error", "The provider failed."),
                },
            ],
            "provider_error",
            1,
        ),
        (Mode::Swallow, vec![], "incomplete_stream", 1),
        (
            Mode::Oversize,
            vec![
                ProviderEvent::Text {
                    text: "hello".into(),
                },
                ProviderEvent::Complete,
            ],
            "output_limit",
            1,
        ),
    ] {
        let provider = Arc::new(LocalProvider {
            events,
            ..Default::default()
        });
        let (runtime, store, _, session) =
            setup(Arc::new(TestDriver(mode)), provider.clone()).await;
        let generation = runtime
            .start(&session.id, "Hello.", None, ProviderConfig::default())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(generation.status, GenerationStatus::Failed);
        assert_eq!(generation.error.unwrap().code, expected);
        assert_eq!(provider.calls.load(Ordering::SeqCst), dispatches);
        assert_eq!(store.messages(&session.id).unwrap().len(), 1);
        if expected == "provider_error" {
            assert_eq!(generation.usage.unwrap()["total_tokens"], 9);
        }
    }
}

struct BlockingDriver {
    gate: Mutex<std::sync::mpsc::Receiver<()>>,
    entered: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}
#[async_trait]
impl ConversationDriver for BlockingDriver {
    fn id(&self) -> &str {
        "blocking/1"
    }
    async fn run(&self, context: &mut DriverContext) -> Result<()> {
        context.next_event().await?;
        self.entered.store(true, Ordering::SeqCst);
        let _ = self
            .gate
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5));
        let result = context.append_text("LATE_TEXT").await;
        assert!(result.is_err());
        self.finished.store(true, Ordering::SeqCst);
        result
    }
}
#[tokio::test]
async fn cancellation_and_deadlines_close_streams_and_retain_blocked_driver_code() {
    for timeout in [false, true] {
        let (release, gate) = std::sync::mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let driver = Arc::new(BlockingDriver {
            gate: Mutex::new(gate),
            entered: entered.clone(),
            finished: finished.clone(),
        });
        let provider = Arc::new(LocalProvider {
            events: vec![ProviderEvent::Text {
                text: "held".into(),
            }],
            hold: true,
            ..Default::default()
        });
        let (runtime, store, registry, session) = setup(driver, provider.clone()).await;
        let config = ProviderConfig {
            timeout_seconds: if timeout { 1 } else { 10 },
            ..Default::default()
        };
        runtime
            .configure_session(&session, config.clone())
            .await
            .unwrap();
        let mut handle = runtime
            .start(&session.id, "Hello.", None, config)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if !timeout {
            handle.cancel();
        }
        let generation = tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            generation.status,
            if timeout {
                GenerationStatus::Failed
            } else {
                GenerationStatus::Cancelled
            }
        );
        assert_eq!(
            generation.error.unwrap().code,
            if timeout {
                "driver_timeout"
            } else {
                "cancelled"
            }
        );
        assert!(provider.closed.load(Ordering::SeqCst));
        assert_eq!(
            registry
                .lock()
                .await
                .eject_owner("test")
                .await
                .unwrap_err()
                .code,
            "plugin_change_deferred"
        );
        tokio::time::timeout(Duration::from_millis(500), async {
            while handle.events.recv().await.is_some() {}
        })
        .await
        .unwrap();
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !finished.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            loop {
                if registry.lock().await.eject_owner("test").await.is_ok() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(store.generation(&generation.id).unwrap().draft.is_empty());
    }
}

#[tokio::test]
async fn the_host_bounds_driver_event_work_and_accepts_its_exact_boundary() {
    for (count, status) in [
        (MAX_DRIVER_EVENTS - 1, GenerationStatus::Completed),
        (MAX_DRIVER_EVENTS, GenerationStatus::Failed),
    ] {
        let provider = Arc::new(LocalProvider {
            count: Some(count),
            ..Default::default()
        });
        let (runtime, _, _, session) = setup(Arc::new(TestDriver(Mode::Swallow)), provider).await;
        let generation = runtime
            .start(&session.id, "Hello.", None, ProviderConfig::default())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(generation.status, status);
        if status == GenerationStatus::Failed {
            assert_eq!(generation.error.unwrap().code, "output_limit");
        }
    }
}

#[test]
fn version_two_archives_remain_readable_without_decoder_metadata() {
    let export = SessionExport::decode(include_bytes!("fixtures/native/session-v2.json")).unwrap();
    assert_eq!(export.schema_version, 2);
    assert!(export.characters.iter().all(|c| c.decoder.is_none()));
    SessionExport::decode(&export.encode().unwrap()).unwrap();
}
