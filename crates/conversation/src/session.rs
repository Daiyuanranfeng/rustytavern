use serde::{Deserialize, Serialize};
use serde_json::Value;

use rustytavern_core::{Error, Result, digest};

pub use rustytavern_core::{Message, Persona, Role};
pub const MAX_SESSION_WORLDBOOKS: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub character_id: String,
    pub persona_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worldbook_ids: Vec<String>,
    pub provider: String,
    pub model: String,
    pub head: Option<String>,
    pub version: i64,
    pub accept_reduced: bool,
    pub options: crate::prompt::PromptOptions,
    #[serde(default)]
    pub composition: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfiguration {
    pub revision: u64,
    pub identity: String,
    pub character_id: String,
    pub persona_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worldbook_ids: Vec<String>,
    pub provider: String,
    pub model: String,
    pub accept_reduced: bool,
    pub options: crate::prompt::PromptOptions,
    pub profile: String,
    pub provider_config: crate::provider::ProviderConfig,
    pub composition: Value,
}
impl SessionConfiguration {
    pub fn new(
        session: &Session,
        profile: String,
        provider_config: crate::provider::ProviderConfig,
        composition: Value,
    ) -> Self {
        Self {
            revision: 0,
            identity: String::new(),
            character_id: session.character_id.clone(),
            persona_id: session.persona_id.clone(),
            worldbook_ids: session.worldbook_ids.clone(),
            provider: session.provider.clone(),
            model: session.model.clone(),
            accept_reduced: session.accept_reduced,
            options: session.options.clone(),
            profile,
            provider_config,
            composition,
        }
    }
    fn hash(&self) -> Result<String> {
        let mut value = serde_json::to_value(self)?;
        value.as_object_mut().unwrap().remove("identity");
        Ok(digest(&serde_json::to_vec(&value)?))
    }
    fn validate(&self) -> Result<()> {
        if self.revision == 0
            || self.identity != self.hash()?
            || [
                &self.character_id,
                &self.persona_id,
                &self.provider,
                &self.model,
                &self.profile,
            ]
            .iter()
            .any(|s| s.trim().is_empty())
            || !self.composition.is_object()
            || self.options.output_limit == 0
            || self.worldbook_ids.len() > MAX_SESSION_WORLDBOOKS
            || self.worldbook_ids.iter().any(|id| {
                id.strip_prefix("worldbook-").is_none_or(|hash| {
                    hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit())
                })
            })
            || self
                .worldbook_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.worldbook_ids.len()
            || self.options.context_limit <= self.options.output_limit
            || self.provider_config.output_limit == 0
            || self.provider_config.context_limit <= self.provider_config.output_limit
            || self.composition["identity"]
                .as_str()
                .is_none_or(str::is_empty)
        {
            return Err(Error::new(
                "invalid_session_configuration",
                "The session configuration is invalid.",
            ));
        }
        Ok(())
    }
}
impl Session {
    pub fn configurations(&self) -> Result<Vec<SessionConfiguration>> {
        if self.composition.get("configuration_schema").is_some()
            && self.composition["configuration_schema"] != 1
        {
            return Err(Error::new(
                "unsupported_session_configuration",
                "The session configuration requires another schema.",
            ));
        }
        let Some(records) = self.composition.get("session_configurations") else {
            if self.composition.get("configuration_schema").is_some() {
                return Err(Error::new(
                    "invalid_session_configuration",
                    "The configuration history is missing.",
                ));
            }
            return Ok(Vec::new());
        };
        if self.composition.get("configuration_schema").is_none() {
            return Err(Error::new(
                "unsupported_session_configuration",
                "The session configuration requires another schema.",
            ));
        }
        if records.as_array().is_none_or(|r| r.len() > 128)
            || serde_json::to_vec(records)?.len() > 8 * 1024 * 1024
        {
            return Err(Error::new(
                "session_configuration_limit",
                "The session configuration history exceeds its limit.",
            ));
        }
        let records: Vec<SessionConfiguration> = serde_json::from_value(records.clone())?;
        for (index, record) in records.iter().enumerate() {
            record.validate()?;
            if record.revision != index as u64 + 1 {
                return Err(Error::new(
                    "invalid_session_configuration",
                    "The configuration revisions are not consecutive.",
                ));
            }
        }
        Ok(records)
    }
    pub fn configuration(&self) -> Result<SessionConfiguration> {
        let record = self.configurations()?.pop().ok_or_else(|| {
            Error::new(
                "session_configuration_required",
                "The session requires an explicit configuration.",
            )
        })?;
        let mut composition = self.composition.clone();
        let fields = composition.as_object_mut().unwrap();
        fields.remove("session_configurations");
        fields.remove("configuration_schema");
        let current = SessionConfiguration::new(
            self,
            record.profile.clone(),
            record.provider_config.clone(),
            composition,
        );
        let mut current = serde_json::to_value(current)?;
        current["revision"] = serde_json::json!(record.revision);
        current["identity"] = serde_json::json!(record.identity);
        if current != serde_json::to_value(&record)? {
            return Err(Error::new(
                "invalid_session_configuration",
                "The session differs from its saved configuration.",
            ));
        }
        Ok(record)
    }
    pub fn append_configuration(&mut self, mut record: SessionConfiguration) -> Result<()> {
        let mut records = self.configurations()?;
        record.revision = records.len() as u64 + 1;
        record.identity = record.hash()?;
        record.validate()?;
        self.character_id = record.character_id.clone();
        self.persona_id = record.persona_id.clone();
        self.worldbook_ids = record.worldbook_ids.clone();
        self.provider = record.provider.clone();
        self.model = record.model.clone();
        self.accept_reduced = record.accept_reduced;
        self.options = record.options.clone();
        self.composition = record.composition.clone();
        records.push(record);
        self.composition["configuration_schema"] = serde_json::json!(1);
        self.composition["session_configurations"] = serde_json::to_value(records)?;
        self.configuration()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStatus {
    Prepared,
    Streaming,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl GenerationStatus {
    pub fn terminal(&self) -> bool {
        !matches!(self, Self::Prepared | Self::Streaming)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generation {
    pub id: String,
    pub session_id: String,
    pub user_node: String,
    pub status: GenerationStatus,
    pub draft: String,
    #[serde(default)]
    pub accepted_message: Option<String>,
    pub snapshot: Value,
    #[serde(default)]
    pub enrichment: Option<crate::prompt::phases::SavedEnrichment>,
    pub request: Option<Value>,
    pub usage: Option<Value>,
    pub error: Option<Error>,
    pub fencing_token: i64,
    pub session_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    pub schema_version: u32,
    pub session: Session,
    pub includes_prompt_traces: bool,
    pub characters: Vec<rustytavern_card::Card>,
    pub personas: Vec<Persona>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worldbooks: Vec<rustytavern_card::WorldBook>,
    pub plugin_state: Vec<PluginStateRevision>,
    pub messages: Vec<Message>,
    pub generations: Vec<Generation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStateRevision {
    pub namespace: String,
    pub key: String,
    pub revision: i64,
    pub payload: Value,
}

#[derive(Deserialize)]
struct ExportHeader {
    schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaExport {
    pub schema_version: u32,
    pub persona: Persona,
}
impl PersonaExport {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.schema_version != 1
            || Persona::new(self.persona.name.clone(), self.persona.description.clone())?.id
                != self.persona.id
        {
            return Err(Error::new(
                "invalid_persona_export",
                "The persona export identity or schema is invalid.",
            ));
        }
        Ok(serde_json::to_vec_pretty(self)?)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let header: ExportHeader = serde_json::from_slice(bytes)?;
        if header.schema_version != 1 {
            return Err(Error::new(
                "unsupported_export_schema",
                "The persona export requires another schema version.",
            ));
        }
        let export: Self = serde_json::from_slice(bytes)?;
        export.encode()?;
        Ok(export)
    }
}

impl SessionExport {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec_pretty(self)?)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let header: ExportHeader = serde_json::from_slice(bytes)?;
        if !matches!(header.schema_version, 2..=4) {
            return Err(Error::new(
                "unsupported_export_schema",
                "The session export requires schema version 2, 3, or 4.",
            ));
        }
        let export: Self = serde_json::from_slice(bytes)?;
        export.validate()?;
        Ok(export)
    }
    pub fn validate(&self) -> Result<()> {
        use std::collections::{BTreeMap, BTreeSet};
        let invalid = || {
            Error::new(
                "invalid_session_export",
                "The session export has invalid identities, references, or trace settings.",
            )
        };
        if !matches!(self.schema_version, 2..=4)
            || self.session.id.is_empty()
            || self.session.version < 0
            || self.schema_version < 4 && !self.worldbooks.is_empty()
        {
            return Err(invalid());
        }
        let mut characters = BTreeSet::new();
        for card in &self.characters {
            if !characters.insert(card.id.as_str())
                || card.validate().is_err()
                || self.schema_version == 2 && card.decoder.is_some()
            {
                return Err(invalid());
            }
        }
        let mut personas = BTreeSet::new();
        for persona in &self.personas {
            if !personas.insert(persona.id.as_str())
                || Persona::new(persona.name.clone(), persona.description.clone())?.id != persona.id
            {
                return Err(invalid());
            }
        }
        let mut worldbooks = BTreeSet::new();
        for book in &self.worldbooks {
            if book.validate().is_err() || !worldbooks.insert(book.id()) {
                return Err(invalid());
            }
        }
        let valid_worldbooks = |ids: &[String]| {
            ids.len() <= MAX_SESSION_WORLDBOOKS
                && ids.iter().collect::<BTreeSet<_>>().len() == ids.len()
                && ids.iter().all(|id| worldbooks.contains(id.as_str()))
                && self
                    .worldbooks
                    .iter()
                    .filter(|book| ids.iter().any(|id| id == book.id()))
                    .map(|book| book.export_original().len())
                    .sum::<usize>()
                    <= rustytavern_card::MAX_JSON_BYTES
        };
        if !characters.contains(self.session.character_id.as_str())
            || !personas.contains(self.session.persona_id.as_str())
            || !valid_worldbooks(&self.session.worldbook_ids)
        {
            return Err(invalid());
        }
        let configurations = self.session.configurations().map_err(|_| invalid())?;
        if !configurations.is_empty() {
            self.session.configuration().map_err(|_| invalid())?;
        }
        for configuration in configurations {
            if !characters.contains(configuration.character_id.as_str())
                || !personas.contains(configuration.persona_id.as_str())
                || !valid_worldbooks(&configuration.worldbook_ids)
            {
                return Err(invalid());
            }
        }
        let mut messages = BTreeMap::new();
        for message in &self.messages {
            if message.id.is_empty()
                || message
                    .parent_id
                    .as_deref()
                    .is_some_and(|id| !messages.contains_key(id))
                || messages.insert(message.id.as_str(), message).is_some()
            {
                return Err(invalid());
            }
        }
        if self
            .session
            .head
            .as_deref()
            .is_some_and(|id| !messages.contains_key(id))
        {
            return Err(invalid());
        }
        let mut generations = BTreeSet::new();
        for generation in &self.generations {
            if generation.id.is_empty()
                || !generations.insert(generation.id.as_str())
                || generation.session_id != self.session.id
                || !messages
                    .get(generation.user_node.as_str())
                    .is_some_and(|m| m.role == Role::User)
                || (!self.includes_prompt_traces && generation.request.is_some())
            {
                return Err(invalid());
            }
            if generation.status == GenerationStatus::Completed
                && (!self.messages.iter().any(|m| {
                    m.generation_id.as_deref() == Some(&generation.id) && m.role == Role::Assistant
                }) || (self.includes_prompt_traces && generation.request.is_none()))
            {
                return Err(invalid());
            }
            if let Some(id) = generation.snapshot["character_id"].as_str() {
                if !characters.contains(id) {
                    return Err(invalid());
                }
            }
            if let Some(id) = generation.snapshot["persona"]["id"].as_str() {
                if !personas.contains(id) {
                    return Err(invalid());
                }
            }
            if let Some(books) = generation.snapshot.get("worldbooks") {
                let books = books.as_array().ok_or_else(invalid)?;
                let ids: Vec<String> =
                    serde_json::from_value(generation.snapshot["session"]["worldbook_ids"].clone())
                        .map_err(|_| invalid())?;
                if !valid_worldbooks(&ids)
                    || books.len() != ids.len()
                    || books.iter().zip(&ids).any(|(book, id)| {
                        book["id"].as_str() != Some(id)
                            || !self.worldbooks.iter().any(|archive| {
                                archive.id() == id && archive.document() == &book["document"]
                            })
                    })
                {
                    return Err(invalid());
                }
            } else if generation.snapshot["session"]["worldbook_ids"]
                .as_array()
                .is_some_and(|ids| !ids.is_empty())
            {
                return Err(invalid());
            }
            if let Some(accepted) = &generation.accepted_message {
                if !matches!(
                    generation.status,
                    GenerationStatus::Failed
                        | GenerationStatus::Cancelled
                        | GenerationStatus::Interrupted
                ) || generation.draft.is_empty()
                    || !messages.get(accepted.as_str()).is_some_and(|m| {
                        m.role == Role::Assistant
                            && m.generation_id.as_ref() == Some(&generation.id)
                            && m.parent_id.as_ref() == Some(&generation.user_node)
                            && m.text == generation.draft
                    })
                {
                    return Err(invalid());
                }
            }
        }
        if self.messages.iter().any(|m| {
            m.generation_id
                .as_deref()
                .is_some_and(|id| !generations.contains(id))
        }) {
            return Err(invalid());
        }
        let mut state = BTreeSet::new();
        for revision in &self.plugin_state {
            if revision.namespace.is_empty()
                || revision.revision < 1
                || !state.insert((&revision.namespace, &revision.key, revision.revision))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
