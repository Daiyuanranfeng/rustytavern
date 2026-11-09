//! The live test sends real requests to OpenRouter.
//! Run it with:
//! `OPENROUTER_API_KEY=... cargo test -p rustytavern --test live_providers -- --ignored --nocapture`
//! Set `LIVE_MODELS` to a comma-separated list to replace the default models.
//! FAIL marks a stream or request error. WARN marks a model that skips a tool or gives no text.

use futures_util::StreamExt;
use rig_agent::agent::MultiTurnStreamItem;
use rig_core::{
    message::ToolName,
    streaming::{Item, StreamEvent},
    tool::{DynamicTool, ToolOutput},
};
use rustytavern::{
    card::Card,
    prompt::{ConservativeCounter, PromptInput, PromptOptions, PromptPlan},
    provider::*,
    session::Persona,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

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

/// The tools copy the schemas of the `web_fetch` and `read_skill` tools and a full MCP server schema.
fn tools() -> Vec<ToolDefinition> {
    [
        ("roll_dice", "Roll one die and return the result.",
            json!({"type":"object","properties":{"sides":{"type":"integer"}},"required":["sides"]})),
        ("web_fetch", "Fetch one public HTTPS page. The tool returns the page as plain text.",
            json!({"type":"object","properties":{"url":{"type":"string","description":"The HTTPS URL."}},"required":["url"],"additionalProperties":false})),
        ("read_skill", "Read one skill from the skill catalog.",
            json!({"type":"object","properties":{"name":{"type":"string","enum":["harbor-lore","tide-tables"]},"path":{"type":"string","description":"A file inside the skill directory. Omit it to read SKILL.md."}},"required":["name"],"additionalProperties":false})),
        ("clock__now", "Return the current UTC time.",
            json!({"type":"object","properties":{},"required":[]})),
        ("notes__search", "Search the keeper's notes.",
            json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,
                "properties":{"query":{"type":"string","default":"storm","format":"plain"},"limit":{"anyOf":[{"type":"integer"},{"type":"null"}]}},
                "required":["query"]})),
    ]
    .into_iter()
    .map(|(name, description, parameters)| ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters,
    })
    .collect()
}

/// The function returns a fixed result for each tool.
fn execute(name: &str, arguments: &Value) -> String {
    match name {
        "roll_dice" => "17".into(),
        "web_fetch" if arguments["url"].as_str().is_some_and(|u| u.ends_with("/a")) => {
            "Page A. The next page is https://example.com/b".into()
        }
        "web_fetch" => format!(
            "Example Domain. {}",
            "The harbor log continues. ".repeat(1200)
        ),
        "read_skill" => "# Harbor lore\nThe harbor has a sunken bell.".into(),
        "clock__now" => "2026-11-05T12:00:00Z".into(),
        "notes__search" => "1. A storm hit the reef in 1888.".into(),
        _ => "The tool is unknown.".into(),
    }
}

/// Each scenario has a name, a prompt, and the tool calls that the prompt needs.
const SCENARIOS: &[(&str, &str, &[&str])] = &[
    ("chat", "Greet me in one sentence.", &[]),
    (
        "dice",
        "Roll a 20-sided die with the roll_dice tool, then tell me the result.",
        &["roll_dice"],
    ),
    (
        "parallel",
        "Roll a 6-sided die and a 20-sided die with roll_dice, then add the results.",
        &["roll_dice", "roll_dice"],
    ),
    (
        "web_fetch",
        "Use web_fetch to read https://example.com and summarize it in one sentence.",
        &["web_fetch"],
    ),
    (
        "two-rounds",
        "Use web_fetch on https://example.com/a. Then fetch the page that it names. Summarize both.",
        &["web_fetch", "web_fetch"],
    ),
    (
        "skill",
        "Read the harbor-lore skill and tell me one fact from it.",
        &["read_skill"],
    ),
    (
        "no-arguments",
        "What time is it now? Use the clock__now tool.",
        &["clock__now"],
    ),
    (
        "mcp",
        "Search the notes for storms and tell me what you find.",
        &["notes__search"],
    ),
];

