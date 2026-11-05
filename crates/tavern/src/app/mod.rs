//! The module installs third-party plugins and assembles the runtime.
//! An installed plugin is a local directory with one `plugin.json` file.
//! The `[plugins]` table of `config.toml` records each installed plugin.
pub use crate::config;
use crate::{Error, Result, external, plugin, runtime, source, store};
use rustytavern_core::{bounded_read, checked_file, digest, native_artifact_hash};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::{InstalledPlugin, PluginSettings};
use crate::plugins::{self, Builtin, Environment};
use external::{ExternalSourcePlugin, ProcessDefinition};
use plugin::{CompositionLease, PluginManifest, PluginRegistry, RuntimePlugin};
use runtime::Runtime;
use rustytavern_card::{Card, CardDecoder};
use source::{Artifact, CardSource};
use store::{StoreBackend, new_id};

const MANIFEST: &str = "plugin.json";
const MAX_PLUGIN_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDefinition {
    pub name: String,
    pub service: String,
    pub operation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Implementation {
    Builtin {
        name: String,
        #[serde(default)]
        configuration: Value,
    },
    Process {
        process: ProcessDefinition,
    },
}

/// The `plugin.json` file holds the runtime manifest, the implementation, and optional commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDefinition {
    #[serde(flatten)]
    pub manifest: PluginManifest,
    pub implementation: Implementation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CommandDefinition>,
}

#[async_trait]
pub trait CommandService: Send + Sync {
    async fn call(
        &self,
        operation: &str,
        arguments: &Value,
        cancellation: CancellationToken,
    ) -> Result<Value>;
}

pub type NativeFactory =
    Arc<dyn Fn(&PluginDefinition) -> Result<Arc<dyn RuntimePlugin>> + Send + Sync>;

/// The manager installs, enables, and removes third-party plugins.
/// The caller saves `settings` into `config.toml` after a change.
pub struct PluginManager {
    store: Arc<dyn StoreBackend>,
    pub registry: Arc<Mutex<PluginRegistry>>,
    settings: PluginSettings,
    active: BTreeMap<String, PluginDefinition>,
    factories: BTreeMap<String, NativeFactory>,
}

impl PluginManager {
    pub fn open(
        store: Arc<dyn StoreBackend>,
        registry: Arc<Mutex<PluginRegistry>>,
        settings: &PluginSettings,
    ) -> Self {
        Self {
            store,
            registry,
            settings: settings.clone(),
            active: BTreeMap::new(),
            factories: BTreeMap::new(),
        }
    }
    pub fn settings(&self) -> &PluginSettings {
        &self.settings
    }

    pub fn register_factory(&mut self, name: String, factory: NativeFactory) -> Result<()> {
        if self.factories.contains_key(&name) {
            return Err(Error::new(
                "invalid_plugin_factory",
                "The factory name already exists.",
            ));
        }
        self.factories.insert(name, factory);
        Ok(())
    }

    /// The method lists each installed plugin without starting its process.
    pub fn list(&self) -> Result<Vec<Value>> {
        let enabled = self.settings.enabled();
        self.settings
            .installed()?
            .into_iter()
            .map(|(id, installed)| {
                let definition = load_definition(&installed.path)?;
                Ok(json!({"manifest":definition.manifest,"path":installed.path,"enabled":enabled.contains(&id),"grants":installed.grants,"process_trust_required":matches!(definition.implementation,Implementation::Process{..})}))
            })
            .collect()
    }

    /// The method activates each enabled installed plugin and applies the bindings.
    pub async fn restore(&mut self) -> Result<()> {
        let enabled = self.settings.enabled();
        let ids = self
            .settings
            .installed()?
            .into_keys()
            .filter(|id| enabled.contains(id))
            .collect::<Vec<_>>();
        if ids.is_empty() && !self.settings.bindings.is_empty() {
            self.registry
                .lock()
                .await
                .replace_bindings(self.settings.bindings.clone())?;
        }
        self.activate(&ids).await
    }

