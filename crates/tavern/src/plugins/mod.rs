//! The crate holds the essential runtime plugins.
//! Each module implements one plugin family.
//! The `essential` function lists the plugins that form the standard agent.
//! A plugin changes agent behavior or adds content to the agent context.

mod drivers;
mod prompts;
mod sources;
mod tools;

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::plugin::{PluginContext, PluginManifest, PluginRegistry, RuntimePlugin};
use crate::provider::{HttpProvider, ProviderAdapter};
use crate::runtime::driver;
use crate::session::prompt::{CoreProfile, PromptProfile};
use crate::store::StoreBackend;
use rustytavern_core::card::{CardDecoder, CcDecoder};
use rustytavern_core::{Error, Result, digest, native_artifact_hash};

/// The owner ID of the essential plugins.
pub const ESSENTIAL_OWNER: &str = "core-product";

/// The environment supplies host resources to the plugins.
#[derive(Clone)]
pub struct Environment {
    pub store: Arc<dyn StoreBackend>,
}
impl Environment {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self { store }
    }
}

/// One plugin from this crate. The factory name selects its implementation.
pub struct Builtin {
    manifest: PluginManifest,
    factory: String,
    configuration: Value,
    environment: Environment,
}
impl Builtin {
    pub fn new(
        manifest: PluginManifest,
        factory: impl Into<String>,
        configuration: Value,
        environment: Environment,
    ) -> Self {
        Self {
            manifest,
            factory: factory.into(),
            configuration,
            environment,
        }
    }
}

#[async_trait]
impl RuntimePlugin for Builtin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let configuration = &self.configuration;
        match self.factory.as_str() {
            "decoder.cc-v2-v3" => {
                let decoder: Arc<dyn CardDecoder> = Arc::new(CcDecoder);
                context.register("card.decoder/1", Arc::new(decoder))
            }
            "profile.core-v1" => {
                let profile: Arc<dyn PromptProfile> = Arc::new(CoreProfile);
                context.register("profile/1", Arc::new(profile))
            }
            "provider.http" => {
                let provider: Arc<dyn ProviderAdapter> = Arc::new(HttpProvider);
                context.register("provider/1", Arc::new(provider))
            }
            "store.sqlite" => context.register("store/1", Arc::new(self.environment.store.clone())),
            "driver.agent" => drivers::agent(context, configuration),
            "tools.web" => tools::web::activate(context),
            "tools.skills" => tools::skills::activate(context, configuration),
            #[cfg(feature = "mcp")]
            "tools.mcp" => tools::mcp::activate(context, configuration).await,
            "prompt.contributions" => prompts::contributions(context, configuration),
            "prompt.edits" => prompts::edits(context, configuration, &self.manifest.provides),
            "source.chub" => sources::activate(context, true),
            "source.risurealm" => sources::activate(context, false),
            _ => Err(Error::new(
                "unsupported_plugin_factory",
                "The application does not provide this native plugin factory.",
            )),
        }
    }
}

/// Each row has a plugin ID, a factory, a service, requested capabilities, and prompt slots.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
);

fn rows() -> Vec<Row> {
    let driver: Vec<&str> = driver::DRIVER_CAPABILITIES.to_vec();
    let mut rows: Vec<Row> = vec![
        (
            "decoder",
            "decoder.cc-v2-v3",
            "card.decoder/1",
            vec![],
            vec![],
        ),
        ("profile", "profile.core-v1", "profile/1", vec![], vec![]),
        ("provider", "provider.http", "provider/1", vec![], vec![]),
        ("store", "store.sqlite", "store/1", vec![], vec![]),
        ("agent", "driver.agent", "driver/1", driver, vec![]),
        (
            "web",
            "tools.web",
            "tools.web/1",
            vec!["network:https:*", "prompt.contribute:before_history"],
            vec!["before_history"],
        ),
        (
            "skills",
            "tools.skills",
            "tools.skills/1",
            vec!["skills.read", "prompt.contribute:before_history"],
            vec!["before_history"],
        ),
        (
            "chub",
            "source.chub",
            "card.source.chub/1",
            vec!["network:https:api.chub.ai"],
            vec![],
        ),
        (
            "risurealm",
            "source.risurealm",
            "card.source.risurealm/1",
            vec!["network:https:realm.risuai.net"],
            vec![],
        ),
    ];
    if cfg!(feature = "mcp") {
        rows.push((
            "mcp",
            "tools.mcp",
            "tools.mcp/1",
            vec!["mcp.connect"],
            vec![],
        ));
    }
    rows
}

/// The function returns the essential plugins. Every essential plugin is always on.
pub fn essential(
    environment: &Environment,
    configuration: &BTreeMap<String, Value>,
) -> Result<Vec<Builtin>> {
    rows()
        .into_iter()
        .map(|(id, factory, service, capabilities, slots)| {
            let configuration = configuration.get(id).cloned().unwrap_or_else(|| json!({}));
            let manifest = PluginManifest {
                id: id.into(),
                provides: vec![service.into()],
                artifact_hash: native_artifact_hash()?,
                configuration_hash: digest(
                    format!("{factory}:{}", serde_json::to_string(&configuration)?).as_bytes(),
                ),
                requested_capabilities: capabilities.into_iter().map(str::to_owned).collect(),
                allowed_slots: slots.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            };
            Ok(Builtin::new(
                manifest,
                factory,
                configuration,
                environment.clone(),
            ))
        })
        .collect()
}

/// The function activates the essential plugins.
/// The host grants each requested capability of an essential plugin.
pub async fn registry(
    environment: &Environment,
    configuration: &BTreeMap<String, Value>,
) -> Result<Arc<Mutex<PluginRegistry>>> {
    let plugins: Vec<Arc<dyn RuntimePlugin>> = essential(environment, configuration)?
        .into_iter()
        .map(|plugin| Arc::new(plugin) as Arc<dyn RuntimePlugin>)
        .collect();
    let grants = plugins
        .iter()
        .map(|p| {
            (
                p.manifest().id.clone(),
                p.manifest().requested_capabilities.clone(),
            )
        })
        .collect();
    let mut registry = PluginRegistry::default();
    registry.inject(plugins, ESSENTIAL_OWNER, &grants).await?;
    Ok(Arc::new(Mutex::new(registry)))
}
