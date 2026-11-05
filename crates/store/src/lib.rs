#[cfg(feature = "sqlite")]
mod sqlite;
use rustytavern_card::{Card, WorldBook};
use rustytavern_conversation::{
    Generation, GenerationStatus, Message, Persona, Session, SessionConfiguration, SessionExport,
};
use rustytavern_core::{Error, Result};
pub use rustytavern_core::{new_id, now_ms};
use serde::Serialize;
use serde_json::Value;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Preparation {
    pub generation: Generation,
    pub created: bool,
}

pub struct Submit<'a> {
    pub session_id: &'a str,
    pub expected_version: i64,
    pub text: &'a str,
    pub idempotency_key: Option<&'a str>,
    pub retry_user: Option<&'a str>,
    pub snapshot: Value,
    pub now: i64,
    pub lease_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginStateSnapshot {
    pub schema: u32,
    pub entries: Value,
}

impl PluginStateSnapshot {
    pub fn require_schema(&self, schema: u32) -> Result<()> {
        if schema == 0 || self.schema != schema {
            return Err(Error::new(
                "unsupported_plugin_state_schema",
                "The stored plugin state requires another schema.",
            ));
        }
        Ok(())
    }
}

/// A backend applies each mutation within one atomic transaction.
pub trait StoreBackend: Send + Sync {
    fn save_worldbook(&self, _book: &WorldBook) -> Result<()> {
        Err(Error::new(
            "unsupported_worldbook_store",
            "The store cannot save worldbooks.",
        ))
    }
    fn worldbook(&self, _id: &str) -> Result<WorldBook> {
        Err(Error::new(
            "unsupported_worldbook_store",
            "The store cannot load worldbooks.",
        ))
    }
    fn worldbooks(&self, ids: &[String]) -> Result<Vec<WorldBook>> {
        if ids.len() > rustytavern_conversation::MAX_SESSION_WORLDBOOKS
            || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
        {
            return Err(Error::new(
                "invalid_worldbook_selection",
                "The worldbook selection exceeds its limit or repeats an ID.",
            ));
        }
        let mut total = 0usize;
        ids.iter()
            .map(|id| {
                let book = self.worldbook(id)?;
                book.validate()?;
                if book.id() != id {
                    return Err(Error::new(
                        "invalid_worldbook_archive",
                        "The worldbook ID differs from the requested ID.",
                    ));
                }
                total = total.saturating_add(book.export_original().len());
                if total > rustytavern_card::MAX_JSON_BYTES {
                    return Err(Error::new(
                        "worldbook_selection_limit",
                        "The selected worldbooks exceed the byte limit.",
                    ));
                }
                Ok(book)
            })
            .collect()
    }
    fn save_card(&self, card: &Card) -> Result<()>;
    fn card(&self, id: &str) -> Result<Card>;
    fn save_persona(&self, persona: &Persona) -> Result<()>;
    fn persona(&self, id: &str) -> Result<Persona>;
    fn create_session(&self, session: &Session, greeting: Option<&str>) -> Result<Session>;
    fn session(&self, id: &str) -> Result<Session>;
    fn configure_session(
        &self,
        id: &str,
        expected_version: i64,
        configuration: SessionConfiguration,
    ) -> Result<Session>;
    fn messages(&self, session: &str) -> Result<Vec<Message>>;
    fn history(&self, session: &str) -> Result<Vec<Message>>;
    fn prepare(&self, input: Submit<'_>) -> Result<Preparation>;
    fn generation(&self, id: &str) -> Result<Generation>;
    fn generations(&self, session: &str) -> Result<Vec<Generation>>;
    fn record_enrichment(
        &self,
        generation: &str,
        token: i64,
        enrichment: &rustytavern_conversation::prompt::phases::SavedEnrichment,
        now: i64,
    ) -> Result<()>;
    fn save_request(&self, generation: &str, token: i64, request: &Value, now: i64) -> Result<()>;
    fn append_draft(
        &self,
        generation: &str,
        token: i64,
        text: &str,
        now: i64,
    ) -> Result<Generation>;
    fn record_usage(
        &self,
        generation: &str,
        token: i64,
        usage: Option<Value>,
        now: i64,
    ) -> Result<()>;
    fn renew(&self, generation: &str, token: i64, now: i64, lease_ms: i64) -> Result<()>;
    fn settle(
        &self,
        generation: &str,
        token: i64,
        status: GenerationStatus,
        usage: Option<Value>,
        error: Option<Error>,
        now: i64,
    ) -> Result<Generation>;
    fn recover(&self, session: &str, now: i64) -> Result<usize>;
    fn select_head(&self, session: &str, node: &str) -> Result<Session>;
    fn edit(&self, session: &str, node: &str, text: &str) -> Result<Message>;
    fn rebind(
        &self,
        session: &str,
        character: Option<&str>,
        persona: Option<&str>,
        accept_reduced: bool,
    ) -> Result<Session>;
    fn accept_draft(
        &self,
        session: &str,
        generation: &str,
        expected_version: i64,
    ) -> Result<Message>;
    fn export_session(&self, session: &str, include_traces: bool) -> Result<SessionExport>;
    fn list(&self, kind: &str) -> Result<Vec<Value>>;
    fn state_snapshot(&self, namespace: &str) -> Result<Value>;
    fn state_read(&self, namespace: &str, key: &str) -> Result<Option<Value>>;
    fn state_write(&self, namespace: &str, key: &str, value: &Value) -> Result<i64>;
    fn state_record(&self, namespace: &str, key: &str) -> Result<Option<(i64, Value)>>;
    fn state_write_if(
        &self,
        namespace: &str,
        key: &str,
        expected: Option<i64>,
        value: &Value,
    ) -> Result<i64>;
    fn plugin_state_snapshot(&self, namespace: &str) -> Result<PluginStateSnapshot>;
    fn plugin_state_write(
        &self,
        namespace: &str,
        schema: u32,
        key: &str,
        value: &Value,
    ) -> Result<i64>;
    fn plugin_state_migrate(
        &self,
        namespace: &str,
        expected: &PluginStateSnapshot,
        schema: u32,
        updates: &BTreeMap<String, Value>,
    ) -> Result<PluginStateSnapshot>;
}
