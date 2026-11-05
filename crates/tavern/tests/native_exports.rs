use std::sync::Arc;

use async_trait::async_trait;
use rustytavern::{
    Result,
    card::Card,
    prompt::PromptOptions,
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderStream,
    },
    runtime::Runtime,
    session::{GenerationStatus, Persona, PersonaExport, Role, Session, SessionExport},
    store::{SqliteStore, StoreBackend, Submit, now_ms},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn session(store: &dyn StoreBackend, card: &Card, persona: &Persona) -> Session {
    store.save_card(card).unwrap();
    store.save_persona(persona).unwrap();
    store.create_session(&Session { id: String::new(), character_id: card.id.clone(), persona_id: persona.id.clone(), worldbook_ids: Vec::new(), provider: "fixture".into(), model: "fixture".into(), head: None, version: 0, accept_reduced: true, options: PromptOptions::default(), composition: json!({"plugins":[{"manifest":{"id":"org.example.future","state_schema":7,"unknown":{"keep":42}}}]}) }, Some("A greeting.")).unwrap()
}
fn submit(store: &dyn StoreBackend, session: &Session) -> rustytavern::store::Preparation {
    store.prepare(Submit { session_id: &session.id, expected_version: session.version, text: "A question.", idempotency_key: None, retry_user: None, snapshot: json!({"character_id":session.character_id,"persona":store.persona(&session.persona_id).unwrap(),"plugin_composition":session.composition}), now: now_ms(), lease_ms: 30_000 }).unwrap()
}
fn draft(
    store: &dyn StoreBackend,
    session: &Session,
    status: GenerationStatus,
    text: &str,
) -> rustytavern::session::Generation {
    let prepared = submit(store, session);
    let generation = prepared.generation;
    store
        .save_request(
            &generation.id,
            generation.fencing_token,
            &json!({"fixture":"request"}),
            now_ms(),
        )
        .unwrap();
    store
        .append_draft(&generation.id, generation.fencing_token, text, now_ms())
        .unwrap();
    store
        .settle(
            &generation.id,
            generation.fencing_token,
            status,
            None,
            None,
            now_ms(),
        )
        .unwrap()
}
fn card() -> Card {
    Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap()
}
fn persona() -> Persona {
    Persona::new("Alex".into(), "A visitor.".into()).unwrap()
}

#[tokio::test]
async fn explicit_draft_acceptance_adds_traceable_input_without_completing_the_generation() {
    for status in [
        GenerationStatus::Cancelled,
        GenerationStatus::Failed,
        GenerationStatus::Interrupted,
    ] {
        let store = Arc::new(SqliteStore::memory().unwrap());
        let session = session(store.as_ref(), &card(), &persona());
        let runtime = Runtime::new(store.clone());
        let session = runtime
            .configure_session(&session, ProviderConfig::default())
            .await
            .unwrap();
        let generation = draft(
            store.as_ref(),
            &session,
            status.clone(),
            "PARTIAL_REPLY_SENTINEL",
        );
        let runtime = Runtime::new(store.clone());
        let before = runtime.inspect(&session.id, "Next.").await.unwrap();
        assert!(
            !before
                .sections
                .iter()
                .any(|s| s.text == "PARTIAL_REPLY_SENTINEL")
        );
        let current = store.session(&session.id).unwrap();
        assert_eq!(
            store
                .accept_draft(&session.id, &generation.id, current.version - 1)
                .unwrap_err()
                .code,
            "session_conflict"
        );
        let accepted = store
            .accept_draft(&session.id, &generation.id, current.version)
            .unwrap();
        assert_eq!(accepted.parent_id.as_ref(), Some(&generation.user_node));
        assert_eq!(accepted.generation_id.as_ref(), Some(&generation.id));
        let result = store.generation(&generation.id).unwrap();
        assert_eq!(result.status, status);
        assert_eq!(result.accepted_message.as_ref(), Some(&accepted.id));
        assert_eq!(
            store
                .append_draft(&generation.id, generation.fencing_token, "Late.", now_ms())
                .unwrap_err()
                .code,
            "lease_lost"
        );
        let after = runtime.inspect(&session.id, "Next.").await.unwrap();
        assert!(
            after
                .sections
                .iter()
                .any(|s| s.text == "PARTIAL_REPLY_SENTINEL" && s.source == "history")
        );
        let repeated = store
            .accept_draft(
                &session.id,
                &generation.id,
                store.session(&session.id).unwrap().version,
            )
            .unwrap();
        assert_eq!(repeated.id, accepted.id);
        assert_eq!(store.messages(&session.id).unwrap().len(), 3);
        let export = store.export_session(&session.id, false).unwrap();
        let mut invalid = export.clone();
        invalid.messages.last_mut().unwrap().role = Role::User;
        assert!(invalid.encode().is_err());
        assert_eq!(
            SessionExport::decode(&export.encode().unwrap())
                .unwrap()
                .generations[0]
                .accepted_message,
            Some(accepted.id)
        );
    }
}

