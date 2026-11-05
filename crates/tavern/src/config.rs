use rustytavern_store::SqliteStore;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, provider::ProviderConfig, store::StoreBackend};

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
