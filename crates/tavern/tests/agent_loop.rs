//! The tests run the Rig agent loop through the runtime with composed tools.

use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    agent::{AgentDriver, AgentTool, ToolSet},
    card::Card,
    plugin::{PluginContext, PluginManifest, PluginRegistry, RuntimePlugin},
    prompt::PromptOptions,
    provider::{ProviderConfig, ToolDefinition},
    runtime::policy::AgentPolicy,
    runtime::{Runtime, RuntimeEvent},
    session::{Generation, GenerationStatus, Persona, Session},
    store::{SqliteStore, StoreBackend},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

mod common;
use common::{Script, Step};

/// The tool records its arguments. `echo` returns its key, `broken` fails, and `slow` waits for cancellation.
struct Fixture(&'static str, Arc<Mutex<Vec<Value>>>);
#[async_trait]
impl AgentTool for Fixture {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.0.into(),
            description: "The fixture tool.".into(),
            parameters: json!({"type":"object","properties":{"key":{"type":"string"}}}),
        }
    }
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String> {
        self.1.lock().unwrap().push(arguments.clone());
        match self.0 {
            "echo" => Ok(format!(
                "echo:{}",
                arguments["key"].as_str().unwrap_or_default()
            )),
            "slow" => {
                cancellation.cancelled().await;
                Err(Error::new("cancelled", "The caller cancelled the tool."))
            }
            _ => Err(Error::new("tool_failed", "The fixture tool failed.")),
        }
    }
}

struct Tools(PluginManifest, Arc<Mutex<Vec<Value>>>);
#[async_trait]
impl RuntimePlugin for Tools {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let tools: ToolSet = ["echo", "broken", "slow"]
            .into_iter()
            .map(|name| Arc::new(Fixture(name, self.1.clone())) as Arc<dyn AgentTool>)
            .collect();
        context.register("tools.fixture/1", Arc::new(tools))
    }
}

