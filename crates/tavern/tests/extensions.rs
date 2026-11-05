use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    app::{CommandService, ExtensionManager, PluginDefinition, default_registry},
    plugin::{PluginContext, PluginManifest, RuntimePlugin},
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

fn package(parent: &Path, id: &str, plugins: Vec<Value>) -> PathBuf {
    let root = parent.join(id);
    std::fs::create_dir_all(root.join("plugins")).unwrap();
    let entries = plugins
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let path = format!("plugins/{i}.json");
            std::fs::write(root.join(&path), serde_json::to_vec(p).unwrap()).unwrap();
            json!({"manifest":path})
        })
        .collect::<Vec<_>>();
    std::fs::write(
        root.join("extension.json"),
        serde_json::to_vec(&json!({"id":id,"version":"0.1.0","product_api":"1","plugins":entries}))
            .unwrap(),
    )
    .unwrap();
    root
}
fn edit_manifest(root: &Path, update: impl FnOnce(&mut Value)) {
    let path = root.join("extension.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    update(&mut value);
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}
async fn manager(data: &Path, store: Arc<dyn StoreBackend>) -> ExtensionManager {
    ExtensionManager::open(data, store.clone(), default_registry(store).await.unwrap()).unwrap()
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
fn fixture_factory(manager: &mut ExtensionManager, cleaned: Arc<AtomicUsize>) {
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
async fn an_asset_only_extension_survives_reopen_and_keeps_explicit_configuration() {
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(data.path().join("state.sqlite3")).unwrap());
    let mut app = manager(data.path(), store.clone()).await;
    let package = app
        .install(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tavern/examples/extensions/harbor-assets"
        )))
        .unwrap();
    assert!(!package.enabled);
    app.enable(from_ref(&package.manifest.id), BTreeSet::new(), false)
        .await
        .unwrap();
    assert!(
        String::from_utf8(
            app.asset(&package.manifest.id, "templates/harbor.txt")
                .unwrap()
        )
        .unwrap()
        .contains("lighthouse")
    );
    let explicit: rustytavern::config::Configuration =
        serde_json::from_value(json!({"example":{"location":"city"},"custom":false})).unwrap();
    assert_eq!(
        app.configuration(&serde_json::to_value(explicit).unwrap())
            .unwrap(),
        json!({"schema_version":1,"providers":{},"example":{"location":"city"},"custom":false})
    );
    let before = app.registry.lock().await.inspect().len();
    assert_eq!(before, 5);
    drop(app);
    drop(store);
    let store = Arc::new(SqliteStore::open(data.path().join("state.sqlite3")).unwrap());
    let mut app = manager(data.path(), store).await;
    app.restore().await.unwrap();
    assert!(app.inspect(&package.manifest.id).unwrap().enabled);
    assert!(
        app.asset(&package.manifest.id, "templates/harbor.txt")
            .is_ok()
    );
}

#[tokio::test]
async fn multi_plugin_activation_follows_dependencies_and_rolls_back_failed_packages() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let cleaned = Arc::new(AtomicUsize::new(0));
    fixture_factory(&mut app, cleaned.clone());
    let original = app.registry.lock().await.freeze().unwrap().snapshot();
    for (id, fail, expected, count) in [
        ("example.multi", false, "", 3),
        ("example.failed", true, "plugin_activation", 5),
        ("example.command", false, "missing_service", 8),
    ] {
        let root = package(
            source.path(),
            id,
            vec![
                definition("alpha", &["a/1", "value/1"], &[], false),
                definition("beta", &["b/1", "value/1"], &["a/1"], fail),
                definition("consumer", &["c/1"], &["value/1"], false),
            ],
        );
        edit_manifest(&root, |m| {
            m["bindings"] = json!({"value/1":"beta"});
            if id == "example.command" {
                m["commands"] =
                    json!([{"name":"remember","service":"absent/1","operation":"remember"}]);
                m["configuration_defaults"] = json!({"rollback":true});
            }
        });
        app.install(&root).unwrap();
        let result = app.enable(&[id.into()], BTreeSet::new(), false).await;
        if expected.is_empty() {
            result.unwrap();
            app.disable(id).await.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, expected);
        }
        assert!(!app.inspect(id).unwrap().enabled);
        assert!(app.commands().unwrap().is_empty());
        assert_eq!(app.configuration(&json!({})).unwrap(), json!({}));
        let current = app.registry.lock().await.freeze().unwrap().snapshot();
        assert_eq!(current, original);
        assert_eq!(cleaned.load(Ordering::SeqCst), count);
    }
}

