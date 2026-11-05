use crate::OptionalField;
mod sillytavern;
pub use sillytavern::{CharacterFilter, RecursionDelay, WorldInfoEntry, WorldInfoFile};

use rustytavern_core::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_BOOK_ENTRIES: usize = 4096;

pub(crate) fn validate_entry_count(book: &Value) -> Result<()> {
    let count = match &book["entries"] {
        Value::Array(entries) => entries.len(),
        Value::Object(entries) => entries.len(),
        _ => 0,
    };
    if count > MAX_BOOK_ENTRIES {
        return Err(Error::new(
            "invalid_worldbook_size",
            "The worldbook exceeds its entry limit.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterBook {
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub name: OptionalField<String>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub description: OptionalField<String>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub scan_depth: OptionalField<serde_json::Number>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub token_budget: OptionalField<serde_json::Number>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub recursive_scanning: OptionalField<bool>,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
    pub entries: Vec<BookEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EntryId {
    Number(serde_json::Number),
    Text(String),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BookEntry {
    pub keys: Vec<String>,
    pub content: String,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub enabled: OptionalField<bool>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub insertion_order: OptionalField<serde_json::Number>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub case_sensitive: OptionalField<bool>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub use_regex: OptionalField<bool>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub constant: OptionalField<bool>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub name: OptionalField<String>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub priority: OptionalField<serde_json::Number>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub id: OptionalField<EntryId>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub comment: OptionalField<String>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub selective: OptionalField<bool>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub secondary_keys: OptionalField<Vec<String>>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub position: OptionalField<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BookSpec {
    #[serde(rename = "lorebook_v3")]
    V3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LorebookV3 {
    pub spec: BookSpec,
    pub data: CharacterBook,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WorldBookDocument {
    V3(LorebookV3),
    CharacterBook(CharacterBook),
    SillyTavern(WorldInfoFile),
}
impl WorldBookDocument {
    pub fn entry_count(&self) -> usize {
        match self {
            Self::V3(book) => book.data.entries.len(),
            Self::CharacterBook(book) => book.entries.len(),
            Self::SillyTavern(book) => book.entries.len(),
        }
    }
}
impl TryFrom<Value> for WorldBookDocument {
    type Error = Error;
    fn try_from(value: Value) -> Result<Self> {
        crate::validate_bounds(&value, 0)?;
        validate_entry_count(if value.get("spec").is_some() {
            &value["data"]
        } else {
            &value
        })?;
        if value.get("spec").is_some() {
            return serde_json::from_value(value)
                .map(Self::V3)
                .map_err(Error::context(
                    "invalid_worldbook_format",
                    "The worldbook envelope is invalid.",
                ));
        }
        if value["entries"].is_array() {
            serde_json::from_value(value)
                .map(Self::CharacterBook)
                .map_err(Error::context(
                    "invalid_worldbook_field",
                    "The character book fields are invalid.",
                ))
        } else if value["entries"].is_object() {
            serde_json::from_value(value)
                .map(Self::SillyTavern)
                .map_err(Error::context(
                    "invalid_worldbook_field",
                    "The World Info fields are invalid.",
                ))
        } else {
            Err(Error::new(
                "invalid_worldbook_format",
                "The worldbook requires an entries array or object.",
            ))
        }
    }
}
impl<'de> Deserialize<'de> for WorldBookDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::try_from(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldBook {
    id: String,
    #[serde(with = "crate::raw_bytes")]
    original: Vec<u8>,
    document: Value,
}
impl WorldBook {
    pub fn import(bytes: &[u8]) -> Result<Self> {
        let document = crate::character::read_json(bytes)?;
        WorldBookDocument::try_from(document.clone())?;
        Ok(Self {
            id: format!("worldbook-{}", rustytavern_core::digest(bytes)),
            original: bytes.to_vec(),
            document,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn validate(&self) -> Result<()> {
        let imported = Self::import(&self.original)?;
        if imported.id != self.id || imported.document != self.document {
            return Err(Error::new(
                "invalid_worldbook_archive",
                "The worldbook archive differs from its original bytes.",
            ));
        }
        Ok(())
    }
    pub fn document(&self) -> &Value {
        &self.document
    }
    pub fn definition(&self) -> Result<WorldBookDocument> {
        WorldBookDocument::try_from(self.document.clone())
    }
    pub fn export_original(&self) -> &[u8] {
        &self.original
    }
    pub fn export_edited(&self, definition: &WorldBookDocument) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec_pretty(definition)?;
        Self::import(&bytes)?;
        Ok(bytes)
    }
}
