//! The module installs third-party plugins and assembles the runtime.
//! A plugin directory contains one `plugin.json` file and the files that the plugin uses.
pub use crate::config;
use crate::{Error, Result, external, plugin, runtime, source, store};
use rustytavern_core::{bounded_read, checked_file, digest, native_artifact_hash, safe_relative};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::plugins::{self, Builtin, Environment};
use external::{ExternalSourcePlugin, ProcessDefinition};
use plugin::{CompositionLease, PluginManifest, PluginRegistry, RuntimePlugin};
use runtime::Runtime;
use rustytavern_card::{Card, CardDecoder};
use source::{Artifact, CardSource};
use store::{StoreBackend, new_id};

const APP_NAMESPACE: &str = "rustytavern.application";
const CATALOG_KEY: &str = "plugins";
const MANIFEST: &str = "plugin.json";
const MAX_PLUGIN_BYTES: usize = 64 * 1024 * 1024;
const MAX_PLUGIN_FILES: usize = 512;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub definition: PluginDefinition,
    pub hash: String,
    pub directory: String,
    pub files: BTreeMap<String, String>,
    pub enabled: bool,
    pub grants: BTreeSet<String>,
    pub trusted_processes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Catalog {
    schema_version: u32,
    plugins: BTreeMap<String, InstalledPlugin>,
    bindings: BTreeMap<String, String>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            schema_version: 1,
            plugins: BTreeMap::new(),
            bindings: BTreeMap::new(),
        }
    }
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
pub struct PluginManager {
    data_dir: PathBuf,
    store: Arc<dyn StoreBackend>,
    pub registry: Arc<Mutex<PluginRegistry>>,
    catalog: Catalog,
    revision: i64,
    factories: BTreeMap<String, NativeFactory>,
}

