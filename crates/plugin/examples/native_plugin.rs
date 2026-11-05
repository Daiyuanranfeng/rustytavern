//! This example injects a native prompt plugin without a model request.

use async_trait::async_trait;
use rustytavern_conversation::prompt::{PromptPlan, apply_contributions};
use rustytavern_core::{Result, Role};
use rustytavern_plugin::{
    Contribution, PluginContext, PluginManifest, PluginRegistry, RuntimePlugin,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

struct HarborNote {
    manifest: PluginManifest,
}
#[async_trait]
impl RuntimePlugin for HarborNote {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        context.contribute(Contribution {
            plugin_id: String::new(),
            provenance: serde_json::Value::Null,
            retention: Default::default(),
            section_id: "harbor".into(),
            slot: "before_character".into(),
            priority: 0,
            role: Role::System,
            text: "The harbor has a chart room.".into(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut registry = PluginRegistry::default();
    let mut original = PromptPlan::default();
    original.refresh()?;
    let capability = "prompt.contribute:before_character".to_owned();
    let plugin = HarborNote {
        manifest: PluginManifest {
            id: "example.native-note".into(),
            allowed_slots: BTreeSet::from(["before_character".into()]),
            requested_capabilities: BTreeSet::from([capability.clone()]),
            ..Default::default()
        },
    };
    let grants = BTreeMap::from([("example.native-note".into(), BTreeSet::from([capability]))]);
    registry
        .inject(vec![Arc::new(plugin)], "example", &grants)
        .await?;
    let mut changed = original.clone();
    apply_contributions(&mut changed, &registry.freeze()?.contributions)?;
    registry.eject_owner("example").await?;
    let mut restored = original.clone();
    apply_contributions(&mut restored, &registry.freeze()?.contributions)?;
    assert_ne!(original.hash, changed.hash);
    assert_eq!(original.hash, restored.hash);
    println!(
        "{}",
        json!({"plugin_changes_prompt":true,"ejection_restores_prompt":true,"model_requests":0})
    );
    Ok(())
}