#[test]
fn acceptance_rejects_live_empty_completed_foreign_and_unselected_drafts() {
    let store = SqliteStore::memory().unwrap();
    let first = session(&store, &card(), &persona());
    let prepared = submit(&store, &first);
    let active = store.session(&first.id).unwrap();
    assert_eq!(
        store
            .accept_draft(&first.id, &prepared.generation.id, active.version)
            .unwrap_err()
            .code,
        "session_conflict"
    );
    store
        .settle(
            &prepared.generation.id,
            prepared.generation.fencing_token,
            GenerationStatus::Cancelled,
            None,
            None,
            now_ms(),
        )
        .unwrap();
    assert_eq!(
        store
            .accept_draft(
                &first.id,
                &prepared.generation.id,
                store.session(&first.id).unwrap().version
            )
            .unwrap_err()
            .code,
        "invalid_draft_acceptance"
    );
    let second = session(&store, &card(), &persona());
    assert_eq!(
        store
            .accept_draft(&second.id, &prepared.generation.id, second.version)
            .unwrap_err()
            .code,
        "not_found"
    );
    let cancelled = draft(
        &store,
        &store.session(&first.id).unwrap(),
        GenerationStatus::Cancelled,
        "A partial reply.",
    );
    store
        .edit(&first.id, &cancelled.user_node, "Another branch.")
        .unwrap();
    assert_eq!(
        store
            .accept_draft(
                &first.id,
                &cancelled.id,
                store.session(&first.id).unwrap().version
            )
            .unwrap_err()
            .code,
        "invalid_draft_acceptance"
    );
    store.select_head(&first.id, &cancelled.user_node).unwrap();
    store
        .accept_draft(
            &first.id,
            &cancelled.id,
            store.session(&first.id).unwrap().version,
        )
        .unwrap();
    let completed = draft(
        &store,
        &second,
        GenerationStatus::Completed,
        "A full reply.",
    );
    assert_eq!(
        store
            .accept_draft(
                &second.id,
                &completed.id,
                store.session(&second.id).unwrap().version
            )
            .unwrap_err()
            .code,
        "invalid_draft_acceptance"
    );
}

