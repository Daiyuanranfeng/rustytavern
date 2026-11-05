use rustytavern_store::SqliteStore;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    provider::{CredentialResolver, EnvironmentCredentials, ProviderConfig},
    store::StoreBackend,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Configuration {
    pub schema_version: u32,
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(flatten)]
    pub settings: BTreeMap<String, serde_json::Value>,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            schema_version: 1,
            providers: BTreeMap::new(),
            settings: BTreeMap::new(),
        }
    }
}

pub struct Application {
    pub data_dir: PathBuf,
    pub config: Configuration,
    pub store: Arc<dyn StoreBackend>,
}

impl Application {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_owned();
        std::fs::create_dir_all(&data_dir)?;
        let config = Configuration::load(&data_dir.join("config.json"))?;
        let store = Arc::new(SqliteStore::open(data_dir.join("sessions.sqlite3"))?);
        Ok(Self {
            data_dir,
            config,
            store,
        })
    }
    pub fn save_config(&self) -> Result<()> {
        self.config.save(&self.data_dir.join("config.json"))
    }
}

impl Configuration {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(path)?;
        if bytes.len() > 1024 * 1024 {
            return Err(Error::new(
                "configuration",
                "The configuration exceeds its byte limit.",
            ));
        }
        let config: Self = serde_json::from_slice(&bytes)?;
        if config.schema_version != 1 {
            return Err(Error::new(
                "configuration",
                "The configuration requires an unsupported schema version.",
            ));
        }
        Ok(config)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

/// The resolver reads `stored:NAME` references from `credentials.json` and other references from the environment.
pub struct StoredCredentials(PathBuf);
impl StoredCredentials {
    pub fn new(data_dir: &Path) -> Self {
        Self(data_dir.join("credentials.json"))
    }
    fn load(&self) -> Result<BTreeMap<String, String>> {
        match std::fs::read(&self.0) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }
    /// The method writes the key with owner-only permissions and returns its reference.
    pub fn save(&self, name: &str, key: &str) -> Result<String> {
        use std::io::Write;
        let mut keys = self.load()?;
        keys.insert(name.into(), key.trim().into());
        let temp = self
            .0
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options
            .open(&temp)?
            .write_all(&serde_json::to_vec(&keys)?)?;
        std::fs::rename(&temp, &self.0)?;
        Ok(format!("stored:{name}"))
    }
}
impl CredentialResolver for StoredCredentials {
    fn resolve(&self, reference: &str) -> Result<String> {
        match reference.strip_prefix("stored:") {
            Some(name) => self.load()?.remove(name).ok_or_else(|| {
                Error::new(
                    "configuration",
                    "The stored API key is unavailable. Use /setup.",
                )
            }),
            None => EnvironmentCredentials.resolve(reference),
        }
    }
}

pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RUSTYTAVERN_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path).join("rustytavern"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::new(
            "configuration",
            "The caller must provide --data-dir when HOME is unavailable.",
        )
    })?;
    #[cfg(target_os = "macos")]
    let path = PathBuf::from(home).join("Library/Application Support/rustytavern");
    #[cfg(not(target_os = "macos"))]
    let path = PathBuf::from(home).join(".local/share/rustytavern");
    Ok(path)
}
