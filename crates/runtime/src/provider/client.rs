//! The module builds the Rig model and the Rig agent run for a prepared request.
//! Rig maps the chat body to each wire protocol and parses each stream.
//! The module maps the Rig errors to the provider contract.

use super::*;
use rig_agent::{Agent, AgentBuilder, AgentRunner, completion::PromptError};
use rig_core::{
    completion::{CacheRetention, CompletionRequest, Effort, GenerationOptions, Reasoning},
    error::ErrorReport,
    message::Message,
    providers::{anthropic::AnthropicConfig, gemini::GeminiConfig, openai},
    tool::DynamicTool,
};
use serde_json::json;
use std::time::Duration;

/// A custom endpoint speaks plain Chat Completions, without OpenRouter fields.
/// It sends a key only when the profile has one, because a local server can reject the header.
pub(super) const CUSTOM: openai::wire::Dialect = {
    let mut dialect = openai::wire::Dialect::gateway("custom", "", "");
    dialect.quirks.auth = openai::wire::Auth::OptionalBearer;
    dialect
};

/// The number of tool calls of one turn that run at the same time.
const TOOL_CONCURRENCY: usize = 4;

/// The function returns the API root that Rig adds its route path to.
pub(super) fn base_url(request: &PreparedRequest) -> String {
    let endpoint = request.endpoint.trim_end_matches('/');
    let suffixes: &[&str] = match request.protocol {
        ProviderKind::Gemini => return endpoint.split("/v1beta").next().unwrap_or(endpoint).into(),
        ProviderKind::Anthropic => &[],
        _ => &["/chat/completions", "/responses"],
    };
    suffixes
        .iter()
        .find_map(|suffix| endpoint.strip_suffix(suffix))
        .unwrap_or(endpoint)
        .to_owned()
}

/// The function resolves the key and builds the transport without redirects and the API root.
/// A request without a credential reference sends no key.
pub(super) fn transport(
    request: &PreparedRequest,
    credentials: &dyn CredentialResolver,
) -> Result<(String, rig_reqwest::ReqwestClient, String)> {
    let key = match &request.credential_reference {
        Some(reference) => credentials.resolve(reference)?,
        None => String::new(),
    };
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(request.timeout_seconds))
        .build()
        .map_err(Error::context("provider_transport", "Provider failed."))?;
    Ok((key, http.into(), base_url(request)))
}

/// The function builds the Rig model on the transport of the request.
pub(super) fn model(
    request: &PreparedRequest,
    credentials: &dyn CredentialResolver,
) -> Result<CompletionModel> {
    if request.credential_reference.is_none() && request.protocol != ProviderKind::Custom {
        return Err(Error::new(
            "configuration",
            "This provider requires a credential reference.",
        ));
    }
    let (key, http, base) = transport(request, credentials)?;
    let model = request.chat()?.model;
    let dialect = match request.protocol {
        ProviderKind::OpenRouter => &openai::wire::OPENROUTER,
        _ => &CUSTOM,
    };
    Ok(match request.protocol {
        ProviderKind::OpenRouter | ProviderKind::Custom => {
            openai::OpenAIConfig::with_key(dialect, key)
                .with_base_url(base)
                .connect(http)
                .chat(model)
                .into()
        }
        ProviderKind::OpenAi => openai::OpenAIConfig::new(key)
            .with_base_url(base)
            .connect(http)
            .responses(model)
            .into(),
        ProviderKind::Anthropic => AnthropicConfig::new(key)
            .with_base_url(base)
            .connect(http)
            .completion(model)
            .into(),
        ProviderKind::Gemini => GeminiConfig::new(key)
            .with_base_url(base)
            .connect(http)
            .completion(model)
            .into(),
    })
}

