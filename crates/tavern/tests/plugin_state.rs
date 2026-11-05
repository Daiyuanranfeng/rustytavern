use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    card::Card,
    plugin::{PluginContext, PluginManifest, PluginRegistry, RuntimePlugin},
    prompt::{PromptOptions, PromptPlan},
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderKind, ProviderStream,
    },
    runtime::Runtime,
    session::{GenerationStatus, Persona, Session, SessionExport},
    store::{SqliteStore, StoreBackend},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

const ID: &str = "example.state-counter";
type StateAction = dyn Fn(&mut PluginContext) -> Result<()> + Send + Sync;

struct StatePlugin {
    manifest: PluginManifest,
    action: Box<StateAction>,
}
fn plugin(
    schema: u32,
    action: impl Fn(&mut PluginContext) -> Result<()> + Send + Sync + 'static,
) -> Arc<dyn RuntimePlugin> {
    Arc::new(StatePlugin {
        manifest: PluginManifest {
            id: ID.into(),
            version: format!("{schema}.0.0"),
            state_schema: schema,
            requested_capabilities: BTreeSet::from([
                format!("state.read:{ID}"),
                format!("state.write:{ID}"),
            ]),
            ..Default::default()
        },
        action: Box::new(action),
    })
}
fn grants() -> BTreeMap<String, BTreeSet<String>> {
    BTreeMap::from([(
        ID.into(),
        BTreeSet::from([format!("state.read:{ID}"), format!("state.write:{ID}")]),
    )])
}
#[async_trait]
impl RuntimePlugin for StatePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        (self.action)(context)
    }
}
struct LocalProvider;
#[async_trait]
impl ProviderAdapter for LocalProvider {
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        HttpProvider.prepare(plan, model, config)
    }
    async fn open_stream(
        &self,
        _: PreparedRequest,
        _: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        Ok(Box::pin(futures_util::stream::iter([
            Ok(ProviderEvent::Text {
                text: "A local reply.".into(),
            }),
            Ok(ProviderEvent::Complete),
        ])))
    }
}
fn session(store: &dyn StoreBackend) -> Session {
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let persona = Persona::new("Alex".into(), "Alex charts the coast.".into()).unwrap();
    store.save_card(&card).unwrap();
    store.save_persona(&persona).unwrap();
    store
        .create_session(
            &Session {
                id: String::new(),
                character_id: card.id,
                persona_id: persona.id,
                worldbook_ids: Vec::new(),
                provider: "local".into(),
                model: "fixture".into(),
                head: None,
                version: 0,
                accept_reduced: false,
                options: PromptOptions::default(),
                composition: json!({"plugins":[{"manifest":{"id":ID}}]}),
            },
            None,
        )
        .unwrap()
}

