pub use crate::config;
use crate::{Error, Result, external, plugin, provider, runtime, source, store};
use rustytavern_core::{
    bounded_read, checked_file, digest, native_artifact_hash, safe_relative, valid_id,
};

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

use crate::config::Configuration;
use crate::plugins::{self, Builtin, Environment};
use external::{ExternalSourcePlugin, ProcessDefinition};
use plugin::{CompositionLease, PluginManifest, PluginRegistry, RuntimePlugin};
use provider::ProviderConfig;
use runtime::Runtime;
use rustytavern_card::{Card, CardDecoder};
use source::{Artifact, CardSource};
use store::{StoreBackend, new_id};

const APP_NAMESPACE: &str = "rustytavern.application";
const CATALOG_KEY: &str = "packages";
const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKAGE_FILES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDefinition {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDefinition {
    pub name: String,
    pub service: String,
    pub operation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEntry {
    pub manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PackageManifest {
    pub id: String,
    pub version: String,
    pub product_api: String,
    pub dependencies: BTreeMap<String, String>,
    pub plugins: Vec<PluginEntry>,
    pub commands: Vec<CommandDefinition>,
    pub configuration_defaults: Value,
    pub bindings: BTreeMap<String, String>,
    pub assets: Vec<AssetDefinition>,
}
impl Default for PackageManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            version: "0.1.0".into(),
            product_api: "1".into(),
            dependencies: BTreeMap::new(),
            plugins: Vec::new(),
            commands: Vec::new(),
            configuration_defaults: json!({}),
            bindings: BTreeMap::new(),
            assets: Vec::new(),
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDefinition {
    #[serde(flatten)]
    pub manifest: PluginManifest,
    pub implementation: Implementation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub manifest: PackageManifest,
    pub package_hash: String,
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
    packages: BTreeMap<String, InstalledPackage>,
    bindings: BTreeMap<String, String>,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            schema_version: 1,
            packages: BTreeMap::new(),
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

pub struct PackageManager {
    data_dir: PathBuf,
    store: Arc<dyn StoreBackend>,
    pub registry: Arc<Mutex<PluginRegistry>>,
    catalog: Catalog,
    revision: i64,
    factories: BTreeMap<String, NativeFactory>,
}

impl PackageManager {
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
                "The package catalog requires another application version.",
            ));
        }
        let data_dir = data_dir.as_ref().to_owned();
        std::fs::create_dir_all(data_dir.join("packages"))?;
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

    pub fn list(&self) -> Vec<InstalledPackage> {
        self.catalog.packages.values().cloned().collect()
    }
    pub fn inspect(&self, id: &str) -> Result<InstalledPackage> {
        self.catalog
            .packages
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "The package is not installed."))
    }

    pub fn install(&mut self, source: &Path) -> Result<InstalledPackage> {
        let (manifest, files) = validate_package(source)?;
        if self
            .catalog
            .packages
            .get(&manifest.id)
            .is_some_and(|p| p.enabled)
        {
            return Err(Error::new(
                "package_in_use",
                "The caller must disable this package before an update.",
            ));
        }
        let package_hash = digest(&serde_json::to_vec(&files)?);
        let directory = format!(
            "{}-{}-{}",
            manifest.id,
            manifest.version,
            &package_hash[..16]
        );
        let final_dir = self.data_dir.join("packages").join(&directory);
        let stage = self.data_dir.join("packages").join(new_id("staging"));
        let staged = (|| -> Result<()> {
            std::fs::create_dir(&stage)?;
            for (relative, hash) in &files {
                let original = checked_file(source, relative)?;
                let bytes = bounded_read(&original, MAX_PACKAGE_BYTES)?;
                if digest(&bytes) != *hash {
                    return Err(Error::new(
                        "invalid_package_hash",
                        "The package changed during installation.",
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
            validate_package(&stage)?;
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
        let package = InstalledPackage {
            manifest,
            package_hash,
            directory,
            files,
            enabled: false,
            grants: BTreeSet::new(),
            trusted_processes: false,
        };
        let mut catalog = self.catalog.clone();
        catalog
            .packages
            .insert(package.manifest.id.clone(), package.clone());
        self.save(catalog)?;
        Ok(package)
    }

    pub async fn restore(&mut self) -> Result<()> {
        let ids = self
            .catalog
            .packages
            .values()
            .filter(|p| p.enabled)
            .map(|p| p.manifest.id.clone())
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
            let package = catalog
                .packages
                .get_mut(id)
                .ok_or_else(|| Error::new("not_found", "The package is not installed."))?;
            if package.enabled {
                return Err(Error::new(
                    "package_in_use",
                    "The package is already enabled.",
                ));
            }
            package.enabled = true;
            package.grants = grants.clone();
            package.trusted_processes = trust_processes;
        }
        self.activate(ids, Some(catalog)).await
    }

    async fn activate(&mut self, ids: &[String], update: Option<Catalog>) -> Result<()> {
        let target = update.as_ref().unwrap_or(&self.catalog);
        let order = package_order(target)?;
        let requested = ids.iter().cloned().collect::<BTreeSet<_>>();
        let bindings = resolved_bindings(target)?;
        let mut desired = Vec::new();
        for id in order.into_iter().filter(|id| requested.contains(id)) {
            let package = &target.packages[&id];
            let root = self.package_root(package)?;
            verify_files(&root, &package.files)?;
            for entry in &package.manifest.plugins {
                let definition = load_definition(&root, &entry.manifest)?;
                if matches!(definition.implementation, Implementation::Process { .. })
                    && !package.trusted_processes
                {
                    return Err(Error::new(
                        "capability_denied",
                        "The package requires explicit trust for its process executable.",
                    ));
                }
                let plugin = self.build_plugin(definition, &root)?;
                desired.push((id.clone(), plugin, package.grants.clone()));
            }
        }
        let mut registry = self.registry.lock().await;
        let previous_bindings = registry.bindings().clone();
        let current = registry
            .inspect()
            .into_iter()
            .map(|p| (p.manifest.id.clone(), p))
            .collect::<BTreeMap<_, _>>();
        let mut unique: BTreeMap<String, Arc<dyn RuntimePlugin>> = BTreeMap::new();
        let mut permissions = BTreeMap::new();
        let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (owner, plugin, grants) in desired {
            let manifest = plugin.manifest();
            let effective = grants
                .intersection(&manifest.requested_capabilities)
                .cloned()
                .collect::<BTreeSet<_>>();
            if let Some(existing) = current.get(&manifest.id) {
                if existing.manifest != *manifest || existing.grants != effective {
                    return Err(Error::new(
                        "invalid_shared_plugin",
                        "The shared plugin requires the same code, configuration, and grants.",
                    ));
                }
            } else if let Some(existing) = unique.get(&manifest.id) {
                if existing.manifest() != manifest
                    || permissions.get(&manifest.id) != Some(&effective)
                {
                    return Err(Error::new(
                        "invalid_shared_plugin",
                        "The package batch contains conflicting plugin instances.",
                    ));
                }
            } else {
                unique.insert(manifest.id.clone(), plugin.clone());
                permissions.insert(manifest.id.clone(), effective);
            }
            owners
                .entry(manifest.id.clone())
                .or_default()
                .insert(package_owner(&owner));
        }
        let batch = new_id("package-activation");
        registry
            .inject_bound(
                unique.into_values().collect(),
                &batch,
                &permissions,
                bindings,
            )
            .await?;
        let validation = registry.freeze().and_then(|lease| {
            for package in target.packages.values().filter(|p| p.enabled) {
                for command in &package.manifest.commands {
                    if !lease.contains_service(&command.service) {
                        return Err(Error::new(
                            "missing_service",
                            "The package command requires an unavailable service.",
                        ));
                    }
                    if lease
                        .service::<Arc<dyn CardSource>>(&command.service)
                        .is_ok()
                    {
                        if command.operation != "fetch" {
                            return Err(Error::new(
                                "unsupported_package_command",
                                "The source command requires the fetch operation.",
                            ));
                        }
                    } else {
                        lease.service::<Arc<dyn CommandService>>(&command.service)?;
                    }
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
        for (plugin, owners) in owners {
            for owner in owners {
                registry.add_owner(&plugin, &owner)?;
            }
        }
        registry.eject_owner(&batch).await?;
        Ok(())
    }

    pub async fn disable(&mut self, id: &str) -> Result<()> {
        let package = self.inspect(id)?;
        if !package.enabled {
            return Ok(());
        }
        let mut catalog = self.catalog.clone();
        catalog
            .packages
            .get_mut(id)
            .ok_or_else(|| Error::new("not_found", "The package is unavailable."))?
            .enabled = false;
        package_order(&catalog)?;
        let mut registry = self.registry.lock().await;
        registry.can_eject_owner(&package_owner(id))?;
        let current = registry.inspect();
        let active_packages = current
            .iter()
            .any(|p| p.owners.iter().any(|owner| owner.starts_with("package:")));
        let mut available = current
            .iter()
            .filter(|p| p.owners.iter().any(|owner| !owner.starts_with("package:")))
            .map(|p| p.manifest.id.clone())
            .collect::<BTreeSet<_>>();
        for package in catalog.packages.values().filter(|p| p.enabled) {
            let root = self.package_root(package)?;
            for entry in &package.manifest.plugins {
                available.insert(load_definition(&root, &entry.manifest)?.manifest.id);
            }
        }
        catalog
            .bindings
            .retain(|_, provider| available.contains(provider));
        let bindings = resolved_bindings(&catalog)?;
        if active_packages {
            for (service, selected) in &bindings {
                if !current
                    .iter()
                    .any(|p| p.manifest.id == *selected && p.manifest.provides.contains(service))
                {
                    return Err(Error::new(
                        "missing_service",
                        "The remaining service binding is unavailable.",
                    ));
                }
            }
        }
        let revision = self.persist(&catalog)?;
        if active_packages {
            registry.eject_owner(&package_owner(id)).await?;
            registry.replace_bindings(bindings)?;
        }
        self.catalog = catalog;
        self.revision = revision;
        Ok(())
    }

    pub fn uninstall(&mut self, id: &str) -> Result<()> {
        let package = self.inspect(id)?;
        if package.enabled {
            return Err(Error::new(
                "package_in_use",
                "The caller must disable the package before uninstall.",
            ));
        }
        let mut catalog = self.catalog.clone();
        catalog.packages.remove(id);
        self.save(catalog)?;
        Ok(())
    }

    pub async fn bind(&mut self, service: String, plugin: String) -> Result<()> {
        let mut registry = self.registry.lock().await;
        let previous = registry.bindings().clone();
        let current = registry.inspect();
        let mut found = current
            .iter()
            .any(|p| p.manifest.id == plugin && p.manifest.provides.contains(&service));
        for package in self.catalog.packages.values().filter(|p| p.enabled) {
            let root = self.package_root(package)?;
            for entry in &package.manifest.plugins {
                let definition = load_definition(&root, &entry.manifest)?;
                found |= definition.manifest.id == plugin
                    && definition.manifest.provides.contains(&service);
            }
        }
        if !found {
            return Err(Error::new(
                "missing_service",
                "The selected plugin does not declare this service.",
            ));
        }
        let active_packages = current
            .iter()
            .any(|p| p.owners.iter().any(|owner| owner.starts_with("package:")));
        if active_packages {
            registry.set_binding(service.clone(), plugin.clone())?;
        }
        let mut catalog = self.catalog.clone();
        catalog.bindings.insert(service, plugin);
        match self.persist(&catalog) {
            Ok(revision) => {
                self.catalog = catalog;
                self.revision = revision;
                Ok(())
            }
            Err(error) => {
                if active_packages {
                    registry.replace_bindings(previous)?;
                }
                Err(error)
            }
        }
    }

    pub fn declared_plugins(&self) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        for package in self.catalog.packages.values() {
            let root = self.package_root(package)?;
            for entry in &package.manifest.plugins {
                let definition = load_definition(&root, &entry.manifest)?;
                result.push(json!({"manifest":definition.manifest,"package":package.manifest.id,"package_version":package.manifest.version,"package_hash":package.package_hash,"state":if package.enabled {"configured"} else {"disabled"},"process_trust_required":matches!(definition.implementation,Implementation::Process{..})}));
            }
        }
        Ok(result)
    }

    pub fn configuration(&self, explicit: &Value) -> Result<Value> {
        let mut result = json!({});
        for id in package_order(&self.catalog)? {
            merge_defaults(
                &mut result,
                &self.catalog.packages[&id].manifest.configuration_defaults,
            );
        }
        merge_defaults(&mut result, explicit);
        Ok(result)
    }

    pub fn provider(&self, explicit: &Configuration, name: &str) -> Result<ProviderConfig> {
        let effective: Configuration =
            serde_json::from_value(self.configuration(&serde_json::to_value(explicit)?)?)?;
        effective.providers.get(name).cloned().ok_or_else(|| {
            Error::new(
                "configuration",
                format!("The provider profile {name} does not exist."),
            )
        })
    }

    pub fn asset(&self, package: &str, relative: &str) -> Result<Vec<u8>> {
        let package = self.inspect(package)?;
        if !package.enabled || !package.manifest.assets.iter().any(|a| a.path == relative) {
            return Err(Error::new(
                "not_found",
                "The enabled package does not declare this asset.",
            ));
        }
        let path = checked_file(&self.package_root(&package)?, relative)?;
        let bytes = bounded_read(&path, MAX_PACKAGE_BYTES)?;
        if package.files.get(relative) != Some(&digest(&bytes)) {
            return Err(Error::new(
                "invalid_package_hash",
                "The installed asset changed.",
            ));
        }
        Ok(bytes)
    }

    pub fn commands(&self) -> Result<BTreeMap<String, CommandDefinition>> {
        let mut commands = BTreeMap::new();
        for id in package_order(&self.catalog)? {
            for command in &self.catalog.packages[&id].manifest.commands {
                commands.insert(command.name.clone(), command.clone());
            }
        }
        Ok(commands)
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
                "invalid_package_command",
                "The package command does not exist.",
            )
        })?;
        let lease = self.registry.lock().await.freeze()?;
        match lease.service::<Arc<dyn CardSource>>(&command.service) {
            Ok(source) if command.operation == "fetch" => {
                let decoder = lease.service::<Arc<dyn CardDecoder>>("card.decoder/1")?;
                let url = arguments["url"].as_str().ok_or_else(|| {
                    Error::new(
                        "invalid_package_command",
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
                "unsupported_package_command",
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
    fn package_root(&self, package: &InstalledPackage) -> Result<PathBuf> {
        safe_relative(&package.directory)?;
        Ok(self.data_dir.join("packages").join(&package.directory))
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
                if let Some(relative) = executable.strip_prefix("{package}/") {
                    process.executable = checked_file(root, relative)?;
                }
                for argument in &mut process.args {
                    if let Some(relative) = argument.strip_prefix("{package}/") {
                        *argument = checked_file(root, relative)?.to_string_lossy().into_owned();
                    }
                }
                let executable = if process.executable.is_absolute() {
                    process.executable.clone()
                } else {
                    return Err(Error::new(
                        "invalid_plugin_process",
                        "A package process requires an absolute or package-relative executable.",
                    ));
                };
                definition.manifest.artifact_hash =
                    digest(&bounded_read(&executable, MAX_PACKAGE_BYTES)?);
                Ok(Arc::new(ExternalSourcePlugin::new(
                    definition.manifest,
                    process.clone(),
                )))
            }
        }
    }
}

fn package_owner(id: &str) -> String {
    format!("package:{id}")
}

/// The function activates the essential plugins of the standard agent.
pub async fn default_registry(store: Arc<dyn StoreBackend>) -> Result<Arc<Mutex<PluginRegistry>>> {
    plugins::registry(&Environment::new(store), &BTreeSet::new()).await
}

pub async fn runtime(manager: &PackageManager) -> Result<Runtime> {
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

fn package_files(root: &Path) -> Result<BTreeMap<String, String>> {
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
                    "invalid_package_asset_path",
                    "The package cannot contain a symbolic link.",
                ));
            }
            if kind.is_dir() {
                pending.push(path);
                if pending.len() + files.len() > MAX_PACKAGE_FILES {
                    return Err(Error::new(
                        "invalid_package_size",
                        "The package contains too many paths.",
                    ));
                }
                continue;
            }
            if !kind.is_file() {
                return Err(Error::new(
                    "invalid_package_asset_path",
                    "The package requires regular files and directories.",
                ));
            }
            if files.len() >= MAX_PACKAGE_FILES {
                return Err(Error::new(
                    "invalid_package_size",
                    "The package contains too many files.",
                ));
            }
            let relative = path
                .strip_prefix(&root)
                .map_err(Error::context(
                    "invalid_package_asset_path",
                    "The package path is invalid.",
                ))?
                .to_str()
                .ok_or_else(|| {
                    Error::new(
                        "invalid_package_asset_path",
                        "The package filename requires UTF-8.",
                    )
                })?
                .to_owned();
            safe_relative(&relative)?;
            let bytes = bounded_read(&path, MAX_PACKAGE_BYTES.saturating_sub(total))?;
            total += bytes.len();
            files.insert(relative, digest(&bytes));
        }
    }
    Ok(files)
}

fn verify_files(root: &Path, expected: &BTreeMap<String, String>) -> Result<()> {
    if package_files(root)? != *expected {
        return Err(Error::new(
            "invalid_package_hash",
            "The installed package changed.",
        ));
    }
    Ok(())
}

fn load_definition(root: &Path, relative: &str) -> Result<PluginDefinition> {
    let bytes = bounded_read(&checked_file(root, relative)?, 1024 * 1024)?;
    let definition: PluginDefinition = serde_json::from_slice(&bytes)?;
    definition.manifest.validate()?;
    if let Implementation::Process { process } = &definition.implementation {
        if let Some(relative) = process
            .executable
            .to_str()
            .and_then(|s| s.strip_prefix("{package}/"))
        {
            checked_file(root, relative)?;
        }
        for argument in &process.args {
            if let Some(relative) = argument.strip_prefix("{package}/") {
                checked_file(root, relative)?;
            }
        }
    }
    Ok(definition)
}

fn validate_package(root: &Path) -> Result<(PackageManifest, BTreeMap<String, String>)> {
    let bytes = bounded_read(&checked_file(root, "manifest.json")?, 1024 * 1024)?;
    let manifest: PackageManifest = serde_json::from_slice(&bytes)?;
    if !valid_id(&manifest.id) || manifest.product_api != "1" {
        return Err(Error::new(
            "invalid_package_manifest",
            "The package ID or product API is invalid.",
        ));
    }
    semver::Version::parse(&manifest.version).map_err(Error::context(
        "invalid_package_version",
        "The package version is invalid.",
    ))?;
    if !manifest.configuration_defaults.is_object() {
        return Err(Error::new(
            "invalid_package_manifest",
            "The package defaults must form an object.",
        ));
    }
    for (id, version) in &manifest.dependencies {
        if !valid_id(id) {
            return Err(Error::new(
                "invalid_package_dependency",
                "The dependency ID is invalid.",
            ));
        }
        semver::VersionReq::parse(version).map_err(Error::context(
            "invalid_package_dependency",
            "The dependency version range is invalid.",
        ))?;
    }
    let mut plugins = BTreeSet::new();
    for entry in &manifest.plugins {
        let definition = load_definition(root, &entry.manifest)?;
        if !plugins.insert(definition.manifest.id) {
            return Err(Error::new(
                "invalid_plugin_id",
                "The package declares a plugin twice.",
            ));
        }
    }
    let mut commands = BTreeSet::new();
    for command in &manifest.commands {
        if !valid_id(&command.name)
            || !commands.insert(command.name.clone())
            || command.service.is_empty()
            || command.operation.is_empty()
        {
            return Err(Error::new(
                "invalid_package_command",
                "The package command is invalid or duplicated.",
            ));
        }
    }
    let files = package_files(root)?;
    let mut assets = BTreeSet::new();
    for asset in &manifest.assets {
        checked_file(root, &asset.path)?;
        if !assets.insert(asset.path.clone()) || files.get(&asset.path) != Some(&asset.sha256) {
            return Err(Error::new(
                "invalid_package_hash",
                "The asset hash is absent, duplicated, or incorrect.",
            ));
        }
    }
    Ok((manifest, files))
}

fn package_order(catalog: &Catalog) -> Result<Vec<String>> {
    let enabled = catalog
        .packages
        .values()
        .filter(|p| p.enabled)
        .collect::<Vec<_>>();
    let mut commands = BTreeSet::new();
    for package in &enabled {
        for command in &package.manifest.commands {
            if !commands.insert(&command.name) {
                return Err(Error::new(
                    "invalid_package_command",
                    "Enabled packages declare the same command.",
                ));
            }
        }
        for (id, version) in &package.manifest.dependencies {
            let dependency = catalog
                .packages
                .get(id)
                .filter(|p| p.enabled)
                .ok_or_else(|| {
                    Error::new(
                        "missing_package_dependency",
                        "The package requires an enabled dependency.",
                    )
                })?;
            let version = semver::VersionReq::parse(version).map_err(Error::context(
                "invalid_package_dependency",
                "The dependency version range is invalid.",
            ))?;
            let actual =
                semver::Version::parse(&dependency.manifest.version).map_err(Error::context(
                    "invalid_package_version",
                    "The dependency version is invalid.",
                ))?;
            if !version.matches(&actual) {
                return Err(Error::new(
                    "incompatible_package_dependency",
                    "The enabled dependency has another version.",
                ));
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    while order.len() < enabled.len() {
        let next = enabled
            .iter()
            .find(|p| {
                !seen.contains(&p.manifest.id)
                    && p.manifest.dependencies.keys().all(|id| seen.contains(id))
            })
            .map(|p| p.manifest.id.clone());
        let Some(id) = next else {
            return Err(Error::new(
                "package_dependency_cycle",
                "The enabled package graph contains a cycle.",
            ));
        };
        seen.insert(id.clone());
        order.push(id);
    }
    Ok(order)
}

fn resolved_bindings(catalog: &Catalog) -> Result<BTreeMap<String, String>> {
    let mut bindings = BTreeMap::new();
    for id in package_order(catalog)? {
        for (service, selected) in &catalog.packages[&id].manifest.bindings {
            if bindings
                .get(service)
                .is_some_and(|previous| previous != selected)
                && !catalog.bindings.contains_key(service)
            {
                return Err(Error::new(
                    "ambiguous_service",
                    "Package defaults require an explicit service binding.",
                ));
            }
            bindings.insert(service.clone(), selected.clone());
        }
    }
    bindings.extend(catalog.bindings.clone());
    Ok(bindings)
}

fn merge_defaults(target: &mut Value, update: &Value) {
    if let Some(values) = update.as_object() {
        if !target.is_object() {
            *target = json!({});
        }
        for (key, value) in values {
            merge_defaults(&mut target[key], value);
        }
    } else {
        *target = update.clone();
    }
}

impl PackageManager {
    pub async fn composition(&self) -> Result<Value> {
        let registry = self.registry.lock().await;
        let lease = registry.freeze()?;
        let packages = self.catalog.packages.values().filter(|p| p.enabled).map(|p| json!({"id":p.manifest.id,"version":p.manifest.version,"package_hash":p.package_hash,"assets":p.manifest.assets})).collect::<Vec<_>>();
        let mut record = lease.snapshot();
        record["preset"] = json!("tavern");
        record["packages"] = json!(packages);
        Ok(record)
    }
}
