pub mod error;
pub use error::{Error, Result};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

/// This function returns a stable SHA-256 identity.
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub requested_url: String,
    pub final_url: String,
    pub fetched_at_ms: i64,
    pub media_type: String,
    pub content_hash: String,
    pub creator: Option<String>,
    pub license: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Artifact {
    pub bytes: Vec<u8>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub description: String,
}

impl Persona {
    pub fn new(name: String, description: String) -> Result<Self> {
        if name.trim().is_empty() {
            return Err(crate::Error::new(
                "invalid_persona",
                "The persona name must not be empty.",
            ));
        }
        let identity = digest(&serde_json::to_vec(&(&name, &description))?);
        Ok(Self {
            id: format!("persona-{}", &identity[..24]),
            name,
            description,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: Role,
    pub text: String,
    pub generation_id: Option<String>,
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

pub fn native_artifact_hash() -> Result<String> {
    static HASH: OnceLock<String> = OnceLock::new();
    if let Some(hash) = HASH.get() {
        return Ok(hash.clone());
    }
    let hash = digest(&bounded_read(&std::env::current_exe()?, 256 * 1024 * 1024)?);
    let _ = HASH.set(hash.clone());
    Ok(hash)
}

pub fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::new(
            "invalid_extension_size",
            "The package file exceeds its byte limit.",
        ));
    }
    Ok(bytes)
}
