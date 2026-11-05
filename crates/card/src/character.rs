use crate::OptionalField;
use crate::worldbook::CharacterBook;
use rustytavern_core::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardVersion {
    #[serde(rename = "chara_card_v2")]
    V2,
    #[serde(rename = "chara_card_v3")]
    V3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterCard {
    #[serde(rename = "spec")]
    pub version: CardVersion,
    pub spec_version: String,
    pub data: CharacterData,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CharacterCard {
    pub fn new(version: CardVersion, mut data: CharacterData) -> Self {
        if version == CardVersion::V3 && data.group_only_greetings.is_missing() {
            data.group_only_greetings = Vec::new().into();
        }
        Self {
            version,
            spec_version: match version {
                CardVersion::V2 => "2.0",
                CardVersion::V3 => "3.0",
            }
            .into(),
            data,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterData {
    pub name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_mes: String,
    pub mes_example: String,
    pub creator_notes: String,
    pub system_prompt: String,
    pub post_history_instructions: String,
    pub creator: String,
    pub character_version: String,
    pub alternate_greetings: Vec<String>,
    pub tags: Vec<String>,
    pub extensions: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub character_book: OptionalField<CharacterBook>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub assets: OptionalField<Vec<CharacterAsset>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub nickname: OptionalField<String>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub creator_notes_multilingual: OptionalField<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub source: OptionalField<Vec<String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub group_only_greetings: OptionalField<Vec<String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub creation_date: OptionalField<serde_json::Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub modification_date: OptionalField<serde_json::Number>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterAsset {
    #[serde(rename = "type")]
    pub kind: String,
    pub uri: String,
    pub name: String,
    pub ext: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LegacyCard {
    pub name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_mes: String,
    pub mes_example: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CardDocument {
    Versioned(Box<CharacterCard>),
    Legacy(LegacyCard),
}

impl CardDocument {
    pub fn parse_json(bytes: &[u8]) -> Result<Self> {
        Self::try_from(read_json(bytes)?)
    }
}

impl TryFrom<Value> for CardDocument {
    type Error = Error;
    fn try_from(value: Value) -> Result<Self> {
        crate::validate_bounds(&value, 0)?;
        if value.get("spec").is_some() {
            crate::worldbook::validate_entry_count(&value["data"]["character_book"])?;
            return serde_json::from_value(value)
                .map(Self::Versioned)
                .map_err(Error::context(
                    "unsupported_card_format",
                    "The versioned character card fields are invalid.",
                ));
        }
        let character = value["name"].is_string()
            && [
                "description",
                "personality",
                "scenario",
                "first_mes",
                "mes_example",
            ]
            .iter()
            .any(|key| value.get(key).is_some());
        if !character {
            return Err(Error::new(
                "unsupported_card_format",
                "The JSON does not identify a character card.",
            ));
        }
        serde_json::from_value(value)
            .map(Self::Legacy)
            .map_err(Error::context(
                "invalid_card_field",
                "The legacy character card fields are invalid.",
            ))
    }
}
impl<'de> Deserialize<'de> for CardDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::try_from(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

pub(super) fn read_json(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > crate::MAX_JSON_BYTES {
        return Err(Error::new(
            "invalid_card_size",
            "The format JSON exceeds its byte limit.",
        ));
    }
    let value = serde_json::from_slice(bytes)?;
    crate::validate_bounds(&value, 0)?;
    Ok(value)
}
