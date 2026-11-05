//! The driver plugins register `driver/1`.
//! The agent loop follows the minimal pi-agent-core design.
//! The loop calls the model and runs the requested tools through the host.
//! The loop stops at the first reply without tool calls.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    Error, Result,
    driver::{ConversationDriver, DRIVER_CAPABILITIES, DriverContext, SingleReplyDriver},
    plugin::PluginContext,
    provider::{AgentProviderEvent, AgentTurn},
};

/// The `driver.single-reply` plugin sends one text request.
pub(crate) fn single_reply(context: &mut PluginContext) -> Result<()> {
    register(context, Arc::new(SingleReplyDriver))
}

/// The `driver.agent` plugin runs the tool loop.
pub(crate) fn agent(context: &mut PluginContext, configuration: &Value) -> Result<()> {
    let steps = configuration["max_steps"].as_u64().unwrap_or(8) as usize;
    register(context, Arc::new(AgentDriver::new(steps)))
}

fn register(context: &mut PluginContext, driver: Arc<dyn ConversationDriver>) -> Result<()> {
    for capability in DRIVER_CAPABILITIES {
        context.authorize(capability)?;
    }
    context.register("driver/1", Arc::new(driver))
}

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
    fn uses_tools(&self) -> bool {
        true
    }
    async fn run(&self, context: &mut DriverContext) -> Result<()> {
        let mut continuation: Vec<AgentTurn> = Vec::new();
        loop {
            context.open_agent_step(&continuation).await?;
            let mut turn = AgentTurn::default();
            while let Some(event) = context.next_agent_event().await? {
                match event {
                    AgentProviderEvent::Text { text } => {
                        context.append_text(&text).await?;
                        turn.text.push_str(&text);
                    }
                    AgentProviderEvent::ToolCalls { calls } => turn.calls = calls,
                    AgentProviderEvent::Failure { error } => return Err(error),
                    AgentProviderEvent::Usage { .. } | AgentProviderEvent::Complete => {}
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
