//! The prompt plugins add static contributions and static section edits.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    Error, Result,
    phases::{HookPolicy, PromptHook, SectionEdit, TransformInput},
    plugin::{Contribution, PluginContext},
};

/// The `prompt.contributions` plugin adds each configured contribution.
pub(crate) fn contributions(context: &mut PluginContext, configuration: &Value) -> Result<()> {
    let contributions: Vec<Contribution> =
        serde_json::from_value(configuration["contributions"].clone())?;
    for contribution in contributions {
        context.contribute(contribution)?;
    }
    Ok(())
}

/// The `prompt.edits` plugin registers one pure hook for each declared service.
pub(crate) fn edits(
    context: &mut PluginContext,
    configuration: &Value,
    provides: &[String],
) -> Result<()> {
    let hook: Arc<dyn PromptHook> = Arc::new(serde_json::from_value::<StaticPromptEdits>(
        configuration.clone(),
    )?);
    for service in provides {
        if !service.starts_with("prompt.hook.") || !service.ends_with("/1") {
            return Err(Error::new(
                "unsupported_plugin_service",
                "The edit factory provides prompt hooks only.",
            ));
        }
        context.register(service, Arc::new(hook.clone()))?;
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StaticPromptEdits {
    policy: HookPolicy,
    edits: Vec<SectionEdit>,
}
#[async_trait]
impl PromptHook for StaticPromptEdits {
    fn policy(&self) -> HookPolicy {
        self.policy.clone()
    }
    fn transform(&self, _input: &TransformInput<'_>) -> Result<Vec<SectionEdit>> {
        Ok(self.edits.clone())
    }
}
