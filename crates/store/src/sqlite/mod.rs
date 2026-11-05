use super::{PluginStateSnapshot, Preparation, StoreBackend, Submit, new_id};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use rustytavern_card::{Card, WorldBook};
use rustytavern_conversation::prompt::expand;
use rustytavern_conversation::{
    Generation, GenerationStatus, Message, Persona, PluginStateRevision, Role, Session,
    SessionConfiguration, SessionExport,
};
use rustytavern_core::{Error, Result, digest, now_ms};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Mutex, MutexGuard},
};

pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }
    pub fn memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }
    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_millis(1500))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 5 {
            return Err(Error::new(
                "future_schema",
                "The database requires a newer RustyTavern version.",
            ));
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(include_str!("schema.sql"))?;
        transaction.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| Error::new("storage", "The store lock is unavailable."))
    }
}

fn decode<T: DeserializeOwned>(text: String) -> Result<T> {
    Ok(serde_json::from_str(&text)?)
}
fn encode<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}
fn absent() -> Error {
    Error::new("not_found", "The requested record does not exist.")
}
fn conflict() -> Error {
    Error::new(
        "session_conflict",
        "The session changed or another writer owns it.",
    )
}

fn read_payload<T: DeserializeOwned>(connection: &Connection, table: &str, id: &str) -> Result<T> {
    decode(
        connection
            .query_row(
                &format!("SELECT payload FROM {table} WHERE id=?1"),
                [id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(absent)?,
    )
}

fn read_session(connection: &Connection, id: &str) -> Result<Session> {
    let (payload, version, head): (String, i64, Option<String>) = connection
        .query_row(
            "SELECT payload,version,head FROM sessions WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or_else(absent)?;
    let mut session: Session = decode(payload)?;
    session.version = version;
    session.head = head;
    Ok(session)
}
fn read_generations(connection: &Connection, session: &str) -> Result<Vec<Generation>> {
    let mut query =
        connection.prepare("SELECT payload FROM generations WHERE session_id=?1 ORDER BY rowid")?;
    let rows = query.query_map([session], |row| row.get::<_, String>(0))?;
    rows.map(|row| decode(row?)).collect()
}

fn save_generation(connection: &Connection, generation: &Generation) -> Result<()> {
    connection.execute(
        "UPDATE generations SET payload=?1 WHERE id=?2",
        params![encode(generation)?, generation.id],
    )?;
    Ok(())
}
fn read_messages(connection: &Connection, session: &str) -> Result<Vec<Message>> {
    let mut statement = connection.prepare("SELECT id,parent_id,role,text,generation_id FROM messages WHERE session_id=?1 ORDER BY seq")?;
    let rows = statement.query_map([session], |row| {
        let role: String = row.get(2)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            role,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut messages = Vec::new();
    for row in rows {
        let (id, parent_id, role, text, generation_id) = row?;
        messages.push(Message {
            id,
            parent_id,
            role: serde_json::from_str(&role)?,
            text,
            generation_id,
        });
    }
    Ok(messages)
}
fn insert_message(connection: &Connection, session: &str, message: &Message) -> Result<()> {
    connection.execute("INSERT INTO messages(id,session_id,parent_id,role,text,generation_id) VALUES(?1,?2,?3,?4,?5,?6)", params![message.id, session, message.parent_id, encode(&message.role)?, message.text, message.generation_id])?;
    Ok(())
}
fn active_owner(connection: &Connection, session: &str, now: i64) -> Result<bool> {
    connection
        .query_row(
            "SELECT owner IS NOT NULL AND lease_until>?2 FROM sessions WHERE id=?1",
            params![session, now],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(absent)
}
fn check_writer(
    connection: &Connection,
    generation: &Generation,
    token: i64,
    now: i64,
) -> Result<()> {
    if generation.status.terminal() || token != generation.fencing_token {
        return Err(Error::new(
            "lease_lost",
            "The generation no longer owns this write.",
        ));
    }
    let valid: bool = connection.query_row(
        "SELECT owner=?2 AND fence=?3 AND lease_until>?4 AND version=?5 FROM sessions WHERE id=?1",
        params![
            generation.session_id,
            generation.id,
            token,
            now,
            generation.session_version
        ],
        |row| row.get(0),
    )?;
    if !valid {
        return Err(Error::new(
            "lease_lost",
            "The generation writer lease is no longer valid.",
        ));
    }
    Ok(())
}
fn recover_expired(connection: &Connection, session: &str, now: i64) -> Result<usize> {
    let (owner, until): (Option<String>, i64) = connection
        .query_row(
            "SELECT owner,lease_until FROM sessions WHERE id=?1",
            [session],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(absent)?;
    let Some(owner) = owner else {
        return Ok(0);
    };
    if until > now {
        return Ok(0);
    }
    let mut generation = read_payload::<Generation>(connection, "generations", &owner)?;
    if !generation.status.terminal() {
        generation.status = GenerationStatus::Interrupted;
        generation.error = Some(Error::new(
            "interrupted",
            "The writer lease expired before completion.",
        ));
        save_generation(connection, &generation)?;
    }
    connection.execute(
        "UPDATE sessions SET owner=NULL,lease_until=0,fence=fence+1,version=version+1 WHERE id=?1",
        [session],
    )?;
    Ok(1)
}

impl StoreBackend for SqliteStore {
    fn save_worldbook(&self, book: &WorldBook) -> Result<()> {
        book.validate()?;
        self.lock()?.execute(
            "INSERT OR IGNORE INTO worldbooks(id,payload) VALUES(?1,?2)",
            params![book.id(), encode(book)?],
        )?;
        Ok(())
    }
    fn worldbook(&self, id: &str) -> Result<WorldBook> {
        let book: WorldBook = read_payload(&*self.lock()?, "worldbooks", id)?;
        book.validate()?;
        Ok(book)
    }
    fn save_card(&self, card: &Card) -> Result<()> {
        card.validate()?;
        self.lock()?.execute(
            "INSERT OR IGNORE INTO cards(id,payload) VALUES(?1,?2)",
            params![card.id, encode(card)?],
        )?;
        Ok(())
    }
    fn card(&self, id: &str) -> Result<Card> {
        read_payload(&*self.lock()?, "cards", id)
    }
    fn save_persona(&self, persona: &Persona) -> Result<()> {
        self.lock()?.execute(
            "INSERT OR IGNORE INTO personas(id,payload) VALUES(?1,?2)",
            params![persona.id, encode(persona)?],
        )?;
        Ok(())
    }
    fn persona(&self, id: &str) -> Result<Persona> {
        read_payload(&*self.lock()?, "personas", id)
    }
    fn create_session(&self, session: &Session, greeting: Option<&str>) -> Result<Session> {
        self.worldbooks(&session.worldbook_ids)?;
        if session.composition.get("session_configurations").is_some() {
            session.configuration()?;
            if session.configurations()?.len() != 1 {
                return Err(Error::new(
                    "invalid_session_configuration",
                    "A new session requires one initial configuration.",
                ));
            }
        }
        let card = self.card(&session.character_id)?;
        let persona = self.persona(&session.persona_id)?;
        card.ensure_compatible(session.accept_reduced)?;
        let mut session = session.clone();
        session.id = if session.id.is_empty() {
            new_id("session")
        } else {
            session.id
        };
        session.version = 0;
        let greeting = greeting.map(|text| expand(text, card.name(), &persona.name, None));
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO sessions(id,payload,version,head) VALUES(?1,?2,0,NULL)",
            params![session.id, encode(&session)?],
        )?;
        if let Some(text) = greeting.filter(|s| !s.is_empty()) {
            let node = Message {
                id: new_id("message"),
                parent_id: None,
                role: Role::Assistant,
                text,
                generation_id: None,
            };
            insert_message(&transaction, &session.id, &node)?;
            transaction.execute(
                "UPDATE sessions SET head=?2 WHERE id=?1",
                params![session.id, node.id],
            )?;
            session.head = Some(node.id);
        }
        transaction.commit()?;
        Ok(session)
    }
    fn session(&self, id: &str) -> Result<Session> {
        read_session(&*self.lock()?, id)
    }
    fn configure_session(
        &self,
        id: &str,
        expected_version: i64,
        configuration: SessionConfiguration,
    ) -> Result<Session> {
        self.worldbooks(&configuration.worldbook_ids)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut session = read_session(&transaction, id)?;
        if session.version != expected_version || active_owner(&transaction, id, now_ms())? {
            return Err(conflict());
        }
        if session.composition.get("session_configurations").is_some() {
            session.configuration()?;
        }
        read_payload::<Card>(&transaction, "cards", &configuration.character_id)?
            .ensure_compatible(configuration.accept_reduced)?;
        read_payload::<Persona>(&transaction, "personas", &configuration.persona_id)?;
        session.append_configuration(configuration)?;
        transaction.execute(
            "UPDATE sessions SET payload=?2,version=version+1 WHERE id=?1",
            params![id, encode(&session)?],
        )?;
        session.version += 1;
        transaction.commit()?;
        Ok(session)
    }
    fn messages(&self, session: &str) -> Result<Vec<Message>> {
        read_messages(&*self.lock()?, session)
    }
    fn history(&self, session: &str) -> Result<Vec<Message>> {
        let connection = self.lock()?;
        let session = read_session(&connection, session)?;
        let all = read_messages(&connection, &session.id)?;
        let map = all
            .iter()
            .map(|m| (m.id.as_str(), m))
            .collect::<std::collections::HashMap<_, _>>();
        let mut selected = Vec::new();
        let mut head = session.head.as_deref();
        while let Some(id) = head {
            let node = map
                .get(id)
                .ok_or_else(|| Error::new("storage", "The branch contains a missing parent."))?;
            selected.push((*node).clone());
            head = node.parent_id.as_deref();
            if selected.len() > all.len() {
                return Err(Error::new("storage", "The branch contains a cycle."));
            }
        }
        selected.reverse();
        Ok(selected)
    }
    fn prepare(&self, input: Submit<'_>) -> Result<Preparation> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        recover_expired(&transaction, input.session_id, input.now)?;
        if let Some(key) = input.idempotency_key {
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT id FROM generations WHERE session_id=?1 AND idempotency_key=?2",
                    params![input.session_id, key],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                let generation = read_payload::<Generation>(&transaction, "generations", &id)?;
                if generation.snapshot["submitted_text"].as_str() != Some(input.text) {
                    return Err(Error::new(
                        "invalid_idempotency_key",
                        "The submission key already identifies different input.",
                    ));
                }
                transaction.commit()?;
                return Ok(Preparation {
                    generation,
                    created: false,
                });
            }
        }
        let session = read_session(&transaction, input.session_id)?;
        if session.version != input.expected_version
            || active_owner(&transaction, &session.id, input.now)?
        {
            return Err(conflict());
        }
        let user_node = if let Some(id) = input.retry_user {
            let role: Option<String> = transaction
                .query_row(
                    "SELECT role FROM messages WHERE id=?1 AND session_id=?2",
                    params![id, session.id],
                    |row| row.get(0),
                )
                .optional()?;
            if role.as_deref() != Some("\"user\"") {
                return Err(Error::new(
                    "invalid_retry",
                    "A retry requires a user node in this session.",
                ));
            }
            id.to_owned()
        } else {
            let message = Message {
                id: new_id("message"),
                parent_id: session.head,
                role: Role::User,
                text: input.text.to_owned(),
                generation_id: None,
            };
            insert_message(&transaction, input.session_id, &message)?;
            message.id
        };
        let token: i64 = transaction.query_row(
            "SELECT max(fence,COALESCE((SELECT json_extract(payload,'$.fencing_token')+1 FROM generations WHERE session_id=?1 ORDER BY rowid DESC LIMIT 1),1)) FROM sessions WHERE id=?1",
            [input.session_id],
            |row| row.get(0),
        )?;
        let mut snapshot = input.snapshot;
        let fields = snapshot.as_object_mut().ok_or(Error::new(
            "invalid_snapshot",
            "The snapshot must be an object.",
        ))?;
        fields.insert("submitted_text".into(), json!(input.text));
        let generation = Generation {
            id: new_id("generation"),
            session_id: input.session_id.to_owned(),
            user_node: user_node.clone(),
            status: GenerationStatus::Prepared,
            draft: String::new(),
            accepted_message: None,
            snapshot,
            request: None,
            enrichment: None,
            usage: None,
            error: None,
            fencing_token: token,
            session_version: session.version + 1,
        };
        transaction.execute(
            "INSERT INTO generations(id,session_id,idempotency_key,payload) VALUES(?1,?2,?3,?4)",
            params![
                generation.id,
                input.session_id,
                input.idempotency_key,
                encode(&generation)?
            ],
        )?;
        transaction.execute("UPDATE sessions SET head=?2,version=version+1,owner=?3,lease_until=?4,fence=?5 WHERE id=?1", params![input.session_id, user_node, generation.id, input.now.saturating_add(input.lease_ms), token])?;
        transaction.commit()?;
        Ok(Preparation {
            generation,
            created: true,
        })
    }
    fn generation(&self, id: &str) -> Result<Generation> {
        read_payload::<Generation>(&*self.lock()?, "generations", id)
    }
    fn generations(&self, session: &str) -> Result<Vec<Generation>> {
        let connection = self.lock()?;
        read_session(&connection, session)?;
        read_generations(&connection, session)
    }
    fn record_enrichment(
        &self,
        id: &str,
        token: i64,
        enrichment: &rustytavern_conversation::prompt::phases::SavedEnrichment,
        now: i64,
    ) -> Result<()> {
        if serde_json::to_vec(enrichment)?.len() > 8 * 1024 * 1024 {
            return Err(Error::new(
                "invalid_enrichment_size",
                "The saved enrichment exceeds its byte limit.",
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut generation = read_payload::<Generation>(&transaction, "generations", id)?;
        check_writer(&transaction, &generation, token, now)?;
        let previous = generation.enrichment.as_ref();
        let count = previous.map_or(0, |p| p.records.len());
        if generation.request.is_some()
            || enrichment.schema_version != 1
            || enrichment.input_hash != digest(&serde_json::to_vec(&generation.snapshot)?)
            || enrichment.records.len() < count
            || enrichment.records.len() > count + 1
            || previous.is_some_and(|p| {
                p.complete
                    || p.input_hash != enrichment.input_hash
                    || p.composition_id != enrichment.composition_id
            })
            || previous
                .map(|p| serde_json::to_value(&p.records))
                .transpose()?
                != previous
                    .map(|_| serde_json::to_value(&enrichment.records[..count]))
                    .transpose()?
        {
            return Err(Error::new(
                "invalid_enrichment_snapshot",
                "The enrichment record changes committed input or repeats a completed phase.",
            ));
        }
        generation.enrichment = Some(enrichment.clone());
        save_generation(&transaction, &generation)?;
        transaction.commit()?;
        Ok(())
    }
    fn save_request(&self, id: &str, token: i64, request: &Value, now: i64) -> Result<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut generation = read_payload::<Generation>(&transaction, "generations", id)?;
        check_writer(&transaction, &generation, token, now)?;
        if generation.request.is_some() {
            return Err(Error::new(
                "invalid_request_snapshot",
                "The request snapshot already exists.",
            ));
        }
        generation.request = Some(request.clone());
        save_generation(&transaction, &generation)?;
        transaction.commit()?;
        Ok(())
    }
    fn append_draft(&self, id: &str, token: i64, text: &str, now: i64) -> Result<Generation> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut generation = read_payload::<Generation>(&transaction, "generations", id)?;
        check_writer(&transaction, &generation, token, now)?;
        if generation.request.is_none() {
            return Err(Error::new(
                "invalid_generation_state",
                "The generation has no prepared request.",
            ));
        }
        if generation.draft.len().saturating_add(text.len()) > 8 * 1024 * 1024 {
            return Err(Error::new(
                "output_limit",
                "The draft exceeds the byte limit.",
            ));
        }
        generation.status = GenerationStatus::Streaming;
        generation.draft.push_str(text);
        save_generation(&transaction, &generation)?;
        transaction.commit()?;
        Ok(generation)
    }
    fn record_usage(&self, id: &str, token: i64, usage: Option<Value>, now: i64) -> Result<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut generation = read_payload::<Generation>(&transaction, "generations", id)?;
        check_writer(&transaction, &generation, token, now)?;
        generation.usage = usage;
        save_generation(&transaction, &generation)?;
        transaction.commit()?;
        Ok(())
    }
    fn renew(&self, id: &str, token: i64, now: i64, lease_ms: i64) -> Result<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation = read_payload::<Generation>(&transaction, "generations", id)?;
        check_writer(&transaction, &generation, token, now)?;
        transaction.execute(
            "UPDATE sessions SET lease_until=?2 WHERE id=?1",
            params![generation.session_id, now.saturating_add(lease_ms)],
        )?;
        transaction.commit()?;
        Ok(())
    }
    fn settle(
        &self,
        id: &str,
        token: i64,
        status: GenerationStatus,
        usage: Option<Value>,
        error: Option<Error>,
        now: i64,
    ) -> Result<Generation> {
        if !status.terminal() {
            return Err(Error::new(
                "invalid_generation_state",
                "A terminal operation requires a terminal status.",
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut generation = read_payload::<Generation>(&transaction, "generations", id)?;
        check_writer(&transaction, &generation, token, now)?;
        if status == GenerationStatus::Completed {
            if generation.request.is_none() {
                return Err(Error::new(
                    "invalid_generation_state",
                    "The completed generation has no request snapshot.",
                ));
            }
            let node = Message {
                id: new_id("message"),
                parent_id: Some(generation.user_node.clone()),
                role: Role::Assistant,
                text: generation.draft.clone(),
                generation_id: Some(generation.id.clone()),
            };
            insert_message(&transaction, &generation.session_id, &node)?;
            transaction.execute(
                "UPDATE sessions SET head=?2 WHERE id=?1",
                params![generation.session_id, node.id],
            )?;
        }
        generation.status = status;
        generation.usage = usage.or(generation.usage);
        generation.error = error;
        save_generation(&transaction, &generation)?;
        transaction.execute(
            "UPDATE sessions SET owner=NULL,lease_until=0,version=version+1 WHERE id=?1",
            [&generation.session_id],
        )?;
        transaction.commit()?;
        Ok(generation)
    }
    fn recover(&self, session: &str, now: i64) -> Result<usize> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count = recover_expired(&transaction, session, now)?;
        transaction.commit()?;
        Ok(count)
    }
    fn select_head(&self, session: &str, node: &str) -> Result<Session> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if active_owner(&transaction, session, now_ms())? {
            return Err(conflict());
        }
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND session_id=?2)",
            params![node, session],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(absent());
        }
        transaction.execute(
            "UPDATE sessions SET head=?2,version=version+1 WHERE id=?1",
            params![session, node],
        )?;
        let session = read_session(&transaction, session)?;
        transaction.commit()?;
        Ok(session)
    }
    fn edit(&self, session: &str, node: &str, text: &str) -> Result<Message> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if active_owner(&transaction, session, now_ms())? {
            return Err(conflict());
        }
        let messages = read_messages(&transaction, session)?;
        let original = messages
            .into_iter()
            .find(|m| m.id == node)
            .ok_or_else(absent)?;
        let new = Message {
            id: new_id("message"),
            parent_id: original.parent_id,
            role: original.role,
            text: text.to_owned(),
            generation_id: None,
        };
        insert_message(&transaction, session, &new)?;
        transaction.execute(
            "UPDATE sessions SET head=?2,version=version+1 WHERE id=?1",
            params![session, new.id],
        )?;
        transaction.commit()?;
        Ok(new)
    }
    fn rebind(
        &self,
        id: &str,
        character: Option<&str>,
        persona: Option<&str>,
        accept_reduced: bool,
    ) -> Result<Session> {
        if let Some(card) = character {
            self.card(card)?.ensure_compatible(accept_reduced)?;
        }
        if let Some(persona) = persona {
            self.persona(persona)?;
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if active_owner(&transaction, id, now_ms())? {
            return Err(conflict());
        }
        let mut session = read_session(&transaction, id)?;
        if let Some(character) = character {
            session.character_id = character.to_owned();
            session.accept_reduced = accept_reduced;
        }
        if let Some(persona) = persona {
            session.persona_id = persona.to_owned();
        }
        if let Some(mut configuration) = session.configurations()?.pop() {
            configuration.character_id = session.character_id.clone();
            configuration.persona_id = session.persona_id.clone();
            configuration.accept_reduced = session.accept_reduced;
            session.append_configuration(configuration)?;
        }
        transaction.execute(
            "UPDATE sessions SET payload=?2,version=version+1 WHERE id=?1",
            params![id, encode(&session)?],
        )?;
        session.version += 1;
        transaction.commit()?;
        Ok(session)
    }
    fn accept_draft(
        &self,
        id: &str,
        generation_id: &str,
        expected_version: i64,
    ) -> Result<Message> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session = read_session(&transaction, id)?;
        if session.version != expected_version || active_owner(&transaction, id, now_ms())? {
            return Err(conflict());
        }
        let mut generation =
            read_payload::<Generation>(&transaction, "generations", generation_id)?;
        if generation.session_id != id {
            return Err(absent());
        }
        if let Some(accepted) = &generation.accepted_message {
            let node = read_messages(&transaction, id)?
                .into_iter()
                .find(|m| &m.id == accepted)
                .ok_or_else(absent)?;
            transaction.commit()?;
            return Ok(node);
        }
        if !matches!(
            generation.status,
            GenerationStatus::Failed | GenerationStatus::Cancelled | GenerationStatus::Interrupted
        ) || generation.draft.is_empty()
            || session.head.as_ref() != Some(&generation.user_node)
        {
            return Err(Error::new(
                "invalid_draft_acceptance",
                "The selected branch does not end at a failed generation with a draft.",
            ));
        }
        let node = Message {
            id: new_id("message"),
            parent_id: Some(generation.user_node.clone()),
            role: Role::Assistant,
            text: generation.draft.clone(),
            generation_id: Some(generation.id.clone()),
        };
        insert_message(&transaction, id, &node)?;
        generation.accepted_message = Some(node.id.clone());
        save_generation(&transaction, &generation)?;
        transaction.execute(
            "UPDATE sessions SET head=?2,version=version+1 WHERE id=?1",
            params![id, node.id],
        )?;
        transaction.commit()?;
        Ok(node)
    }
    fn export_session(&self, id: &str, include_traces: bool) -> Result<SessionExport> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let session = read_session(&transaction, id)?;
        let messages = read_messages(&transaction, id)?;
        let mut generations = read_generations(&transaction, id)?;
        let mut character_ids = BTreeSet::from([session.character_id.clone()]);
        let mut persona_ids = BTreeSet::from([session.persona_id.clone()]);
        let mut worldbook_ids: BTreeSet<String> = session.worldbook_ids.iter().cloned().collect();
        let mut namespaces = BTreeSet::new();
        let configurations = session.configurations()?;
        for configuration in &configurations {
            character_ids.insert(configuration.character_id.clone());
            persona_ids.insert(configuration.persona_id.clone());
            worldbook_ids.extend(configuration.worldbook_ids.iter().cloned());
        }
        for composition in std::iter::once(&session.composition)
            .chain(configurations.iter().map(|c| &c.composition))
            .chain(
                generations
                    .iter()
                    .map(|g| &g.snapshot["plugin_composition"]),
            )
        {
            if let Some(plugins) = composition["plugins"].as_array() {
                for plugin in plugins {
                    if let Some(id) = plugin["manifest"]["id"].as_str() {
                        namespaces.insert(id.to_owned());
                    }
                }
            }
        }
        for generation in &mut generations {
            if let Some(books) = generation.snapshot["worldbooks"].as_array() {
                for book in books {
                    worldbook_ids.insert(
                        book["id"]
                            .as_str()
                            .ok_or_else(|| {
                                Error::new("invalid_snapshot", "The worldbook snapshot has no ID.")
                            })?
                            .to_owned(),
                    );
                }
            }
            if let Some(id) = generation.snapshot["character_id"].as_str() {
                character_ids.insert(id.to_owned());
            }
            if let Some(id) = generation.snapshot["persona"]["id"].as_str() {
                persona_ids.insert(id.to_owned());
            }
            if let Some(state) = generation.snapshot["plugin_state"].as_object() {
                namespaces.extend(state.keys().cloned());
            }
            if !include_traces {
                generation.request = None;
            }
        }
        let characters = character_ids
            .into_iter()
            .map(|id| read_payload(&transaction, "cards", &id))
            .collect::<Result<_>>()?;
        let personas = persona_ids
            .into_iter()
            .map(|id| read_payload(&transaction, "personas", &id))
            .collect::<Result<_>>()?;
        let worldbooks = worldbook_ids
            .into_iter()
            .map(|id| read_payload(&transaction, "worldbooks", &id))
            .collect::<Result<_>>()?;
        let schema_namespaces = namespaces
            .iter()
            .map(|id| state_schema_namespace(id))
            .collect::<Vec<_>>();
        namespaces.extend(schema_namespaces);
        let mut plugin_state = Vec::new();
        for namespace in namespaces {
            let mut query = transaction.prepare("SELECT key,revision,payload FROM plugin_state WHERE namespace=?1 ORDER BY key,revision")?;
            let rows = query.query_map([&namespace], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (key, revision, payload) = row?;
                plugin_state.push(PluginStateRevision {
                    namespace: namespace.clone(),
                    key,
                    revision,
                    payload: decode(payload)?,
                });
            }
        }
        transaction.commit()?;
        Ok(SessionExport {
            schema_version: 4,
            includes_prompt_traces: include_traces,
            session,
            characters,
            personas,
            worldbooks,
            plugin_state,
            messages,
            generations,
        })
    }
    fn list(&self, kind: &str) -> Result<Vec<Value>> {
        let table = match kind {
            "card" => "cards",
            "worldbook" => "worldbooks",
            "persona" => "personas",
            "session" => "sessions",
            _ => {
                return Err(Error::new(
                    "invalid_record_kind",
                    "The record kind is not supported.",
                ));
            }
        };
        let connection = self.lock()?;
        let mut statement =
            connection.prepare(&format!("SELECT payload FROM {table} ORDER BY rowid"))?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut result = Vec::new();
        for row in rows {
            let mut value: Value = decode(row?)?;
            if matches!(table, "cards" | "worldbooks") {
                value.as_object_mut().map(|o| o.remove("original"));
            }
            result.push(value);
        }
        Ok(result)
    }
    fn state_snapshot(&self, namespace: &str) -> Result<Value> {
        let connection = self.lock()?;
        read_state_snapshot(&connection, namespace)
    }
    fn state_read(&self, namespace: &str, key: &str) -> Result<Option<Value>> {
        Ok(self.state_record(namespace, key)?.map(|(_, value)| value))
    }
    fn state_record(&self, namespace: &str, key: &str) -> Result<Option<(i64, Value)>> {
        let row: Option<(i64, String)> = self.lock()?.query_row("SELECT revision,payload FROM plugin_state WHERE namespace=?1 AND key=?2 ORDER BY revision DESC LIMIT 1", params![namespace, key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        row.map(|(revision, payload)| Ok((revision, decode(payload)?)))
            .transpose()
    }
    fn state_write(&self, namespace: &str, key: &str, value: &Value) -> Result<i64> {
        self.state_write_if(namespace, key, None, value)
    }
    fn state_write_if(
        &self,
        namespace: &str,
        key: &str,
        expected: Option<i64>,
        value: &Value,
    ) -> Result<i64> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = append_state(&transaction, namespace, key, expected, value)?;
        transaction.commit()?;
        Ok(revision)
    }
    fn plugin_state_snapshot(&self, namespace: &str) -> Result<PluginStateSnapshot> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let snapshot = read_plugin_state(&transaction, namespace)?;
        transaction.commit()?;
        Ok(snapshot)
    }
    fn plugin_state_write(
        &self,
        namespace: &str,
        schema: u32,
        key: &str,
        value: &Value,
    ) -> Result<i64> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if schema == 0 || read_state_schema(&transaction, namespace)? != schema {
            return Err(Error::new(
                "unsupported_plugin_state_schema",
                "The stored plugin state requires another schema.",
            ));
        }
        let revision = append_state(&transaction, namespace, key, None, value)?;
        transaction.commit()?;
        Ok(revision)
    }
    fn plugin_state_migrate(
        &self,
        namespace: &str,
        expected: &PluginStateSnapshot,
        schema: u32,
        updates: &BTreeMap<String, Value>,
    ) -> Result<PluginStateSnapshot> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_plugin_state(&transaction, namespace)?;
        if current != *expected {
            return Err(Error::new(
                "configuration_conflict",
                "Another process changed the plugin state.",
            ));
        }
        if schema <= current.schema {
            return Err(Error::new(
                "unsupported_plugin_state_schema",
                "The migration requires a newer schema.",
            ));
        }
        for (key, value) in updates {
            append_state(&transaction, namespace, key, None, value)?;
        }
        append_state(
            &transaction,
            &state_schema_namespace(namespace),
            "schema",
            None,
            &json!(schema),
        )?;
        let snapshot = read_plugin_state(&transaction, namespace)?;
        transaction.commit()?;
        Ok(snapshot)
    }
}