#[tokio::test]
async fn native_upgrade_retains_history_and_pins_the_schema_in_a_real_generation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let mut registry = PluginRegistry::default();
    let seed = store.clone();
    let opaque = json!({"future":{"items":[1,"港",null]}});
    let preserved = opaque.clone();
    registry
        .inject(
            vec![plugin(1, move |context| {
                context.state_write(seed.as_ref(), ID, "counter", &json!(7))?;
                context.state_write(seed.as_ref(), ID, "opaque", &preserved)?;
                Ok(())
            })],
            "seed",
            &grants(),
        )
        .await
        .unwrap();
    let before = store.plugin_state_snapshot(ID).unwrap();
    assert_eq!(before.schema, 1);
    registry.eject_owner("seed").await.unwrap();
    let upgraded = store.clone();
    registry
        .inject(
            vec![plugin(2, move |context| {
                context.state_migrate(upgraded.as_ref(), |old| {
                    assert_eq!(old.schema, 1);
                    Ok(BTreeMap::from([(
                        "counter".into(),
                        json!({"count":old.entries["counter"]["value"]}),
                    )]))
                })?;
                context.state_migrate(upgraded.as_ref(), |_| {
                    panic!("The host must skip a completed migration.")
                })?;
                assert_eq!(
                    context.state_read(upgraded.as_ref(), "counter")?,
                    Some(json!({"count":7}))
                );
                assert_eq!(
                    context
                        .state_write(upgraded.as_ref(), "another", "key", &json!(1))
                        .unwrap_err()
                        .code,
                    "capability_denied"
                );
                Ok(())
            })],
            "upgrade",
            &grants(),
        )
        .await
        .unwrap();
    let after = store.plugin_state_snapshot(ID).unwrap();
    assert_eq!(after.schema, 2);
    assert_eq!(after.entries["counter"]["revision"], 2);
    assert_eq!(after.entries["opaque"], before.entries["opaque"]);
    assert_eq!(
        store
            .plugin_state_write(ID, 1, "counter", &json!(99))
            .unwrap_err()
            .code,
        "unsupported_plugin_state_schema"
    );
    assert_eq!(
        store
            .plugin_state_write(ID, 2, "counter", &json!({"count":8}))
            .unwrap(),
        3
    );
    assert_eq!(
        SqliteStore::open(&path)
            .unwrap()
            .plugin_state_snapshot(ID)
            .unwrap(),
        store.plugin_state_snapshot(ID).unwrap()
    );
    let session = session(store.as_ref());
    let registry = Arc::new(tokio::sync::Mutex::new(registry));
    let runtime = Runtime::new(store.clone())
        .with_provider(Arc::new(LocalProvider))
        .with_plugins(registry.clone());
    let config = ProviderConfig {
        kind: ProviderKind::Custom,
        endpoint: Some("http://127.0.0.1:1/unused".into()),
        api_key_env: None,
        ..Default::default()
    };
    runtime
        .configure_session(&session, config.clone())
        .await
        .unwrap();
    let mut handle = runtime
        .start(&session.id, "Hello.", None, config)
        .await
        .unwrap();
    let generation = handle.wait().await.unwrap();
    assert_eq!(generation.status, GenerationStatus::Completed);
    assert_eq!(
        generation.snapshot["plugin_state"][ID]["counter"]["value"],
        json!({"count":8})
    );
    assert_eq!(
        generation.snapshot["plugin_composition"]["plugins"][0]["manifest"]["state_schema"],
        2
    );
    let exported = store.export_session(&session.id, true).unwrap();
    let decoded = SessionExport::decode(&exported.encode().unwrap()).unwrap();
    assert!(
        decoded
            .plugin_state
            .iter()
            .any(|r| r.namespace == ID && r.key == "counter" && r.revision == 1 && r.payload == 7)
    );
    assert!(
        decoded
            .plugin_state
            .iter()
            .any(|r| r.namespace == ID && r.key == "opaque" && r.payload == opaque)
    );
    assert!(
        decoded
            .plugin_state
            .iter()
            .any(|r| r.namespace == format!("@state-schema/{ID}") && r.payload == 2)
    );
    let old = store.plugin_state_snapshot(ID).unwrap();
    store
        .plugin_state_migrate(
            ID,
            &old,
            3,
            &BTreeMap::from([("counter".into(), json!({"count":9}))]),
        )
        .unwrap();
    let replay = runtime.replay(&generation.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        generation.request.unwrap()
    );
    assert_eq!(
        runtime
            .inspect(&session.id, "Next.")
            .await
            .unwrap_err()
            .code,
        "unsupported_plugin_state_schema"
    );
    assert_eq!(
        store.history(&session.id).unwrap().last().unwrap().text,
        "A local reply."
    );
}

#[tokio::test]
async fn callback_failure_panic_and_denied_grants_leave_the_namespace_unchanged() {
    let store = Arc::new(SqliteStore::memory().unwrap());
    store.state_write(ID, "counter", &json!(7)).unwrap();
    let before = store.plugin_state_snapshot(ID).unwrap();
    for panic in [false, true] {
        let saved = store.clone();
        let mut registry = PluginRegistry::default();
        let result = registry
            .inject(
                vec![plugin(2, move |context| {
                    context.state_migrate(saved.as_ref(), |_| {
                        if panic {
                            panic!("The migration fixture panics.");
                        }
                        Err(Error::new(
                            "migration_fixture",
                            "The migration fixture fails.",
                        ))
                    })?;
                    Ok(())
                })],
                "fixture",
                &grants(),
            )
            .await;
        assert_eq!(
            result.unwrap_err().code,
            if panic {
                "plugin_state_migration"
            } else {
                "migration_fixture"
            }
        );
        assert!(registry.inspect().is_empty());
        assert_eq!(store.plugin_state_snapshot(ID).unwrap(), before);
    }
    for allowed in [
        BTreeSet::new(),
        BTreeSet::from([format!("state.read:{ID}")]),
        BTreeSet::from([format!("state.write:{ID}")]),
    ] {
        let saved = store.clone();
        let mut registry = PluginRegistry::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let invoked = calls.clone();
        let result = registry
            .inject(
                vec![plugin(2, move |context| {
                    context.state_migrate(saved.as_ref(), |_| {
                        invoked.fetch_add(1, Ordering::SeqCst);
                        Ok(BTreeMap::new())
                    })?;
                    Ok(())
                })],
                "fixture",
                &BTreeMap::from([(ID.into(), allowed)]),
            )
            .await;
        assert_eq!(result.unwrap_err().code, "capability_denied");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.plugin_state_snapshot(ID).unwrap(), before);
    }
    let mut registry = PluginRegistry::default();
    assert_eq!(
        registry
            .inject(
                vec![plugin(0, |_| panic!("The host must reject schema zero."))],
                "fixture",
                &grants()
            )
            .await
            .unwrap_err()
            .code,
        "unsupported_plugin_state_schema"
    );
    assert_eq!(store.plugin_state_snapshot(ID).unwrap(), before);
}

