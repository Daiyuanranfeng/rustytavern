//! The minimal agent loop follows the pi-agent-core design.
//! The loop calls the model and runs the requested tools.
//! The loop stops at the first reply without tool calls.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    driver::{ConversationDriver, DriverContext, SingleReplyDriver},
    provider::{AgentProviderEvent, AgentRequest, AgentTurn, ToolDefinition, ToolResult},
};

/// The driver truncates each tool result to this byte count.
pub const MAX_TOOL_RESULT_BYTES: usize = 65_536;

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String>;
}

pub struct AgentDriver {
    tools: Vec<Arc<dyn AgentTool>>,
    max_steps: usize,
}
impl AgentDriver {
    /// The provider contract limits a continuation to 64 steps.
    pub fn new(tools: Vec<Arc<dyn AgentTool>>, max_steps: usize) -> Self {
        Self {
            tools,
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
        if self.tools.is_empty() {
            return SingleReplyDriver.run(context).await;
        }
        let mut agent = AgentRequest {
            tools: self.tools.iter().map(|tool| tool.definition()).collect(),
            continuation: Vec::new(),
        };
        loop {
            context.open_agent_step(&agent).await?;
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
            if agent.continuation.len() >= self.max_steps {
                return Err(Error::new(
                    "tool_step_limit",
                    "The agent exceeds its tool step limit.",
                ));
            }
            for call in &turn.calls {
                context.report_tool(call, None)?;
                let index = agent.tools.iter().position(|t| t.name == call.name);
                let result = match index {
                    Some(index) => {
                        self.tools[index]
                            .call(call.arguments.clone(), context.cancellation_token())
                            .await
                    }
                    None => Err(Error::new("unknown_tool", "The tool does not exist.")),
                };
                let mut content = match result {
                    Ok(content) => content,
                    Err(error) if error.code == "cancelled" => return Err(error),
                    Err(error) => format!("Error {}: {}", error.code, error.message),
                };
                if content.len() > MAX_TOOL_RESULT_BYTES {
                    let mut end = MAX_TOOL_RESULT_BYTES;
                    while !content.is_char_boundary(end) {
                        end -= 1;
                    }
                    content.truncate(end);
                }
                context.report_tool(call, Some(&content))?;
                turn.results.push(ToolResult {
                    call_id: call.id.clone(),
                    content,
                });
            }
            agent.continuation.push(turn);
        }
    }
}