#[test]
fn native_exports_retain_revision_archives_branches_and_opaque_state() {
    let store = SqliteStore::memory().unwrap();
    let original = Card::import(include_bytes!("fixtures/synthetic/dual.png")).unwrap();
    let first_persona = persona();
    let first = session(&store, &original, &first_persona);
    let unknown =
        json!({"schema_version":7,"unknown":{"nested":[null,true,"KEEP"],"future":{"value":42}}});
    store
        .state_write("org.example.future", "opaque", &unknown)
        .unwrap();
    let generation = draft(
        &store,
        &first,
        GenerationStatus::Completed,
        "A first reply.",
    );
    let old_head = store.session(&first.id).unwrap().head.unwrap();
    store
        .edit(&first.id, &generation.user_node, "Another question.")
        .unwrap();
    let mut changed = original.document.clone();
    changed["data"]["description"] = json!("A changed description.");
    let updated = Card::import(&serde_json::to_vec(&changed).unwrap()).unwrap();
    let second_persona = Persona::new("Blake".into(), "Another visitor.".into()).unwrap();
    store.save_card(&updated).unwrap();
    store.save_persona(&second_persona).unwrap();
    store
        .rebind(&first.id, Some(&updated.id), Some(&second_persona.id), true)
        .unwrap();
    store
        .state_write("org.example.future", "opaque", &json!({"revision":"new"}))
        .unwrap();
    store
        .state_write("unrelated", "private", &json!("UNRELATED_STATE_SENTINEL"))
        .unwrap();
    let export = store.export_session(&first.id, false).unwrap();
    assert!(!export.includes_prompt_traces);
    assert!(export.generations.iter().all(|g| g.request.is_none()));
    assert_eq!(export.characters.len(), 2);
    assert_eq!(export.personas.len(), 2);
    assert_eq!(
        export
            .characters
            .iter()
            .find(|c| c.id == original.id)
            .unwrap()
            .export_original(),
        original.export_original()
    );
    assert!(export.messages.iter().any(|m| m.id == old_head));
    assert_eq!(export.plugin_state.len(), 2);
    assert_eq!(export.plugin_state[0].payload, unknown);
    assert_eq!(export.plugin_state[0].revision, 1);
    let bytes = export.encode().unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("UNRELATED_STATE_SENTINEL"));
    let decoded = SessionExport::decode(&bytes).unwrap();
    assert_eq!(
        serde_json::to_value(&decoded).unwrap(),
        serde_json::to_value(&export).unwrap()
    );
    assert!(
        store.export_session(&first.id, true).unwrap().generations[0]
            .request
            .is_some()
    );
    let envelope = PersonaExport {
        schema_version: 1,
        persona: first_persona,
    };
    assert_eq!(
        PersonaExport::decode(&envelope.encode().unwrap())
            .unwrap()
            .persona
            .id,
        envelope.persona.id
    );
    let mut future = serde_json::to_value(export).unwrap();
    future["schema_version"] = json!(99);
    assert_eq!(
        SessionExport::decode(&serde_json::to_vec(&future).unwrap())
            .unwrap_err()
            .code,
        "unsupported_export_schema"
    );
}

#[test]
fn export_decoders_reject_broken_references_and_raw_card_changes() {
    let store = SqliteStore::memory().unwrap();
    let session = session(&store, &card(), &persona());
    draft(&store, &session, GenerationStatus::Completed, "A reply.");
    let valid = serde_json::to_value(store.export_session(&session.id, true).unwrap()).unwrap();
    for field in [
        "head",
        "character",
        "parent",
        "bytes",
        "trace",
        "persona",
        "format",
    ] {
        let mut invalid = valid.clone();
        match field {
            "head" => invalid["session"]["head"] = json!("missing"),
            "character" => invalid["characters"] = json!([]),
            "parent" => invalid["messages"][0]["parent_id"] = json!("missing"),
            "bytes" => invalid["characters"][0]["document"]["data"]["name"] = json!("Wrong name"),
            "trace" => invalid["includes_prompt_traces"] = json!(false),
            "format" => invalid["characters"][0]["format"] = json!("png"),
            _ => invalid["personas"][0]["name"] = json!("Wrong persona"),
        }
        assert_eq!(
            SessionExport::decode(&serde_json::to_vec(&invalid).unwrap())
                .unwrap_err()
                .code,
            "invalid_session_export"
        );
    }
}

