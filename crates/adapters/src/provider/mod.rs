pub mod catalog;

mod agent;
pub use agent::{AgentProviderEvent, AgentProviderStream, AgentStreamNormalizer};
use std::{collections::VecDeque, pin::Pin, time::Duration};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use rustytavern_conversation::prompt::PromptPlan;
use rustytavern_core::{Error, Result, Role};

pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent>> + Send>>;

pub use rustytavern_conversation::provider::{
    AgentRequest, AgentTurn, OpenRouterRouting, ProviderCapabilities, ProviderConfig, ProviderKind,
    ToolCall, ToolDefinition, ToolResult,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedRequest {
    #[serde(default)]
    pub contract_id: String,
    pub protocol: ProviderKind,
    pub endpoint: String,
    pub body: Value,
    pub credential_reference: Option<String>,
    pub timeout_seconds: u64,
    pub prompt_hash: String,
    pub transformations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    Text { text: String },
    Usage { usage: Value },
    Failure { error: Error },
    Complete,
}

pub trait CredentialResolver: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<String>;
}

pub struct EnvironmentCredentials;
impl CredentialResolver for EnvironmentCredentials {
    fn resolve(&self, reference: &str) -> Result<String> {
        std::env::var(reference)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                Error::new(
                    "configuration",
                    format!("The credential environment variable {reference} is unavailable."),
                )
            })
    }
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> &str {
        std::any::type_name::<Self>()
    }
    fn validate_config(&self, model: &str, config: &ProviderConfig) -> Result<()> {
        HttpProvider
            .prepare(&PromptPlan::default(), model, config)
            .map(|_| ())
    }
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest>;
    fn prepare_agent(
        &self,
        _plan: &PromptPlan,
        _model: &str,
        _config: &ProviderConfig,
        _agent: &AgentRequest,
    ) -> Result<PreparedRequest> {
        Err(Error::new(
            "unsupported_provider_tools",
            "This adapter does not support tool calls.",
        ))
    }
    async fn open_agent_stream(
        &self,
        _request: PreparedRequest,
        _credentials: &dyn CredentialResolver,
        _cancellation: CancellationToken,
    ) -> Result<AgentProviderStream> {
        Err(Error::new(
            "unsupported_provider_tools",
            "This adapter does not support tool calls.",
        ))
    }
    async fn open_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream>;
}

#[derive(Default)]
pub struct HttpProvider;