    /// The method checks and activates a plugin directory, then records and enables it.
    pub async fn install(
        &mut self,
        path: &Path,
        grants: BTreeSet<String>,
        trust_processes: bool,
    ) -> Result<PluginDefinition> {
        let path = path.canonicalize()?;
        let definition = validate_plugin(&path)?;
        let id = definition.manifest.id.clone();
        if self.active.contains_key(&id) {
            return Err(Error::new(
                "plugin_in_use",
                "The caller must disable this plugin before an update.",
            ));
        }
        let installed = InstalledPlugin {
            path,
            grants,
            trust_processes,
        };
        let previous = self.settings.clone();
        self.settings
            .entries
            .insert(id.clone(), serde_json::to_value(&installed)?);
        let mut enabled = self.settings.enabled();
        enabled.insert(id.clone());
        self.settings.enabled = Some(enabled);
        if let Err(error) = self.activate(std::slice::from_ref(&id)).await {
            self.settings = previous;
            return Err(error);
        }
        Ok(definition)
    }

    pub async fn uninstall(&mut self, id: &str) -> Result<()> {
        if !self.settings.installed()?.contains_key(id) {
            return Err(Error::new("not_found", "The plugin is not installed."));
        }
        self.disable(id).await?;
        self.settings.entries.remove(id);
        Ok(())
    }

    /// The method adds an installed plugin to the enabled list. The essential plugins are always on.
    pub async fn enable(&mut self, id: &str) -> Result<()> {
        if !self.settings.installed()?.contains_key(id) {
            return Err(Error::new("not_found", "The plugin is not installed."));
        }
        let mut enabled = self.settings.enabled();
        enabled.insert(id.into());
        self.settings.enabled = Some(enabled);
        if !self.active.contains_key(id) {
            self.activate(&[id.into()]).await?;
        }
        Ok(())
    }

    /// The method removes an installed plugin from the enabled list. The essential plugins are always on.
    pub async fn disable(&mut self, id: &str) -> Result<()> {
        if !self.settings.installed()?.contains_key(id) {
            return Err(Error::new("not_found", "The plugin is not installed."));
        }
        if self.active.contains_key(id) {
            self.registry.lock().await.eject_owner(&owner(id)).await?;
            self.active.remove(id);
        }
        let mut enabled = self.settings.enabled();
        enabled.remove(id);
        self.settings.enabled = Some(enabled);
        self.settings.bindings.retain(|_, provider| provider != id);
        Ok(())
    }

    pub async fn bind(&mut self, service: String, plugin: String) -> Result<()> {
        let mut registry = self.registry.lock().await;
        let active = registry
            .inspect()
            .iter()
            .any(|p| p.manifest.id == plugin && p.manifest.provides.contains(&service));
        if !active {
            return Err(Error::new(
                "missing_service",
                "The selected plugin does not provide this service.",
            ));
        }
        registry.set_binding(service.clone(), plugin.clone())?;
        self.settings.bindings.insert(service, plugin);
        Ok(())
    }

    async fn activate(&mut self, ids: &[String]) -> Result<()> {
        let installed = self.settings.installed()?;
        let mut plugins: Vec<Arc<dyn RuntimePlugin>> = Vec::new();
        let mut permissions = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        for id in ids {
            let entry = installed
                .get(id)
                .ok_or_else(|| Error::new("not_found", "The plugin is not installed."))?;
            let definition = load_definition(&entry.path)?;
            if definition.manifest.id != *id {
                return Err(Error::new(
                    "invalid_plugin_id",
                    "The plugin.json ID differs from its configuration table name.",
                ));
            }
            if matches!(definition.implementation, Implementation::Process { .. })
                && !entry.trust_processes
            {
                return Err(Error::new(
                    "capability_denied",
                    "The plugin requires explicit trust for its process executable.",
                ));
            }
            let effective = entry
                .grants
                .intersection(&definition.manifest.requested_capabilities)
                .cloned()
                .collect::<BTreeSet<_>>();
            permissions.insert(id.clone(), effective);
            plugins.push(self.build_plugin(definition.clone(), &entry.path)?);
            definitions.insert(id.clone(), definition);
        }
        if plugins.is_empty() {
            return Ok(());
        }
        let mut registry = self.registry.lock().await;
        let previous_bindings = registry.bindings().clone();
        let batch = new_id("plugin-activation");
        registry
            .inject_bound(
                plugins,
                &batch,
                &permissions,
                self.settings.bindings.clone(),
            )
            .await?;
        let validation = registry
            .freeze()
            .and_then(|lease| validate_commands(&lease, definitions.values()));
        if let Err(error) = validation {
            registry.eject_owner(&batch).await?;
            registry.replace_bindings(previous_bindings)?;
            return Err(error);
        }
        for id in ids {
            registry.add_owner(id, &owner(id))?;
        }
        registry.eject_owner(&batch).await?;
        self.active.extend(definitions);
        Ok(())
    }