/// The function returns a runtime with the fixture tools, a session ID, and the tool call record.
async fn setup(
    script: Arc<Script>,
    policy: impl Into<AgentPolicy>,
) -> (Runtime, String, Arc<Mutex<Vec<Value>>>) {
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
    let calls = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
    let manifest = PluginManifest {
        id: "fixture-tools".into(),
        provides: vec!["tools.fixture/1".into()],
        ..Default::default()
    };
    registry
        .lock()
        .await
        .inject(
            vec![Arc::new(Tools(manifest, calls.clone()))],
            "test",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    let runtime = Runtime::new(store.clone())
        .with_provider(script)
        .with_driver(Arc::new(AgentDriver::with_policy(policy.into())))
        .with_plugins(registry);
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    (runtime, session.id, calls)
}

/// The function runs one reply and returns the generation and its events.
async fn reply(runtime: &Runtime, session: &str) -> (Generation, Vec<RuntimeEvent>) {
    let mut handle = runtime
        .start(session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = handle.events.recv().await {
        events.push(event);
    }
    (handle.wait().await.unwrap(), events)
}
fn texts(events: &[RuntimeEvent], kind: &str) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.r#type == kind)
        .map(|e| e.text.clone().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn tool_rounds_run_parallel_calls_and_return_tool_errors_to_the_model() {
    let script = Script {
        turns: vec![
            vec![
                Step::Call("c1", "echo", json!({"key":"a"})),
                Step::Call("c2", "echo", json!({"key":"b"})),
            ],
            vec![Step::Call("c3", "broken", json!({}))],
        ],
        steps: vec![common::text("Done.")],
        ..Default::default()
    }
    .arc();
    let (runtime, session, calls) = setup(script.clone(), 8).await;
    let (generation, events) = reply(&runtime, &session).await;
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(generation.draft, "Done.");
    // The calls of one turn run side by side, so the test sorts their records.
    let mut arguments: Vec<String> = calls.lock().unwrap().iter().map(Value::to_string).collect();
    arguments.sort();
    assert_eq!(arguments, [r#"{"key":"a"}"#, r#"{"key":"b"}"#, "{}"]);
    assert_eq!(texts(&events, "tool_call").len(), 3);
    let mut results = texts(&events, "tool_result");
    results.sort();
    assert_eq!(
        results,
        [
            "Error tool_failed: The fixture tool failed.",
            "echo:a",
            "echo:b"
        ]
    );
    // Each later model call receives the results of the earlier tool rounds.
    let sent = script.sent.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert!(sent[1].contains("echo:a") && sent[1].contains("echo:b"));
    assert!(sent[2].contains("Error tool_failed"));
}

#[tokio::test]
async fn the_step_limit_stops_a_model_that_never_stops_calling_tools() {
    let script = Script::new(vec![Step::Call("c", "echo", json!({"key":"a"}))]).arc();
    let (runtime, session, calls) = setup(script.clone(), 2).await;
    let (generation, _) = reply(&runtime, &session).await;
    assert_eq!(generation.status, GenerationStatus::Failed);
    assert_eq!(generation.error.unwrap().code, "tool_step_limit");
    // The run makes two tool rounds and stops before the tools of the third model call run.
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert_eq!(script.sent.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn an_unknown_tool_call_returns_to_the_model_and_runs_no_tool() {
    let script = Script {
        turns: vec![vec![Step::Call("c1", "missing", json!({}))]],
        steps: vec![common::text("Recovered.")],
        ..Default::default()
    }
    .arc();
    let (runtime, session, calls) = setup(script.clone(), 8).await;
    let (generation, _) = reply(&runtime, &session).await;
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(generation.draft, "Recovered.");
    assert!(calls.lock().unwrap().is_empty());
    // Rig tells the model that the tool does not exist.
    let sent = script.sent.lock().unwrap();
    assert!(sent[1].contains("missing"), "{}", sent[1]);
}

#[tokio::test]
async fn cancellation_stops_a_running_tool() {
    let script = Script::new(vec![Step::Call("c1", "slow", json!({}))]).arc();
    let (runtime, session, calls) = setup(script, 8).await;
    let mut handle = runtime
        .start(&session, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap();
    while let Some(event) = handle.events.recv().await {
        if event.r#type == "tool_call" {
            handle.cancel();
        }
    }
    let generation = handle.wait().await.unwrap();
    assert_eq!(generation.status, GenerationStatus::Cancelled);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn approval_runs_an_allowed_call_and_returns_a_declined_call_to_the_model() {
    for allow in [true, false] {
        let script = Script {
            turns: vec![vec![Step::Call("c1", "echo", json!({"key":"a"}))]],
            steps: vec![common::text("Done.")],
            ..Default::default()
        }
        .arc();
        let policy = AgentPolicy {
            max_steps: 8,
            confirm_tools: ["echo".to_owned()].into(),
            ..Default::default()
        };
        let (runtime, session, calls) = setup(script.clone(), policy).await;
        let mut handle = runtime
            .start(&session, "Hello.", None, ProviderConfig::default())
            .await
            .unwrap();
        let mut asked = Vec::new();
        while let Some(event) = handle.events.recv().await {
            if event.r#type == "tool_approval" {
                asked.push(event.text.clone().unwrap());
                assert!(
                    handle
                        .approver
                        .approve(event.call_id.as_deref().unwrap(), allow)
                );
            }
        }
        let generation = handle.wait().await.unwrap();
        assert_eq!(generation.status, GenerationStatus::Completed);
        assert_eq!(asked, [r#"echo {"key":"a"}"#]);
        assert_eq!(calls.lock().unwrap().len(), usize::from(allow));
        let sent = script.sent.lock().unwrap();
        assert_eq!(sent[1].contains("declined"), !allow);
    }
}

#[tokio::test]
async fn a_truncated_reply_repeats_with_a_larger_cap_and_drops_the_partial_text() {
    let script = Script {
        turns: vec![vec![
            common::text("Partial"),
            Step::Finish(rig_core::completion::FinishReason::Length),
        ]],
        steps: vec![common::text("Whole.")],
        ..Default::default()
    }
    .arc();
    let policy = AgentPolicy {
        max_steps: 8,
        retry_truncated: 1,
        ..Default::default()
    };
    let (runtime, session, _) = setup(script.clone(), policy).await;
    let (generation, events) = reply(&runtime, &session).await;
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(generation.draft, "Whole.");
    assert_eq!(texts(&events, "draft_reset"), [""]);
    let sent = script.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].contains("max_tokens: Some(16384)"), "{}", sent[1]);
}

#[tokio::test]
async fn the_token_budget_stops_the_next_model_call() {
    let script = Script {
        turns: vec![vec![Step::Call("c1", "echo", json!({"key":"a"}))]],
        steps: vec![common::text("Done.")],
        total_tokens: Some(10),
        ..Default::default()
    }
    .arc();
    let policy = AgentPolicy {
        max_steps: 8,
        max_tokens: 5,
        ..Default::default()
    };
    let (runtime, session, calls) = setup(script.clone(), policy).await;
    let (generation, _) = reply(&runtime, &session).await;
    assert_eq!(generation.status, GenerationStatus::Failed);
    assert_eq!(generation.error.unwrap().code, "token_budget");
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(script.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_first_openrouter_call_repeats_without_the_web_search_tool() {
    let script = Script {
        turns: vec![vec![Step::Status(502)]],
        steps: vec![common::text("Searched.")],
        ..Default::default()
    }
    .arc();
    let (runtime, session, _) = setup(script.clone(), 8).await;
    let (generation, _) = reply(&runtime, &session).await;
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(generation.draft, "Searched.");
    let sent = script.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(
        sent[0].contains("openrouter:web_search") && !sent[1].contains("openrouter:web_search")
    );
}

#[tokio::test]
async fn parallel_approvals_wait_together_and_waiting_does_not_spend_the_deadline() {
    let script = Script {
        turns: vec![vec![
            Step::Call("c1", "echo", json!({"key":"a"})),
            Step::Call("c2", "echo", json!({"key":"b"})),
        ]],
        steps: vec![common::text("Done.")],
        ..Default::default()
    }
    .arc();
    let policy = AgentPolicy {
        max_steps: 8,
        confirm_tools: ["echo".to_owned()].into(),
        ..Default::default()
    };
    let (runtime, session, calls) = setup(script, policy).await;
    // The deadline is one second. The user answers after two seconds.
    let config = ProviderConfig {
        timeout_seconds: 1,
        ..Default::default()
    };
    let saved = runtime.store.session(&session).unwrap();
    runtime
        .configure_session(&saved, config.clone())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session, "Hello.", None, config)
        .await
        .unwrap();
    let mut asked = 0;
    while let Some(event) = handle.events.recv().await {
        if event.r#type == "tool_approval" {
            asked += 1;
            if asked == 2 {
                assert_eq!(handle.approver.pending().len(), 2);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                for (call, _) in handle.approver.pending() {
                    assert!(handle.approver.approve(&call, true));
                }
            }
        }
    }
    let generation = handle.wait().await.unwrap();
    assert_eq!(
        generation.status,
        GenerationStatus::Completed,
        "{:?}",
        generation.error
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_repeat_does_not_spend_a_tool_step_and_the_larger_cap_ends_with_it() {
    let script = Script {
        turns: vec![
            vec![
                common::text("Cut"),
                Step::Finish(rig_core::completion::FinishReason::Length),
            ],
            vec![Step::Call("c1", "echo", json!({"key":"a"}))],
        ],
        steps: vec![common::text("Done.")],
        total_tokens: Some(10),
        ..Default::default()
    }
    .arc();
    let policy = AgentPolicy {
        max_steps: 1,
        retry_truncated: 1,
        ..Default::default()
    };
    let (runtime, session, calls) = setup(script.clone(), policy).await;
    let (generation, _) = reply(&runtime, &session).await;
    assert_eq!(
        generation.status,
        GenerationStatus::Completed,
        "{:?}",
        generation.error
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    // The usage of the reply sums its three model calls.
    assert_eq!(generation.usage.unwrap()["total_tokens"], 30);
    let sent = script.sent.lock().unwrap();
    assert!(sent[1].contains("max_tokens: Some(16384)"));
    assert!(sent[2].contains("max_tokens: Some(8192)"), "{}", sent[2]);
}

#[test]
fn the_agent_table_keeps_accepting_unknown_keys() {
    let policy: AgentPolicy = serde_json::from_value(json!({"max_steps":3,"future":true})).unwrap();
    assert_eq!(policy.max_steps, 3);
    let empty: AgentPolicy = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty.max_steps, rustytavern::agent::DEFAULT_MAX_STEPS);
}
