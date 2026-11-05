//! The module sends a prepared request through the `genai` library.
//! The library maps the chat body to each wire protocol and parses each stream.
//! The module maps the library events and errors to the provider contract.

use super::*;
use futures_util::StreamExt;
use genai::{
    Client, ModelIden, ServiceTarget,
    adapter::AdapterKind,
    chat::{self as g, ChatStreamEvent, StopReason},
    resolver::{AuthData, Endpoint},
};
use serde_json::json;
use std::{collections::VecDeque, time::Duration};

const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;

/// The function maps the protocol-neutral request to a `genai` request and options.
fn chat_request(body: &ChatBody) -> (g::ChatRequest, g::ChatOptions) {
    let messages = body
        .messages
        .iter()
        .map(|message| match message {
            ChatMessage::System { content } => g::ChatMessage::system(content.as_str()),
            ChatMessage::User { content } => g::ChatMessage::user(content.as_str()),
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                let text = (!content.is_empty()).then(|| content.as_str().into());
                let calls = tool_calls.iter().map(|call| {
                    g::ToolCall {
                        call_id: call.id.clone(),
                        fn_name: call.name.clone(),
                        fn_arguments: call.arguments.clone(),
                        thought_signatures: call.signature.clone().map(|s| vec![s]),
                    }
                    .into()
                });
                let parts: Vec<g::ContentPart> = text.into_iter().chain(calls).collect();
                g::ChatMessage::assistant(g::MessageContent::from_parts(parts))
            }
            ChatMessage::Tool {
                tool_call_id,
                content,
            } => g::ToolResponse::new(tool_call_id.as_str(), content.as_str()).into(),
        })
        .collect();
    let mut request = g::ChatRequest::new(messages);
    if !body.tools.is_empty() {
        request.tools = Some(
            body.tools
                .iter()
                .map(|tool| {
                    g::Tool::new(tool.name.as_str())
                        .with_description(tool.description.as_str())
                        .with_schema(tool.parameters.clone())
                })
                .collect(),
        );
    }
    let mut options = g::ChatOptions::default()
        .with_capture_usage(true)
        .with_capture_content(true)
        .with_capture_tool_calls(true)
        .with_max_tokens(u32::try_from(body.max_tokens).unwrap_or(u32::MAX))
        .with_stop_sequences(body.stop.clone());
    options.temperature = body.temperature;
    options.top_p = body.top_p;
    if let Some(routing) = &body.provider {
        options = options.with_extra_body(json!({"provider": routing}));
    }
    (request, options)
}

/// The function sets the exact endpoint and the credential headers of the protocol.
fn target(
    request: &PreparedRequest,
    model: &str,
    credentials: &dyn CredentialResolver,
) -> Result<ServiceTarget> {
    let mut headers = Vec::new();
    if let Some(reference) = &request.credential_reference {
        let key = credentials.resolve(reference)?;
        headers.push(match request.protocol {
            ProviderKind::Anthropic => ("x-api-key".to_owned(), key),
            ProviderKind::Gemini => ("x-goog-api-key".to_owned(), key),
            _ => ("Authorization".to_owned(), format!("Bearer {key}")),
        });
    } else if request.protocol != ProviderKind::Custom {
        return Err(Error::new(
            "configuration",
            "This provider requires a credential reference.",
        ));
    }
    if request.protocol == ProviderKind::Anthropic {
        headers.push(("anthropic-version".to_owned(), "2023-06-01".to_owned()));
    }
    let kind = match request.protocol {
        ProviderKind::OpenRouter => AdapterKind::OpenRouter,
        ProviderKind::Custom => AdapterKind::OpenAI,
        ProviderKind::OpenAi => AdapterKind::OpenAIResp,
        ProviderKind::Anthropic => AdapterKind::Anthropic,
        ProviderKind::Gemini => AdapterKind::Gemini,
    };
    Ok(ServiceTarget {
        endpoint: Endpoint::from_owned(request.endpoint.clone()),
        auth: AuthData::RequestOverride {
            url: request.endpoint.clone(),
            headers: headers.into(),
        },
        model: ModelIden::new(kind, model.to_owned()),
    })
}