#[async_trait]
impl ProviderAdapter for HttpProvider {
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        if model.trim().is_empty() {
            return Err(Error::new(
                "configuration",
                "The provider requires a model ID.",
            ));
        }
        if config.timeout_seconds == 0 {
            return Err(Error::new(
                "configuration",
                "The provider timeout must be positive.",
            ));
        }
        if config.routing.is_some() && config.kind != ProviderKind::OpenRouter {
            return Err(Error::new(
                "unsupported_provider_parameter",
                "This adapter does not support OpenRouter routing.",
            ));
        }
        if config.merge_system_messages
            && !matches!(config.kind, ProviderKind::OpenRouter | ProviderKind::Custom)
        {
            return Err(Error::new(
                "unsupported_provider_parameter",
                "This protocol does not support the system message merge option.",
            ));
        }
        let mut messages: Vec<(Role, String)> = Vec::new();
        for section in &plan.sections {
            match messages.last_mut() {
                Some((Role::System, text))
                    if config.merge_system_messages && section.role == Role::System =>
                {
                    text.push_str("\n\n");
                    text.push_str(&section.text);
                }
                _ => messages.push((section.role.clone(), section.text.clone())),
            }
        }
        let roles = messages
            .into_iter()
            .map(|(role, text)| json!({"role":role,"content":text}))
            .collect::<Vec<_>>();
        let systems = plan
            .sections
            .iter()
            .filter(|s| s.role == Role::System)
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut transformations = Vec::new();
        if config.merge_system_messages {
            transformations
                .push("The adapter joins adjacent system sections with two line breaks.".into());
        }
        let (default_endpoint, mut body) = match config.kind {
            ProviderKind::OpenRouter | ProviderKind::Custom => (
                "https://openrouter.ai/api/v1/chat/completions".to_owned(),
                json!({"model":model,"messages":roles,"stream":true,"max_tokens":plan.output_reserve,"stream_options":{"include_usage":true}}),
            ),
            ProviderKind::OpenAi => (
                "https://api.openai.com/v1/responses".to_owned(),
                json!({"model":model,"input":roles,"stream":true,"max_output_tokens":plan.output_reserve,"store":false}),
            ),
            ProviderKind::Anthropic => {
                transformations.push(
                    "The adapter consolidates system sections into the top-level system field."
                        .into(),
                );
                let messages = plan
                    .sections
                    .iter()
                    .filter(|s| s.role != Role::System)
                    .map(|s| json!({"role":s.role,"content":s.text}))
                    .collect::<Vec<_>>();
                (
                    "https://api.anthropic.com/v1/messages".to_owned(),
                    json!({"model":model,"system":systems,"messages":messages,"stream":true,"max_tokens":plan.output_reserve}),
                )
            }
            ProviderKind::Gemini => {
                if !model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                {
                    return Err(Error::new(
                        "configuration",
                        "The Gemini model ID contains an unsupported path character.",
                    ));
                }
                transformations.push("The adapter consolidates system sections into systemInstruction and maps assistant roles to model.".into());
                let contents = plan.sections.iter().filter(|s| s.role != Role::System).map(|s| json!({"role":if s.role==Role::Assistant { "model" } else { "user" },"parts":[{"text":s.text}]})).collect::<Vec<_>>();
                (
                    format!(
                        "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse"
                    ),
                    json!({"systemInstruction":{"parts":[{"text":systems}]},"contents":contents,"generationConfig":{"maxOutputTokens":plan.output_reserve}}),
                )
            }
        };
        if config.kind == ProviderKind::OpenRouter {
            let routing = config.routing.clone().unwrap_or_default();
            if routing
                .order
                .iter()
                .any(|provider| provider.trim().is_empty())
            {
                return Err(Error::new(
                    "invalid_provider_parameter",
                    "The provider routing order contains an empty ID.",
                ));
            }
            body["provider"] = serde_json::to_value(routing)?;
        }
        let capabilities = config.kind.capabilities();
        for (key, value) in &config.parameters {
            if !capabilities.parameters.contains(&key.as_str()) {
                return Err(Error::new(
                    "unsupported_provider_parameter",
                    format!("The adapter does not support the {key} parameter."),
                ));
            }
            let valid = match key.as_str() {
                "temperature" => value.as_f64().is_some_and(|n| {
                    n >= 0.0
                        && n <= if config.kind == ProviderKind::Anthropic {
                            1.0
                        } else {
                            2.0
                        }
                }),
                "top_p" => value.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n)),
                "stop" => {
                    value.as_str().is_some_and(|s| !s.is_empty())
                        || value.as_array().is_some_and(|items| {
                            !items.is_empty()
                                && items.len() <= capabilities.max_stop_sequences
                                && items
                                    .iter()
                                    .all(|s| s.as_str().is_some_and(|s| !s.is_empty()))
                        })
                }
                _ => false,
            };
            if !valid {
                return Err(Error::new(
                    "invalid_provider_parameter",
                    format!("The {key} parameter has an invalid type or value."),
                ));
            }
            let value = if key == "stop"
                && value.is_string()
                && matches!(config.kind, ProviderKind::Gemini | ProviderKind::Anthropic)
            {
                json!([value])
            } else {
                value.clone()
            };
            match config.kind {
                ProviderKind::Gemini => {
                    let field = match key.as_str() {
                        "top_p" => "topP",
                        "stop" => "stopSequences",
                        _ => key,
                    };
                    body["generationConfig"][field] = value.clone();
                }
                ProviderKind::Anthropic if key == "stop" => body["stop_sequences"] = value.clone(),
                _ => body[key] = value.clone(),
            }
        }
        let endpoint = config.endpoint.clone().unwrap_or(default_endpoint);
        let url = url::Url::parse(&endpoint).map_err(Error::context(
            "configuration",
            "The provider endpoint is invalid.",
        ))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query_pairs().any(|(key, _)| {
                matches!(
                    key.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
                    "key"
                        | "apikey"
                        | "token"
                        | "accesstoken"
                        | "authorization"
                        | "secret"
                        | "password"
                        | "signature"
                        | "credential"
                )
            })
        {
            return Err(Error::new(
                "configuration",
                "The provider endpoint must be an HTTP URL without credentials, authentication query parameters, or fragments.",
            ));
        }
        if config.kind == ProviderKind::Custom && config.endpoint.is_none() {
            return Err(Error::new(
                "configuration",
                "A custom provider requires an explicit endpoint.",
            ));
        }
        Ok(PreparedRequest {
            contract_id: capabilities.contract_id.into(),
            protocol: config.kind,
            endpoint,
            body,
            credential_reference: config.api_key_env.clone(),
            timeout_seconds: config.timeout_seconds,
            prompt_hash: plan.hash.clone(),
            transformations,
        })
    }

    fn prepare_agent(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
        agent: &AgentRequest,
    ) -> Result<PreparedRequest> {
        agent::prepare(self.prepare(plan, model, config)?, agent)
    }
    async fn open_agent_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
    ) -> Result<AgentProviderStream> {
        self.open_http_stream(request, credentials, cancellation, true)
            .await
    }
    async fn open_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream> {
        let stream = self
            .open_http_stream(request, credentials, cancellation, false)
            .await?;
        Ok(Box::pin(stream.map(|event| {
            event.and_then(AgentProviderEvent::into_text)
        })))
    }
}