#[tokio::test]
async fn shared_plugins_keep_their_remaining_owner_and_uninstall_keeps_user_state() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::memory().unwrap());
    let mut app = manager(data.path(), store.clone()).await;
    let cleaned = Arc::new(AtomicUsize::new(0));
    fixture_factory(&mut app, cleaned.clone());
    for id in ["example.first", "example.second"] {
        let root = package(
            source.path(),
            id,
            vec![definition("shared", &["shared/1"], &[], false)],
        );
        app.install(&root).unwrap();
        app.enable(&[id.into()], BTreeSet::new(), false)
            .await
            .unwrap();
    }
    store
        .state_write("shared", "note", &json!({"keep":true}))
        .unwrap();
    app.disable("example.first").await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);
    assert_eq!(app.registry.lock().await.inspect().len(), 6);
    app.uninstall("example.first").unwrap();
    assert!(app.inspect("example.first").is_err());
    assert_eq!(
        store.state_read("shared", "note").unwrap(),
        Some(json!({"keep":true}))
    );
    app.disable("example.second").await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn extension_dependencies_block_disable_and_detect_cycles() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::memory().unwrap());
    let mut app = manager(data.path(), store).await;
    let a = package(source.path(), "example.a", vec![]);
    let b = package(source.path(), "example.b", vec![]);
    edit_manifest(&b, |m| m["dependencies"] = json!({"example.a":"^0.1"}));
    app.install(&a).unwrap();
    app.install(&b).unwrap();
    assert_eq!(
        app.enable(&["example.b".into()], BTreeSet::new(), false)
            .await
            .unwrap_err()
            .code,
        "missing_extension_dependency"
    );
    app.enable(
        &["example.a".into(), "example.b".into()],
        BTreeSet::new(),
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        app.disable("example.a").await.unwrap_err().code,
        "missing_extension_dependency"
    );
    app.disable("example.b").await.unwrap();
    app.disable("example.a").await.unwrap();
    let c = package(source.path(), "example.c", vec![]);
    let d = package(source.path(), "example.d", vec![]);
    edit_manifest(&c, |m| m["dependencies"] = json!({"example.d":"*"}));
    edit_manifest(&d, |m| m["dependencies"] = json!({"example.c":"*"}));
    app.install(&c).unwrap();
    app.install(&d).unwrap();
    assert_eq!(
        app.enable(
            &["example.c".into(), "example.d".into()],
            BTreeSet::new(),
            false
        )
        .await
        .unwrap_err()
        .code,
        "extension_dependency_cycle"
    );
}

#[tokio::test]
async fn catalog_revision_conflicts_remove_staged_plugins() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(data.path().join("state.sqlite3")).unwrap());
    let mut first = manager(data.path(), store.clone()).await;
    let root = package(
        source.path(),
        "example.catalog",
        vec![
            definition("alpha", &["value/1"], &[], false),
            definition("beta", &["value/1"], &[], false),
            definition("consumer", &["c/1"], &["value/1"], false),
        ],
    );
    edit_manifest(&root, |m| m["bindings"] = json!({"value/1":"beta"}));
    first.install(&root).unwrap();
    let mut stale = manager(data.path(), store.clone()).await;
    let cleaned = Arc::new(AtomicUsize::new(0));
    fixture_factory(&mut stale, cleaned.clone());
    let unrelated = package(source.path(), "example.unrelated", vec![]);
    first.install(&unrelated).unwrap();
    let error = stale
        .enable(&["example.catalog".into()], BTreeSet::new(), false)
        .await
        .unwrap_err();
    assert_eq!(error.code, "configuration_conflict");
    assert_eq!(cleaned.load(Ordering::SeqCst), 3);
    assert_eq!(stale.registry.lock().await.inspect().len(), 5);
    let current = manager(data.path(), store).await;
    assert!(!current.inspect("example.catalog").unwrap().enabled);
}

