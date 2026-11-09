//! The module prepares provider requests and builds Rig models.
//! - `request` builds one protocol-neutral chat request from a prompt plan.
//! - `client` builds the Rig model and the Rig agent run for a prepared request.

pub mod catalog;

mod client;
mod request;
pub use client::{agent, map_error, search_fallback, web_searches};
pub use request::{ChatBody, ChatMessage};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::prompt::PromptPlan;
use rustytavern_core::{Error, Result, Role};

pub use crate::session::provider::{
    OpenRouterRouting, ProviderCapabilities, ProviderConfig, ProviderKind, ToolDefinition,
};

/// The Rig completion model that serves one prepared request.
pub type CompletionModel = rig_core::DynModel<rig_core::operation::Completion>;

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

pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> &str {
        std::any::type_name::<Self>()
    }
    /// The method checks the settings and asks Rig which parameters the model refuses.
    fn validate_config(&self, model: &str, config: &ProviderConfig) -> Result<()> {
        client::check(&HttpProvider.prepare(&PromptPlan::default(), model, config)?)
    }
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest>;
    /// The pure method adds the tool definitions to a prepared request.
    fn prepare_agent(
        &self,
        mut request: PreparedRequest,
        tools: &[ToolDefinition],
    ) -> Result<PreparedRequest> {
        if tools.len() > 64 {
            return Err(Error::new(
                "invalid_provider_tool",
                "The tool request exceeds its count bound.",
            ));
        }
        let mut body = request.chat()?;
        body.tools = tools.to_vec();
        request.body = serde_json::to_value(body)?;
        Ok(request)
    }
    /// The method builds the Rig model that serves the prepared request.
    fn model(
        &self,
        request: &PreparedRequest,
        credentials: &dyn CredentialResolver,
    ) -> Result<CompletionModel>;
}

#[derive(Default)]
pub struct HttpProvider;

impl ProviderAdapter for HttpProvider {
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        request::prepare(plan, model, config)
    }

    fn model(
        &self,
        request: &PreparedRequest,
        credentials: &dyn CredentialResolver,
    ) -> Result<CompletionModel> {
        client::model(request, credentials)
    }
}
