use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rustytavern::{
    Result,
    card::{Card, WorldBook},
    prompt::PromptOptions,
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderStream,
    },
    runtime::Runtime,
    session::{GenerationStatus, Persona, Session, SessionExport},
    store::{SqliteStore, StoreBackend},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

struct Capture(Arc<Mutex<Vec<Value>>>);
#[async_trait]
impl ProviderAdapter for Capture {
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
        _: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        self.0.lock().unwrap().push(request.body);
        Ok(Box::pin(futures_util::stream::iter([Ok(
            ProviderEvent::Complete,
        )])))
    }
}
fn book(content: &str) -> WorldBook {
    WorldBook::import(
        &serde_json::to_vec_pretty(&json!({
            "spec":"lorebook_v3", "data":{"name":"Harbor", "entries":[{
                "id":"place", "keys":["harbor"], "content":content, "enabled":true,
                "position":"after_char", "insertion_order":1
            }], "extensions":{"org.example.keep":{"value":42}}}
        }))
        .unwrap(),
    )
    .unwrap()
}
fn target(store: &dyn StoreBackend, ids: Vec<String>) -> Session {
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let persona = Persona::new("Alex".into(), "A visitor.".into()).unwrap();
    store.save_card(&card).unwrap();
    store.save_persona(&persona).unwrap();
    Session {
        id: String::new(),
        character_id: card.id,
        persona_id: persona.id,
        worldbook_ids: ids,
        provider: "fixture".into(),
        model: "fixture".into(),
        head: None,
        version: 0,
        accept_reduced: false,
        options: PromptOptions::default(),
        composition: json!({}),
    }
}
#[test]
fn archives_retain_original_bytes_and_reject_forged_records() {
    let original = book("{{char}} knows the harbor.");
    let bytes = serde_json::to_vec(&original).unwrap();
    let decoded: WorldBook = serde_json::from_slice(&bytes).unwrap();
    decoded.validate().unwrap();
    assert_eq!(original.export_original(), decoded.export_original());
    assert_eq!(original.document(), decoded.document());
    for field in ["id", "document"] {
        let mut forged = serde_json::to_value(&original).unwrap();
        forged[field] = json!("changed");
        let forged: WorldBook = serde_json::from_value(forged).unwrap();
        assert_eq!(
            forged.validate().unwrap_err().code,
            "invalid_worldbook_archive"
        );
    }
    assert!(WorldBook::import(br#"{"entries":[{"keys":[],"content":false}]}"#).is_err());
}
#[tokio::test]
async fn revisions_pin_books_in_requests_replay_and_exports_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite3");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let old = book("OLD_WORLD_SENTINEL {{char}} meets {{user}}.");
    let new = book("NEW_WORLD_SENTINEL");
    store.save_worldbook(&old).unwrap();
    store.save_worldbook(&old).unwrap();
    assert_eq!(store.list("worldbook").unwrap().len(), 1);
    let capture = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::new(store.clone()).with_provider(Arc::new(Capture(capture.clone())));
    let session = runtime
        .create_session(
            &target(store.as_ref(), vec![old.id().into()]),
            None,
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    let inspected = runtime
        .inspect_request(&session.id, "The harbor.", ProviderConfig::default())
        .await
        .unwrap();
    assert!(
        inspected
            .plan
            .sections
            .iter()
            .any(|s| s.text == "OLD_WORLD_SENTINEL Lumen meets Alex.")
    );
    let first = runtime
        .start(&session.id, "The harbor.", None, ProviderConfig::default())
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(first.status, GenerationStatus::Completed);
    assert_eq!(capture.lock().unwrap()[0], inspected.prepared.body);
    let replay = runtime.replay(&first.id).await.unwrap();
    assert_eq!(replay.prepared.body, capture.lock().unwrap()[0]);
    store.save_worldbook(&new).unwrap();
    let current = store.session(&session.id).unwrap();
    let mut configuration = current.configuration().unwrap();
    configuration.worldbook_ids = vec![new.id().into()];
    store
        .configure_session(&session.id, current.version, configuration)
        .unwrap();
    let next = runtime.inspect(&session.id, "The harbor.").await.unwrap();
    assert!(next.sections.iter().any(|s| s.text == "NEW_WORLD_SENTINEL"));
    assert!(
        !next
            .sections
            .iter()
            .any(|s| s.text.contains("OLD_WORLD_SENTINEL"))
    );
    assert_eq!(
        runtime.replay(&first.id).await.unwrap().prepared.body,
        capture.lock().unwrap()[0]
    );
    let export = store.export_session(&session.id, true).unwrap();
    assert_eq!(export.schema_version, 4);
    assert_eq!(export.worldbooks.len(), 2);
    let bytes = export.encode().unwrap();
    SessionExport::decode(&bytes).unwrap();
    for corruption in ["missing", "document", "snapshot", "schema"] {
        let mut invalid: Value = serde_json::from_slice(&bytes).unwrap();
        match corruption {
            "missing" => invalid["worldbooks"] = json!([]),
            "document" => invalid["worldbooks"][0]["document"] = json!({"entries":[]}),
            "snapshot" => {
                invalid["generations"][0]["snapshot"]["worldbooks"][0]["document"] =
                    json!({"entries":[]})
            }
            _ => invalid["schema_version"] = json!(3),
        }
        assert_eq!(
            SessionExport::decode(&serde_json::to_vec(&invalid).unwrap())
                .unwrap_err()
                .code,
            "invalid_session_export"
        );
    }
    drop(runtime);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&path).unwrap());
    assert_eq!(
        reopened.worldbook(old.id()).unwrap().export_original(),
        old.export_original()
    );
    let runtime = Runtime::new(reopened.clone()).with_provider(Arc::new(Capture(capture.clone())));
    assert_eq!(
        runtime.replay(&first.id).await.unwrap().prepared.body,
        capture.lock().unwrap()[0]
    );
    assert_eq!(
        reopened.session(&session.id).unwrap().worldbook_ids,
        vec![new.id()]
    );
}
#[tokio::test]
async fn invalid_selections_leave_the_session_and_generation_list_unchanged() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let selected = book("The harbor.");
    store.save_worldbook(&selected).unwrap();
    let runtime = Runtime::new(store.clone());
    let session = runtime
        .create_session(
            &target(store.as_ref(), vec![]),
            None,
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    let original = serde_json::to_value(&session).unwrap();
    for ids in [
        vec![selected.id().into(); 2],
        vec![selected.id().into(); 17],
        vec![format!("worldbook-{}", "0".repeat(64))],
    ] {
        let mut configuration = session.configuration().unwrap();
        configuration.worldbook_ids = ids;
        assert!(
            store
                .configure_session(&session.id, session.version, configuration)
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(store.session(&session.id).unwrap()).unwrap(),
            original
        );
    }
    let mut configuration = session.configuration().unwrap();
    configuration.worldbook_ids = vec![selected.id().into()];
    assert_eq!(
        store
            .configure_session(&session.id, session.version + 1, configuration)
            .unwrap_err()
            .code,
        "session_conflict"
    );
    assert!(store.generations(&session.id).unwrap().is_empty());
    let mut ids = Vec::new();
    for index in 0..17 {
        let archive = book(&format!("Book {index}"));
        store.save_worldbook(&archive).unwrap();
        ids.push(archive.id().to_owned());
    }
    assert_eq!(store.worldbooks(&ids[..16]).unwrap().len(), 16);
    assert_eq!(
        store.worldbooks(&ids).unwrap_err().code,
        "invalid_worldbook_selection"
    );
    let current = store.session(&session.id).unwrap();
    store
        .prepare(rustytavern::store::Submit {
            session_id: &session.id,
            expected_version: current.version,
            text: "The harbor.",
            idempotency_key: None,
            retry_user: None,
            snapshot: json!({}),
            now: rustytavern::store::now_ms(),
            lease_ms: 30_000,
        })
        .unwrap();
    assert_eq!(
        store
            .configure_session(
                &session.id,
                current.version,
                current.configuration().unwrap()
            )
            .unwrap_err()
            .code,
        "session_conflict"
    );
}
#[tokio::test]
async fn unsupported_rules_require_explicit_acceptance_and_native_books_remain_archived() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let recursive = WorldBook::import(br#"{"entries":[],"recursive_scanning":true}"#).unwrap();
    let native = WorldBook::import(br#"{"name":"Native","entries":{"0":{"uid":0,"content":"KEEP","constant":true,"future":42}}}"#).unwrap();
    for book in [&recursive, &native] {
        store.save_worldbook(book).unwrap();
    }
    let runtime = Runtime::new(store.clone());
    let mut session = runtime
        .create_session(
            &target(store.as_ref(), vec![recursive.id().into()]),
            None,
            ProviderConfig::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .inspect(&session.id, "Next.")
            .await
            .unwrap_err()
            .code,
        "unsupported_behavior"
    );
    session.accept_reduced = true;
    let session = runtime
        .configure_session(&session, ProviderConfig::default())
        .await
        .unwrap();
    assert!(
        runtime
            .inspect(&session.id, "Next.")
            .await
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.contains("recursive_scanning"))
    );
    let mut configuration = session.configuration().unwrap();
    configuration.worldbook_ids = vec![native.id().into()];
    store
        .configure_session(&session.id, session.version, configuration)
        .unwrap();
    assert_eq!(
        runtime
            .inspect(&session.id, "Next.")
            .await
            .unwrap_err()
            .code,
        "unsupported_worldbook_profile"
    );
    assert_eq!(
        store.worldbook(native.id()).unwrap().export_original(),
        native.export_original()
    );
    assert!(store.generations(&session.id).unwrap().is_empty());
}
#[test]
fn version_four_database_migration_retains_cards_and_adds_worldbooks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let session = target(&store, vec![]);
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE worldbooks; PRAGMA user_version=4;")
        .unwrap();
    drop(connection);
    let store = SqliteStore::open(&path).unwrap();
    store.card(&session.character_id).unwrap();
    let book = book("The harbor.");
    store.save_worldbook(&book).unwrap();
    assert_eq!(store.worldbook(book.id()).unwrap().id(), book.id());
    drop(store);
    let connection = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn selected_archives_have_a_total_byte_limit() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    let mut ids = Vec::new();
    for index in 0..9 {
        let book = book(&format!("{index}{}", "x".repeat(950_000)));
        store.save_worldbook(&book).unwrap();
        ids.push(book.id().into());
    }
    assert_eq!(
        store.worldbooks(&ids).unwrap_err().code,
        "worldbook_selection_limit"
    );
    let runtime = Runtime::new(store.clone());
    assert_eq!(
        runtime
            .create_session(
                &target(store.as_ref(), ids),
                None,
                ProviderConfig::default()
            )
            .await
            .unwrap_err()
            .code,
        "worldbook_selection_limit"
    );
    assert!(store.list("session").unwrap().is_empty());
}