/// The function builds the Rig agent run for a prepared request.
/// Each vendor protocol gets its native web search tool. A custom endpoint gets none.
pub fn agent(
    request: &PreparedRequest,
    model: CompletionModel,
    tools: Vec<DynamicTool>,
    max_turns: usize,
    retry_truncated: usize,
    web_search: bool,
) -> Result<AgentRunner> {
    let mut body = request.chat()?;
    let catalog = rig_core::catalog::Catalog::builtin();
    let spec = super::catalog::spec(catalog, request.protocol, &body.model);
    let ceiling = spec.and_then(|spec| spec.max_output_tokens).map(u64::from);
    // A model that refuses a prefill, such as Claude 4.6, gets the request without it.
    if spec.is_some_and(|spec| !spec.compat.prefill)
        && matches!(body.messages.last(), Some(ChatMessage::Assistant { .. }))
    {
        body.messages.pop();
    }
    let mut messages: Vec<Message> = std::mem::take(&mut body.messages)
        .into_iter()
        .map(|message| match message {
            ChatMessage::System { content } => Message::system(content),
            ChatMessage::User { content } => Message::user(content),
            ChatMessage::Assistant { content } => Message::assistant(content),
        })
        .collect();
    let prompt = messages.pop().ok_or_else(|| {
        Error::new(
            "invalid_provider_request",
            "The prepared request has no messages.",
        )
    })?;
    let mut extra = serde_json::Map::new();
    let search = match request.protocol {
        ProviderKind::OpenRouter => Some(json!({"type":"openrouter:web_search"})),
        ProviderKind::OpenAi => Some(json!({"type":"web_search"})),
        ProviderKind::Anthropic => Some(json!({"type":"web_search_20250305","name":"web_search"})),
        ProviderKind::Gemini => Some(json!({"google_search":{}})),
        ProviderKind::Custom => None,
    };
    if let Some(search) = search.filter(|_| web_search) {
        extra.insert("tools".into(), json!([search]));
    }
    if let Some(routing) = &body.provider {
        extra.insert("provider".into(), json!(routing));
    }
    let mut builder = AgentBuilder::new(model)
        .without_preamble()
        .options(options(&body))
        .max_tokens(u64::try_from(body.max_tokens).unwrap_or(u64::MAX))
        .accept_unknown_finish_reasons(true)
        .dynamic_tools(tools);
    if let Some(temperature) = body.temperature {
        builder = builder.temperature(temperature);
    }
    if !extra.is_empty() {
        builder = builder.additional_params(Value::Object(extra));
    }
    let agent: Agent = builder.build();
    // Rig runs the tool calls of one turn side by side.
    Ok(agent
        .prompt(prompt)
        .history(messages)
        .max_turns(max_turns)
        .tool_concurrency(TOOL_CONCURRENCY)
        .add_hook(crate::policy::FinishHook {
            retry_truncated,
            ceiling,
        }))
}

/// The function maps the checked parameters of the body to Rig generation options.
fn options(body: &ChatBody) -> GenerationOptions {
    let mut options = GenerationOptions::new();
    if let Some(top_p) = body.top_p {
        options = options.top_p(top_p);
    }
    if !body.stop.is_empty() {
        options = options.stop(body.stop.clone());
    }
    let effort = |level: &str| match level {
        "minimal" => Some(Effort::Minimal),
        "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "xhigh" => Some(Effort::XHigh),
        "max" => Some(Effort::Max),
        _ => None,
    };
    let reasoning = match &body.reasoning {
        Some(Value::String(level)) if level == "off" => Some(Reasoning::Off),
        Some(Value::String(level)) => effort(level).map(Reasoning::Effort),
        Some(value) => value.as_u64().map(|tokens| Reasoning::Budget {
            tokens: u32::try_from(tokens).unwrap_or(u32::MAX),
        }),
        None => None,
    };
    if let Some(reasoning) = reasoning {
        options = options.reasoning(reasoning);
    }
    let cache = match body.cache.as_deref() {
        Some("none") => Some(CacheRetention::None),
        Some("short") => Some(CacheRetention::Short),
        Some("long") => Some(CacheRetention::Long),
        _ => None,
    };
    if let Some(cache) = cache {
        options = options.cache(cache);
    }
    options
}

/// The resolver supplies a placeholder key. A check sends nothing.
struct CheckKey;
impl CredentialResolver for CheckKey {
    fn resolve(&self, _: &str) -> Result<String> {
        Ok("check".into())
    }
}

/// The function asks Rig which options of the request the protocol and the model refuse.
/// Rig checks the model against its catalog. Nothing goes over the network.
pub(super) fn check(request: &PreparedRequest) -> Result<()> {
    let body = request.chat()?;
    let completion = CompletionRequest::new("check")
        .temperature(body.temperature)
        .max_tokens(u64::try_from(body.max_tokens).ok())
        .options(options(&body));
    let mut request = request.clone();
    request
        .credential_reference
        .get_or_insert_with(|| "check".into());
    model(&request, &CheckKey)?
        .check(&completion)
        .map_err(|error| {
            Error::new(
                "unsupported_provider_parameter",
                format!("The model refuses a parameter: {error}"),
            )
        })
}

/// The function reports whether a failed first model call can repeat without the web search tool.
/// Some OpenRouter endpoints return an invalid upstream response when the request carries
/// `openrouter:web_search` with function tools and system messages.
pub fn search_fallback(request: &PreparedRequest, error: &Error) -> bool {
    request.protocol == ProviderKind::OpenRouter && error.code == "provider_unavailable"
}

