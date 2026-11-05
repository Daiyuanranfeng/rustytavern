use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    app::{CommandService, PluginDefinition, PluginManager, default_registry},
    plugin::{PluginContext, PluginManifest, RuntimePlugin},
    plugins::{self, Environment},
    source::CardSource,
    store::{SqliteStore, StoreBackend},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    slice::from_ref,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

/// The function writes one plugin directory with `plugin.json`.
fn plugin(parent: &Path, definition: Value) -> PathBuf {
    let root = parent.join(definition["id"].as_str().unwrap());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("plugin.json"),
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    root
}
async fn manager(data: &Path, store: Arc<dyn StoreBackend>) -> PluginManager {
    PluginManager::open(data, store.clone(), default_registry(store).await.unwrap()).unwrap()
}
fn definition(id: &str, provides: &[&str], requires: &[&str], fail: bool) -> Value {
    json!({"id":id,"version":"0.1.0","host_protocol":"1","provides":provides,"requires":requires,"implementation":{"kind":"builtin","name":"fixture","configuration":{"fail":fail}}})
}

struct FixturePlugin {
    definition: PluginDefinition,
    cleaned: Arc<AtomicUsize>,
}
#[async_trait]
impl RuntimePlugin for FixturePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.definition.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let cleaned = self.cleaned.clone();
        context.own(move || {
            cleaned.fetch_add(1, Ordering::SeqCst);
        });
        for name in &self.definition.manifest.requires {
            assert_eq!(*context.dependency::<usize>(name)?, 7);
        }
        for name in &self.definition.manifest.provides {
            context.register(name, Arc::new(7usize))?;
        }
        if serde_json::to_value(&self.definition.implementation)?["configuration"]["fail"] == true {
            return Err(Error::new(
                "plugin_activation",
                "The fixture activation fails.",
            ));
        }
        Ok(())
    }
}
fn fixture_factory(manager: &mut PluginManager, cleaned: Arc<AtomicUsize>) {
    manager
        .register_factory(
            "fixture".into(),
            Arc::new(move |definition| {
                Ok(Arc::new(FixturePlugin {
                    definition: definition.clone(),
                    cleaned: cleaned.clone(),
                }))
            }),
        )
        .unwrap();
}

#[tokio::test]
async fn an_installed_plugin_survives_reopen_and_rejects_changed_files() {
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(data.path().join("state.sqlite3")).unwrap());
    let mut app = manager(data.path(), store.clone()).await;
    let installed = app
        .install(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/plugins/lighthouse-notes"
        )))
        .unwrap();
    let id = installed.definition.manifest.id.clone();
    assert!(!installed.enabled);
    assert_eq!(
        app.enable(from_ref(&id), BTreeSet::new(), false)
            .await
            .unwrap_err()
            .code,
        "capability_denied"
    );
    let grants = BTreeSet::from([
        "prompt.contribute:before_character".into(),
        "prompt.contribute:after_character".into(),
    ]);
    app.enable(from_ref(&id), grants, false).await.unwrap();
    assert_eq!(app.registry.lock().await.inspect().len(), 11);
    drop(app);
    let mut app = manager(data.path(), store.clone()).await;
    app.restore().await.unwrap();
    assert!(app.inspect(&id).unwrap().enabled);
    std::fs::write(
        data.path()
            .join("plugins")
            .join(&installed.directory)
            .join("README.md"),
        "Changed.",
    )
    .unwrap();
    let mut reopened = manager(data.path(), store).await;
    assert_eq!(
        reopened.restore().await.unwrap_err().code,
        "invalid_plugin_hash"
    );
}

#[tokio::test]
async fn failed_activation_and_missing_command_services_roll_back() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let cleaned = Arc::new(AtomicUsize::new(0));
    fixture_factory(&mut app, cleaned.clone());
    let original = app.registry.lock().await.freeze().unwrap().snapshot();
    for (id, fail, expected, count) in [
        ("example.ok", false, "", 1),
        ("example.failed", true, "plugin_activation", 2),
        ("example.command", false, "missing_service", 3),
    ] {
        let mut value = definition(id, &["value/1"], &[], fail);
        if id == "example.command" {
            value["commands"] =
                json!([{"name":"remember","service":"absent/1","operation":"remember"}]);
        }
        app.install(&plugin(source.path(), value)).unwrap();
        let result = app.enable(&[id.into()], BTreeSet::new(), false).await;
        if expected.is_empty() {
            result.unwrap();
            app.disable(id).await.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, expected);
        }
        assert!(!app.inspect(id).unwrap().enabled);
        assert!(app.commands().unwrap().is_empty());
        assert_eq!(
            app.registry.lock().await.freeze().unwrap().snapshot(),
            original
        );
        assert_eq!(cleaned.load(Ordering::SeqCst), count);
    }
}