#[test]
fn sql_failure_rolls_back_all_values_and_the_schema_marker() {
    for failed_namespace in [ID, "@state-schema/example.state-counter"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let store = SqliteStore::open(&path).unwrap();
        store.state_write(ID, "counter", &json!(7)).unwrap();
        let before = store.plugin_state_snapshot(ID).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(&format!("CREATE TRIGGER fail_state BEFORE INSERT ON plugin_state WHEN NEW.namespace='{failed_namespace}' AND NEW.key='{}' BEGIN SELECT RAISE(ABORT,'The migration fixture fails.'); END;",if failed_namespace==ID {"second"}else{"schema"})).unwrap();
        let result = store.plugin_state_migrate(
            ID,
            &before,
            2,
            &BTreeMap::from([
                ("counter".into(), json!({"count":7})),
                ("second".into(), json!(true)),
            ]),
        );
        assert_eq!(result.unwrap_err().code, "storage");
        assert_eq!(store.plugin_state_snapshot(ID).unwrap(), before);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM plugin_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[tokio::test]
async fn a_concurrent_writer_prevents_a_stale_migration_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    store.state_write(ID, "counter", &json!(7)).unwrap();
    let other = SqliteStore::open(&path).unwrap();
    let saved = store.clone();
    let mut registry = PluginRegistry::default();
    let result = registry
        .inject(
            vec![plugin(2, move |context| {
                context.state_migrate(saved.as_ref(), |_| {
                    other.plugin_state_write(ID, 1, "counter", &json!(8))?;
                    Ok(BTreeMap::from([("counter".into(), json!({"count":7}))]))
                })?;
                Ok(())
            })],
            "fixture",
            &grants(),
        )
        .await;
    assert_eq!(result.unwrap_err().code, "configuration_conflict");
    let current = store.plugin_state_snapshot(ID).unwrap();
    assert_eq!(current.schema, 1);
    assert_eq!(current.entries["counter"], json!({"revision":2,"value":8}));
    assert_eq!(
        store
            .state_read(&format!("@state-schema/{ID}"), "schema")
            .unwrap(),
        None
    );
}

#[test]
fn competing_migrations_commit_one_consistent_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    store.state_write(ID, "counter", &json!(7)).unwrap();
    let snapshot = store.plugin_state_snapshot(ID).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let tasks: Vec<_> = [2, 3]
        .into_iter()
        .map(|schema| {
            let store = SqliteStore::open(&path).unwrap();
            let snapshot = snapshot.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.plugin_state_migrate(
                    ID,
                    &snapshot,
                    schema,
                    &BTreeMap::from([("counter".into(), json!({"schema":schema}))]),
                )
            })
        })
        .collect();
    let results: Vec<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results.iter().find_map(|r| r.as_ref().err()).unwrap().code,
        "configuration_conflict"
    );
    let current = store.plugin_state_snapshot(ID).unwrap();
    assert_eq!(
        current.entries["counter"]["value"]["schema"],
        current.schema
    );
    assert_eq!(current.entries["counter"]["revision"], 2);
}

