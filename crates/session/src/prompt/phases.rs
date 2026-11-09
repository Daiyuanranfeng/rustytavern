use super::Contribution;
use rustytavern_core::{Error, Result, digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct HookPolicy {
    pub priority: i32,
    pub required: bool,
    pub timeout_ms: u64,
}
impl Default for HookPolicy {
    fn default() -> Self {
        Self {
            priority: 0,
            required: true,
            timeout_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnrichmentOutput {
    pub data: Value,
    pub contributions: Vec<Contribution>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditMode {
    Prepend,
    Append,
    Replace,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionEdit {
    pub section_id: String,
    pub mode: EditMode,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichmentRecord {
    pub service: String,
    pub plugin_id: String,
    pub policy: HookPolicy,
    pub output: Option<EnrichmentOutput>,
    pub error: Option<Error>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedEnrichment {
    pub schema_version: u32,
    pub input_hash: String,
    pub composition_id: String,
    pub complete: bool,
    pub records: Vec<EnrichmentRecord>,
}
impl SavedEnrichment {
    pub fn new(snapshot: &Value, composition_id: String) -> Result<Self> {
        Ok(Self {
            schema_version: 1,
            input_hash: digest(&serde_json::to_vec(snapshot)?),
            composition_id,
            complete: false,
            records: Vec::new(),
        })
    }
    pub fn validate(&self, snapshot: &Value, composition_id: &str) -> Result<()> {
        if self.schema_version != 1
            || self.composition_id != composition_id
            || self.input_hash != digest(&serde_json::to_vec(snapshot)?)
        {
            return Err(Error::new(
                "invalid_enrichment_snapshot",
                "The enrichment does not match the pinned input and composition.",
            ));
        }
        if !self.complete {
            return Err(Error::new(
                "incomplete_enrichment",
                "The enrichment phase does not complete.",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedEdit {
    pub plugin_id: String,
    pub service: String,
    pub edit: SectionEdit,
    pub before_hash: String,
    pub after_hash: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhaseTrace {
    pub base_profile: String,
    pub composition_id: String,
    pub enrichment_hash: String,
    pub contributions: Vec<Contribution>,
    pub edits: Vec<AppliedEdit>,
}