/// The function returns the HTTP status and body behind a library error.
fn http_failure(error: &genai::Error) -> Option<(u16, &str)> {
    let error = match error {
        genai::Error::WebStream { error, .. } => error.downcast_ref::<genai::Error>()?,
        error => error,
    };
    match error {
        genai::Error::HttpError { status, body, .. } => Some((status.as_u16(), body)),
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => match webc_error {
            genai::webc::Error::ResponseFailedStatus { status, body, .. } => {
                Some((status.as_u16(), body))
            }
            _ => None,
        },
        _ => None,
    }
}

fn map_error(error: genai::Error) -> Error {
    if let Some((status, body)) = http_failure(&error) {
        let mut code = match status {
            401 | 403 => "provider_auth",
            402 => "provider_credit",
            429 => "provider_rate_limit",
            413 => "context_exceeded",
            _ => "provider_http",
        };
        if status == 400
            && body.len() <= 65536
            && let Ok(value) = serde_json::from_str::<Value>(body)
        {
            code = native_error_code(&value["error"]).unwrap_or(code);
        }
        return Error::new(code, format!("The provider returned HTTP {status}."));
    }
    match error {
        genai::Error::StreamParse { .. } => {
            Error::context("invalid_provider_stream", "Frame failed.")(error)
        }
        _ => Error::context("provider_transport", "Provider failed.")(error),
    }
}

fn reason_text(reason: StopReason) -> String {
    match reason {
        StopReason::Completed(r)
        | StopReason::MaxTokens(r)
        | StopReason::ToolCall(r)
        | StopReason::ContentFilter(r)
        | StopReason::StopSequence(r)
        | StopReason::Other(r) => r,
    }
}

struct State {
    stream: g::ChatStream,
    cancellation: CancellationToken,
    tools: Vec<ToolDefinition>,
    pending: VecDeque<Result<ProviderEvent>>,
    bytes: usize,
    done: bool,
}

