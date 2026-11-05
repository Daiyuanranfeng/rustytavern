//! The `agent` plugin registers the agent driver as `driver/1`.

use std::sync::Arc;

use serde_json::Value;

use crate::{
    Result,
    agent::{AgentDriver, DEFAULT_MAX_STEPS},
    driver::{ConversationDriver, DRIVER_CAPABILITIES},
    plugin::PluginContext,
};

/// The `driver.agent` plugin runs the tool loop.
pub(crate) fn agent(context: &mut PluginContext, configuration: &Value) -> Result<()> {
    let steps = configuration["max_steps"]
        .as_u64()
        .map_or(DEFAULT_MAX_STEPS, |steps| steps as usize);
    let driver: Arc<dyn ConversationDriver> = Arc::new(AgentDriver::new(steps));
    for capability in DRIVER_CAPABILITIES {
        context.authorize(capability)?;
    }
    context.register("driver/1", Arc::new(driver))
}
