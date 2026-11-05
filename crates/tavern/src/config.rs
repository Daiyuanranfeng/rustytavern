//! The home directory holds `config.toml` and `rustytavern.db`.
//! The `config.toml` file holds every setting, including API keys.

use rustytavern_store::SqliteStore;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Error, Result,
    extension::Extension,
    provider::{CredentialResolver, ProviderConfig},
    store::StoreBackend,
};

pub const CONFIG_FILE: &str = "config.toml";
pub const DATABASE_FILE: &str = "rustytavern.db";
/// The database namespace of the per-session settings.
const SESSION_SETTINGS: &str = "rustytavern.session";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// The provider profile for new chats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderProfile>,
    #[serde(skip_serializing_if = "PluginSettings::is_empty")]
    pub plugins: PluginSettings,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Extension>,
}

/// A provider profile adds an optional API key to the provider settings.
/// A session records only the `config:NAME` reference, never the key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderProfile {
    #[serde(flatten)]
    pub settings: ProviderConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// The `[plugins]` table lists the enabled plugins. Each `[plugins.NAME]` table configures one plugin.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginSettings {
    /// An absent list enables the default plugins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<BTreeSet<String>>,
    /// Each entry selects one plugin for one service.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, String>,
    #[serde(flatten)]
    pub entries: BTreeMap<String, Value>,
}

/// An installed plugin table has a `path` to a local directory with a `plugin.json` file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InstalledPlugin {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub grants: BTreeSet<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trust_processes: bool,
}

impl PluginSettings {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.bindings.is_empty() && self.entries.is_empty()
    }
    pub fn enabled(&self) -> BTreeSet<String> {
        self.enabled.clone().unwrap_or_else(|| {
            crate::plugins::DEFAULT_ENABLED
                .iter()
                .map(|id| id.to_string())
                .collect()
        })
    }
    /// The method returns each table with a `path` key.
    pub fn installed(&self) -> Result<BTreeMap<String, InstalledPlugin>> {
        self.entries
            .iter()
            .filter(|(_, value)| value.get("path").is_some())
            .map(|(id, value)| Ok((id.clone(), serde_json::from_value(value.clone())?)))
            .collect()
    }
    /// The method returns the configuration of each essential plugin.
    pub fn essential_configuration(&self, home: &Path) -> BTreeMap<String, Value> {
        let mut result = self
            .entries
            .iter()
            .filter(|(_, value)| value.get("path").is_none())
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let skills = result.entry("skills".into()).or_insert_with(|| json!({}));
        if skills.get("paths").is_none() {
            skills["paths"] = json!([home.join("skills")]);
        }
        result
    }
}

impl Configuration {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = String::from_utf8(rustytavern_core::bounded_read(path, 1024 * 1024)?).map_err(
            Error::context("configuration", "The configuration is not UTF-8."),
        )?;
        let mut config: Self = toml_edit::de::from_str(&text).map_err(|error| {
            Error::new(
                "configuration",
                format!("The configuration is invalid: {error}"),
            )
        })?;
        for profile in config.providers.values_mut() {
            profile.settings.api_key_env = None;
        }
        Ok(config)
    }
    /// The method keeps the comments and key order of the existing file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut document: toml_edit::DocumentMut = existing.parse().unwrap_or_default();
        let next = toml_edit::ser::to_document(self).map_err(|error| {
            Error::new(
                "configuration",
                format!("The configuration cannot be saved: {error}"),
            )
        })?;
        merge(document.as_item_mut(), next.as_item(), 0);
        let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temp, document.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }
    /// The method returns the provider settings with a `config:NAME` key reference.
    pub fn provider(&self, name: &str) -> Result<ProviderConfig> {
        let profile = self.providers.get(name).ok_or_else(|| {
            Error::new(
                "configuration",
                format!("The provider profile {name} does not exist."),
            )
        })?;
        let mut settings = profile.settings.clone();
        if profile.api_key.is_some() {
            settings.api_key_env = Some(format!("config:{name}"));
        }
        Ok(settings)
    }
    /// The method replaces the provider settings and keeps an existing API key.
    pub fn set_provider(&mut self, name: &str, settings: ProviderConfig, api_key: Option<String>) {
        let profile = self.providers.entry(name.into()).or_default();
        profile.settings = settings;
        if api_key.is_some() {
            profile.api_key = api_key;
        }
        profile.settings.api_key_env = None;
    }
    /// The method hides each API key and MCP server environment value.
    pub fn redacted(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self)?;
        if let Some(providers) = value["providers"].as_object_mut() {
            for provider in providers.values_mut() {
                if provider.get("api_key").is_some() {
                    provider["api_key"] = json!("<hidden>");
                }
            }
        }
        if let Some(servers) = value
            .pointer_mut("/plugins/mcp/servers")
            .and_then(Value::as_object_mut)
        {
            for server in servers.values_mut() {
                if let Some(env) = server.get_mut("env").and_then(Value::as_object_mut) {
                    env.values_mut().for_each(|v| *v = json!("<hidden>"));
                }
            }
        }
        Ok(value)
    }
}