#[test]
fn future_schema_markers_block_writes_and_survive_export() {
    for marker in [json!(7), json!({"future":{"schema":7}}), json!(0)] {
        let store = SqliteStore::memory().unwrap();
        let opaque = json!({"unknown":["保留",false,null]});
        store.state_write(ID, "opaque", &opaque).unwrap();
        store
            .state_write(&format!("@state-schema/{ID}"), "schema", &marker)
            .unwrap();
        assert_eq!(
            store
                .plugin_state_write(ID, 1, "opaque", &json!(null))
                .unwrap_err()
                .code,
            "unsupported_plugin_state_schema"
        );
        assert_eq!(
            store.state_read(ID, "opaque").unwrap(),
            Some(opaque.clone())
        );
        let session = session(&store);
        let exported = store.export_session(&session.id, false).unwrap();
        let decoded = SessionExport::decode(&exported.encode().unwrap()).unwrap();
        assert!(
            decoded
                .plugin_state
                .iter()
                .any(|r| r.namespace == format!("@state-schema/{ID}") && r.payload == marker)
        );
        assert!(
            decoded
                .plugin_state
                .iter()
                .any(|r| r.namespace == ID && r.payload == opaque)
        );
    }
}

#[tokio::test]
async fn extension_upgrade_restores_its_native_migration_factory() {
    use rustytavern::app::{ExtensionManager, NativeFactory, PluginDefinition};
    fn factory(store: Arc<SqliteStore>) -> NativeFactory {
        Arc::new(move |definition: &PluginDefinition| {
            let saved = store.clone();
            let schema = definition.manifest.state_schema;
            Ok(Arc::new(StatePlugin {
                manifest: definition.manifest.clone(),
                action: Box::new(move |context| {
                    if schema == 2 {
                        context.state_migrate(saved.as_ref(), |before| {
                            Ok(BTreeMap::from([(
                                "counter".into(),
                                json!({"count":before.entries["counter"]["value"]}),
                            )]))
                        })?;
                    } else {
                        assert_eq!(
                            context.state_read(saved.as_ref(), "counter")?,
                            Some(json!(7))
                        );
                    }
                    Ok(())
                }),
            }) as Arc<dyn RuntimePlugin>)
        })
    }
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let path = data.path().join("sessions.sqlite3");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    store.state_write(ID, "counter", &json!(7)).unwrap();
    let registry = Arc::new(tokio::sync::Mutex::new(PluginRegistry::default()));
    let mut manager = ExtensionManager::open(data.path(), store.clone(), registry).unwrap();
    manager
        .register_factory("fixture.state".into(), factory(store.clone()))
        .unwrap();
    for schema in [1, 2] {
        std::fs::write(source.path().join("extension.json"),serde_json::to_vec(&json!({"id":"example.state-package","version":format!("{schema}.0.0"),"product_api":"1","plugins":[{"manifest":"plugin.json"}]})).unwrap()).unwrap();
        std::fs::write(
            source.path().join("README.md"),
            "The fixture tests native state upgrades.\n",
        )
        .unwrap();
        std::fs::write(source.path().join("plugin.json"),serde_json::to_vec(&json!({"id":ID,"version":format!("{schema}.0.0"),"host_protocol":"1","state_schema":schema,"requested_capabilities":[format!("state.read:{ID}"),format!("state.write:{ID}")],"implementation":{"kind":"builtin","name":"fixture.state","configuration":{}}})).unwrap()).unwrap();
        manager.install(source.path()).unwrap();
        manager
            .enable(
                &["example.state-package".into()],
                grants().remove(ID).unwrap(),
                false,
            )
            .await
            .unwrap();
        assert_eq!(store.plugin_state_snapshot(ID).unwrap().schema, schema);
        if schema == 1 {
            manager.disable("example.state-package").await.unwrap();
        }
    }
    drop(manager);
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let before = store.plugin_state_snapshot(ID).unwrap();
    let mut restored = ExtensionManager::open(
        data.path(),
        store.clone(),
        Arc::new(tokio::sync::Mutex::new(PluginRegistry::default())),
    )
    .unwrap();
    restored
        .register_factory("fixture.state".into(), factory(store.clone()))
        .unwrap();
    restored.restore().await.unwrap();
    assert_eq!(store.plugin_state_snapshot(ID).unwrap(), before);
    assert_eq!(before.entries["counter"]["value"], json!({"count":7}));
    restored.disable("example.state-package").await.unwrap();
    restored.uninstall("example.state-package").unwrap();
    assert_eq!(store.plugin_state_snapshot(ID).unwrap(), before);
}