const CREDENTIAL_SENTINEL: &str = "NATIVE_EXPORT_CREDENTIAL_SENTINEL";
struct Credentials;
impl CredentialResolver for Credentials {
    fn resolve(&self, _: &str) -> Result<String> {
        Ok(CREDENTIAL_SENTINEL.into())
    }
}
struct SecretProvider;
#[async_trait]
impl ProviderAdapter for SecretProvider {
    fn prepare(
        &self,
        plan: &rustytavern::prompt::PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        HttpProvider.prepare(plan, model, config)
    }
    async fn open_stream(
        &self,
        request: PreparedRequest,
        credentials: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        assert_eq!(
            credentials.resolve(request.credential_reference.as_deref().unwrap())?,
            CREDENTIAL_SENTINEL
        );
        Ok(Box::pin(futures_util::stream::iter([
            Ok(ProviderEvent::Text {
                text: "A reply.".into(),
            }),
            Ok(ProviderEvent::Complete),
        ])))
    }
}
#[tokio::test]
async fn both_export_modes_exclude_the_resolved_provider_credential() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let session = session(store.as_ref(), &card(), &persona());
    let runtime = Runtime::new(store.clone())
        .with_provider(Arc::new(SecretProvider))
        .with_credentials(Arc::new(Credentials));
    runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    runtime
        .start(&session.id, "Hello.", None, ProviderConfig::default())
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    for traces in [false, true] {
        let export = store.export_session(&session.id, traces).unwrap();
        let bytes = export.encode().unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(CREDENTIAL_SENTINEL));
        SessionExport::decode(&bytes).unwrap();
    }
}

#[test]
fn schema_upgrade_preserves_old_rows_and_a_failed_upgrade_rolls_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let session = session(&store, &card(), &persona());
    let generation = draft(&store, &session, GenerationStatus::Cancelled, "A draft.");
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let payload: String = connection
        .query_row(
            "SELECT payload FROM generations WHERE id=?1",
            [&generation.id],
            |r| r.get(0),
        )
        .unwrap();
    let mut old: Value = serde_json::from_str(&payload).unwrap();
    old.as_object_mut().unwrap().remove("enrichment");
    old.as_object_mut().unwrap().remove("accepted_message");
    connection
        .execute(
            "UPDATE generations SET payload=?1 WHERE id=?2",
            rusqlite::params![old.to_string(), generation.id],
        )
        .unwrap();
    let original = card();
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy["original"] = json!(original.original);
    connection
        .execute(
            "UPDATE cards SET payload=?1 WHERE id=?2",
            rusqlite::params![legacy.to_string(), original.id],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    drop(connection);
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.card(&original.id).unwrap().original,
        original.original
    );
    let migrated = store.generation(&generation.id).unwrap();
    assert!(migrated.enrichment.is_none() && migrated.accepted_message.is_none());
    assert_eq!(migrated.draft, "A draft.");
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    drop(connection);
    let broken = directory.path().join("broken.sqlite3");
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 2)
        .unwrap();
    assert_eq!(
        SqliteStore::open(&path)
            .unwrap()
            .generation(&generation.id)
            .unwrap()
            .draft,
        "A draft."
    );
    let connection = rusqlite::Connection::open(&broken).unwrap();
    connection.execute_batch("CREATE TABLE cards(id TEXT PRIMARY KEY,payload TEXT);INSERT INTO cards VALUES('keep','opaque-data');CREATE TABLE generations(id TEXT PRIMARY KEY,payload TEXT);PRAGMA user_version=1;").unwrap();
    drop(connection);
    assert!(SqliteStore::open(&broken).is_err());
    let connection = rusqlite::Connection::open(&broken).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT payload FROM cards WHERE id='keep'", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "opaque-data"
    );
    assert!(
        !connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='sessions')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    connection.pragma_update(None, "user_version", 6).unwrap();
    drop(connection);
    assert_eq!(
        SqliteStore::open(&broken).err().unwrap().code,
        "future_schema"
    );
}

#[test]
fn base64_card_archives_remain_compact_and_accept_legacy_stored_bytes() {
    let mut raw = vec![b' '; 1024 * 1024];
    raw.extend_from_slice(include_bytes!("fixtures/synthetic/card-v3.json"));
    let original = Card::import(&raw).unwrap();
    let store = SqliteStore::memory().unwrap();
    let session = session(&store, &original, &persona());
    let bytes = store
        .export_session(&session.id, false)
        .unwrap()
        .encode()
        .unwrap();
    assert!(bytes.len() < raw.len() * 2);
    let decoded = SessionExport::decode(&bytes).unwrap();
    assert_eq!(decoded.characters[0].original, raw);
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy["original"] = json!(raw);
    assert_eq!(
        serde_json::from_value::<Card>(legacy).unwrap().original,
        original.original
    );
}