#[tokio::test]
async fn package_hashes_and_paths_block_tampered_assets() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::memory().unwrap());
    let mut app = manager(data.path(), store.clone()).await;
    let root = package(source.path(), "example.paths", vec![]);
    edit_manifest(&root, |m| {
        m["assets"] = json!([{"path":"../outside.txt","sha256":"bad"}])
    });
    assert_eq!(
        app.install(&root).unwrap_err().code,
        "invalid_extension_asset_path"
    );
    let package = app
        .install(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tavern/examples/extensions/harbor-assets"
        )))
        .unwrap();
    app.enable(from_ref(&package.manifest.id), BTreeSet::new(), false)
        .await
        .unwrap();
    std::fs::write(
        data.path()
            .join("extensions")
            .join(&package.directory)
            .join("templates/harbor.txt"),
        "Changed.",
    )
    .unwrap();
    assert_eq!(
        app.asset(&package.manifest.id, "templates/harbor.txt")
            .unwrap_err()
            .code,
        "invalid_extension_hash"
    );
    let mut reopened = manager(data.path(), store).await;
    assert_eq!(
        reopened.restore().await.unwrap_err().code,
        "invalid_extension_hash"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn package_symbolic_links_cannot_escape_the_source_directory() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let root = package(source.path(), "example.symlink", vec![]);
    let outside = source.path().join("outside.txt");
    std::fs::write(&outside, "Outside.").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape.txt")).unwrap();
    assert_eq!(
        app.install(&root).unwrap_err().code,
        "invalid_extension_asset_path"
    );
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "Outside.");
}

#[tokio::test]
async fn bundled_sources_require_explicit_network_grants() {
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let package = app.install_builtin("chub").unwrap();
    assert_eq!(
        app.enable(from_ref(&package.manifest.id), BTreeSet::new(), false)
            .await
            .unwrap_err()
            .code,
        "capability_denied"
    );
    app.enable(
        from_ref(&package.manifest.id),
        BTreeSet::from(["network:https:api.chub.ai".into()]),
        false,
    )
    .await
    .unwrap();
    let lease = app.registry.lock().await.freeze().unwrap();
    let source = lease
        .service::<Arc<dyn CardSource>>("card.source.chub/1")
        .unwrap();
    assert!(source.recognize("https://chub.ai/author/character"));
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
                "unsupported_extension_command",
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
async fn extension_commands_call_typed_native_services() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let root = package(
        source.path(),
        "example.command",
        vec![definition("echo", &["command.echo/1"], &[], false)],
    );
    edit_manifest(&root, |m| {
        m["commands"] = json!([{"name":"say","service":"command.echo/1","operation":"echo"}])
    });
    app.register_factory(
        "fixture".into(),
        Arc::new(|definition| {
            Ok(Arc::new(EchoPlugin {
                manifest: definition.manifest.clone(),
            }))
        }),
    )
    .unwrap();
    app.install(&root).unwrap();
    app.enable(&["example.command".into()], BTreeSet::new(), false)
        .await
        .unwrap();
    assert_eq!(
        app.run_command("say", json!({"text":"Hello."}), CancellationToken::new())
            .await
            .unwrap(),
        json!({"echo":{"text":"Hello."}})
    );
    app.disable("example.command").await.unwrap();
    assert_eq!(
        app.run_command("say", json!({}), CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        "invalid_extension_command"
    );
}

#[tokio::test]
async fn provider_defaults_resolve_without_becoming_explicit_configuration() {
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut app = manager(data.path(), Arc::new(SqliteStore::memory().unwrap())).await;
    let root = package(source.path(), "example.provider", vec![]);
    edit_manifest(
        &root,
        |m| m["configuration_defaults"] = json!({"providers":{"local":{"kind":"custom","model":"fixture","endpoint":"http://127.0.0.1:1","api_key_env":null}}}),
    );
    app.install(&root).unwrap();
    app.enable(&["example.provider".into()], BTreeSet::new(), false)
        .await
        .unwrap();
    let mut explicit = rustytavern::config::Configuration::default();
    assert_eq!(app.provider(&explicit, "local").unwrap().model, "fixture");
    assert!(explicit.providers.is_empty());
    explicit.providers.insert(
        "local".into(),
        rustytavern::provider::ProviderConfig {
            model: "chosen".into(),
            ..Default::default()
        },
    );
    assert_eq!(app.provider(&explicit, "local").unwrap().model, "chosen");
    app.disable("example.provider").await.unwrap();
    assert!(
        app.provider(&rustytavern::config::Configuration::default(), "local")
            .is_err()
    );
}