impl HttpProvider {
    async fn open_http_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
        agent_mode: bool,
    ) -> Result<AgentProviderStream> {
        if agent_mode {
            agent::check_protocol(request.protocol)?;
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(request.timeout_seconds))
            .build()
            .map_err(Error::context("provider_transport", "Provider failed."))?;
        let mut builder = client
            .post(&request.endpoint)
            .header("Accept", "text/event-stream")
            .json(&request.body);
        if let Some(reference) = &request.credential_reference {
            let key = credentials.resolve(reference)?;
            builder = match request.protocol {
                ProviderKind::Anthropic => builder.header("x-api-key", key),
                ProviderKind::Gemini => builder.header("x-goog-api-key", key),
                _ => builder.bearer_auth(key),
            };
        } else if request.protocol != ProviderKind::Custom {
            return Err(Error::new(
                "configuration",
                "This provider requires a credential reference.",
            ));
        }
        if request.protocol == ProviderKind::Anthropic {
            builder = builder.header("anthropic-version", "2023-06-01");
        }
        let mut response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the request.")),
            response = builder.send() => response.map_err(Error::context("provider_transport", "Provider failed."))?,
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let mut code = match status {
                401 | 403 => "provider_auth",
                402 => "provider_credit",
                429 => "provider_rate_limit",
                413 => "context_exceeded",
                _ => "provider_http",
            };
            if status == 400 {
                let mut body = Vec::new();
                while let Some(chunk) = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the request.")),
                    chunk = response.chunk() => chunk.map_err(Error::context("provider_transport", "The error response failed."))?,
                } {
                    if body.len() + chunk.len() > 65536 {
                        body.clear();
                        break;
                    }
                    body.extend_from_slice(&chunk);
                }
                if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                    code = native_error_code(&value["error"]).unwrap_or(code);
                }
            }
            return Err(Error::new(
                code,
                format!("The provider returned HTTP {status}."),
            ));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !content_type.starts_with("text/event-stream") {
            return Err(Error::new(
                "invalid_provider_stream",
                "The provider did not return an SSE response.",
            ));
        }
        let chunks = response.bytes_stream().map(|result| {
            result
                .map(|bytes| bytes.to_vec())
                .map_err(Error::context("provider_transport", "Provider failed."))
        });
        let state = HttpStreamState {
            chunks: Box::pin(chunks),
            decoder: SseDecoder::default(),
            normalizer: AgentStreamNormalizer::new(request.protocol, agent_mode)
                .with_request(&request)?,
            pending: VecDeque::new(),
            cancellation,
            done: false,
            bytes: 0,
        };
        Ok(Box::pin(futures_util::stream::unfold(
            state,
            |mut state| async move {
                loop {
                    if (!state.done || !state.pending.is_empty())
                        && state.cancellation.is_cancelled()
                    {
                        state.done = true;
                        state.pending.clear();
                        return Some((
                            Err(Error::new("cancelled", "The caller cancelled the request.")),
                            state,
                        ));
                    }
                    if let Some(event) = state.pending.pop_front() {
                        return Some((event, state));
                    }
                    if state.done {
                        return None;
                    }
                    let next = tokio::select! { biased; _ = state.cancellation.cancelled() => None, value = state.chunks.next() => value };
                    let result = match next {
                        Some(bytes) => bytes.and_then(|bytes| {
                            state.bytes = state.bytes.saturating_add(bytes.len());
                            if state.bytes > 16 * 1024 * 1024 {
                                return Err(Error::new(
                                    "output_limit",
                                    "The provider stream exceeds the byte limit.",
                                ));
                            }
                            for frame in state.decoder.push(&bytes)? {
                                state
                                    .pending
                                    .extend(state.normalizer.frame(&frame)?.into_iter().map(Ok));
                                if state.normalizer.completed {
                                    state.done = true;
                                    break;
                                }
                            }
                            Ok(())
                        }),
                        None => Err(if state.cancellation.is_cancelled() {
                            Error::new("cancelled", "The caller cancelled the request.")
                        } else {
                            Error::new(
                                "incomplete_stream",
                                "The provider closed the stream without terminal success.",
                            )
                        }),
                    };
                    if let Err(error) = result {
                        state.done = true;
                        state.pending.push_back(Err(error));
                    }
                }
            },
        )))
    }
}

