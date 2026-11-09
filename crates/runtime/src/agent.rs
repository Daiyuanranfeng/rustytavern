//! The module defines the tool contract and the agent driver.
//! Plugins register tools as `tools.NAME/1` services.
//! Rig runs each tool call for the driver.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use rig_core::{
    message::ToolName,
    tool::{DynamicTool, ToolExecutionError, ToolOutput},
};

use super::{EventSink, RuntimeEvent, policy::AgentPolicy};
use crate::{
    Error, Result,
    driver::{AgentEvent, ConversationDriver, DriverContext},
    plugin::CompositionLease,
    provider::ToolDefinition,
};

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String>;
}

/// A Rig tool serves as a host tool. The host renders its output as text.
#[async_trait]
impl AgentTool for DynamicTool {
    fn definition(&self) -> ToolDefinition {
        let definition = DynamicTool::definition(self);
        ToolDefinition {
            name: definition.name.as_str().to_owned(),
            description: definition.description,
            parameters: definition.parameters,
        }
    }
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(Error::new("cancelled", "The caller cancelled the tool call.")),
            output = self.run(arguments) => output
                .map(|output| output.render())
                .map_err(|error| Error::new("tool_error", error.model_output().render())),
        }
    }
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
pub const DEFAULT_MAX_STEPS: usize = 32;
/// The host truncates each tool result to this byte count.
pub const MAX_TOOL_RESULT_BYTES: usize = 65_536;

/// The function adapts a host tool to Rig.
/// The adapter emits `tool_call` and `tool_result` events.
/// A failed tool returns its error text to the model.
pub(super) fn bridge(
    tool: Arc<dyn AgentTool>,
    sink: EventSink,
    generation_id: String,
    cancellation: CancellationToken,
) -> Result<DynamicTool> {
    let definition = tool.definition();
    let name = ToolName::new(definition.name.clone()).map_err(Error::context(
        "invalid_provider_tool",
        "The tool name is empty.",
    ))?;
    let emit = move |sink: &EventSink, kind: &str, text: String| {
        let mut event = RuntimeEvent::new(kind, &generation_id);
        event.text = Some(text);
        sink.emit(event);
    };
    Ok(DynamicTool::new(
        name,
        definition.description,
        definition.parameters,
        move |arguments| {
            let (tool, sink, cancellation, emit) = (
                tool.clone(),
                sink.clone(),
                cancellation.clone(),
                emit.clone(),
            );
            let name = definition.name.clone();
            Box::pin(async move {
                emit(&sink, "tool_call", format!("{name} {arguments}"));
                let mut content = match tool.call(arguments, cancellation).await {
                    Ok(content) => content,
                    Err(error) if error.code == "cancelled" => {
                        return Err(ToolExecutionError::cancelled(error.message));
                    }
                    Err(error) => format!("Error {}: {}", error.code, error.message),
                };
                if content.len() > MAX_TOOL_RESULT_BYTES {
                    let mut end = MAX_TOOL_RESULT_BYTES;
                    while !content.is_char_boundary(end) {
                        end -= 1;
                    }
                    content.truncate(end);
                }
                emit(&sink, "tool_result", content.clone());
                Ok(ToolOutput::text(content))
            })
        },
    ))
}

/// The agent driver is the only built-in conversation driver.
/// Rig runs the loop: it calls the model and runs the requested tools.
/// The loop stops at the first reply without tool calls.
pub struct AgentDriver {
    policy: AgentPolicy,
}
impl AgentDriver {
    pub fn new(max_steps: usize) -> Self {
        Self::with_policy(max_steps.into())
    }
    /// The step limit is at most 64.
    pub fn with_policy(mut policy: AgentPolicy) -> Self {
        policy.max_steps = policy.max_steps.min(64);
        Self { policy }
    }
}

#[async_trait]
impl ConversationDriver for AgentDriver {
    fn id(&self) -> &str {
        "agent/1"
    }
    async fn run(&self, context: &mut DriverContext) -> Result<()> {
        context.open(self.policy.clone()).await?;
        while let Some(event) = context.next_event().await? {
            match event {
                AgentEvent::Text { text } => context.append_text(&text).await?,
                AgentEvent::Reasoning { text } => context.append_reasoning(&text)?,
                AgentEvent::Complete => {}
            }
        }
        Ok(())
    }
}
