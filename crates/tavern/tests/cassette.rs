//! The test replays one recorded OpenRouter tool round through the Rig cassette.
//! It sends no network request. Record it again with:
//! `RECORD=1 OPENROUTER_API_KEY=... cargo test -p rustytavern --test cassette`

use futures_util::StreamExt;
use rig_agent::agent::MultiTurnStreamItem;
use rig_cassette::http::{CassetteMode, CassetteSpec, ProviderCassette, RecordVia};
use rig_core::{
    message::ToolName,
    streaming::{Item, StreamEvent},
    tool::{DynamicTool, ToolOutput},
};
use rustytavern::{
    prompt::{PromptPlan, Section},
    provider::{
        CredentialResolver, HttpProvider, ProviderAdapter, ProviderConfig, ToolDefinition, agent,
    },
    session::Role,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const MODEL: &str = "deepseek/deepseek-v4.1-flash-1017";

struct Key(String);
impl CredentialResolver for Key {
    fn resolve(&self, _: &str) -> rustytavern::Result<String> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn a_recorded_openrouter_tool_round_replays_offline() {
    let record = std::env::var("RECORD").is_ok();
    let mode = if record {
        CassetteMode::Record
    } else {
        CassetteMode::Replay
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cassettes/openrouter-tool-round.yaml"
    );
    let spec = CassetteSpec::new("openrouter-tool-round");
    let cassette = ProviderCassette::try_start_at(
        RecordVia::Proxy,
        "openrouter",
        spec,
        "https://openrouter.ai/api/v1",
        mode,
        path.into(),
    )
    .await
    .unwrap();
    let key = Key(cassette.api_key("OPENROUTER_API_KEY"));
    let config = ProviderConfig {
        model: MODEL.into(),
        endpoint: Some(format!("{}/chat/completions", cassette.base_url())),
        api_key_env: Some("config:live".into()),
        output_limit: 256,
        ..Default::default()
    };
    let plan = PromptPlan {
        sections: vec![Section {
            id: "message".into(),
            role: Role::User,
            text: "Call the lookup tool with the key a. Then reply with the value that it returns."
                .into(),
            source: "test".into(),
            mandatory: true,
            units: 20,
        }],
        output_reserve: 256,
        ..Default::default()
    };
    let parameters =
        json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]});
    let definition = ToolDefinition {
        name: "lookup".into(),
        description: "The tool returns the value of a key.".into(),
        parameters: parameters.clone(),
    };
    let request = HttpProvider.prepare(&plan, MODEL, &config).unwrap();
    let request = HttpProvider.prepare_agent(request, &[definition]).unwrap();
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let record_calls = calls.clone();
    let lookup = DynamicTool::new(
        ToolName::new("lookup").unwrap(),
        "The tool returns the value of a key.",
        parameters,
        move |arguments| {
            record_calls.lock().unwrap().push(arguments);
            Box::pin(async { Ok(ToolOutput::text("VALUE_A")) })
        },
    );
    let model = HttpProvider.model(&request, &key).unwrap();
    let mut stream = agent(&request, model, vec![lookup], 3, 0, true)
        .unwrap()
        .stream();
    let (mut text, mut complete) = (String::new(), false);
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                text: delta,
                ..
            })) => text.push_str(&delta),
            MultiTurnStreamItem::FinalResponse(_) => complete = true,
            _ => {}
        }
    }
    cassette.try_finish().await.unwrap();
    assert!(complete);
    assert_eq!(*calls.lock().unwrap(), [json!({"key":"a"})]);
    assert!(text.contains("VALUE_A"), "{text}");
}
