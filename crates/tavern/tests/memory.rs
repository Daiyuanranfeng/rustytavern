//! The tests update the chat memory with a scripted model and read it in the next prompt.

use rustytavern::{
    card::Card,
    prompt::PromptOptions,
    provider::ProviderConfig,
    runtime::{Runtime, memory},
    session::{GenerationStatus, Persona, Session},
    store::{SqliteStore, StoreBackend},
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

mod common;
use common::{Script, text};

#[tokio::test]
async fn the_memory_updates_after_the_interval_and_enters_the_next_prompt() {
    let script = Script {
        turns: vec![
            vec![text("Welcome to the lighthouse.")],
            vec![text(
                "<summary>Alex arrived at the lighthouse.</summary>\n<facts>\n- Alex likes tea.\n</facts>",
            )],
        ],
        ..Default::default()
    }
    .arc();
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
                options: PromptOptions {
                    memory_interval: Some(2),
                    ..Default::default()
                },
                composition: json!({}),
                title: None,
                hidden: Vec::new(),
            },
            None,
        )
        .unwrap();
    let runtime = Runtime::new(store.clone()).with_provider(script.clone());
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    let id = session.id.as_str();
    let config = ProviderConfig::default();
    assert!(!memory::due(store.as_ref(), id).unwrap());
    let mut handle = runtime.start(id, "Hello.", None, config).await.unwrap();
    while handle.events.recv().await.is_some() {}
    assert_eq!(
        handle.wait().await.unwrap().status,
        GenerationStatus::Completed
    );
    assert!(memory::due(store.as_ref(), id).unwrap());

    let token = CancellationToken::new();
    let result = runtime
        .update_memory(id, false, token.clone())
        .await
        .unwrap();
    assert_eq!(result["updated"], true);
    assert_eq!(result["pending"], 0);
    assert_eq!(result["memory"]["facts"], json!(["Alex likes tea."]));
    let sent = script.sent.lock().unwrap()[1].clone();
    assert!(sent.contains("Alex: Hello.") && sent.contains("Welcome to the lighthouse."));
    assert!(!memory::due(store.as_ref(), id).unwrap());
    let again = runtime.update_memory(id, true, token).await.unwrap();
    assert_eq!(again["updated"], false);

    let plan = runtime.inspect(id, "Next.").await.unwrap();
    let position = |id: &str| plan.sections.iter().position(|s| s.id == id);
    let summary = position("plugin:memory:summary").unwrap();
    let facts = position("plugin:memory:facts").unwrap();
    let history = plan
        .sections
        .iter()
        .position(|s| s.source == "history")
        .unwrap();
    assert!(summary < facts && facts < history);
    assert!(plan.sections[summary].text.contains("Alex arrived"));
    assert!(plan.sections[facts].text.contains("- Alex likes tea."));
}
