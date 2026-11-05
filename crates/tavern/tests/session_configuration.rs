use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::StreamExt;
use rustytavern::{
    Result,
    card::Card,
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
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct LocalProvider {
    text: &'static str,
    hold: bool,
    calls: Arc<AtomicUsize>,
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
        let text = futures_util::stream::once(std::future::ready(Ok(ProviderEvent::Text {
            text: self.text.into(),
        })));
        Ok(if self.hold {
            Box::pin(text.chain(futures_util::stream::pending()))
        } else {
            Box::pin(text.chain(futures_util::stream::once(std::future::ready(Ok(
                ProviderEvent::Complete,
            )))))
        })
    }
}
fn provider(text: &'static str, hold: bool) -> Arc<LocalProvider> {
    Arc::new(LocalProvider {
        text,
        hold,
        calls: Arc::new(AtomicUsize::new(0)),
    })
}
fn target(store: &dyn StoreBackend) -> Session {
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let persona = Persona::new("Alex".into(), "A visitor.".into()).unwrap();
    store.save_card(&card).unwrap();
    store.save_persona(&persona).unwrap();
    Session {
        id: String::new(),
        character_id: card.id,
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
async fn setup() -> (Runtime, Arc<SqliteStore>, Session) {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let runtime = Runtime::new(store.clone()).with_provider(provider("Original.", false));
    let session = runtime
        .create_session(
            &target(store.as_ref()),
            Some("Hello, {{user}}."),
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    (runtime, store, session)
}
struct Replacement {
    manifest: PluginManifest,
}
#[async_trait]
impl RuntimePlugin for Replacement {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let provider: Arc<dyn ProviderAdapter> = provider("Replacement.", false);
        context.register("provider/1", Arc::new(provider))
    }
}

#[tokio::test]
async fn provider_settings_require_a_revision_and_old_snapshots_remain_immutable() {
    let (runtime, store, session) = setup().await;
    let first = runtime
        .start(&session.id, "First.", None, ProviderConfig::default())
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let original = first.snapshot.clone();
    let config = ProviderConfig {
        parameters: BTreeMap::from([("temperature".into(), json!(0.7))]),
        ..Default::default()
    };
    assert_eq!(
        runtime
            .start(&session.id, "Second.", None, config.clone())
            .await
            .err()
            .unwrap()
            .code,
        "session_configuration_changed"
    );
    assert_eq!(store.generations(&session.id).unwrap().len(), 1);
    let changed = runtime
        .configure_session(&store.session(&session.id).unwrap(), config.clone())
        .await
        .unwrap();
    assert_eq!(changed.configurations().unwrap().len(), 2);
    assert_eq!(changed.configuration().unwrap().revision, 2);
    assert_eq!(
        serde_json::to_value(runtime.provider_config(&session.id).await.unwrap()).unwrap(),
        serde_json::to_value(&config).unwrap()
    );
    let second = runtime
        .start(&session.id, "Second.", None, config)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(second.status, GenerationStatus::Completed);
    runtime.inspect(&session.id, "Next.").await.unwrap();
    assert_eq!(store.generation(&first.id).unwrap().snapshot, original);
    assert_eq!(
        original["session"]["composition"]["session_configurations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        second.snapshot["session"]["composition"]["session_configurations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn changed_and_missing_plugins_block_generation_until_explicit_adoption() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let registry = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
    let runtime = Runtime::new(store.clone())
        .with_provider(provider("Original.", false))
        .with_plugins(registry.clone());
    let session = runtime
        .create_session(&target(store.as_ref()), None, ProviderConfig::default())
        .await
        .unwrap();
    let plugin = Replacement {
        manifest: PluginManifest {
            id: "replacement".into(),
            provides: vec!["provider/1".into()],
            artifact_hash: rustytavern::digest(b"replacement"),
            configuration_hash: rustytavern::digest(b"configuration"),
            ..Default::default()
        },
    };
    registry
        .lock()
        .await
        .inject(vec![Arc::new(plugin)], "test", &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(
        runtime
            .start(&session.id, "Hello.", None, ProviderConfig::default())
            .await
            .err()
            .unwrap()
            .code,
        "pinned_composition_unavailable"
    );
    assert!(store.generations(&session.id).unwrap().is_empty());
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    let generated = runtime
        .start(&session.id, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(generated.draft, "Replacement.");
    registry.lock().await.eject_owner("test").await.unwrap();
    assert_eq!(
        runtime
            .inspect(&session.id, "Next.")
            .await
            .unwrap_err()
            .code,
        "pinned_composition_unavailable"
    );
    SessionExport::decode(
        &store
            .export_session(&session.id, true)
            .unwrap()
            .encode()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        store.history(&session.id).unwrap().last().unwrap().text,
        "Replacement."
    );
    runtime
        .configure_session(
            &store.session(&session.id).unwrap(),
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .start(&session.id, "Next.", None, ProviderConfig::default())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap()
            .draft,
        "Original."
    );
}

#[tokio::test]
async fn extension_package_and_asset_changes_require_adoption() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let registry = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
    let lock = registry.lock().await.freeze().unwrap().snapshot();
    let mut initial = lock.clone();
    initial["extensions"] = json!([{"id":"theme","version":"1.0.0","package_hash":"first","assets":[{"path":"theme.txt","sha256":"asset-first"}]}]);
    let runtime = Runtime::new(store.clone())
        .with_provider(provider("Reply.", false))
        .with_plugins(registry.clone())
        .with_product_composition(initial.clone());
    let session = runtime
        .create_session(&target(store.as_ref()), None, ProviderConfig::default())
        .await
        .unwrap();
    for key in ["package_hash", "assets"] {
        let mut update = initial.clone();
        update["extensions"][0][key] = if key == "assets" {
            json!([{"path":"theme.txt","sha256":"asset-second"}])
        } else {
            json!("second")
        };
        let changed = Runtime::new(store.clone())
            .with_provider(provider("Reply.", false))
            .with_plugins(registry.clone())
            .with_product_composition(update);
        assert_eq!(
            changed
                .inspect(&session.id, "Hello.")
                .await
                .unwrap_err()
                .code,
            "pinned_composition_unavailable"
        );
    }
    assert_eq!(store.session(&session.id).unwrap().version, session.version);
}

#[tokio::test]
async fn configuration_changes_use_compare_and_swap_and_reject_an_active_writer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite3");
    let first = Arc::new(SqliteStore::open(&path).unwrap());
    let second = Arc::new(SqliteStore::open(&path).unwrap());
    let runtime = Runtime::new(first.clone()).with_provider(provider("Held.", true));
    let other = Runtime::new(second.clone()).with_provider(provider("Held.", true));
    let session = runtime
        .create_session(&target(first.as_ref()), None, ProviderConfig::default())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        runtime.configure_session(&session, ProviderConfig::default()),
        other.configure_session(&session, ProviderConfig::default())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        a.err().or_else(|| b.err()).unwrap().code,
        "session_conflict"
    );
    let mut handle = runtime
        .start(&session.id, "Wait.", None, ProviderConfig::default())
        .await
        .unwrap();
    let current = first.session(&session.id).unwrap();
    assert_eq!(
        other
            .configure_session(&current, ProviderConfig::default())
            .await
            .unwrap_err()
            .code,
        "session_conflict"
    );
    assert_eq!(
        first
            .session(&session.id)
            .unwrap()
            .configurations()
            .unwrap()
            .len(),
        2
    );
    handle.cancel();
    assert_eq!(
        handle.wait().await.unwrap().status,
        GenerationStatus::Cancelled
    );
}

#[tokio::test]
async fn rebinds_keep_configuration_archives_even_without_generations() {
    let (_, store, session) = setup().await;
    let mut document = store.card(&session.character_id).unwrap().document;
    document["data"]["name"] = json!("Beacon");
    let card = Card::import(&serde_json::to_vec(&document).unwrap()).unwrap();
    let persona = Persona::new("Robin".into(), "Another visitor.".into()).unwrap();
    store.save_card(&card).unwrap();
    store.save_persona(&persona).unwrap();
    let revised = store
        .rebind(&session.id, Some(&card.id), Some(&persona.id), false)
        .unwrap();
    assert_eq!(revised.configuration().unwrap().revision, 2);
    assert_eq!(store.history(&session.id).unwrap()[0].text, "Hello, Alex.");
    let export = store.export_session(&session.id, false).unwrap();
    assert_eq!(export.characters.len(), 2);
    assert_eq!(export.personas.len(), 2);
    assert!(export.generations.is_empty());
    SessionExport::decode(&export.encode().unwrap()).unwrap();
}

#[tokio::test]
async fn legacy_sessions_require_configuration_and_invalid_settings_never_enter_history() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let session = store
        .create_session(&target(store.as_ref()), Some("A stored greeting."))
        .unwrap();
    let runtime = Runtime::new(store.clone()).with_provider(provider("Reply.", false));
    assert_eq!(
        runtime
            .start(&session.id, "Hello.", None, ProviderConfig::default())
            .await
            .err()
            .unwrap()
            .code,
        "session_configuration_required"
    );
    assert_eq!(
        store.history(&session.id).unwrap()[0].text,
        "A stored greeting."
    );
    let invalid = ProviderConfig {
        parameters: BTreeMap::from([("api_key".into(), json!("PRIVATE_CONFIG_SENTINEL"))]),
        ..Default::default()
    };
    assert_eq!(
        runtime
            .configure_session(&session, invalid)
            .await
            .unwrap_err()
            .code,
        "unsupported_provider_parameter"
    );
    assert!(
        store
            .session(&session.id)
            .unwrap()
            .configurations()
            .unwrap()
            .is_empty()
    );
    assert!(
        !String::from_utf8(
            store
                .export_session(&session.id, true)
                .unwrap()
                .encode()
                .unwrap()
        )
        .unwrap()
        .contains("PRIVATE_CONFIG_SENTINEL")
    );
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    runtime.inspect(&session.id, "Hello.").await.unwrap();
}

#[tokio::test]
async fn configuration_history_limits_roll_back_without_losing_previous_revisions() {
    let (runtime, store, session) = setup().await;
    let oversized = runtime.clone().with_product_composition(
        json!({"extensions":[{"id":"large","asset_metadata":"x".repeat(8 * 1024 * 1024)}]}),
    );
    assert_eq!(
        oversized
            .create_session(&target(store.as_ref()), None, ProviderConfig::default())
            .await
            .unwrap_err()
            .code,
        "session_configuration_limit"
    );
    assert_eq!(store.list("session").unwrap().len(), 1);
    for _ in 1..128 {
        runtime
            .configure_session(
                &store.session(&session.id).unwrap(),
                ProviderConfig::default(),
            )
            .await
            .unwrap();
    }
    let before = store.session(&session.id).unwrap();
    assert_eq!(
        runtime
            .configure_session(&before, ProviderConfig::default())
            .await
            .unwrap_err()
            .code,
        "session_configuration_limit"
    );
    assert_eq!(
        serde_json::to_value(store.session(&session.id).unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[tokio::test]
async fn malformed_and_future_configuration_records_block_mutation_without_hiding_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite3");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let runtime = Runtime::new(store.clone()).with_provider(provider("Reply.", false));
    let session = runtime
        .create_session(
            &target(store.as_ref()),
            Some("Keep this greeting."),
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    let saved = serde_json::to_value(&session).unwrap();
    for (field, code) in [
        ("hash", "invalid_session_configuration"),
        ("root", "invalid_session_configuration"),
        ("future", "unsupported_session_configuration"),
    ] {
        let mut corrupt = saved.clone();
        match field {
            "hash" => {
                corrupt["composition"]["session_configurations"][0]["identity"] = json!("wrong")
            }
            "root" => corrupt["model"] = json!("wrong"),
            _ => corrupt["composition"]["configuration_schema"] = json!(99),
        };
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE sessions SET payload=?2 WHERE id=?1",
                rusqlite::params![session.id, corrupt.to_string()],
            )
            .unwrap();
        let current = store.session(&session.id).unwrap();
        assert_eq!(
            runtime
                .configure_session(&current, ProviderConfig::default())
                .await
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(
            store.history(&session.id).unwrap()[0].text,
            "Keep this greeting."
        );
        assert!(store.generations(&session.id).unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_running_generation_keeps_its_registry_after_the_caller_releases_it() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let registry = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
    let weak = Arc::downgrade(&registry);
    let runtime = Runtime::new(store.clone())
        .with_provider(provider("Held.", true))
        .with_plugins(registry.clone());
    let session = runtime
        .create_session(&target(store.as_ref()), None, ProviderConfig::default())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session.id, "Wait.", None, ProviderConfig::default())
        .await
        .unwrap();
    while handle.events.recv().await.unwrap().r#type != "text_delta" {}
    drop(runtime);
    drop(registry);
    assert!(weak.upgrade().is_some());
    handle.cancel();
    handle.wait().await.unwrap();
    drop(handle);
    assert!(weak.upgrade().is_none());
}
