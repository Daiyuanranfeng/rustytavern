use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    plugin::{Contribution, PluginContext, PluginManifest, PluginRegistry, RuntimePlugin},
    session::Role,
};

struct TestPlugin {
    manifest: PluginManifest,
    fail: bool,
    cleaned: Arc<AtomicUsize>,
    contribution: Option<Contribution>,
    denied: bool,
}
impl TestPlugin {
    fn new(id: &str, provides: &[&str], requires: &[&str]) -> Self {
        Self {
            manifest: PluginManifest {
                id: id.into(),
                provides: provides.iter().map(|s| s.to_string()).collect(),
                requires: requires.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            fail: false,
            cleaned: Arc::new(AtomicUsize::new(0)),
            contribution: None,
            denied: false,
        }
    }
}
#[async_trait]
impl RuntimePlugin for TestPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        let cleaned = self.cleaned.clone();
        context.own(move || {
            cleaned.fetch_add(1, Ordering::SeqCst);
        });
        for service in &self.manifest.provides {
            context.register(service, Arc::new(7usize))?;
        }
        if self.denied {
            context.authorize("network:https:example.com")?;
        }
        if let Some(contribution) = &self.contribution {
            context.contribute(contribution.clone())?;
        }
        if self.fail {
            return Err(Error::new(
                "plugin_activation",
                "The fixture activation fails.",
            ));
        }
        Ok(())
    }
}
#[tokio::test]
async fn plugin_graph_reports_missing_services_cycles_and_protocol_errors() {
    let mut registry = PluginRegistry::default();
    let error = registry
        .inject(
            vec![Arc::new(TestPlugin::new("a", &[], &["missing/1"]))],
            "caller",
            &BTreeMap::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "missing_service");
    let error = registry
        .inject(
            vec![
                Arc::new(TestPlugin::new("a", &["a/1"], &["b/1"])),
                Arc::new(TestPlugin::new("b", &["b/1"], &["a/1"])),
            ],
            "caller",
            &BTreeMap::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "dependency_cycle");
    let mut bad = TestPlugin::new("bad", &[], &[]);
    bad.manifest.host_protocol = "2".into();
    let error = registry
        .inject(vec![Arc::new(bad)], "caller", &BTreeMap::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "incompatible_plugin_protocol");
    assert!(registry.inspect().is_empty());
}
#[tokio::test]
async fn activation_is_atomic_and_cleanup_runs_in_failed_batches() {
    let mut registry = PluginRegistry::default();
    let first = TestPlugin::new("a", &["a/1"], &[]);
    let clean_first = first.cleaned.clone();
    let mut second = TestPlugin::new("b", &["b/1"], &["a/1"]);
    second.fail = true;
    let clean_second = second.cleaned.clone();
    assert!(
        registry
            .inject(
                vec![Arc::new(second), Arc::new(first)],
                "extension",
                &BTreeMap::new()
            )
            .await
            .is_err()
    );
    assert!(registry.inspect().is_empty());
    assert_eq!(clean_first.load(Ordering::SeqCst), 1);
    assert_eq!(clean_second.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn grants_require_both_manifest_request_and_explicit_user_selection() {
    let mut registry = PluginRegistry::default();
    let mut plugin = TestPlugin::new("a", &[], &[]);
    plugin.denied = true;
    plugin
        .manifest
        .requested_capabilities
        .insert("network:https:example.com".into());
    assert_eq!(
        registry
            .inject(vec![Arc::new(plugin)], "caller", &BTreeMap::new())
            .await
            .unwrap_err()
            .code,
        "capability_denied"
    );
    let mut plugin = TestPlugin::new("a", &[], &[]);
    plugin.denied = true;
    let grants = BTreeMap::from([(
        "a".into(),
        BTreeSet::from(["network:https:example.com".into()]),
    )]);
    assert_eq!(
        registry
            .inject(vec![Arc::new(plugin)], "caller", &grants)
            .await
            .unwrap_err()
            .code,
        "capability_denied"
    );
}
#[tokio::test]
async fn plugin_snapshots_keep_services_and_defer_injection_until_release() {
    let mut registry = PluginRegistry::default();
    registry
        .inject(
            vec![Arc::new(TestPlugin::new("a", &["value/1"], &[]))],
            "caller",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    let composition = registry.freeze().unwrap();
    assert_eq!(*composition.service::<usize>("value/1").unwrap(), 7);
    assert_eq!(
        registry
            .inject(
                vec![Arc::new(TestPlugin::new("b", &[], &[]))],
                "caller",
                &BTreeMap::new()
            )
            .await
            .unwrap_err()
            .code,
        "plugin_change_deferred"
    );
    drop(composition);
    registry
        .inject(
            vec![Arc::new(TestPlugin::new("b", &[], &[]))],
            "caller",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    registry.eject_owner("caller").await.unwrap();
    assert!(registry.inspect().is_empty());
}
#[tokio::test]
async fn shared_ownership_and_dependency_checks_prevent_early_ejection() {
    let mut registry = PluginRegistry::default();
    let plugin = TestPlugin::new("a", &["a/1"], &[]);
    let cleaned = plugin.cleaned.clone();
    registry
        .inject(vec![Arc::new(plugin)], "extension-a", &BTreeMap::new())
        .await
        .unwrap();
    registry.add_owner("a", "caller").unwrap();
    assert!(
        registry
            .eject_owner("extension-a")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);
    registry
        .inject(
            vec![Arc::new(TestPlugin::new("b", &[], &["a/1"]))],
            "extension-b",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.eject_owner("caller").await.unwrap_err().code,
        "plugin_in_use"
    );
    registry.eject_owner("extension-b").await.unwrap();
    registry.eject_owner("caller").await.unwrap();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn prompt_contributions_use_declared_slots_and_stable_plugin_order() {
    let mut registry = PluginRegistry::default();
    let mut plugins: Vec<Arc<dyn RuntimePlugin>> = Vec::new();
    let mut grants = BTreeMap::new();
    for id in ["b", "a"] {
        let mut plugin = TestPlugin::new(id, &[], &[]);
        plugin
            .manifest
            .allowed_slots
            .insert("before_character".into());
        plugin
            .manifest
            .requested_capabilities
            .insert("prompt.contribute:before_character".into());
        plugin.contribution = Some(Contribution {
            plugin_id: "ignored".into(),
            provenance: serde_json::Value::Null,
            retention: Default::default(),
            section_id: "s".into(),
            slot: "before_character".into(),
            priority: 0,
            role: Role::System,
            text: id.into(),
        });
        grants.insert(
            id.into(),
            BTreeSet::from(["prompt.contribute:before_character".into()]),
        );
        plugins.push(Arc::new(plugin));
    }
    registry.inject(plugins, "caller", &grants).await.unwrap();
    let composition = registry.freeze().unwrap();
    assert_eq!(
        composition
            .contributions
            .iter()
            .map(|c| c.plugin_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}
#[tokio::test]
async fn multiple_service_providers_require_an_explicit_binding() {
    let mut registry = PluginRegistry::default();
    registry
        .inject(
            vec![
                Arc::new(TestPlugin::new("a", &["value/1"], &[])),
                Arc::new(TestPlugin::new("b", &["value/1"], &[])),
            ],
            "caller",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(registry.freeze().err().unwrap().code, "ambiguous_service");
    registry.set_binding("value/1".into(), "b".into()).unwrap();
    registry
        .inject(
            vec![Arc::new(TestPlugin::new("consumer", &[], &["value/1"]))],
            "consumer",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        registry
            .set_binding("value/1".into(), "a".into())
            .unwrap_err()
            .code,
        "plugin_in_use"
    );
    assert_eq!(
        registry.replace_bindings(BTreeMap::new()).unwrap_err().code,
        "plugin_in_use"
    );
    assert_eq!(registry.bindings()["value/1"], "b");
    assert_eq!(
        *registry
            .freeze()
            .unwrap()
            .service::<usize>("value/1")
            .unwrap(),
        7
    );
}