/// The function reads one TOML value, such as `["agent"]`, `8`, or `"text"`.
pub fn parse_value(text: &str) -> Result<Value> {
    let mut table: serde_json::Map<String, Value> =
        toml_edit::de::from_str(&format!("value = {text}")).map_err(|error| {
            Error::new("invalid_input", format!("The value is not TOML: {error}"))
        })?;
    Ok(table.remove("value").unwrap_or(Value::Null))
}

/// The function merges objects by key. Any other value replaces the target value.
fn merge_json(target: &mut Value, source: &Value) {
    match (target.as_object_mut(), source.as_object()) {
        (Some(target), Some(source)) => {
            for (key, value) in source {
                merge_json(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        _ => *target = source.clone(),
    }
}

/// The function sets one nested value or removes it and its empty parents.
fn set_path(target: &mut Value, path: &[&str], value: Option<Value>) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let Some(object) = target.as_object_mut() else {
        return;
    };
    match parents.first() {
        Some(first) => {
            let child = object.entry(first.to_string()).or_insert_with(|| json!({}));
            set_path(child, &path[1..], value);
            if child.as_object().is_some_and(|c| c.is_empty()) {
                object.remove(*first);
            }
        }
        None => match value {
            Some(value) => {
                object.insert(last.to_string(), value);
            }
            None => {
                object.remove(*last);
            }
        },
    }
}

/// The function copies `source` into `target` and keeps the decoration of retained items.
fn merge(target: &mut toml_edit::Item, source: &toml_edit::Item, depth: usize) {
    if let (Some(target), Some(source)) = (target.as_table_like_mut(), source.as_table_like()) {
        let stale = target
            .iter()
            .map(|(key, _)| key.to_owned())
            .filter(|key| source.get(key).is_none())
            .collect::<Vec<_>>();
        for key in stale {
            target.remove(&key);
        }
        for (key, value) in source.iter() {
            match target.get_mut(key) {
                Some(existing) => merge(existing, value, depth + 1),
                None => {
                    let mut value = value.clone();
                    expand(&mut value, depth);
                    target.insert(key, value);
                }
            }
        }
        return;
    }
    match (target.as_value_mut(), source.as_value()) {
        (Some(existing), Some(value))
            if existing.to_string().trim() != value.to_string().trim() =>
        {
            let decor = existing.decor().clone();
            *existing = value.clone();
            *existing.decor_mut() = decor;
        }
        (Some(_), Some(_)) => {}
        _ => {
            *target = source.clone();
            expand(target, depth);
        }
    }
}

/// The function writes a section header for each top-level table, each named entry, and each table of tables.
/// A small map, such as `parameters` or `env`, stays inline.
fn expand(item: &mut toml_edit::Item, depth: usize) {
    if let Some(inline) = item.as_inline_table() {
        let nested = inline.iter().any(|(_, value)| value.is_inline_table());
        if depth <= 1 || nested {
            *item = toml_edit::Item::Table(inline.clone().into_table());
        }
    }
    if let Some(table) = item.as_table_mut() {
        for (_, child) in table.iter_mut() {
            expand(child, depth + 1);
        }
        let headers_only = table.iter().all(|(_, child)| child.is_table());
        table.set_implicit(headers_only);
    }
}

pub struct Application {
    pub home: PathBuf,
    pub config: Configuration,
    pub store: Arc<dyn StoreBackend>,
}

impl Application {
    pub fn open(home: impl AsRef<Path>) -> Result<Self> {
        let home = home.as_ref().to_owned();
        std::fs::create_dir_all(&home)?;
        let config = Configuration::load(&home.join(CONFIG_FILE))?;
        let store = Arc::new(SqliteStore::open(home.join(DATABASE_FILE))?);
        Ok(Self {
            home,
            config,
            store,
        })
    }
    /// The method activates the essential plugins of `config.toml`.
    pub async fn registry(&self) -> Result<Arc<tokio::sync::Mutex<crate::plugin::PluginRegistry>>> {
        self.registry_with(&self.config).await
    }
    /// The method activates the essential plugins of a configuration.
    pub async fn registry_with(
        &self,
        config: &Configuration,
    ) -> Result<Arc<tokio::sync::Mutex<crate::plugin::PluginRegistry>>> {
        crate::plugins::registry(
            &crate::plugins::Environment::new(self.store.clone()),
            &config.plugins.enabled(),
            &config.plugins.essential_configuration(&self.home),
        )
        .await
    }
    /// The method returns the settings that one session adds to `config.toml`.
    pub fn session_settings(&self, session: &str) -> Result<Value> {
        Ok(self
            .store
            .state_read(SESSION_SETTINGS, session)?
            .unwrap_or_else(|| json!({})))
    }
    /// The method returns `config.toml` with the settings of one session.
    pub fn configuration_for(&self, session: Option<&str>) -> Result<Configuration> {
        let Some(session) = session else {
            return Ok(self.config.clone());
        };
        self.merged(&self.session_settings(session)?)
    }
    fn merged(&self, settings: &Value) -> Result<Configuration> {
        let mut value = serde_json::to_value(&self.config)?;
        merge_json(&mut value, settings);
        serde_json::from_value(value).map_err(|error| {
            Error::new(
                "invalid_session_setting",
                format!("The session setting is invalid: {error}"),
            )
        })
    }
    /// The method sets or removes one dotted `plugins` key for one session.
    pub fn set_session_setting(
        &self,
        session: &str,
        key: &str,
        value: Option<Value>,
    ) -> Result<Value> {
        self.store.session(session)?;
        let path = key.split('.').collect::<Vec<_>>();
        if path.len() < 2 || path[0] != "plugins" || path.iter().any(|part| part.is_empty()) {
            return Err(Error::new(
                "invalid_session_setting",
                "A session setting key starts with plugins, for example plugins.enabled.",
            ));
        }
        let mut settings = self.session_settings(session)?;
        set_path(&mut settings, &path, value);
        self.merged(&settings)?;
        self.store
            .state_write(SESSION_SETTINGS, session, &settings)?;
        Ok(settings)
    }
    pub fn credentials(&self) -> ConfigCredentials {
        ConfigCredentials::new(&self.config)
    }
    pub fn save_config(&self) -> Result<()> {
        self.config.save(&self.home.join(CONFIG_FILE))
    }
}

/// The resolver reads `config:NAME` references from the provider profiles.
pub struct ConfigCredentials(BTreeMap<String, String>);
impl ConfigCredentials {
    pub fn new(config: &Configuration) -> Self {
        Self(
            config
                .providers
                .iter()
                .filter_map(|(name, p)| Some((name.clone(), p.api_key.clone()?)))
                .collect(),
        )
    }
}
impl CredentialResolver for ConfigCredentials {
    fn resolve(&self, reference: &str) -> Result<String> {
        let name = reference.strip_prefix("config:").unwrap_or(reference);
        self.0.get(name).cloned().ok_or_else(|| {
            Error::new(
                "configuration",
                format!("The provider profile {name} has no API key. Use /provider."),
            )
        })
    }
}

/// The home is `RUSTYTAVERN_HOME`, or `.rustytavern` in the user home directory.
pub fn default_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RUSTYTAVERN_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::new(
            "configuration",
            "The caller must provide --home when HOME is unavailable.",
        )
    })?;
    Ok(PathBuf::from(home).join(".rustytavern"))
}