fn state_schema_namespace(namespace: &str) -> String {
    format!("@state-schema/{namespace}")
}

fn read_state_schema(connection: &Connection, namespace: &str) -> Result<u32> {
    let payload: Option<String> = connection.query_row("SELECT payload FROM plugin_state WHERE namespace=?1 AND key='schema' ORDER BY revision DESC LIMIT 1", [state_schema_namespace(namespace)], |row| row.get(0)).optional()?;
    match payload {
        None => Ok(1),
        Some(payload) => serde_json::from_str::<u32>(&payload)
            .ok()
            .filter(|schema| *schema > 0)
            .ok_or_else(|| {
                Error::new(
                    "unsupported_plugin_state_schema",
                    "The plugin state schema marker is unreadable.",
                )
            }),
    }
}

fn read_plugin_state(connection: &Connection, namespace: &str) -> Result<PluginStateSnapshot> {
    Ok(PluginStateSnapshot {
        schema: read_state_schema(connection, namespace)?,
        entries: read_state_snapshot(connection, namespace)?,
    })
}

fn read_state_snapshot(connection: &Connection, namespace: &str) -> Result<Value> {
    let mut query = connection.prepare("SELECT s.key,s.revision,s.payload FROM plugin_state s WHERE s.namespace=?1 AND s.revision=(SELECT MAX(revision) FROM plugin_state WHERE namespace=s.namespace AND key=s.key) ORDER BY s.key")?;
    let rows = query.query_map([namespace], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut result = serde_json::Map::new();
    for row in rows {
        let (key, revision, payload) = row?;
        result.insert(
            key,
            json!({"revision":revision,"value":decode::<Value>(payload)?}),
        );
    }
    Ok(Value::Object(result))
}

fn append_state(
    connection: &Connection,
    namespace: &str,
    key: &str,
    expected: Option<i64>,
    value: &Value,
) -> Result<i64> {
    let previous: i64 = connection.query_row(
        "SELECT COALESCE(MAX(revision),0) FROM plugin_state WHERE namespace=?1 AND key=?2",
        params![namespace, key],
        |row| row.get(0),
    )?;
    if expected.is_some_and(|revision| revision != previous) {
        return Err(Error::new(
            "configuration_conflict",
            "Another process changed this configuration.",
        ));
    }
    let revision = previous
        .checked_add(1)
        .ok_or_else(|| Error::new("storage", "The revision range is exhausted."))?;
    connection.execute(
        "INSERT INTO plugin_state(namespace,key,revision,payload) VALUES(?1,?2,?3,?4)",
        params![namespace, key, revision, encode(value)?],
    )?;
    Ok(revision)
}