impl PluginManager {
    pub fn open(
        data_dir: impl AsRef<Path>,
        store: Arc<dyn StoreBackend>,
        registry: Arc<Mutex<PluginRegistry>>,
    ) -> Result<Self> {
        let (revision, catalog) = match store.state_record(APP_NAMESPACE, CATALOG_KEY)? {
            Some((revision, value)) => (revision, serde_json::from_value::<Catalog>(value)?),
            None => (0, Catalog::default()),
        };
        if catalog.schema_version != 1 {
            return Err(Error::new(
                "future_schema",
                "The plugin catalog requires another application version.",
            ));
        }
        let data_dir = data_dir.as_ref().to_owned();
        std::fs::create_dir_all(data_dir.join("plugins"))?;
        Ok(Self {
            data_dir,
            store,
            registry,
            catalog,
            revision,
            factories: BTreeMap::new(),
        })
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

    pub fn list(&self) -> Vec<InstalledPlugin> {
        self.catalog.plugins.values().cloned().collect()
    }
    pub fn inspect(&self, id: &str) -> Result<InstalledPlugin> {
        self.catalog
            .plugins
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "The plugin is not installed."))
    }

    /// The method copies and hashes a plugin directory. It does not start the plugin.
    pub fn install(&mut self, source: &Path) -> Result<InstalledPlugin> {
        let (definition, files) = validate_plugin(source)?;
        let id = definition.manifest.id.clone();
        if self.catalog.plugins.get(&id).is_some_and(|p| p.enabled) {
            return Err(Error::new(
                "plugin_in_use",
                "The caller must disable this plugin before an update.",
            ));
        }
        let hash = digest(&serde_json::to_vec(&files)?);
        let directory = format!("{id}-{}-{}", definition.manifest.version, &hash[..16]);
        let final_dir = self.data_dir.join("plugins").join(&directory);
        let stage = self.data_dir.join("plugins").join(new_id("staging"));
        let staged = (|| -> Result<()> {
            std::fs::create_dir(&stage)?;
            for (relative, expected) in &files {
                let original = checked_file(source, relative)?;
                let bytes = bounded_read(&original, MAX_PLUGIN_BYTES)?;
                if digest(&bytes) != *expected {
                    return Err(Error::new(
                        "invalid_plugin_hash",
                        "The plugin changed during installation.",
                    ));
                }
                let destination = stage.join(relative);
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&destination, bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(&original)?.permissions().mode() & 0o777;
                    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(mode))?;
                }
            }
            validate_plugin(&stage)?;
            if final_dir.exists() {
                verify_files(&final_dir, &files)?;
                std::fs::remove_dir_all(&stage)?;
            } else {
                std::fs::rename(&stage, &final_dir)?;
            }
            Ok(())
        })();
        if staged.is_err() {
            let _ = std::fs::remove_dir_all(&stage);
        }
        staged?;
        let installed = InstalledPlugin {
            definition,
            hash,
            directory,
            files,
            enabled: false,
            grants: BTreeSet::new(),
            trusted_processes: false,
        };
        let mut catalog = self.catalog.clone();
        catalog.plugins.insert(id, installed.clone());
        self.save(catalog)?;
        Ok(installed)
    }

    pub async fn restore(&mut self) -> Result<()> {
        let ids = self
            .catalog
            .plugins
            .values()
            .filter(|p| p.enabled)
            .map(|p| p.definition.manifest.id.clone())
            .collect::<Vec<_>>();
        self.activate(&ids, None).await
    }

    pub async fn enable(
        &mut self,
        ids: &[String],
        grants: BTreeSet<String>,
        trust_processes: bool,
    ) -> Result<()> {
        let mut catalog = self.catalog.clone();
        for id in ids {
            let installed = catalog
                .plugins
                .get_mut(id)
                .ok_or_else(|| Error::new("not_found", "The plugin is not installed."))?;
            if installed.enabled {
                return Err(Error::new(
                    "plugin_in_use",
                    "The plugin is already enabled.",
                ));
            }
            installed.enabled = true;
            installed.grants = grants.clone();
            installed.trusted_processes = trust_processes;
        }
        self.activate(ids, Some(catalog)).await
    }

    async fn activate(&mut self, ids: &[String], update: Option<Catalog>) -> Result<()> {
        let target = update.as_ref().unwrap_or(&self.catalog);
        commands(target)?;
        let mut plugins: Vec<Arc<dyn RuntimePlugin>> = Vec::new();
        let mut permissions = BTreeMap::new();
        for id in ids {
            let installed = &target.plugins[id];
            let root = self.plugin_root(installed)?;
            verify_files(&root, &installed.files)?;
            let definition = load_definition(&root)?;
            if matches!(definition.implementation, Implementation::Process { .. })
                && !installed.trusted_processes
            {
                return Err(Error::new(
                    "capability_denied",
                    "The plugin requires explicit trust for its process executable.",
                ));
            }
            let effective = installed
                .grants
                .intersection(&definition.manifest.requested_capabilities)
                .cloned()
                .collect::<BTreeSet<_>>();
            permissions.insert(id.clone(), effective);
            plugins.push(self.build_plugin(definition, &root)?);
        }
        let mut registry = self.registry.lock().await;
        let previous_bindings = registry.bindings().clone();
        let batch = new_id("plugin-activation");
        registry
            .inject_bound(plugins, &batch, &permissions, target.bindings.clone())
            .await?;
        let validation = registry.freeze().and_then(|lease| {
            for command in commands(target)?.values() {
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
        });
        let committed = validation.and_then(|()| {
            update
                .as_ref()
                .map_or(Ok(self.revision), |catalog| self.persist(catalog))
        });
        let revision = match committed {
            Ok(revision) => revision,
            Err(error) => {
                registry.eject_owner(&batch).await?;
                registry.replace_bindings(previous_bindings)?;
                return Err(error);
            }
        };
        if let Some(catalog) = update {
            self.catalog = catalog;
            self.revision = revision;
        }
        for id in ids {
            registry.add_owner(id, &owner(id))?;
        }
        registry.eject_owner(&batch).await?;
        Ok(())
    }

    pub async fn disable(&mut self, id: &str) -> Result<()> {
        if !self.inspect(id)?.enabled {
            return Ok(());
        }
        let mut catalog = self.catalog.clone();
        if let Some(installed) = catalog.plugins.get_mut(id) {
            installed.enabled = false;
        }
        catalog.bindings.retain(|_, provider| provider != id);
        let mut registry = self.registry.lock().await;
        registry.can_eject_owner(&owner(id))?;
        let revision = self.persist(&catalog)?;
        registry.eject_owner(&owner(id)).await?;
        registry.replace_bindings(catalog.bindings.clone())?;
        self.catalog = catalog;
        self.revision = revision;
        Ok(())
    }

    pub fn uninstall(&mut self, id: &str) -> Result<()> {
        if self.inspect(id)?.enabled {
            return Err(Error::new(
                "plugin_in_use",
                "The caller must disable the plugin before uninstall.",
            ));
        }
        let mut catalog = self.catalog.clone();
        catalog.plugins.remove(id);
        self.save(catalog)
    }

    pub async fn bind(&mut self, service: String, plugin: String) -> Result<()> {
        let mut registry = self.registry.lock().await;
        let previous = registry.bindings().clone();
        let found =
            registry
                .inspect()
                .iter()
                .any(|p| p.manifest.id == plugin && p.manifest.provides.contains(&service))
                || self.catalog.plugins.get(&plugin).is_some_and(|p| {
                    p.enabled && p.definition.manifest.provides.contains(&service)
                });
        if !found {
            return Err(Error::new(
                "missing_service",
                "The selected plugin does not declare this service.",
            ));
        }
        registry.set_binding(service.clone(), plugin.clone())?;
        let mut catalog = self.catalog.clone();
        catalog.bindings.insert(service, plugin);
        match self.persist(&catalog) {
            Ok(revision) => {
                self.catalog = catalog;
                self.revision = revision;
                Ok(())
            }
            Err(error) => {
                registry.replace_bindings(previous)?;
                Err(error)
            }
        }
    }

    /// The method lists each installed plugin without starting its process.
    pub fn declared_plugins(&self) -> Vec<Value> {
        self.catalog.plugins.values().map(|p| json!({"manifest":p.definition.manifest,"hash":p.hash,"state":if p.enabled {"configured"} else {"disabled"},"process_trust_required":matches!(p.definition.implementation,Implementation::Process{..})})).collect()
    }

    pub fn commands(&self) -> Result<BTreeMap<String, CommandDefinition>> {
        commands(&self.catalog)
    }

    pub async fn fetch_card(
        &self,
        url: &str,
        source: &str,
        cancellation: CancellationToken,
    ) -> Result<Card> {
        let service = format!("card.source.{source}/1");
        self.fetch_service(url, &service, cancellation).await
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
                let card = self.accept_artifact(lease, decoder, artifact).await?;
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
        cancellation: CancellationToken,
    ) -> Result<Card> {
        let lease = self.registry.lock().await.freeze()?;
        let decoder = lease.service::<Arc<dyn CardDecoder>>("card.decoder/1")?;
        let source = lease.service::<Arc<dyn CardSource>>(service)?;
        let artifact = source.fetch(url, cancellation).await?;
        self.accept_artifact(lease, decoder, artifact).await
    }

    async fn accept_artifact(
        &self,
        lease: CompositionLease,
        decoder: Arc<Arc<dyn CardDecoder>>,
        artifact: Artifact,
    ) -> Result<Card> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            let card = Card::import_with(&artifact.bytes, decoder.as_ref().as_ref())?
                .with_provenance(&artifact.provenance)?;
            store.save_card(&card)?;
            Ok(card)
        })
        .await
        .map_err(Error::context(
            "card_decoder_failure",
            "The card decoder worker failed.",
        ))?
    }

    fn save(&mut self, catalog: Catalog) -> Result<()> {
        let revision = self.persist(&catalog)?;
        self.catalog = catalog;
        self.revision = revision;
        Ok(())
    }
    fn persist(&self, catalog: &Catalog) -> Result<i64> {
        self.store.state_write_if(
            APP_NAMESPACE,
            CATALOG_KEY,
            Some(self.revision),
            &serde_json::to_value(catalog)?,
        )
    }
    fn plugin_root(&self, installed: &InstalledPlugin) -> Result<PathBuf> {
        safe_relative(&installed.directory)?;
        Ok(self.data_dir.join("plugins").join(&installed.directory))
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
                let environment =
                    Environment::new(self.store.clone()).with_data_dir(&self.data_dir);
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

/// The function returns the commands of the enabled plugins.
fn commands(catalog: &Catalog) -> Result<BTreeMap<String, CommandDefinition>> {
    let mut commands = BTreeMap::new();
    for installed in catalog.plugins.values().filter(|p| p.enabled) {
        for command in &installed.definition.commands {
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

/// The function activates the essential plugins of the standard agent.
pub async fn default_registry(store: Arc<dyn StoreBackend>) -> Result<Arc<Mutex<PluginRegistry>>> {
    plugins::registry(&Environment::new(store), &BTreeSet::new()).await
}

pub async fn runtime(manager: &PluginManager) -> Result<Runtime> {
    let lease = manager.registry.lock().await.freeze()?;
    let store = lease
        .service::<Arc<dyn StoreBackend>>("store/1")?
        .as_ref()
        .clone();
    Ok(Runtime::new(store)
        .with_credentials(Arc::new(crate::config::StoredCredentials::new(
            &manager.data_dir,
        )))
        .with_plugins(manager.registry.clone())
        .with_product_composition(manager.composition().await?))
}

fn plugin_files(root: &Path) -> Result<BTreeMap<String, String>> {
    let root = root.canonicalize()?;
    let mut pending = vec![root.clone()];
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(Error::new(
                    "invalid_plugin_path",
                    "The plugin directory cannot contain a symbolic link.",
                ));
            }
            if kind.is_dir() {
                pending.push(path);
                if pending.len() + files.len() > MAX_PLUGIN_FILES {
                    return Err(Error::new(
                        "invalid_plugin_size",
                        "The plugin directory contains too many paths.",
                    ));
                }
                continue;
            }
            if !kind.is_file() {
                return Err(Error::new(
                    "invalid_plugin_path",
                    "The plugin directory requires regular files and directories.",
                ));
            }
            if files.len() >= MAX_PLUGIN_FILES {
                return Err(Error::new(
                    "invalid_plugin_size",
                    "The plugin directory contains too many files.",
                ));
            }
            let relative = path
                .strip_prefix(&root)
                .map_err(Error::context(
                    "invalid_plugin_path",
                    "The plugin path is invalid.",
                ))?
                .to_str()
                .ok_or_else(|| {
                    Error::new("invalid_plugin_path", "The plugin filename requires UTF-8.")
                })?
                .to_owned();
            safe_relative(&relative)?;
            let bytes = bounded_read(&path, MAX_PLUGIN_BYTES.saturating_sub(total))?;
            total += bytes.len();
            files.insert(relative, digest(&bytes));
        }
    }
    Ok(files)
}

fn verify_files(root: &Path, expected: &BTreeMap<String, String>) -> Result<()> {
    if plugin_files(root)? != *expected {
        return Err(Error::new(
            "invalid_plugin_hash",
            "The installed plugin changed.",
        ));
    }
    Ok(())
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

fn validate_plugin(root: &Path) -> Result<(PluginDefinition, BTreeMap<String, String>)> {
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
    Ok((definition, plugin_files(root)?))
}
