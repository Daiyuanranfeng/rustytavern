use rustytavern_core::Role;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
    Gemini,
    Custom,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderCapabilities {
    pub contract_id: &'static str,
    pub protocol: ProviderKind,
    pub roles: Vec<Role>,
    pub input_types: Vec<&'static str>,
    pub output_types: Vec<&'static str>,
    pub streaming: bool,
    pub tools: bool,
    pub reasoning_controls: bool,
    pub token_count: &'static str,
    pub system_placement: &'static str,
    pub parameters: Vec<&'static str>,
    pub model_constraints: &'static str,
}
impl ProviderKind {
    pub fn capabilities(self) -> ProviderCapabilities {
        ProviderCapabilities {
            contract_id: match self {
                Self::OpenRouter => "openrouter.chat_completions.agent-v1",
                Self::OpenAi => "openai.responses.agent-v1",
                Self::Anthropic => "anthropic.messages.agent-v1",
                Self::Gemini => "gemini.generate_content.agent-v1",
                Self::Custom => "custom.chat_completions.agent-v1",
            },
            protocol: self,
            roles: vec![Role::System, Role::User, Role::Assistant],
            input_types: vec!["text"],
            output_types: vec!["text"],
            streaming: true,
            tools: true,
            reasoning_controls: true,
            token_count: "estimated_utf8_bytes",
            system_placement: match self {
                Self::Anthropic => "top_level_system",
                Self::Gemini => "system_instruction",
                _ => "ordered_messages",
            },
            parameters: vec!["temperature", "top_p", "stop", "reasoning", "cache"],
            model_constraints: "Rig checks the options that the protocol and the selected model support.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OpenRouterRouting {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    pub allow_fallbacks: bool,
    pub require_parameters: bool,
}
impl Default for OpenRouterRouting {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            // A model often has one endpoint that is down, rate limited, or needs another key.
            // Fallback keeps the same model and still requires every requested parameter.
            allow_fallbacks: true,
            require_parameters: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
    pub context_limit: usize,
    pub output_limit: usize,
    pub parameters: BTreeMap<String, Value>,
    pub routing: Option<OpenRouterRouting>,
    pub timeout_seconds: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub merge_system_messages: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::OpenRouter,
            model: String::new(),
            endpoint: None,
            api_key_env: None,
            context_limit: 131072,
            output_limit: 8192,
            parameters: BTreeMap::new(),
            routing: None,
            timeout_seconds: 120,
            merge_system_messages: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
