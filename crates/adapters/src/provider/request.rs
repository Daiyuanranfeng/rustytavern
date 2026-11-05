//! The module builds the protocol-neutral chat request from a prompt plan.
//! The request has one shape for every protocol. The client maps it to the wire format.

use super::*;

/// One message of the protocol-neutral chat request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ChatMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// The protocol-neutral chat request. `PreparedRequest::body` stores it as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatBody {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// OpenRouter routing preferences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<OpenRouterRouting>,
}

impl PreparedRequest {
    /// The method reads the body as a protocol-neutral chat request.
    pub fn chat(&self) -> Result<ChatBody> {
        serde_json::from_value(self.body.clone()).map_err(Error::context(
            "invalid_provider_request",
            "The prepared request has an invalid body.",
        ))
    }
}

fn configuration(message: &str) -> Error {
    Error::new("configuration", message)
}

fn check_config(model: &str, config: &ProviderConfig) -> Result<()> {
    if model.trim().is_empty() {
        return Err(configuration("The provider requires a model ID."));
    }
    if config.timeout_seconds == 0 {
        return Err(configuration("The provider timeout must be positive."));
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
    if config.kind == ProviderKind::Custom && config.endpoint.is_none() {
        return Err(configuration(
            "A custom provider requires an explicit endpoint.",
        ));
    }
    Ok(())
}

/// The function returns the endpoint of the protocol and the notes on its prompt mapping.
fn endpoint(model: &str, config: &ProviderConfig) -> Result<(String, Vec<String>)> {
    let (default, note) = match config.kind {
        ProviderKind::OpenRouter | ProviderKind::Custom => (
            "https://openrouter.ai/api/v1/chat/completions".to_owned(),
            None,
        ),
        ProviderKind::OpenAi => ("https://api.openai.com/v1/responses".to_owned(), None),
        ProviderKind::Anthropic => (
            "https://api.anthropic.com/v1/messages".to_owned(),
            Some("The adapter consolidates system sections into the top-level system field."),
        ),
        ProviderKind::Gemini => {
            if !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(configuration(
                    "The Gemini model ID contains an unsupported path character.",
                ));
            }
            (
                format!(
                    "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse"
                ),
                Some(
                    "The adapter consolidates system sections into systemInstruction and maps assistant roles to model.",
                ),
            )
        }
    };
    let endpoint = config.endpoint.clone().unwrap_or(default);
    let url = url::Url::parse(&endpoint).map_err(Error::context(
        "configuration",
        "The provider endpoint is invalid.",
    ))?;
    let secret_query = url.query_pairs().any(|(key, _)| {
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
    });
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || secret_query
    {
        return Err(configuration(
            "The provider endpoint must be an HTTP URL without credentials, authentication query parameters, or fragments.",
        ));
    }
    Ok((endpoint, note.into_iter().map(str::to_owned).collect()))
}

/// The function checks the sampling parameters and writes them to the body.
fn apply_parameters(body: &mut ChatBody, config: &ProviderConfig) -> Result<()> {
    let capabilities = config.kind.capabilities();
    let max_temperature = if config.kind == ProviderKind::Anthropic {
        1.0
    } else {
        2.0
    };
    for (key, value) in &config.parameters {
        if !capabilities.parameters.contains(&key.as_str()) {
            return Err(Error::new(
                "unsupported_provider_parameter",
                format!("The adapter does not support the {key} parameter."),
            ));
        }
        let number = value.as_f64();
        let stops = match value {
            Value::String(stop) => vec![Some(stop.as_str())],
            Value::Array(stops) => stops.iter().map(Value::as_str).collect(),
            _ => vec![],
        };
        let valid = match key.as_str() {
            "temperature" => {
                body.temperature = number;
                number.is_some_and(|n| (0.0..=max_temperature).contains(&n))
            }
            "top_p" => {
                body.top_p = number;
                number.is_some_and(|n| (0.0..=1.0).contains(&n))
            }
            "stop" => {
                body.stop = stops.iter().flatten().map(|s| (*s).to_owned()).collect();
                !stops.is_empty()
                    && stops.len() <= capabilities.max_stop_sequences
                    && stops.iter().all(|s| s.is_some_and(|s| !s.is_empty()))
            }
            _ => false,
        };
        if !valid {
            return Err(Error::new(
                "invalid_provider_parameter",
                format!("The {key} parameter has an invalid type or value."),
            ));
        }
    }
    Ok(())
}

pub(super) fn prepare(
    plan: &PromptPlan,
    model: &str,
    config: &ProviderConfig,
) -> Result<PreparedRequest> {
    check_config(model, config)?;
    let (endpoint, mut transformations) = endpoint(model, config)?;
    let mut messages: Vec<ChatMessage> = Vec::new();
    for section in &plan.sections {
        let content = section.text.clone();
        match (messages.last_mut(), &section.role) {
            (Some(ChatMessage::System { content: text }), Role::System)
                if config.merge_system_messages =>
            {
                text.push_str("\n\n");
                text.push_str(&content);
            }
            (_, Role::System) => messages.push(ChatMessage::System { content }),
            (_, Role::User) => messages.push(ChatMessage::User { content }),
            (_, Role::Assistant) => messages.push(ChatMessage::Assistant {
                content,
                tool_calls: vec![],
            }),
        }
    }
    if config.merge_system_messages {
        transformations.insert(
            0,
            "The adapter joins adjacent system sections with two line breaks.".into(),
        );
    }
    let mut body = ChatBody {
        model: model.to_owned(),
        messages,
        max_tokens: plan.output_reserve,
        ..Default::default()
    };
    if config.kind == ProviderKind::OpenRouter {
        let routing = config.routing.clone().unwrap_or_default();
        if routing.order.iter().any(|id| id.trim().is_empty()) {
            return Err(Error::new(
                "invalid_provider_parameter",
                "The provider routing order contains an empty ID.",
            ));
        }
        body.provider = Some(routing);
    }
    apply_parameters(&mut body, config)?;
    Ok(PreparedRequest {
        contract_id: config.kind.capabilities().contract_id.into(),
        protocol: config.kind,
        endpoint,
        body: serde_json::to_value(body)?,
        credential_reference: config.api_key_env.clone(),
        timeout_seconds: config.timeout_seconds,
        prompt_hash: plan.hash.clone(),
        transformations,
    })
}
