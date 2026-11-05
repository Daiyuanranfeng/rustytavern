use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rustytavern::{
    Result,
    card::Card,
    prompt::PromptOptions,
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderStream,
    },
    runtime::Runtime,
    session::{GenerationStatus, Persona, Role, Session},
    store::{SqliteStore, StoreBackend},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct MockProvider {
    delayed: bool,
    chunks: usize,
}
#[async_trait]
impl ProviderAdapter for MockProvider {
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
        _: PreparedRequest,
        _: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        let delayed = self.delayed;
        let total = self.chunks;
        Ok(Box::pin(futures_util::stream::unfold(
            0,
            move |index| async move {
                if index > total {
                    return None;
                }
                if delayed && index == 1 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                let event = if index == total {
                    ProviderEvent::Complete
                } else {
                    ProviderEvent::Text { text: "x".into() }
                };
                Some((Ok(event), index + 1))
            },
        )))
    }
}
async fn setup(delayed: bool, chunks: usize) -> (Runtime, Arc<SqliteStore>, String) {
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
                provider: "mock".into(),
                model: "mock".into(),
                head: None,
                version: 0,
                accept_reduced: false,
                options: PromptOptions::default(),
                composition: json!({}),
            },
            None,
        )
        .unwrap();
    let runtime =
        Runtime::new(store.clone()).with_provider(Arc::new(MockProvider { delayed, chunks }));
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    (runtime, store, session.id)
}
#[tokio::test]
async fn cancellation_keeps_the_acknowledged_draft_and_rejects_late_text() {
    let (runtime, store, session) = setup(true, 2).await;
    let mut handle = runtime
        .start(&session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    while let Some(event) = handle.events.recv().await {
        if event.r#type == "text_delta" {
            assert_eq!(store.generation(&handle.generation_id).unwrap().draft, "x");
            break;
        }
    }
    let timed_out = tokio::time::timeout(Duration::from_millis(1), handle.wait()).await;
    assert!(timed_out.is_err());
    handle.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), handle.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, GenerationStatus::Cancelled);
    assert_eq!(result.draft, "x");
    let history = store.history(&session).unwrap();
    assert!(history.iter().all(|m| m.role != Role::Assistant));
}
#[tokio::test]
async fn dropping_a_generation_handle_cancels_its_work() {
    let (runtime, store, session) = setup(true, 2).await;
    let handle = runtime
        .start(&session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    let id = handle.generation_id.clone();
    drop(handle);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if store.generation(&id).unwrap().status.terminal() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        store.generation(&id).unwrap().status,
        GenerationStatus::Cancelled
    );
}
#[tokio::test]
async fn a_slow_observer_cannot_block_a_durable_completion() {
    let (runtime, store, session) = setup(false, 128).await;
    let mut handle = runtime
        .start(&session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), handle.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.status, GenerationStatus::Completed);
    assert_eq!(result.draft.len(), 128);
    assert_eq!(
        store.history(&session).unwrap().last().unwrap().text.len(),
        128
    );
}
#[tokio::test]
async fn a_repeated_submission_key_does_not_call_the_provider_again() {
    let (runtime, store, session) = setup(false, 1).await;
    let mut first = runtime
        .start(&session, "Hello.", Some("same"), ProviderConfig::default())
        .await
        .unwrap();
    let result = first.wait().await.unwrap();
    let mut repeated = runtime
        .start(&session, "Hello.", Some("same"), ProviderConfig::default())
        .await
        .unwrap();
    let again = repeated.wait().await.unwrap();
    assert_eq!(result.id, again.id);
    assert_eq!(store.messages(&session).unwrap().len(), 2);
    assert_eq!(
        runtime
            .start(
                &session,
                "Different.",
                Some("same"),
                ProviderConfig::default()
            )
            .await
            .err()
            .unwrap()
            .code,
        "invalid_idempotency_key"
    );
}

struct ReplacementPlugin {
    manifest: rustytavern::plugin::PluginManifest,
}
#[async_trait]
impl rustytavern::plugin::RuntimePlugin for ReplacementPlugin {
    fn manifest(&self) -> &rustytavern::plugin::PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut rustytavern::plugin::PluginContext) -> Result<()> {
        let provider: Arc<dyn ProviderAdapter> = Arc::new(MockProvider {
            delayed: false,
            chunks: 2,
        });
        context.register("provider/1", Arc::new(provider))
    }
}

#[tokio::test]
async fn injecting_and_ejecting_a_plugin_changes_the_actual_runtime() {
    let (runtime, store, session) = setup(false, 1).await;
    let registry = Arc::new(tokio::sync::Mutex::new(
        rustytavern::plugin::PluginRegistry::default(),
    ));
    let plugin = ReplacementPlugin {
        manifest: rustytavern::plugin::PluginManifest {
            id: "replacement".into(),
            provides: vec!["provider/1".into()],
            ..Default::default()
        },
    };
    registry
        .lock()
        .await
        .inject(
            vec![Arc::new(plugin)],
            "caller",
            &std::collections::BTreeMap::new(),
        )
        .await
        .unwrap();
    let runtime = runtime.with_plugins(registry.clone());
    runtime
        .configure_session(&store.session(&session).unwrap(), ProviderConfig::default())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session, "First.", None, ProviderConfig::default())
        .await
        .unwrap();
    let replaced = handle.wait().await.unwrap();
    assert_eq!(replaced.draft, "xx");
    assert_eq!(
        store.generation(&replaced.id).unwrap().snapshot["plugin_composition"]["plugins"][0]["manifest"]
            ["id"],
        "replacement"
    );
    registry.lock().await.eject_owner("caller").await.unwrap();
    runtime
        .configure_session(&store.session(&session).unwrap(), ProviderConfig::default())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session, "Next.", None, ProviderConfig::default())
        .await
        .unwrap();
    assert_eq!(handle.wait().await.unwrap().draft, "x");
}
