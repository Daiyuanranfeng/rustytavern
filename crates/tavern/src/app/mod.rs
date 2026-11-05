pub use crate::config;
use crate::{
    Error, Result, driver, external, phases, plugin, prompt, provider, runtime, source, store,
};
use rustytavern_core::{bounded_read, digest, native_artifact_hash};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::Configuration;
use driver::{ConversationDriver, DRIVER_CAPABILITIES, SingleReplyDriver};
use external::{ExternalSourcePlugin, ProcessDefinition};
use phases::{HookPolicy, PromptHook, SectionEdit, TransformInput};
use plugin::{
    CompositionLease, Contribution, PluginContext, PluginManifest, PluginRegistry, RuntimePlugin,
};
use prompt::{CoreProfile, PromptProfile};
use provider::{HttpProvider, ProviderAdapter, ProviderConfig};
use runtime::Runtime;
use rustytavern_card::{Card, CardDecoder, CcDecoder};
use source::{Artifact, ArtifactFetcher, CardSource, ChubSource, PublicFetcher, RisuRealmSource};
use store::{StoreBackend, new_id};

const APP_NAMESPACE: &str = "rustytavern.application";
const CATALOG_KEY: &str = "extensions";
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
pub struct ExtensionManifest {
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
impl Default for ExtensionManifest {
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
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
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
    packages: BTreeMap<String, InstalledExtension>,
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

pub struct ExtensionManager {
    data_dir: PathBuf,
    store: Arc<dyn StoreBackend>,
    pub registry: Arc<Mutex<PluginRegistry>>,
    catalog: Catalog,
    revision: i64,
    factories: BTreeMap<String, NativeFactory>,
}

impl ExtensionManager {
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
                "The extension catalog requires another application version.",
            ));
        }
        let data_dir = data_dir.as_ref().to_owned();
        std::fs::create_dir_all(data_dir.join("extensions"))?;
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

    pub fn list(&self) -> Vec<InstalledExtension> {
        self.catalog.packages.values().cloned().collect()
    }
    pub fn inspect(&self, id: &str) -> Result<InstalledExtension> {
        self.catalog
            .packages
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "The extension is not installed."))
    }

    pub fn install(&mut self, source: &Path) -> Result<InstalledExtension> {
        let (manifest, files) = validate_package(source)?;
        if self
            .catalog
            .packages
            .get(&manifest.id)
            .is_some_and(|p| p.enabled)
        {
            return Err(Error::new(
                "extension_in_use",
                "The caller must disable this extension before an update.",
            ));
        }
        let package_hash = digest(&serde_json::to_vec(&files)?);
        let directory = format!(
            "{}-{}-{}",
            manifest.id,
            manifest.version,
            &package_hash[..16]
        );
        let final_dir = self.data_dir.join("extensions").join(&directory);
        let stage = self.data_dir.join("extensions").join(new_id("staging"));
        let staged = (|| -> Result<()> {
            std::fs::create_dir(&stage)?;
            for (relative, hash) in &files {
                let original = checked_file(source, relative)?;
                let bytes = bounded_read(&original, MAX_PACKAGE_BYTES)?;
                if digest(&bytes) != *hash {
                    return Err(Error::new(
                        "invalid_extension_hash",
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
        let package = InstalledExtension {
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
                .ok_or_else(|| Error::new("not_found", "The extension is not installed."))?;
            if package.enabled {
                return Err(Error::new(
                    "extension_in_use",
                    "The extension is already enabled.",
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
        let order = extension_order(target)?;
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
                        "The extension requires explicit trust for its process executable.",
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
                .insert(extension_owner(&owner));
        }
        let batch = new_id("extension-activation");
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
                            "The extension command requires an unavailable service.",
                        ));
                    }
                    if lease
                        .service::<Arc<dyn CardSource>>(&command.service)
                        .is_ok()
                    {
                        if command.operation != "fetch" {
                            return Err(Error::new(
                                "unsupported_extension_command",
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
            .ok_or_else(|| Error::new("not_found", "The extension is unavailable."))?
            .enabled = false;
        extension_order(&catalog)?;
        let mut registry = self.registry.lock().await;
        registry.can_eject_owner(&extension_owner(id))?;
        let current = registry.inspect();
        let active_packages = current
            .iter()
            .any(|p| p.owners.iter().any(|owner| owner.starts_with("extension:")));
        let mut available = current
            .iter()
            .filter(|p| {
                p.owners
                    .iter()
                    .any(|owner| !owner.starts_with("extension:"))
            })
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
            registry.eject_owner(&extension_owner(id)).await?;
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
                "extension_in_use",
                "The caller must disable the extension before uninstall.",
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
            .any(|p| p.owners.iter().any(|owner| owner.starts_with("extension:")));
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
                result.push(json!({"manifest":definition.manifest,"extension":package.manifest.id,"extension_version":package.manifest.version,"package_hash":package.package_hash,"state":if package.enabled {"configured"} else {"disabled"},"process_trust_required":matches!(definition.implementation,Implementation::Process{..})}));
            }
        }
        Ok(result)
    }

    pub fn configuration(&self, explicit: &Value) -> Result<Value> {
        let mut result = json!({});
        for id in extension_order(&self.catalog)? {
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

    pub fn asset(&self, extension: &str, relative: &str) -> Result<Vec<u8>> {
        let package = self.inspect(extension)?;
        if !package.enabled || !package.manifest.assets.iter().any(|a| a.path == relative) {
            return Err(Error::new(
                "not_found",
                "The enabled extension does not declare this asset.",
            ));
        }
        let path = checked_file(&self.package_root(&package)?, relative)?;
        let bytes = bounded_read(&path, MAX_PACKAGE_BYTES)?;
        if package.files.get(relative) != Some(&digest(&bytes)) {
            return Err(Error::new(
                "invalid_extension_hash",
                "The installed asset changed.",
            ));
        }
        Ok(bytes)
    }

    pub fn commands(&self) -> Result<BTreeMap<String, CommandDefinition>> {
        let mut commands = BTreeMap::new();
        for id in extension_order(&self.catalog)? {
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
                "invalid_extension_command",
                "The extension command does not exist.",
            )
        })?;
        let lease = self.registry.lock().await.freeze()?;
        match lease.service::<Arc<dyn CardSource>>(&command.service) {
            Ok(source) if command.operation == "fetch" => {
                let decoder = lease.service::<Arc<dyn CardDecoder>>("card.decoder/1")?;
                let url = arguments["url"].as_str().ok_or_else(|| {
                    Error::new(
                        "invalid_extension_command",
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
                "unsupported_extension_command",
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
    fn package_root(&self, package: &InstalledExtension) -> Result<PathBuf> {
        safe_relative(&package.directory)?;
        Ok(self.data_dir.join("extensions").join(&package.directory))
    }

    fn build_plugin(
        &self,
        mut definition: PluginDefinition,
        root: &Path,
    ) -> Result<Arc<dyn RuntimePlugin>> {
        definition.manifest.configuration_hash =
            digest(&serde_json::to_vec(&definition.implementation)?);
        match &mut definition.implementation {
            Implementation::Builtin { name, .. } => {
                definition.manifest.artifact_hash = native_artifact_hash()?;
                if let Some(factory) = self.factories.get(name) {
                    return factory(&definition);
                }
                Ok(Arc::new(BuiltinPlugin {
                    definition,
                    store: self.store.clone(),
                }))
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

fn extension_owner(id: &str) -> String {
    format!("extension:{id}")
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

struct BuiltinPlugin {
    definition: PluginDefinition,
    store: Arc<dyn StoreBackend>,
}
#[async_trait]
impl RuntimePlugin for BuiltinPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.definition.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let Implementation::Builtin {
            name,
            configuration,
        } = &self.definition.implementation
        else {
            return Err(Error::new(
                "invalid_plugin_factory",
                "The native factory received another implementation kind.",
            ));
        };
        match name.as_str() {
            "decoder.cc-v2-v3" => {
                let decoder: Arc<dyn CardDecoder> = Arc::new(CcDecoder);
                context.register("card.decoder/1", Arc::new(decoder))?;
            }
            "driver.single-reply" => {
                for capability in DRIVER_CAPABILITIES {
                    context.authorize(capability)?;
                }
                let driver: Arc<dyn ConversationDriver> = Arc::new(SingleReplyDriver);
                context.register("driver/1", Arc::new(driver))?;
            }
            "profile.core-v1" => {
                let profile: Arc<dyn PromptProfile> = Arc::new(CoreProfile);
                context.register("profile/1", Arc::new(profile))?;
            }
            "provider.http" => {
                let provider: Arc<dyn ProviderAdapter> = Arc::new(HttpProvider);
                context.register("provider/1", Arc::new(provider))?;
            }
            "store.sqlite" => {
                context.register("store/1", Arc::new(self.store.clone()))?;
            }
            "prompt.contributions" => {
                let contributions: Vec<Contribution> =
                    serde_json::from_value(configuration["contributions"].clone())?;
                for contribution in contributions {
                    context.contribute(contribution)?;
                }
            }
            "prompt.edits" => {
                let hook: Arc<dyn PromptHook> = Arc::new(serde_json::from_value::<
                    StaticPromptEdits,
                >(configuration.clone())?);
                for service in &self.definition.manifest.provides {
                    if !service.starts_with("prompt.hook.") || !service.ends_with("/1") {
                        return Err(Error::new(
                            "unsupported_plugin_service",
                            "The edit factory provides prompt hooks only.",
                        ));
                    }
                    context.register(service, Arc::new(hook.clone()))?;
                }
            }
            "source.chub" | "source.risurealm" => {
                let (host, service) = if name == "source.chub" {
                    ("api.chub.ai", "card.source.chub/1")
                } else {
                    ("realm.risuai.net", "card.source.risurealm/1")
                };
                context.authorize(&format!("network:https:{host}"))?;
                let hosts = context
                    .grants()
                    .iter()
                    .filter_map(|g| g.strip_prefix("network:https:").map(str::to_owned));
                let fetcher: Arc<dyn ArtifactFetcher> = Arc::new(PublicFetcher::new(hosts));
                let source: Arc<dyn CardSource> = if name == "source.chub" {
                    Arc::new(ChubSource::new(fetcher))
                } else {
                    Arc::new(RisuRealmSource::new(fetcher))
                };
                context.register(service, Arc::new(source))?;
            }
            _ => {
                return Err(Error::new(
                    "unsupported_plugin_factory",
                    "The application does not provide this native plugin factory.",
                ));
            }
        }
        Ok(())
    }
}

pub async fn default_registry(store: Arc<dyn StoreBackend>) -> Result<Arc<Mutex<PluginRegistry>>> {
    let mut plugins: Vec<Arc<dyn RuntimePlugin>> = Vec::new();
    for (name, service) in [
        ("decoder.cc-v2-v3", "card.decoder/1"),
        ("driver.single-reply", "driver/1"),
        ("profile.core-v1", "profile/1"),
        ("provider.http", "provider/1"),
        ("store.sqlite", "store/1"),
    ] {
        let definition = PluginDefinition {
            manifest: PluginManifest {
                id: format!("org.rustytavern.{name}"),
                provides: vec![service.into()],
                artifact_hash: native_artifact_hash()?,
                configuration_hash: digest(name.as_bytes()),
                requested_capabilities: if service == "driver/1" {
                    DRIVER_CAPABILITIES.into_iter().map(str::to_owned).collect()
                } else {
                    BTreeSet::new()
                },
                ..Default::default()
            },
            implementation: Implementation::Builtin {
                name: name.into(),
                configuration: json!({}),
            },
        };
        plugins.push(Arc::new(BuiltinPlugin {
            definition,
            store: store.clone(),
        }));
    }
    let mut registry = PluginRegistry::default();
    let grants = plugins
        .iter()
        .map(|plugin| {
            (
                plugin.manifest().id.clone(),
                plugin.manifest().requested_capabilities.clone(),
            )
        })
        .collect();
    registry.inject(plugins, "core-product", &grants).await?;
    Ok(Arc::new(Mutex::new(registry)))
}

pub async fn runtime(manager: &ExtensionManager) -> Result<Runtime> {
    let lease = manager.registry.lock().await.freeze()?;
    let store = lease
        .service::<Arc<dyn StoreBackend>>("store/1")?
        .as_ref()
        .clone();
    Ok(Runtime::new(store)
        .with_plugins(manager.registry.clone())
        .with_product_composition(manager.composition().await?))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn safe_relative(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if value.is_empty()
        || value.len() > 512
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::new(
            "invalid_extension_asset_path",
            "The package path must stay inside its directory.",
        ));
    }
    Ok(path)
}

fn checked_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = safe_relative(relative)?;
    let root = root.canonicalize()?;
    let mut path = root.clone();
    for component in relative.components() {
        path.push(component.as_os_str());
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(Error::new(
                "invalid_extension_asset_path",
                "The package cannot contain a symbolic link.",
            ));
        }
    }
    if !path.is_file() || !path.canonicalize()?.starts_with(root) {
        return Err(Error::new(
            "invalid_extension_asset_path",
            "The declared package file is unavailable.",
        ));
    }
    Ok(path)
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
                    "invalid_extension_asset_path",
                    "The package cannot contain a symbolic link.",
                ));
            }
            if kind.is_dir() {
                pending.push(path);
                if pending.len() + files.len() > MAX_PACKAGE_FILES {
                    return Err(Error::new(
                        "invalid_extension_size",
                        "The package contains too many paths.",
                    ));
                }
                continue;
            }
            if !kind.is_file() {
                return Err(Error::new(
                    "invalid_extension_asset_path",
                    "The package requires regular files and directories.",
                ));
            }
            if files.len() >= MAX_PACKAGE_FILES {
                return Err(Error::new(
                    "invalid_extension_size",
                    "The package contains too many files.",
                ));
            }
            let relative = path
                .strip_prefix(&root)
                .map_err(Error::context(
                    "invalid_extension_asset_path",
                    "The package path is invalid.",
                ))?
                .to_str()
                .ok_or_else(|| {
                    Error::new(
                        "invalid_extension_asset_path",
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
            "invalid_extension_hash",
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

fn validate_package(root: &Path) -> Result<(ExtensionManifest, BTreeMap<String, String>)> {
    let bytes = bounded_read(&checked_file(root, "extension.json")?, 1024 * 1024)?;
    let manifest: ExtensionManifest = serde_json::from_slice(&bytes)?;
    if !valid_id(&manifest.id) || manifest.product_api != "1" {
        return Err(Error::new(
            "invalid_extension_manifest",
            "The extension ID or product API is invalid.",
        ));
    }
    semver::Version::parse(&manifest.version).map_err(Error::context(
        "invalid_extension_version",
        "The extension version is invalid.",
    ))?;
    if !manifest.configuration_defaults.is_object() {
        return Err(Error::new(
            "invalid_extension_manifest",
            "The extension defaults must form an object.",
        ));
    }
    for (id, version) in &manifest.dependencies {
        if !valid_id(id) {
            return Err(Error::new(
                "invalid_extension_dependency",
                "The dependency ID is invalid.",
            ));
        }
        semver::VersionReq::parse(version).map_err(Error::context(
            "invalid_extension_dependency",
            "The dependency version range is invalid.",
        ))?;
    }
    let mut plugins = BTreeSet::new();
    for entry in &manifest.plugins {
        let definition = load_definition(root, &entry.manifest)?;
        if !plugins.insert(definition.manifest.id) {
            return Err(Error::new(
                "invalid_plugin_id",
                "The extension declares a plugin twice.",
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
                "invalid_extension_command",
                "The extension command is invalid or duplicated.",
            ));
        }
    }
    let files = package_files(root)?;
    let mut assets = BTreeSet::new();
    for asset in &manifest.assets {
        checked_file(root, &asset.path)?;
        if !assets.insert(asset.path.clone()) || files.get(&asset.path) != Some(&asset.sha256) {
            return Err(Error::new(
                "invalid_extension_hash",
                "The asset hash is absent, duplicated, or incorrect.",
            ));
        }
    }
    Ok((manifest, files))
}

fn extension_order(catalog: &Catalog) -> Result<Vec<String>> {
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
                    "invalid_extension_command",
                    "Enabled extensions declare the same command.",
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
                        "missing_extension_dependency",
                        "The extension requires an enabled dependency.",
                    )
                })?;
            let version = semver::VersionReq::parse(version).map_err(Error::context(
                "invalid_extension_dependency",
                "The dependency version range is invalid.",
            ))?;
            let actual =
                semver::Version::parse(&dependency.manifest.version).map_err(Error::context(
                    "invalid_extension_version",
                    "The dependency version is invalid.",
                ))?;
            if !version.matches(&actual) {
                return Err(Error::new(
                    "incompatible_extension_dependency",
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
                "extension_dependency_cycle",
                "The enabled extension graph contains a cycle.",
            ));
        };
        seen.insert(id.clone());
        order.push(id);
    }
    Ok(order)
}

fn resolved_bindings(catalog: &Catalog) -> Result<BTreeMap<String, String>> {
    let mut bindings = BTreeMap::new();
    for id in extension_order(catalog)? {
        for (service, selected) in &catalog.packages[&id].manifest.bindings {
            if bindings
                .get(service)
                .is_some_and(|previous| previous != selected)
                && !catalog.bindings.contains_key(service)
            {
                return Err(Error::new(
                    "ambiguous_service",
                    "Extension defaults require an explicit service binding.",
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

impl ExtensionManager {
    pub fn install_builtin(&mut self, name: &str) -> Result<InstalledExtension> {
        let (manifest, plugin, readme, plugin_readme): (&[u8], &[u8], &[u8], &[u8]) = match name {
            "chub" => (
                include_bytes!("../../bundled/chub/extension.json"),
                include_bytes!("../../bundled/chub/plugins/source.json"),
                include_bytes!("../../bundled/chub/README.md"),
                include_bytes!("../../bundled/chub/plugins/README.md"),
            ),
            "risurealm" => (
                include_bytes!("../../bundled/risurealm/extension.json"),
                include_bytes!("../../bundled/risurealm/plugins/source.json"),
                include_bytes!("../../bundled/risurealm/README.md"),
                include_bytes!("../../bundled/risurealm/plugins/README.md"),
            ),
            _ => {
                return Err(Error::new(
                    "invalid_extension_name",
                    "The bundled extension does not exist.",
                ));
            }
        };
        let stage = self.data_dir.join("extensions").join(new_id("template"));
        std::fs::create_dir_all(stage.join("plugins"))?;
        let result = (|| {
            std::fs::write(stage.join("extension.json"), manifest)?;
            std::fs::write(stage.join("plugins/source.json"), plugin)?;
            std::fs::write(stage.join("README.md"), readme)?;
            std::fs::write(stage.join("plugins/README.md"), plugin_readme)?;
            self.install(&stage)
        })();
        let _ = std::fs::remove_dir_all(stage);
        result
    }
}

impl ExtensionManager {
    pub async fn composition(&self) -> Result<Value> {
        let registry = self.registry.lock().await;
        let lease = registry.freeze()?;
        let packages = self.catalog.packages.values().filter(|p| p.enabled).map(|p| json!({"id":p.manifest.id,"version":p.manifest.version,"package_hash":p.package_hash,"assets":p.manifest.assets})).collect::<Vec<_>>();
        let mut record = lease.snapshot();
        record["preset"] = json!("tavern");
        record["extensions"] = json!(packages);
        Ok(record)
    }
}