struct HttpStreamState {
    chunks: Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>,
    decoder: SseDecoder,
    normalizer: AgentStreamNormalizer,
    pending: VecDeque<Result<AgentProviderEvent>>,
    cancellation: CancellationToken,
    done: bool,
    bytes: usize,
}

#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    data: Vec<String>,
    data_bytes: usize,
    skip_lf: bool,
}

impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        let mut frames = Vec::new();
        for byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if *byte == b'\r' || *byte == b'\n' {
                self.skip_lf = *byte == b'\r';
                let line = std::str::from_utf8(&self.line)
                    .map_err(Error::context("invalid_provider_stream", "Frame failed."))?;
                if line.is_empty() {
                    if !self.data.is_empty() {
                        frames.push(self.data.join("\n"));
                        self.data.clear();
                        self.data_bytes = 0;
                    }
                } else if let Some(data) = line.strip_prefix("data:") {
                    let data = data.strip_prefix(' ').unwrap_or(data);
                    self.data_bytes = self.data_bytes.saturating_add(data.len().saturating_add(1));
                    if self.data_bytes > 1024 * 1024 {
                        return Err(Error::new(
                            "invalid_provider_stream",
                            "The SSE frame exceeds its byte limit.",
                        ));
                    }
                    self.data.push(data.to_owned());
                }
                self.line.clear();
            } else {
                if self.line.len() == 1024 * 1024 {
                    return Err(Error::new(
                        "invalid_provider_stream",
                        "The SSE line exceeds its byte limit.",
                    ));
                }
                self.line.push(*byte);
            }
        }
        Ok(frames)
    }
}

pub struct StreamNormalizer {
    kind: ProviderKind,
    pub completed: bool,
    finish_reason: Option<String>,
}

