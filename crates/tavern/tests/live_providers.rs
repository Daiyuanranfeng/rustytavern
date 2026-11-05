//! The live test sends real requests to OpenRouter.
//! Run it with:
//! `OPENROUTER_API_KEY=... cargo test -p rustytavern --test live_providers -- --ignored --nocapture`
//! Set `LIVE_MODELS` to a comma-separated list to replace the default models.

use futures_util::StreamExt;
use rustytavern::{
    card::Card,
    prompt::{ConservativeCounter, PromptInput, PromptOptions, PromptPlan},
    provider::*,
    session::Persona,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const MODELS: &[&str] = &[
    "deepseek/deepseek-v4.1-flash-1017",
    "openai/gpt-6.1-terra",
    "anthropic/claude-haiku-5.5",
    "google/gemini-3.8-flash",
    "x-ai/grok-4.7",
    "meta-llama/llama-4-maverick",
    "mistralai/mistral-small-2603",
    "qwen/qwen3.8-flash",
    "moonshotai/kimi-k3",
    "z-ai/glm-5.3-flash",
    "minimax/minimax-m3.1-flash",
];

struct Key(String);
impl CredentialResolver for Key {
    fn resolve(&self, _: &str) -> rustytavern::Result<String> {
        Ok(self.0.clone())
    }
}

fn plan(message: &str) -> PromptPlan {
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let persona = Persona::new("Alex".into(), "Alex charts the coast.".into()).unwrap();
    rustytavern::prompt::compile(&PromptInput {
        card: &card,
        persona: &persona,
        history: &[],
        message,
        options: &PromptOptions::default(),
        accept_reduced: false,
        counter: &ConservativeCounter,
    })
    .unwrap()
}

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "roll_dice".into(),
        description: "The tool rolls one die and returns the result.".into(),
        parameters: json!({"type":"object","properties":{"sides":{"type":"integer"}},"required":["sides"]}),
    }]
}

/// The function returns the text, the tool calls, and the last error of one stream.
async fn run(
    model: &str,
    message: &str,
    agent: Option<&AgentRequest>,
    key: &Key,
) -> (String, Vec<ToolCall>, Option<String>) {
    let config = ProviderConfig {
        model: model.into(),
        api_key_env: Some("config:live".into()),
        output_limit: 512,
        ..Default::default()
    };
    let mut request = HttpProvider
        .prepare(&plan(message), model, &config)
        .unwrap();
    let stream = match agent {
        Some(agent) => {
            request = HttpProvider.prepare_agent(request, agent).unwrap();
            HttpProvider
                .open_agent_stream(request, key, CancellationToken::new())
                .await
        }
        None => HttpProvider
            .open_stream(request, key, CancellationToken::new())
            .await
            .map(|s| Box::pin(s.map(|e| e.map(Into::into))) as AgentProviderStream),
    };
    let (mut text, mut calls, mut error) = (String::new(), vec![], None);
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(e) => return (text, calls, Some(format!("{e:?}"))),
    };
    let mut complete = false;
    while let Some(event) = stream.next().await {
        match event {
            Ok(AgentProviderEvent::Text { text: t }) => text.push_str(&t),
            Ok(AgentProviderEvent::ToolCalls { calls: c }) => calls = c,
            Ok(AgentProviderEvent::Failure { error: e }) => error = Some(format!("{e:?}")),
            Ok(AgentProviderEvent::Complete) => complete = true,
            Ok(AgentProviderEvent::Usage { .. }) => {}
            Err(e) => error = Some(format!("{e:?}")),
        }
    }
    if !complete && error.is_none() {
        error = Some("The stream has no completion.".into());
    }
    (text, calls, error)
}

#[tokio::test]
#[ignore = "The test sends paid requests to OpenRouter."]
async fn live_models_complete_text_and_tool_conversations() {
    let key = Key(std::env::var("OPENROUTER_API_KEY").expect("Set OPENROUTER_API_KEY."));
    let models: Vec<String> = std::env::var("LIVE_MODELS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|_| MODELS.iter().map(|m| m.to_string()).collect());
    let mut failures = vec![];
    for model in &models {
        let mut check = |case: &str, ok: bool, detail: String| {
            // A rate limit is an upstream condition, not a parser failure.
            let skip = detail.contains("provider_rate_limit");
            let status = if ok {
                "PASS"
            } else if skip {
                "SKIP"
            } else {
                "FAIL"
            };
            println!("{status} {model} {case}: {detail}");
            if !ok && !skip {
                failures.push(format!("{model} {case}: {detail}"));
            }
        };
        let (text, _, error) = run(model, "Greet me in one sentence.", None, &key).await;
        check(
            "text",
            error.is_none() && !text.is_empty(),
            format!("{error:?} {text:.60}"),
        );

        let mut agent = AgentRequest {
            tools: tools(),
            continuation: vec![],
        };
        let (text, calls, error) =
            run(model, "Greet me in one sentence.", Some(&agent), &key).await;
        check(
            "agent-text",
            error.is_none() && (!text.is_empty() || !calls.is_empty()),
            format!("{error:?} {text:.60}"),
        );

        let ask = "Roll a 20-sided die with the roll_dice tool, then tell me the result.";
        let (text, calls, error) = run(model, ask, Some(&agent), &key).await;
        check(
            "agent-call",
            error.is_none() && !calls.is_empty(),
            format!("{error:?} {} calls {text:.60}", calls.len()),
        );
        if calls.is_empty() {
            continue;
        }
        agent.continuation.push(AgentTurn {
            text,
            results: calls
                .iter()
                .map(|c| ToolResult {
                    call_id: c.id.clone(),
                    content: "17".into(),
                })
                .collect(),
            calls,
        });
        let (text, _, error) = run(model, ask, Some(&agent), &key).await;
        check(
            "agent-result",
            error.is_none() && !text.is_empty(),
            format!("{error:?} {text:.60}"),
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
