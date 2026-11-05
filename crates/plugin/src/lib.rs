#[cfg(feature = "process")]
pub mod process;
pub mod prompt;

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustytavern_core::{Error, Result, digest};
use rustytavern_store::{PluginStateSnapshot, StoreBackend};

pub type ServiceValue = Arc<dyn Any + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub host_protocol: String,
    pub provides: Vec<String>,
    pub requires: Vec<String>,
    pub requested_capabilities: BTreeSet<String>,
    pub allowed_slots: BTreeSet<String>,
    pub state_schema: u32,
    pub artifact_hash: String,
    pub configuration_hash: String,
}
impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            version: "0.1.0".into(),
            host_protocol: "1".into(),
            provides: Vec::new(),
            requires: Vec::new(),
            requested_capabilities: BTreeSet::new(),
            allowed_slots: BTreeSet::new(),
            state_schema: 1,
            artifact_hash: String::new(),
            configuration_hash: String::new(),
        }
    }
}
impl PluginManifest {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(Error::new(
                "invalid_plugin_id",
                "The plugin ID contains unsupported characters.",
            ));
        }
        if self.host_protocol != "1" {
            return Err(Error::new(
                "incompatible_plugin_protocol",
                "The plugin requires another host protocol.",
            ));
        }
        if self.state_schema == 0 {
            return Err(Error::new(
                "unsupported_plugin_state_schema",
                "The plugin state schema must be positive.",
            ));
        }
        semver::Version::parse(&self.version).map_err(|_| {
            Error::new(
                "invalid_plugin_version",
                "The plugin version must use semantic versioning.",
            )
        })?;
        for names in [&self.provides, &self.requires] {
            let unique = names.iter().collect::<BTreeSet<_>>();
            if unique.len() != names.len()
                || names
                    .iter()
                    .any(|name| name.is_empty() || !name.contains('/'))
            {
                return Err(Error::new(
                    "invalid_plugin_services",
                    "The plugin service declarations are invalid or duplicated.",
                ));
            }
        }
        Ok(())
    }
}

pub use rustytavern_conversation::prompt::{Contribution, Retention, apply_contributions};

pub fn normalize_contribution(
    manifest: &PluginManifest,
    grants: &BTreeSet<String>,
    mut contribution: Contribution,
) -> Result<Contribution> {
    if !manifest.allowed_slots.contains(&contribution.slot)
        || !matches!(
            contribution.slot.as_str(),
            "before_character" | "after_character" | "before_history" | "after_history"
        )
    {
        return Err(Error::new(
            "invalid_prompt_slot",
            "The plugin or profile does not declare this prompt slot.",
        ));
    }
    if !grants.contains(&format!("prompt.contribute:{}", contribution.slot)) {
        return Err(Error::new(
            "capability_denied",
            "The plugin has no grant for this prompt slot.",
        ));
    }
    if contribution.section_id.is_empty() {
        return Err(Error::new(
            "invalid_section_id",
            "The contribution section ID is empty.",
        ));
    }
    contribution.plugin_id = manifest.id.clone();
    if contribution.provenance.is_null() {
        contribution.provenance = serde_json::json!({"plugin_id":manifest.id,"version":manifest.version,"artifact_hash":manifest.artifact_hash});
    }
    Ok(contribution)
}

/// A plugin registers services and resources within one owned activation.
#[async_trait]
pub trait RuntimePlugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;
    async fn activate(&self, context: &mut PluginContext) -> Result<()>;
    async fn deactivate(&self) -> Result<()> {
        Ok(())
    }
}

