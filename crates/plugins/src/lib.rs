//! The crate holds the essential runtime plugins.
//! Each module implements one plugin family.
//! The `essential` function lists the plugins that form the standard agent.
//! A plugin changes agent behavior or adds content to the agent context.

mod drivers;
mod prompts;
mod sources;
mod tools;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use rustytavern_adapters::provider::{HttpProvider, ProviderAdapter};
use rustytavern_adapters::{provider, source};
use rustytavern_card::{CardDecoder, CcDecoder};
use rustytavern_conversation::prompt::{self, CoreProfile, PromptProfile};
use rustytavern_core::{Error, Result, digest, native_artifact_hash};
use rustytavern_plugin::{
    self as plugin, PluginContext, PluginManifest, PluginRegistry, RuntimePlugin,
};
use rustytavern_runtime::{agent, driver, phases};
use rustytavern_store::StoreBackend;

/// The owner ID of the essential plugins.
pub const ESSENTIAL_OWNER: &str = "core-product";

/// The environment supplies host resources to the plugins.
#[derive(Clone)]
pub struct Environment {
    pub store: Arc<dyn StoreBackend>,
    /// The skills and MCP plugins read files from this directory.
    pub data_dir: Option<PathBuf>,
}
impl Environment {
    pub fn new(store: Arc<dyn StoreBackend>) -> Self {
        Self {
            store,
            data_dir: None,
        }
    }
    pub fn with_data_dir(mut self, data_dir: impl AsRef<Path>) -> Self {
        self.data_dir = Some(data_dir.as_ref().to_owned());
        self
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
        let data_dir = self.environment.data_dir.as_deref();
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
            "driver.single-reply" => drivers::single_reply(context),
            "driver.agent" => drivers::agent(context, configuration),
            "tools.web" => tools::web::activate(context),
            "tools.skills" => tools::skills::activate(context, data_dir),
            #[cfg(feature = "mcp")]
            "tools.mcp" => tools::mcp::activate(context, data_dir).await,
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

/// The function returns the essential plugins of the standard agent.
/// Each plugin requests only the capabilities that its factory uses.
pub fn essential(environment: &Environment) -> Result<Vec<Builtin>> {
    let driver: Vec<&str> = driver::DRIVER_CAPABILITIES.to_vec();
    let mut rows: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        ("decoder.cc-v2-v3", "card.decoder/1", vec![], vec![]),
        ("profile.core-v1", "profile/1", vec![], vec![]),
        ("provider.http", "provider/1", vec![], vec![]),
        ("store.sqlite", "store/1", vec![], vec![]),
        ("driver.agent", "driver/1", driver, vec![]),
        ("tools.web", "tools.web/1", vec!["network:https:*"], vec![]),
        (
            "tools.skills",
            "tools.skills/1",
            vec!["skills.read", "prompt.contribute:before_history"],
            vec!["before_history"],
        ),
        (
            "source.chub",
            "card.source.chub/1",
            vec!["network:https:api.chub.ai"],
            vec![],
        ),
        (
            "source.risurealm",
            "card.source.risurealm/1",
            vec!["network:https:realm.risuai.net"],
            vec![],
        ),
    ];
    if cfg!(feature = "mcp") {
        rows.push(("tools.mcp", "tools.mcp/1", vec!["mcp.connect"], vec![]));
    }
    rows.into_iter()
        .map(|(factory, service, capabilities, slots)| {
            let manifest = PluginManifest {
                id: format!("org.rustytavern.{factory}"),
                provides: vec![service.into()],
                artifact_hash: native_artifact_hash()?,
                configuration_hash: digest(factory.as_bytes()),
                requested_capabilities: capabilities.into_iter().map(str::to_owned).collect(),
                allowed_slots: slots.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            };
            Ok(Builtin::new(
                manifest,
                factory,
                json!({}),
                environment.clone(),
            ))
        })
        .collect()
}

/// The function activates the essential plugins, except the disabled IDs.
/// The host grants each requested capability of an essential plugin.
pub async fn registry(
    environment: &Environment,
    disabled: &BTreeSet<String>,
) -> Result<Arc<Mutex<PluginRegistry>>> {
    let plugins: Vec<Arc<dyn RuntimePlugin>> = essential(environment)?
        .into_iter()
        .filter(|plugin| !disabled.contains(&plugin.manifest.id))
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
