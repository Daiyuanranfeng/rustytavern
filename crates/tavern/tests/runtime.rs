use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rustytavern::{
    Result,
    card::Card,
    prompt::PromptOptions,
    provider::{ProviderAdapter, ProviderConfig},
    runtime::Runtime,
    session::{GenerationMode, GenerationStatus, Persona, Role, Session},
    store::{SqliteStore, StoreBackend},
};
use serde_json::json;

mod common;
use common::{Script, Step};

/// The provider streams `chunks` text chunks. A delayed provider waits before the second chunk.
fn provider(delayed: bool, chunks: usize) -> Arc<Script> {
    let mut steps = vec![Step::Text("x".into()); chunks];
    if delayed && chunks > 1 {
        steps.insert(1, Step::Delay(Duration::from_secs(5)));
    }
    Script::new(steps).arc()
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
                title: None,
                hidden: Vec::new(),
            },
            None,
        )
        .unwrap();
    let runtime = Runtime::new(store.clone()).with_provider(provider(delayed, chunks));
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
        let provider: Arc<dyn ProviderAdapter> = provider(false, 2);
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

async fn reply(runtime: &Runtime, session: &str, text: &str) {
    let mut handle = runtime
        .start(session, text, None, ProviderConfig::default())
        .await
        .unwrap();
    assert_eq!(
        handle.wait().await.unwrap().status,
        GenerationStatus::Completed
    );
}

#[tokio::test]
async fn a_continuation_keeps_the_original_and_an_impersonation_stores_no_message() {
    let (runtime, store, session) = setup(false, 2).await;
    reply(&runtime, &session, "Hello.").await;
    let original = store.history(&session).unwrap().pop().unwrap();
    let mut handle = runtime
        .continue_reply(&session, ProviderConfig::default())
        .await
        .unwrap();
    let continued = handle.wait().await.unwrap();
    assert_eq!(continued.mode, GenerationMode::Continue);
    assert!(
        continued.snapshot["submitted_text"]
            .as_str()
            .unwrap()
            .contains("Continue your last message")
    );
    let head = store.history(&session).unwrap().pop().unwrap();
    assert_eq!(head.text, "xx xx");
    assert_eq!(head.parent_id, original.parent_id);
    assert!(store.messages(&session).unwrap().contains(&original));
    let count = store.messages(&session).unwrap().len();
    let mut handle = runtime
        .impersonate(&session, ProviderConfig::default())
        .await
        .unwrap();
    let written = handle.wait().await.unwrap();
    assert_eq!(written.status, GenerationStatus::Completed);
    assert_eq!(written.draft, "xx");
    assert!(
        written.snapshot["submitted_text"]
            .as_str()
            .unwrap()
            .contains("next message of Alex")
    );
    assert_eq!(store.messages(&session).unwrap().len(), count);
    assert_eq!(store.session(&session).unwrap().head, Some(head.id));
    let export = store.export_session(&session, true).unwrap();
    assert!(export.encode().is_ok());
    let error = store
        .accept_draft(
            &session,
            &written.id,
            store.session(&session).unwrap().version,
        )
        .unwrap_err();
    assert_eq!(error.code, "invalid_draft_acceptance");
}

#[tokio::test]
async fn hidden_messages_leave_the_prompt_and_forks_copy_the_branch() {
    let (runtime, store, session) = setup(false, 1).await;
    reply(&runtime, &session, "First.").await;
    reply(&runtime, &session, "Second.").await;
    let history = store.history(&session).unwrap();
    let version = store.session(&session).unwrap().version;
    let hidden = store.hide_message(&session, &history[0].id, true).unwrap();
    assert_eq!(hidden.hidden, vec![history[0].id.clone()]);
    assert_eq!(hidden.version, version + 1);
    let request = runtime
        .inspect_request(&session, "Third.", ProviderConfig::default())
        .await
        .unwrap();
    assert!(!request.plan.history_ids.contains(&history[0].id));
    assert!(request.plan.history_ids.contains(&history[1].id));
    let fork = store
        .fork_session(&session, &history.last().unwrap().id)
        .unwrap();
    let copied = store.history(&fork.id).unwrap();
    assert_ne!(fork.id, session);
    assert_eq!(
        copied.iter().map(|m| &m.text).collect::<Vec<_>>(),
        history.iter().map(|m| &m.text).collect::<Vec<_>>()
    );
    assert_eq!(fork.hidden, vec![copied[0].id.clone()]);
    assert!(
        store
            .export_session(&fork.id, false)
            .unwrap()
            .encode()
            .is_ok()
    );
    reply(&runtime, &fork.id, "Third.").await;
    assert_eq!(store.history(&session).unwrap().len(), 4);
    let renamed = store.rename_session(&fork.id, " Harbor ").unwrap();
    assert_eq!(renamed.title.as_deref(), Some("Harbor"));
    assert_eq!(renamed.version, store.session(&fork.id).unwrap().version);
    store.delete_session(&fork.id).unwrap();
    assert_eq!(store.session(&fork.id).unwrap_err().code, "not_found");
    assert_eq!(store.list("session").unwrap().len(), 1);
}
