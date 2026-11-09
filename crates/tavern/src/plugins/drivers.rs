//! The `agent` plugin registers the agent driver as `driver/1`.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{
    Error, Result,
    agent::AgentDriver,
    driver::{ConversationDriver, DRIVER_CAPABILITIES},
    plugin::PluginContext,
    runtime::policy::AgentPolicy,
};

/// The `driver.agent` plugin runs the tool loop with the run policy of `[plugins.agent]`.
pub(crate) fn agent(context: &mut PluginContext, configuration: &Value) -> Result<()> {
    let table = if configuration.is_null() {
        json!({})
    } else {
        configuration.clone()
    };
    let policy: AgentPolicy = serde_json::from_value(table).map_err(Error::context(
        "invalid_plugin_configuration",
        "The [plugins.agent] table is invalid.",
    ))?;
    let driver: Arc<dyn ConversationDriver> = Arc::new(AgentDriver::with_policy(policy));
    for capability in DRIVER_CAPABILITIES {
        context.authorize(capability)?;
    }
    context.register("driver/1", Arc::new(driver))
}