impl State {
    async fn next_raw(&mut self) -> Option<genai::Result<ChatStreamEvent>> {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => None,
            event = self.stream.next() => event,
        }
    }
    /// The method maps one library event. An error ends the stream.
    fn map(&mut self, event: Option<genai::Result<ChatStreamEvent>>) -> Result<()> {
        let event = match event {
            _ if self.cancellation.is_cancelled() => {
                return Err(Error::new("cancelled", "The caller cancelled the request."));
            }
            None => {
                return Err(Error::new(
                    "incomplete_stream",
                    "The provider closed the stream without terminal success.",
                ));
            }
            Some(Err(genai::Error::ChatResponse { body, .. })) => {
                let error = Error::new(
                    native_error_code(body.get("error").unwrap_or(&body))
                        .unwrap_or("provider_error"),
                    "The provider reported a generation error.",
                );
                self.done = true;
                self.pending.push_back(Ok(ProviderEvent::Failure { error }));
                return Ok(());
            }
            Some(Err(error)) => return Err(map_error(error)),
            Some(Ok(event)) => event,
        };
        let reasoning = matches!(event, ChatStreamEvent::ReasoningChunk(_));
        match event {
            ChatStreamEvent::Chunk(chunk) | ChatStreamEvent::ReasoningChunk(chunk)
                if !chunk.content.is_empty() =>
            {
                self.bytes = self.bytes.saturating_add(chunk.content.len());
                if self.bytes > MAX_STREAM_BYTES {
                    return Err(Error::new(
                        "output_limit",
                        "The provider stream exceeds the byte limit.",
                    ));
                }
                let text = chunk.content;
                self.pending.push_back(Ok(if reasoning {
                    ProviderEvent::Reasoning { text }
                } else {
                    ProviderEvent::Text { text }
                }));
            }
            ChatStreamEvent::End(end) => {
                self.done = true;
                if let Some(usage) = end.captured_usage {
                    self.pending.push_back(Ok(ProviderEvent::Usage {
                        usage: serde_json::to_value(usage)?,
                    }));
                }
                let calls = end
                    .captured_content
                    .map(g::MessageContent::into_tool_calls)
                    .unwrap_or_default();
                let reason = end.captured_stop_reason.map(reason_text);
                let finish = match reason.as_deref() {
                    Some("tool_calls" | "tool_use" | "function_call") if !calls.is_empty() => {
                        Ok(())
                    }
                    Some(reason) => check_finish(reason),
                    None => Err(Error::new(
                        "incomplete_stream",
                        "The stream has no terminal success reason.",
                    )),
                };
                if let Err(error) = finish {
                    self.pending.push_back(Ok(ProviderEvent::Failure { error }));
                    return Ok(());
                }
                if !calls.is_empty() {
                    let calls = calls
                        .into_iter()
                        .map(|call| {
                            let arguments = match call.fn_arguments {
                                Value::String(text) if text.trim().is_empty() => json!({}),
                                Value::String(text) => {
                                    serde_json::from_str(&text).map_err(|_| {
                                        Error::new(
                                            "invalid_provider_tool",
                                            "The tool arguments contain invalid JSON.",
                                        )
                                    })?
                                }
                                value => value,
                            };
                            Ok(ToolCall {
                                id: call.call_id,
                                name: call.fn_name,
                                arguments,
                                signature: call
                                    .thought_signatures
                                    .and_then(|s| s.into_iter().next()),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let calls = tools::check_calls(&self.tools, calls)?;
                    self.pending
                        .push_back(Ok(ProviderEvent::ToolCalls { calls }));
                }
                self.pending.push_back(Ok(ProviderEvent::Complete));
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) async fn open(
    request: PreparedRequest,
    credentials: &dyn CredentialResolver,
    cancellation: CancellationToken,
) -> Result<ProviderStream> {
    let body = request.chat()?;
    let (chat, options) = chat_request(&body);
    let target = target(&request, &body.model, credentials)?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(request.timeout_seconds))
        .build()
        .map_err(Error::context("provider_transport", "Provider failed."))?;
    let client = Client::builder()
        .with_reqwest(http)
        .build()
        .map_err(Error::context("provider_transport", "Provider failed."))?;
    let response = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the request.")),
        response = client.exec_chat_stream(target, chat, Some(&options)) => response.map_err(map_error)?,
    };
    let mut state = State {
        stream: response.stream,
        cancellation,
        tools: body.tools,
        pending: VecDeque::new(),
        bytes: 0,
        done: false,
    };
    // The library reports an HTTP error as the first stream event.
    // The function returns that error from the open call.
    let first = loop {
        match state.next_raw().await {
            Some(Ok(ChatStreamEvent::Start)) => continue,
            event => break event,
        }
    };
    state.map(first)?;
    Ok(Box::pin(futures_util::stream::unfold(
        state,
        |mut state| async move {
            loop {
                if state.cancellation.is_cancelled() && (!state.done || !state.pending.is_empty()) {
                    state.done = true;
                    state.pending.clear();
                    let error = Error::new("cancelled", "The caller cancelled the request.");
                    return Some((Err(error), state));
                }
                if let Some(event) = state.pending.pop_front() {
                    return Some((event, state));
                }
                if state.done {
                    return None;
                }
                let event = state.next_raw().await;
                if let Err(error) = state.map(event) {
                    state.done = true;
                    state.pending.clear();
                    state.pending.push_back(Err(error));
                }
            }
        },
    )))
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
        "stop" | "end_turn" | "stop_sequence" | "STOP" | "completed" => return Ok(()),
        "model_context_window_exceeded" => (
            "context_exceeded",
            "The provider exhausted its context window.",
        ),
        "length" | "max_tokens" | "MAX_TOKENS" | "incomplete" | "max_output_tokens" => (
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