impl StreamNormalizer {
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            kind,
            completed: false,
            finish_reason: None,
        }
    }
    pub fn frame(&mut self, text: &str) -> Result<Vec<ProviderEvent>> {
        if self.completed {
            return Ok(Vec::new());
        }
        if text == "[DONE]" {
            if !matches!(self.kind, ProviderKind::Custom | ProviderKind::OpenRouter)
                || self.finish_reason.as_deref() != Some("stop")
            {
                return Err(Error::new(
                    "incomplete_stream",
                    "The stream marker has no terminal success reason.",
                ));
            }
            self.completed = true;
            return Ok(vec![ProviderEvent::Complete]);
        }
        let value: Value = serde_json::from_str(text)
            .map_err(Error::context("invalid_provider_stream", "Frame failed."))?;
        let mut failure = (value.get("error").is_some()
            || matches!(value["type"].as_str(), Some("error" | "response.failed")))
        .then(|| {
            Error::new(
                native_error_code(value.get("error").unwrap_or(&value["response"]["error"]))
                    .unwrap_or("provider_error"),
                "The provider reported a generation error.",
            )
        });
        let mut events = Vec::new();
        let mut complete = false;
        if failure.is_none() {
            match self.kind {
                ProviderKind::Custom | ProviderKind::OpenRouter => {
                    if let Some(choices) = value["choices"].as_array() {
                        if choices.len() > 1 {
                            return Err(Error::new(
                                "unsupported_provider_output",
                                "The adapter accepts one response choice.",
                            ));
                        }
                        if let Some(choice) = choices.first() {
                            if choice["delta"].get("tool_calls").is_some()
                                || choice["delta"].get("function_call").is_some()
                            {
                                return Err(Error::new(
                                    "unsupported_provider_output",
                                    "The default driver accepts text only.",
                                ));
                            }
                            if !choice["delta"]["content"].is_null()
                                && !choice["delta"]["content"].is_string()
                            {
                                return Err(Error::new(
                                    "invalid_provider_stream",
                                    "The content must be text or null.",
                                ));
                            }
                            if let Some(content) = choice["delta"]["content"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                            {
                                events.push(ProviderEvent::Text {
                                    text: content.to_owned(),
                                });
                            }
                            if let Some(reason) = choice["finish_reason"].as_str() {
                                failure = check_finish(reason).err();
                                self.finish_reason = Some(reason.to_owned());
                            }
                        }
                    } else {
                        return Err(Error::new(
                            "invalid_provider_stream",
                            "The completion chunk has no choices array.",
                        ));
                    }
                }
                ProviderKind::OpenAi => match value["type"].as_str() {
                    Some("response.output_item.added" | "response.output_item.done") => {
                        match value["item"]["type"].as_str() {
                            Some("message" | "reasoning") => {}
                            Some(_) => {
                                return Err(Error::new(
                                    "unsupported_provider_output",
                                    "The default driver accepts text only.",
                                ));
                            }
                            None => {
                                return Err(Error::new(
                                    "invalid_provider_stream",
                                    "The output item type is missing.",
                                ));
                            }
                        }
                    }
                    Some("response.output_text.delta") => {
                        let text = value["delta"].as_str().ok_or_else(|| {
                            Error::new("invalid_provider_stream", "The text delta is missing.")
                        })?;
                        events.push(ProviderEvent::Text {
                            text: text.to_owned(),
                        });
                    }
                    Some("response.completed") => {
                        if value["response"]["status"] != "completed" {
                            return Err(Error::new(
                                "incomplete_stream",
                                "The response does not report completion.",
                            ));
                        }
                        complete = true;
                    }
                    Some("response.incomplete") => {
                        failure = Some(Error::new(
                            "output_limit",
                            "The provider returned an incomplete response.",
                        ));
                    }
                    Some("response.refusal.delta") => {
                        failure = Some(Error::new(
                            "provider_refusal",
                            "The provider refused the request.",
                        ));
                    }
                    _ => {}
                },
                ProviderKind::Anthropic => match value["type"].as_str() {
                    Some("content_block_start" | "content_block_delta") => {
                        let block = if value["type"] == "content_block_start" {
                            &value["content_block"]
                        } else {
                            &value["delta"]
                        };
                        match block["type"].as_str() {
                            Some("text" | "text_delta") => events.push(ProviderEvent::Text {
                                text: block["text"]
                                    .as_str()
                                    .ok_or_else(|| {
                                        Error::new(
                                            "invalid_provider_stream",
                                            "The text delta is missing.",
                                        )
                                    })?
                                    .to_owned(),
                            }),
                            Some(
                                "thinking" | "redacted_thinking" | "thinking_delta"
                                | "signature_delta",
                            ) => {}
                            Some(_) => {
                                return Err(Error::new(
                                    "unsupported_provider_output",
                                    "The default driver accepts text blocks only.",
                                ));
                            }
                            None => {
                                return Err(Error::new(
                                    "invalid_provider_stream",
                                    "The content block type is missing.",
                                ));
                            }
                        }
                    }
                    Some("message_delta") => {
                        if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                            failure = check_finish(reason).err();
                            self.finish_reason = Some(reason.to_owned());
                        }
                    }
                    Some("message_stop") => {
                        if !matches!(
                            self.finish_reason.as_deref(),
                            Some("end_turn" | "stop_sequence")
                        ) {
                            return Err(Error::new(
                                "incomplete_stream",
                                "The message has no terminal success reason.",
                            ));
                        }
                        complete = true;
                    }
                    _ => {}
                },
                ProviderKind::Gemini => {
                    if value["promptFeedback"].get("blockReason").is_some() {
                        return Err(Error::new(
                            "provider_refusal",
                            "The provider blocked the prompt.",
                        ));
                    }
                    if let Some(candidates) = value["candidates"].as_array() {
                        if candidates.len() != 1 {
                            return Err(Error::new(
                                "unsupported_provider_output",
                                "The adapter accepts one response candidate.",
                            ));
                        }
                        let candidate = &candidates[0];
                        if let Some(content) = candidate.get("content") {
                            for part in content["parts"].as_array().ok_or_else(|| {
                                Error::new(
                                    "invalid_provider_stream",
                                    "The content parts array is missing.",
                                )
                            })? {
                                if part["thought"].as_bool() == Some(true) {
                                    continue;
                                }
                                events.push(ProviderEvent::Text {
                                    text: part["text"]
                                        .as_str()
                                        .ok_or_else(|| {
                                            Error::new(
                                                "unsupported_provider_output",
                                                "The default driver accepts text parts only.",
                                            )
                                        })?
                                        .to_owned(),
                                });
                            }
                        }
                        if let Some(reason) = candidate["finishReason"].as_str() {
                            failure = check_finish(reason).err();
                            complete = true;
                        }
                    }
                }
            }
        }
        let usage = match self.kind {
            ProviderKind::OpenAi => value["response"].get("usage"),
            ProviderKind::Anthropic if value["type"] == "message_start" => {
                value["message"].get("usage")
            }
            ProviderKind::Gemini => value.get("usageMetadata"),
            _ => value.get("usage"),
        };
        if let Some(usage) = usage.filter(|v| !v.is_null()) {
            events.push(ProviderEvent::Usage {
                usage: usage.clone(),
            });
        }
        if let Some(error) = failure {
            self.completed = true;
            events.push(ProviderEvent::Failure { error });
        } else if complete {
            self.completed = true;
            events.push(ProviderEvent::Complete);
        }
        Ok(events)
    }
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

fn check_finish(reason: &str) -> Result<()> {
    let (code, message) = match reason {
        "stop" | "end_turn" | "stop_sequence" | "STOP" => return Ok(()),
        "model_context_window_exceeded" => (
            "context_exceeded",
            "The provider exhausted its context window.",
        ),
        "length" | "max_tokens" | "MAX_TOKENS" => (
            "output_limit",
            "The provider exhausted the output allowance.",
        ),
        "content_filter" | "SAFETY" | "RECITATION" => {
            ("provider_refusal", "The provider blocked the response.")
        }
        _ => (
            "unsupported_provider_output",
            "The provider returned an unsupported completion reason.",
        ),
    };
    Err(Error::new(code, message))
}