#[tokio::test]
async fn catalog_revision_conflicts_remove_staged_plugins() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(data.path().join("state.sqlite3")).unwrap());
    let mut first = manager(data.path(), store.clone()).await;
    first
        .install(&plugin(
            source.path(),
            definition("example.catalog", &["value/1"], &[], false),
        ))
        .unwrap();
    let mut stale = manager(data.path(), store.clone()).await;
    let cleaned = Arc::new(AtomicUsize::new(0));
    fixture_factory(&mut stale, cleaned.clone());
    first
        .install(&plugin(
            source.path(),
            definition("example.unrelated", &["other/1"], &[], false),
        ))
        .unwrap();
    let error = stale
        .enable(&["example.catalog".into()], BTreeSet::new(), false)
        .await
        .unwrap_err();
    assert_eq!(error.code, "configuration_conflict");
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    assert_eq!(stale.registry.lock().await.inspect().len(), 10);
    let current = manager(data.path(), store).await;
    assert!(!current.inspect("example.catalog").unwrap().enabled);
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_symbolic_links_cannot_escape_the_source_directory() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let root = plugin(
        source.path(),
        definition("example.symlink", &["value/1"], &[], false),
    );
    let outside = source.path().join("outside.txt");
    std::fs::write(&outside, "Outside.").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape.txt")).unwrap();
    assert_eq!(app.install(&root).unwrap_err().code, "invalid_plugin_path");
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "Outside.");
}

#[tokio::test]
async fn essential_plugins_form_the_standard_agent() {
    let store: Arc<dyn StoreBackend> = Arc::new(SqliteStore::memory().unwrap());
    let environment = Environment::new(store);
    let registry = plugins::registry(&environment, &BTreeSet::new())
        .await
        .unwrap();
    let lease = registry.lock().await.freeze().unwrap();
    assert_eq!(
        lease.service_owner("driver/1").unwrap().manifest.id,
        "org.rustytavern.driver.agent"
    );
    assert!(lease.contains_service("tools.web/1") && lease.contains_service("tools.skills/1"));
    let source = lease
        .service::<Arc<dyn CardSource>>("card.source.chub/1")
        .unwrap();
    assert!(source.recognize("https://chub.ai/author/character"));
    let disabled = BTreeSet::from(["org.rustytavern.tools.web".into()]);
    let registry = plugins::registry(&environment, &disabled).await.unwrap();
    assert!(
        !registry
            .lock()
            .await
            .freeze()
            .unwrap()
            .contains_service("tools.web/1")
    );
}

struct Echo;
#[async_trait]
impl CommandService for Echo {
    async fn call(
        &self,
        operation: &str,
        arguments: &Value,
        _: CancellationToken,
    ) -> Result<Value> {
        if operation != "echo" {
            return Err(Error::new(
                "unsupported_plugin_command",
                "The command operation is unknown.",
            ));
        }
        Ok(json!({"echo":arguments}))
    }
}
struct EchoPlugin {
    manifest: PluginManifest,
}
#[async_trait]
impl RuntimePlugin for EchoPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let command: Arc<dyn CommandService> = Arc::new(Echo);
        context.register("command.echo/1", Arc::new(command))
    }
}
#[tokio::test]
async fn plugin_commands_call_typed_native_services() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let mut value = definition("example.echo", &["command.echo/1"], &[], false);
    value["commands"] = json!([{"name":"say","service":"command.echo/1","operation":"echo"}]);
    app.register_factory(
        "fixture".into(),
        Arc::new(|definition| {
            Ok(Arc::new(EchoPlugin {
                manifest: definition.manifest.clone(),
            }))
        }),
    )
    .unwrap();
    app.install(&plugin(source.path(), value)).unwrap();
    app.enable(&["example.echo".into()], BTreeSet::new(), false)
        .await
        .unwrap();
    assert_eq!(
        app.run_command("say", json!({"text":"Hello."}), CancellationToken::new())
            .await
            .unwrap(),
        json!({"echo":{"text":"Hello."}})
    );
    app.disable("example.echo").await.unwrap();
    assert_eq!(
        app.run_command("say", json!({}), CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        "invalid_plugin_command"
    );
}