/// The function runs one scenario to its final text and returns an error or a warning.
/// Rig runs the tool rounds. The tools record their names and return fixed results.
async fn scenario(
    model: &str,
    prompt: &str,
    expected: &[&str],
    key: &Key,
) -> Result<String, String> {
    let config = ProviderConfig {
        model: model.into(),
        api_key_env: Some("config:live".into()),
        output_limit: 512,
        ..Default::default()
    };
    let definitions = if expected.is_empty() { vec![] } else { tools() };
    let request = HttpProvider.prepare(&plan(prompt), model, &config).unwrap();
    let request = HttpProvider.prepare_agent(request, &definitions).unwrap();
    let names = Arc::new(Mutex::new(Vec::<String>::new()));
    let tools = definitions
        .into_iter()
        .map(|tool| {
            let names = names.clone();
            let name = tool.name.clone();
            DynamicTool::new(
                ToolName::new(tool.name).unwrap(),
                tool.description,
                tool.parameters,
                move |arguments| {
                    names.lock().unwrap().push(name.clone());
                    let output = execute(&name, &arguments);
                    Box::pin(async move { Ok(ToolOutput::text(output)) })
                },
            )
        })
        .collect::<Vec<_>>();
    // The runtime repeats a failed first model call once without the web search tool.
    // The test applies the same rule.
    let (mut text, mut complete) = (String::new(), false);
    'run: for web_search in [true, false] {
        let rig = HttpProvider
            .model(&request, key)
            .map_err(|e| format!("{e:?}"))?;
        let runner = agent(&request, rig, tools.clone(), 5, 0, web_search).unwrap();
        let (mut stream, mut answered) = (runner.stream(), false);
        while let Some(item) = stream.next().await {
            let item = match item.map_err(map_error) {
                Err(e)
                    if web_search
                        && !answered
                        && text.is_empty()
                        && search_fallback(&request, &e) =>
                {
                    continue 'run;
                }
                Err(e) if e.code == "tool_step_limit" => {
                    return Ok("The model did not stop after four rounds.".into());
                }
                item => item.map_err(|e| format!("{e:?}"))?,
            };
            match item {
                MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                    text: delta,
                    ..
                })) => text.push_str(&delta),
                MultiTurnStreamItem::CompletionCall(_) => answered = true,
                MultiTurnStreamItem::FinalResponse(_) => complete = true,
                _ => {}
            }
        }
        break;
    }
    if !complete {
        return Err("The stream has no completion.".into());
    }
    let mut names = names.lock().unwrap().clone();
    let mut missing: Vec<&str> = expected.to_vec();
    missing.retain(|n| {
        names
            .iter()
            .position(|c| c == n)
            .map(|i| names.remove(i))
            .is_none()
    });
    Ok(match (text.is_empty(), missing.is_empty()) {
        (true, _) => "The final reply is empty.".into(),
        (false, false) => format!("The model skipped {missing:?}."),
        _ => String::new(),
    })
}

#[tokio::test]
#[ignore = "The test sends paid requests to OpenRouter."]
async fn live_models_complete_text_and_tool_conversations() {
    let key = Key(std::env::var("OPENROUTER_API_KEY").expect("Set OPENROUTER_API_KEY."));
    let models: Vec<String> = std::env::var("LIVE_MODELS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|_| MODELS.iter().map(|m| m.to_string()).collect());
    // The models run in parallel. The scenarios of one model run in sequence.
    let key = &key;
    let reports = futures_util::future::join_all(models.iter().map(|model| async move {
        let mut lines = vec![];
        for (name, prompt, expected) in SCENARIOS {
            let mut result = scenario(model, prompt, expected, key).await;
            // A rate limit is an upstream condition. The test tries again one time.
            if result
                .as_ref()
                .is_err_and(|e| e.contains("provider_rate_limit"))
            {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                result = scenario(model, prompt, expected, key).await;
            }
            lines.push(match result {
                Err(e) if e.contains("provider_rate_limit") => {
                    (false, format!("SKIP {model} {name}: {e}"))
                }
                Err(e) => (true, format!("FAIL {model} {name}: {e}")),
                Ok(w) if w.is_empty() => (false, format!("PASS {model} {name}")),
                Ok(w) => (false, format!("WARN {model} {name}: {w}")),
            });
        }
        lines
    }))
    .await;
    let mut failures = vec![];
    for (failed, line) in reports.into_iter().flatten() {
        println!("{line}");
        if failed {
            failures.push(line);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