    pub fn commands(&self) -> Result<BTreeMap<String, CommandDefinition>> {
        let mut commands = BTreeMap::new();
        for definition in self.active.values() {
            for command in &definition.commands {
                if commands
                    .insert(command.name.clone(), command.clone())
                    .is_some()
                {
                    return Err(Error::new(
                        "invalid_plugin_command",
                        "Enabled plugins declare the same command.",
                    ));
                }
            }
        }
        Ok(commands)
    }

    /// The method downloads and decodes a card. It saves the card only when `save` is true.
    pub async fn fetch_card(
        &self,
        url: &str,
        source: &str,
        save: bool,
        cancellation: CancellationToken,
    ) -> Result<Card> {
        let service = format!("card.source.{source}/1");
        self.fetch_service(url, &service, save, cancellation).await
    }

    pub async fn run_command(
        &self,
        name: &str,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<Value> {
        let command = self.commands()?.remove(name).ok_or_else(|| {
            Error::new(
                "invalid_plugin_command",
                "The plugin command does not exist.",
            )
        })?;
        let lease = self.registry.lock().await.freeze()?;
        match lease.service::<Arc<dyn CardSource>>(&command.service) {
            Ok(source) if command.operation == "fetch" => {
                let decoder = lease.service::<Arc<dyn CardDecoder>>("card.decoder/1")?;
                let url = arguments["url"].as_str().ok_or_else(|| {
                    Error::new(
                        "invalid_plugin_command",
                        "The source command requires a URL.",
                    )
                })?;
                let artifact = source.fetch(url, cancellation).await?;
                let card = self.accept_artifact(lease, decoder, artifact, true).await?;
                let mut result = serde_json::to_value(card)?;
                result.as_object_mut().map(|value| value.remove("original"));
                Ok(result)
            }
            Ok(_) => Err(Error::new(
                "unsupported_plugin_command",
                "The source does not support this operation.",
            )),
            Err(error) if error.code == "invalid_service_type" => {
                lease
                    .service::<Arc<dyn CommandService>>(&command.service)?
                    .call(&command.operation, &arguments, cancellation)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn fetch_service(
        &self,
        url: &str,
        service: &str,
        save: bool,
        cancellation: CancellationToken,
    ) -> Result<Card> {
        let lease = self.registry.lock().await.freeze()?;
        let decoder = lease.service::<Arc<dyn CardDecoder>>("card.decoder/1")?;
        let source = lease.service::<Arc<dyn CardSource>>(service)?;
        let artifact = source.fetch(url, cancellation).await?;
        self.accept_artifact(lease, decoder, artifact, save).await
    }

    async fn accept_artifact(
        &self,
        lease: CompositionLease,
        decoder: Arc<Arc<dyn CardDecoder>>,
        artifact: Artifact,
        save: bool,
    ) -> Result<Card> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let card = Card::import_with(&artifact.bytes, decoder.as_ref().as_ref())?
                .with_provenance(&artifact.provenance)?;
            if save {
                store.save_card(&card)?;
            }
            Ok(card)
        })
        .await
        .map_err(Error::context(
            "card_decoder_failure",
            "The card decoder worker failed.",
        ))?
    }

    fn build_plugin(
        &self,
        mut definition: PluginDefinition,
        root: &Path,
    ) -> Result<Arc<dyn RuntimePlugin>> {
        definition.manifest.configuration_hash =
            digest(&serde_json::to_vec(&definition.implementation)?);
        match &mut definition.implementation {
            Implementation::Builtin {
                name,
                configuration,
            } => {
                definition.manifest.artifact_hash = native_artifact_hash()?;
                if let Some(factory) = self.factories.get(name) {
                    return factory(&definition);
                }
                let environment = Environment::new(self.store.clone());
                Ok(Arc::new(Builtin::new(
                    definition.manifest.clone(),
                    name.clone(),
                    configuration.clone(),
                    environment,
                )))
            }
            Implementation::Process { process } => {
                let executable = process.executable.to_string_lossy().to_string();
                if let Some(relative) = executable.strip_prefix("{plugin}/") {
                    process.executable = checked_file(root, relative)?;
                }
                for argument in &mut process.args {
                    if let Some(relative) = argument.strip_prefix("{plugin}/") {
                        *argument = checked_file(root, relative)?.to_string_lossy().into_owned();
                    }
                }
                let executable = if process.executable.is_absolute() {
                    process.executable.clone()
                } else {
                    return Err(Error::new(
                        "invalid_plugin_process",
                        "A plugin process requires an absolute or plugin-relative executable.",
                    ));
                };
                definition.manifest.artifact_hash =
                    digest(&bounded_read(&executable, MAX_PLUGIN_BYTES)?);
                Ok(Arc::new(ExternalSourcePlugin::new(
                    definition.manifest,
                    process.clone(),
                )))
            }
        }
    }

    pub async fn composition(&self) -> Result<Value> {
        let mut record = self.registry.lock().await.freeze()?.snapshot();
        record["preset"] = json!("tavern");
        Ok(record)
    }
}

fn owner(id: &str) -> String {
    format!("plugin:{id}")
}

fn validate_commands<'a>(
    lease: &CompositionLease,
    definitions: impl Iterator<Item = &'a PluginDefinition>,
) -> Result<()> {
    for command in definitions.flat_map(|d| &d.commands) {
        if !lease.contains_service(&command.service) {
            return Err(Error::new(
                "missing_service",
                "The plugin command requires an unavailable service.",
            ));
        }
        if lease
            .service::<Arc<dyn CardSource>>(&command.service)
            .is_ok()
        {
            if command.operation != "fetch" {
                return Err(Error::new(
                    "unsupported_plugin_command",
                    "The source command requires the fetch operation.",
                ));
            }
        } else {
            lease.service::<Arc<dyn CommandService>>(&command.service)?;
        }
    }
    Ok(())
}

