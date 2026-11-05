//! The module prepares provider requests and streams provider events.
//! - `request` builds one protocol-neutral chat request from a prompt plan.
//! - `tools` checks tool definitions, continuations, and returned calls.
//! - `client` sends the request through the `genai` library and maps its events.

pub mod catalog;

mod client;
mod request;
mod tools;
pub use request::{ChatBody, ChatMessage};
use std::pin::Pin;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

pub type AgentProviderStream = Pin<Box<dyn Stream<Item = Result<AgentProviderEvent>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentProviderEvent {
    Text { text: String },
    Usage { usage: Value },
    ToolCalls { calls: Vec<ToolCall> },
    Failure { error: Error },
    Complete,
}
impl AgentProviderEvent {
    pub fn into_text(self) -> Result<ProviderEvent> {
        Ok(match self {
            Self::Text { text } => ProviderEvent::Text { text },
            Self::Usage { usage } => ProviderEvent::Usage { usage },
            Self::Failure { error } => ProviderEvent::Failure { error },
            Self::Complete => ProviderEvent::Complete,
            Self::ToolCalls { .. } => {
                return Err(tools::invalid("The text contract rejects tool calls."));
            }
        })
    }
}
impl From<ProviderEvent> for AgentProviderEvent {
    fn from(event: ProviderEvent) -> Self {
        match event {
            ProviderEvent::Text { text } => Self::Text { text },
            ProviderEvent::Usage { usage } => Self::Usage { usage },
            ProviderEvent::Failure { error } => Self::Failure { error },
            ProviderEvent::Complete => Self::Complete,
        }
    }
}
pub trait CredentialResolver: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<String>;
}

/// The resolver has no keys. The application supplies the keys from `config.toml`.
pub struct NoCredentials;
impl CredentialResolver for NoCredentials {
    fn resolve(&self, reference: &str) -> Result<String> {
        Err(Error::new(
            "configuration",
            format!("The credential {reference} is unavailable."),
        ))
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
    /// The pure method adds tools and a continuation to a prepared text request.
    fn prepare_agent(
        &self,
        _request: PreparedRequest,
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
        request::prepare(plan, model, config)
    }

    fn prepare_agent(
        &self,
        request: PreparedRequest,
        agent: &AgentRequest,
    ) -> Result<PreparedRequest> {
        tools::attach(request, agent)
    }
    async fn open_agent_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
    ) -> Result<AgentProviderStream> {
        client::open(request, credentials, cancellation, true).await
    }
    async fn open_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
    ) -> Result<ProviderStream> {
        let stream = client::open(request, credentials, cancellation, false).await?;
        Ok(Box::pin(stream.map(|event| {
            event.and_then(AgentProviderEvent::into_text)
        })))
    }
}