pub struct PluginContext {
    manifest: PluginManifest,
    grants: BTreeSet<String>,
    dependencies: BTreeMap<String, ServiceValue>,
    registrations: BTreeMap<String, ServiceValue>,
    contributions: Vec<Contribution>,
    cleanup: Vec<Box<dyn FnOnce() + Send>>,
}
impl PluginContext {
    pub fn register<T: Any + Send + Sync>(&mut self, service: &str, value: Arc<T>) -> Result<()> {
        if !self.manifest.provides.iter().any(|s| s == service) {
            return Err(Error::new(
                "invalid_plugin_registration",
                "The plugin did not declare this service.",
            ));
        }
        if self.registrations.contains_key(service) {
            return Err(Error::new(
                "invalid_plugin_registration",
                "The plugin registered this service twice.",
            ));
        }
        self.registrations.insert(service.to_owned(), value);
        Ok(())
    }
    pub fn dependency<T: Any + Send + Sync>(&self, service: &str) -> Result<Arc<T>> {
        self.dependencies
            .get(service)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "missing_service",
                    "The plugin did not receive this dependency.",
                )
            })?
            .downcast::<T>()
            .map_err(|_| {
                Error::new(
                    "invalid_service_type",
                    "The dependency has another Rust service type.",
                )
            })
    }
    pub fn grants(&self) -> &BTreeSet<String> {
        &self.grants
    }

    pub fn authorize(&self, capability: &str) -> Result<()> {
        if self.grants.contains(capability) {
            Ok(())
        } else {
            Err(Error::new(
                "capability_denied",
                "The plugin has no grant for this host operation.",
            ))
        }
    }
    pub fn own(&mut self, cleanup: impl FnOnce() + Send + 'static) {
        self.cleanup.push(Box::new(cleanup));
    }
    pub fn contribute(&mut self, contribution: Contribution) -> Result<()> {
        let contribution = normalize_contribution(&self.manifest, &self.grants, contribution)?;
        if self
            .contributions
            .iter()
            .any(|c| c.section_id == contribution.section_id)
        {
            return Err(Error::new(
                "invalid_section_id",
                "The contribution section ID is duplicated.",
            ));
        }
        self.contributions.push(contribution);
        Ok(())
    }
    pub fn state_write(
        &self,
        store: &dyn StoreBackend,
        namespace: &str,
        key: &str,
        value: &Value,
    ) -> Result<i64> {
        if namespace != self.manifest.id {
            return Err(Error::new(
                "capability_denied",
                "The plugin cannot write another state namespace.",
            ));
        }
        self.authorize(&format!("state.write:{namespace}"))?;
        store.plugin_state_write(namespace, self.manifest.state_schema, key, value)
    }
    pub fn state_read(&self, store: &dyn StoreBackend, key: &str) -> Result<Option<Value>> {
        self.authorize(&format!("state.read:{}", self.manifest.id))?;
        let snapshot = store.plugin_state_snapshot(&self.manifest.id)?;
        snapshot.require_schema(self.manifest.state_schema)?;
        Ok(snapshot
            .entries
            .get(key)
            .map(|entry| entry["value"].clone()))
    }
    pub fn state_migrate(
        &self,
        store: &dyn StoreBackend,
        migrate: impl FnOnce(&PluginStateSnapshot) -> Result<BTreeMap<String, Value>>,
    ) -> Result<PluginStateSnapshot> {
        self.authorize(&format!("state.read:{}", self.manifest.id))?;
        self.authorize(&format!("state.write:{}", self.manifest.id))?;
        let before = store.plugin_state_snapshot(&self.manifest.id)?;
        if before.schema == self.manifest.state_schema {
            return Ok(before);
        }
        if before.schema > self.manifest.state_schema {
            return Err(Error::new(
                "unsupported_plugin_state_schema",
                "The plugin cannot downgrade its state schema.",
            ));
        }
        let updates =
            std::panic::catch_unwind(AssertUnwindSafe(|| migrate(&before))).map_err(|_| {
                Error::new(
                    "plugin_state_migration",
                    "The plugin state migration panicked.",
                )
            })??;
        store.plugin_state_migrate(
            &self.manifest.id,
            &before,
            self.manifest.state_schema,
            &updates,
        )
    }
}
impl Drop for PluginContext {
    fn drop(&mut self) {
        for cleanup in self.cleanup.drain(..).rev() {
            let _ = std::panic::catch_unwind(AssertUnwindSafe(cleanup));
        }
    }
}

struct ActivePlugin {
    plugin: Arc<dyn RuntimePlugin>,
    context: PluginContext,
    owners: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub manifest: PluginManifest,
    pub owners: BTreeSet<String>,
    pub grants: BTreeSet<String>,
    pub state: String,
}

#[derive(Default)]
pub struct PluginRegistry {
    active: BTreeMap<String, ActivePlugin>,
    bindings: BTreeMap<String, String>,
    active_generations: Arc<AtomicUsize>,
}

