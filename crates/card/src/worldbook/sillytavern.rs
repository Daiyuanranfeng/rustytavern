use super::{CharacterBook, EntryId};
use crate::OptionalField;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldInfoFile {
    pub entries: BTreeMap<String, WorldInfoEntry>,
    #[serde(default, skip_serializing_if = "OptionalField::is_missing")]
    pub name: OptionalField<String>,
    #[serde(
        default,
        rename = "originalData",
        skip_serializing_if = "OptionalField::is_missing"
    )]
    pub original_data: OptionalField<CharacterBook>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RecursionDelay {
    Boolean(bool),
    Number(Number),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterFilter {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub tags: Vec<EntryId>,
    #[serde(default)]
    pub is_exclude: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorldInfoEntry {
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub uid: OptionalField<EntryId>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub key: OptionalField<Vec<String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub keysecondary: OptionalField<Vec<String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub comment: OptionalField<String>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub content: OptionalField<String>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub constant: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub vectorized: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub selective: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub selective_logic: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub add_memo: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub order: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub position: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub disable: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub ignore_budget: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub exclude_recursion: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub prevent_recursion: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_persona_description: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_character_description: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_character_personality: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_character_depth_prompt: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_scenario: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_creator_notes: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub delay_until_recursion: OptionalField<RecursionDelay>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub probability: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub use_probability: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub depth: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub outlet_name: OptionalField<String>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub group: OptionalField<String>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub group_override: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub group_weight: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub scan_depth: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub case_sensitive: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub match_whole_words: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub use_group_scoring: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub automation_id: OptionalField<String>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub role: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub sticky: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub cooldown: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub delay: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub character_filter_names: OptionalField<Vec<String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub character_filter_tags: OptionalField<Vec<EntryId>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub character_filter_exclude: OptionalField<bool>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub character_filter: OptionalField<CharacterFilter>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub triggers: OptionalField<Vec<String>>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub display_index: OptionalField<Number>,
    #[serde(skip_serializing_if = "OptionalField::is_missing")]
    pub extensions: OptionalField<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