/// The function reads the provider web searches of one model call from its raw response.
/// Each entry holds the call text and the result text of one search.
/// OpenRouter reports only a search count and the cited pages. It gives one entry.
pub fn web_searches(raw: &Value) -> Vec<(String, String)> {
    fn all(value: &Value) -> Vec<&Value> {
        value.as_array().into_iter().flatten().collect()
    }
    let pages = |pages: Vec<&Value>| {
        let mut list: Vec<String> = Vec::new();
        for page in pages {
            let Some(url) = page["url"].as_str().or(page["uri"].as_str()) else {
                continue;
            };
            let line = match page["title"].as_str() {
                // Some models give the citation number as the title.
                Some(title) if title.parse::<u32>().is_err() => format!("{title} {url}"),
                _ => url.to_owned(),
            };
            if !list.contains(&line) {
                list.push(line);
            }
        }
        list.join("\n")
    };
    let mut searches: Vec<(String, String)> = Vec::new();
    // Anthropic sends server tool blocks. The OpenAI Responses API sends web search items.
    for item in all(&raw["content"]).into_iter().chain(all(&raw["output"])) {
        match item["type"].as_str().unwrap_or_default() {
            "server_tool_use" => searches.push((
                format!(
                    "{} {}",
                    item["name"].as_str().unwrap_or_default(),
                    item["input"]
                ),
                String::new(),
            )),
            "web_search_tool_result" => {
                if let Some(last) = searches.last_mut() {
                    last.1 = pages(all(&item["content"]));
                }
            }
            "web_search_call" => searches.push((
                format!("web_search {}", item["action"]),
                pages(all(&item["action"]["sources"])),
            )),
            _ => {}
        }
    }
    let grounding = &raw["candidates"][0]["groundingMetadata"];
    if grounding["webSearchQueries"].is_array() {
        let chunks = all(&grounding["groundingChunks"]);
        searches.push((
            format!("web_search {}", grounding["webSearchQueries"]),
            pages(chunks.into_iter().map(|chunk| &chunk["web"]).collect()),
        ));
    }
    let count = raw
        .pointer("/usage/server_tool_use_details/web_search_requests")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if count > 0 {
        let notes = all(&raw["choices"][0]["message"]["annotations"]);
        searches.push((
            format!("web_search ×{count}"),
            pages(
                notes
                    .into_iter()
                    .map(|note| &note["url_citation"])
                    .collect(),
            ),
        ));
    }
    searches
}

/// The function maps a Rig run error to the provider contract.
pub fn map_error(error: PromptError) -> Error {
    let report: ErrorReport = match error {
        PromptError::Provider(error) => error.report(),
        PromptError::Report(report) => report,
        PromptError::MaxTurns { .. } => {
            return Error::new("tool_step_limit", "The agent exceeds its tool step limit.");
        }
        // The run policy stops a run with its error code as the reason.
        PromptError::Cancelled { reason, .. } => {
            return match reason.as_str() {
                "tool_step_limit" => {
                    Error::new("tool_step_limit", "The agent exceeds its tool step limit.")
                }
                "token_budget" => Error::new("token_budget", "The reply exceeds its token budget."),
                "output_limit" => Error::new(
                    "output_limit",
                    "The provider exhausted the output allowance.",
                ),
                "provider_refusal" => {
                    Error::new("provider_refusal", "The provider blocked the response.")
                }
                _ => Error::new("cancelled", "The caller cancelled the request."),
            };
        }
        PromptError::UnknownToolCall { .. } => {
            return Error::new("invalid_provider_tool", "The model called an unknown tool.");
        }
        error => return Error::context("provider_transport", "Provider failed.")(error),
    };
    if report.refusal {
        return Error::new("provider_refusal", "The provider blocked the response.");
    }
    if let Some(status) = report.http_status {
        let mut code = match status {
            401 | 403 => "provider_auth",
            402 => "provider_credit",
            429 => "provider_rate_limit",
            413 => "context_exceeded",
            502..=504 => "provider_unavailable",
            _ => "provider_http",
        };
        if status == 400
            && report
                .provider_response_body()
                .is_some_and(|body| body.len() <= 65536)
            && let Ok(Some(value)) = report.provider_response_json()
        {
            code = native_error_code(&value["error"]).unwrap_or(code);
        }
        return Error::new(code, format!("The provider returned HTTP {status}."));
    }
    if let Ok(Some(value)) = report.provider_response_json()
        && let Some(code) = native_error_code(value.get("error").unwrap_or(&value))
    {
        return Error::new(code, "The provider reported a generation error.");
    }
    let (code, message) = match report.kind {
        rig_core::ErrorKind::Cancelled => ("cancelled", "The caller cancelled the request."),
        rig_core::ErrorKind::Provider | rig_core::ErrorKind::ProviderResponse => (
            "provider_error",
            "The provider reported a generation error.",
        ),
        rig_core::ErrorKind::Response | rig_core::ErrorKind::Json => {
            ("invalid_provider_stream", "The provider stream is invalid.")
        }
        _ => ("provider_transport", "Provider failed."),
    };
    Error::new(code, message)
}

fn native_error_code(error: &Value) -> Option<&'static str> {
    let message = error["message"].as_str().unwrap_or_default();
    let prompt_too_long = message.starts_with("prompt is too long");
    match error["code"].as_str().or(error["type"].as_str()) {
        Some("context_length_exceeded") => Some("context_exceeded"),
        Some("authentication_error" | "invalid_api_key") => Some("provider_auth"),
        Some("rate_limit_error" | "rate_limit_exceeded") => Some("provider_rate_limit"),
        Some("invalid_request_error") if prompt_too_long => Some("context_exceeded"),
        _ if error["status"] == "INVALID_ARGUMENT"
            && message.starts_with("The input token count")
            && message.contains("exceeds the maximum number of tokens allowed") =>
        {
            Some("context_exceeded")
        }
        _ => None,
    }
}
