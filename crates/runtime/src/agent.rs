//! The module defines the tool contract and the agent driver.
//! Plugins register tools as `tools.NAME/1` services.
//! The host runs each tool call for the driver.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    driver::{ConversationDriver, DriverContext},
    plugin::CompositionLease,
    provider::{AgentTurn, ProviderEvent, ToolDefinition},
};

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

/// The default limit of tool steps for one reply.
pub const DEFAULT_MAX_STEPS: usize = 8;

/// The agent driver is the only built-in conversation driver.
/// The loop calls the model and runs the requested tools through the host.
/// The loop stops at the first reply without tool calls.
pub struct AgentDriver {
    max_steps: usize,
}
impl AgentDriver {
    /// The provider contract limits a continuation to 64 steps.
    pub fn new(max_steps: usize) -> Self {
        Self {
            max_steps: max_steps.min(64),
        }
    }
}

#[async_trait]
impl ConversationDriver for AgentDriver {
    fn id(&self) -> &str {
        "agent/1"
    }
    async fn run(&self, context: &mut DriverContext) -> Result<()> {
        let mut continuation: Vec<AgentTurn> = Vec::new();
        loop {
            context.open_step(&continuation).await?;
            let mut turn = AgentTurn::default();
            while let Some(event) = context.next_event().await? {
                match event {
                    ProviderEvent::Text { text } => {
                        context.append_text(&text).await?;
                        turn.text.push_str(&text);
                    }
                    ProviderEvent::Reasoning { text } => context.append_reasoning(&text)?,
                    ProviderEvent::ToolCalls { calls } => turn.calls = calls,
                    ProviderEvent::Failure { error } => return Err(error),
                    ProviderEvent::Usage { .. } | ProviderEvent::Complete => {}
                }
            }
            if turn.calls.is_empty() {
                return Ok(());
            }
            if continuation.len() >= self.max_steps {
                return Err(Error::new(
                    "tool_step_limit",
                    "The agent exceeds its tool step limit.",
                ));
            }
            for call in &turn.calls {
                turn.results.push(context.call_tool(call).await?);
            }
            continuation.push(turn);
        }
    }
}