pub struct CompositionLease {
    pub identity: String,
    pub plugins: Vec<PluginInfo>,
    pub contributions: Vec<Contribution>,
    services: BTreeMap<String, ServiceValue>,
    service_owners: BTreeMap<String, String>,
    bindings: BTreeMap<String, String>,
    active_generations: Arc<AtomicUsize>,
}
impl CompositionLease {
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"identity":self.identity,"plugins":self.plugins,"bindings":self.bindings,"resolved_bindings":self.service_owners,"contributions":self.contributions})
    }
    pub fn service_names(&self, prefix: &str) -> Vec<String> {
        self.services
            .keys()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect()
    }
    pub fn service_owner(&self, name: &str) -> Result<&PluginInfo> {
        self.service_owners
            .get(name)
            .and_then(|id| self.plugins.iter().find(|p| &p.manifest.id == id))
            .ok_or_else(|| Error::new("missing_service", "The service owner is unavailable."))
    }
    pub fn contains_service(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }

    pub fn service<T: Any + Send + Sync>(&self, name: &str) -> Result<Arc<T>> {
        self.services
            .get(name)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "missing_service",
                    "The composition has no selected service.",
                )
            })?
            .downcast::<T>()
            .map_err(|_| Error::new("invalid_service_type", "The service has another Rust type."))
    }
}
impl Drop for CompositionLease {
    fn drop(&mut self) {
        self.active_generations.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PluginRegistry {
    pub fn inspect(&self) -> Vec<PluginInfo> {
        self.active
            .values()
            .map(|p| PluginInfo {
                manifest: p.context.manifest.clone(),
                owners: p.owners.clone(),
                grants: p.context.grants.clone(),
                state: "active".into(),
            })
            .collect()
    }
    pub fn set_binding(&mut self, service: String, plugin: String) -> Result<()> {
        let mut bindings = self.bindings.clone();
        bindings.insert(service, plugin);
        self.replace_bindings(bindings)
    }
    pub fn bindings(&self) -> &BTreeMap<String, String> {
        &self.bindings
    }

    pub fn replace_bindings(&mut self, bindings: BTreeMap<String, String>) -> Result<()> {
        self.require_idle()?;
        for (service, selected) in &bindings {
            if !self
                .active
                .get(selected)
                .is_some_and(|p| p.context.registrations.contains_key(service))
            {
                return Err(Error::new(
                    "missing_service",
                    "The selected binding has no provider.",
                ));
            }
        }
        self.validate_consumers(&bindings)?;
        self.bindings = bindings;
        Ok(())
    }

    pub async fn inject_bound(
        &mut self,
        plugins: Vec<Arc<dyn RuntimePlugin>>,
        owner: &str,
        grants: &BTreeMap<String, BTreeSet<String>>,
        bindings: BTreeMap<String, String>,
    ) -> Result<Vec<String>> {
        self.require_idle()?;
        for (service, selected) in &bindings {
            let exists = self
                .active
                .get(selected)
                .is_some_and(|p| p.context.manifest.provides.contains(service))
                || plugins.iter().any(|p| {
                    p.manifest().id == *selected && p.manifest().provides.contains(service)
                });
            if !exists {
                return Err(Error::new(
                    "missing_service",
                    "The selected binding has no declared provider.",
                ));
            }
        }
        self.validate_consumers(&bindings)?;
        let previous = std::mem::replace(&mut self.bindings, bindings);
        match self.inject(plugins, owner, grants).await {
            Ok(order) => Ok(order),
            Err(error) => {
                self.bindings = previous;
                Err(error)
            }
        }
    }

    pub fn freeze(&self) -> Result<CompositionLease> {
        let mut services = BTreeMap::new();
        let mut service_owners = BTreeMap::new();
        for (id, plugin) in &self.active {
            for name in plugin.context.registrations.keys() {
                if !services.contains_key(name) {
                    services.insert(name.clone(), self.resolve(name)?);
                    service_owners
                        .insert(name.clone(), self.bindings.get(name).unwrap_or(id).clone());
                }
            }
        }
        let plugins = self.inspect();
        let mut contributions = self
            .active
            .values()
            .flat_map(|p| p.context.contributions.iter().cloned())
            .collect::<Vec<_>>();
        contributions.sort_by(|a, b| {
            (&a.slot, a.priority, &a.plugin_id, &a.section_id).cmp(&(
                &b.slot,
                b.priority,
                &b.plugin_id,
                &b.section_id,
            ))
        });
        let identity = digest(&serde_json::to_vec(&(
            &plugins,
            &self.bindings,
            &contributions,
        ))?);
        self.active_generations.fetch_add(1, Ordering::SeqCst);
        Ok(CompositionLease {
            identity,
            plugins,
            contributions,
            services,
            service_owners,
            bindings: self.bindings.clone(),
            active_generations: self.active_generations.clone(),
        })
    }
    pub async fn inject(
        &mut self,
        plugins: Vec<Arc<dyn RuntimePlugin>>,
        owner: &str,
        grants: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Vec<String>> {
        self.require_idle()?;
        let mut pending = BTreeMap::new();
        for plugin in plugins {
            plugin.manifest().validate()?;
            let id = plugin.manifest().id.clone();
            if self.active.contains_key(&id) || pending.insert(id, plugin).is_some() {
                return Err(Error::new(
                    "invalid_plugin_id",
                    "The plugin ID already exists.",
                ));
            }
        }
        let mut all_providers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, plugin) in &self.active {
            for name in &plugin.context.manifest.provides {
                all_providers
                    .entry(name.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        for (id, plugin) in &pending {
            for name in &plugin.manifest().provides {
                all_providers
                    .entry(name.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        for plugin in pending.values() {
            for required in &plugin.manifest().requires {
                let options = all_providers.get(required).ok_or_else(|| {
                    Error::new(
                        "missing_service",
                        format!("The plugin requires {required}."),
                    )
                })?;
                if options.len() > 1 && !self.bindings.contains_key(required) {
                    return Err(Error::new(
                        "ambiguous_service",
                        format!("The service {required} needs an explicit binding."),
                    ));
                }
            }
        }
        let mut available = self.active.keys().cloned().collect::<BTreeSet<_>>();
        let mut order = Vec::new();
        while order.len() < pending.len() {
            let next = pending
                .iter()
                .find(|(id, plugin)| {
                    !available.contains(*id)
                        && plugin.manifest().requires.iter().all(|service| {
                            let providers = &all_providers[service];
                            let provider = self.bindings.get(service).unwrap_or(&providers[0]);
                            available.contains(provider)
                        })
                })
                .map(|(id, _)| id.clone());
            let Some(next) = next else {
                return Err(Error::new(
                    "dependency_cycle",
                    "The plugin service graph contains a cycle.",
                ));
            };
            available.insert(next.clone());
            order.push(next);
        }
        let mut staged: BTreeMap<String, ActivePlugin> = BTreeMap::new();
        for id in &order {
            let plugin = pending[id].clone();
            let manifest = plugin.manifest().clone();
            let mut dependencies = BTreeMap::new();
            for required in &manifest.requires {
                let providers = &all_providers[required];
                let selected = self.bindings.get(required).unwrap_or(&providers[0]);
                let active = staged
                    .get(selected)
                    .or_else(|| self.active.get(selected))
                    .ok_or_else(|| {
                        Error::new("missing_service", "The selected dependency is unavailable.")
                    })?;
                let value = active.context.registrations.get(required).ok_or_else(|| {
                    Error::new(
                        "missing_service",
                        "The dependency did not register its declared service.",
                    )
                })?;
                dependencies.insert(required.clone(), value.clone());
            }
            let allowed = grants
                .get(id)
                .cloned()
                .unwrap_or_default()
                .intersection(&manifest.requested_capabilities)
                .cloned()
                .collect();
            let mut context = PluginContext {
                manifest,
                grants: allowed,
                dependencies,
                registrations: BTreeMap::new(),
                contributions: Vec::new(),
                cleanup: Vec::new(),
            };
            let result = AssertUnwindSafe(plugin.activate(&mut context))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(Error::new(
                        "plugin_activation",
                        "The plugin activation panicked.",
                    ))
                });
            let result = result.and_then(|()| {
                if context
                    .manifest
                    .provides
                    .iter()
                    .all(|s| context.registrations.contains_key(s))
                {
                    Ok(())
                } else {
                    Err(Error::new(
                        "missing_service",
                        "The plugin did not register every declared service.",
                    ))
                }
            });
            if let Err(error) = result {
                let _ = AssertUnwindSafe(plugin.deactivate()).catch_unwind().await;
                drop(context);
                for previous in order.iter().rev() {
                    if let Some(active) = staged.remove(previous) {
                        let _ = AssertUnwindSafe(active.plugin.deactivate())
                            .catch_unwind()
                            .await;
                        drop(active);
                    }
                }
                return Err(error);
            }
            staged.insert(
                id.clone(),
                ActivePlugin {
                    plugin,
                    context,
                    owners: BTreeSet::from([owner.to_owned()]),
                },
            );
        }
        self.active.extend(staged);
        Ok(order)
    }
    pub fn add_owner(&mut self, plugin: &str, owner: &str) -> Result<()> {
        self.require_idle()?;
        self.active
            .get_mut(plugin)
            .ok_or_else(|| Error::new("not_found", "The plugin is unavailable."))?
            .owners
            .insert(owner.to_owned());
        Ok(())
    }
    fn removable(&self, owner: &str) -> Result<BTreeSet<String>> {
        self.require_idle()?;
        let removing = self
            .active
            .iter()
            .filter(|(_, p)| p.owners.len() == 1 && p.owners.contains(owner))
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        for (id, plugin) in &self.active {
            if removing.contains(id) {
                continue;
            }
            for required in &plugin.context.manifest.requires {
                let providers = self
                    .active
                    .iter()
                    .filter(|(_, p)| p.context.registrations.contains_key(required))
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>();
                let selected = self
                    .bindings
                    .get(required)
                    .or_else(|| providers.first().copied());
                if selected.is_some_and(|id| removing.contains(id)) {
                    return Err(Error::new(
                        "plugin_in_use",
                        "An active plugin requires a service from this owner.",
                    ));
                }
            }
        }
        Ok(removing)
    }

    pub fn can_eject_owner(&self, owner: &str) -> Result<()> {
        self.removable(owner).map(|_| ())
    }

    pub async fn eject_owner(&mut self, owner: &str) -> Result<Vec<String>> {
        let removing = self.removable(owner)?;
        let mut removed = Vec::new();
        let mut remaining = removing;
        while !remaining.is_empty() {
            let id = remaining
                .iter()
                .find(|candidate| {
                    let provides = &self.active[*candidate].context.manifest.provides;
                    !remaining.iter().any(|other| {
                        other != *candidate
                            && self.active[other]
                                .context
                                .manifest
                                .requires
                                .iter()
                                .any(|r| provides.contains(r))
                    })
                })
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        "dependency_cycle",
                        "The active dependency graph contains a cycle.",
                    )
                })?;
            let active = self
                .active
                .remove(&id)
                .ok_or_else(|| Error::new("not_found", "The plugin disappeared during removal."))?;
            let _ = AssertUnwindSafe(active.plugin.deactivate())
                .catch_unwind()
                .await;
            self.bindings.retain(|_, provider| provider != &id);
            remaining.remove(&id);
            removed.push(id);
            drop(active);
        }
        for active in self.active.values_mut() {
            active.owners.remove(owner);
        }
        Ok(removed)
    }
    fn validate_consumers(&self, bindings: &BTreeMap<String, String>) -> Result<()> {
        for active in self.active.values() {
            for (service, dependency) in &active.context.dependencies {
                if !self
                    .resolve_with(service, bindings)
                    .is_ok_and(|value| Arc::ptr_eq(dependency, &value))
                {
                    return Err(Error::new(
                        "plugin_in_use",
                        "An active consumer requires the previous service binding.",
                    ));
                }
            }
        }
        Ok(())
    }
    fn resolve(&self, name: &str) -> Result<ServiceValue> {
        self.resolve_with(name, &self.bindings)
    }
    fn resolve_with(
        &self,
        name: &str,
        bindings: &BTreeMap<String, String>,
    ) -> Result<ServiceValue> {
        if let Some(selected) = bindings.get(name) {
            return self
                .active
                .get(selected)
                .and_then(|p| p.context.registrations.get(name))
                .cloned()
                .ok_or_else(|| {
                    Error::new("missing_service", "The selected service is unavailable.")
                });
        }
        let options = self
            .active
            .values()
            .filter_map(|p| p.context.registrations.get(name))
            .collect::<Vec<_>>();
        match options.as_slice() {
            [value] => Ok((*value).clone()),
            [] => Err(Error::new(
                "missing_service",
                "The service has no provider.",
            )),
            _ => Err(Error::new(
                "ambiguous_service",
                "The service has several providers and no binding.",
            )),
        }
    }
    fn require_idle(&self) -> Result<()> {
        if self.active_generations.load(Ordering::SeqCst) > 0 {
            Err(Error::new(
                "plugin_change_deferred",
                "The host applies plugin changes after active generations finish.",
            ))
        } else {
            Ok(())
        }
    }
}

impl CompositionLease {
    pub fn optional_service<T: Any + Send + Sync>(&self, name: &str) -> Result<Option<Arc<T>>> {
        if self.services.contains_key(name) {
            self.service(name).map(Some)
        } else {
            Ok(None)
        }
    }
}