/// The function activates the essential plugins of the standard agent.
/// The function activates the default essential plugins without configuration.
pub async fn default_registry(store: Arc<dyn StoreBackend>) -> Result<Arc<Mutex<PluginRegistry>>> {
    plugins::registry(&Environment::new(store), &BTreeMap::new()).await
}

pub async fn runtime(
    manager: &PluginManager,
    credentials: Arc<dyn crate::provider::CredentialResolver>,
) -> Result<Runtime> {
    let lease = manager.registry.lock().await.freeze()?;
    let store = lease
        .service::<Arc<dyn StoreBackend>>("store/1")?
        .as_ref()
        .clone();
    Ok(Runtime::new(store)
        .with_credentials(credentials)
        .with_plugins(manager.registry.clone())
        .with_product_composition(manager.composition().await?))
}

/// The function builds the runtime for one session with its session settings.
pub async fn session_runtime(app: &config::Application, session: &str) -> Result<Runtime> {
    let config = app.configuration_for(Some(session))?;
    let registry = app.registry_with(&config).await?;
    let mut manager = PluginManager::open(app.store.clone(), registry, &config.plugins);
    manager.restore().await?;
    runtime(&manager, Arc::new(app.credentials())).await
}

/// The function pins the current plugin composition of one session and keeps its provider settings.
pub async fn adopt_session(
    app: &config::Application,
    session: &str,
) -> Result<crate::session::Session> {
    let runtime = session_runtime(app, session).await?;
    let provider = runtime.provider_config(session).await?;
    let target = app.store.session(session)?;
    runtime.configure_session(&target, provider).await
}

fn load_definition(root: &Path) -> Result<PluginDefinition> {
    let bytes = bounded_read(&checked_file(root, MANIFEST)?, 1024 * 1024)?;
    let definition: PluginDefinition = serde_json::from_slice(&bytes)?;
    definition.manifest.validate()?;
    if let Implementation::Process { process } = &definition.implementation {
        if let Some(relative) = process
            .executable
            .to_str()
            .and_then(|s| s.strip_prefix("{plugin}/"))
        {
            checked_file(root, relative)?;
        }
        for argument in &process.args {
            if let Some(relative) = argument.strip_prefix("{plugin}/") {
                checked_file(root, relative)?;
            }
        }
    }
    Ok(definition)
}

fn validate_plugin(root: &Path) -> Result<PluginDefinition> {
    let definition = load_definition(root)?;
    let mut names = BTreeSet::new();
    for command in &definition.commands {
        if !rustytavern_core::valid_id(&command.name)
            || !names.insert(command.name.clone())
            || command.service.is_empty()
            || command.operation.is_empty()
        {
            return Err(Error::new(
                "invalid_plugin_command",
                "The plugin command is invalid or duplicated.",
            ));
        }
    }
    Ok(definition)
}
