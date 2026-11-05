//! The module defines the tool contract.
//! Plugins register tools as `tools.NAME/1` services.
//! The host runs each tool call for a driver.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{Error, Result, plugin::CompositionLease, provider::ToolDefinition};

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String>;
}

/// A plugin registers this type as a `tools.NAME/1` service.
pub type ToolSet = Vec<Arc<dyn AgentTool>>;

/// The function collects every composed tool set.
pub(crate) fn tools(composition: &CompositionLease) -> Result<ToolSet> {
    let mut tools: ToolSet = Vec::new();
    for service in composition.service_names("tools.") {
        if !service.ends_with("/1") {
            return Err(Error::new(
                "incompatible_plugin_protocol",
                "The tool set requires another service protocol.",
            ));
        }
        for tool in composition.service::<ToolSet>(&service)?.iter() {
            let name = tool.definition().name;
            if tools.iter().any(|t| t.definition().name == name) {
                return Err(Error::new(
                    "duplicate_tool",
                    format!("Two plugins register the {name} tool."),
                ));
            }
            tools.push(tool.clone());
        }
    }
    Ok(tools)
}
