#![cfg(unix)]

use async_trait::async_trait;
use rustytavern::{
    Result,
    card::Card,
    external::{ExternalSourcePlugin, ProcessDefinition},
    plugin::{PluginContext, PluginManifest, PluginRegistry, RuntimePlugin},
    source::{Artifact, ArtifactFetcher, CardSource, Provenance},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn manifest() -> PluginManifest {
    PluginManifest {
        id: "fixture".into(),
        provides: vec!["card.source/1".into()],
        ..Default::default()
    }
}
fn handshake() -> String {
    format!("IFS= read -r line\nprintf '%s\\n' {}\n",quote(&json!({"jsonrpc":"2.0","id":1,"result":{"host_protocol":"1","plugin_id":"fixture","service_families":["card.source/1"]}}).to_string()))
}
fn definition(script: String) -> ProcessDefinition {
    ProcessDefinition {
        executable: "/bin/sh".into(),
        args: vec!["-c".into(), script],
        accepted_hosts: BTreeSet::from(["public.example".into()]),
        timeout_ms: 1000,
    }
}

#[tokio::test]
async fn an_external_source_handshakes_and_imports_a_card_without_inherited_keys() {
    let card: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let script = format!(
        "[ -z \"${{OPENROUTER_API_KEY+x}}\" ] || exit 17\n{}IFS= read -r line\nprintf '%s\\n' {}\nIFS= read -r line\n",
        handshake(),
        quote(&json!({"jsonrpc":"2.0","id":2,"result":{"card":card}}).to_string())
    );
    let plugin = ExternalSourcePlugin::new(manifest(), definition(script));
    let mut registry = PluginRegistry::default();
    registry
        .inject(vec![Arc::new(plugin)], "extension", &BTreeMap::new())
        .await
        .unwrap();
    let composition = registry.freeze().unwrap();
    let source = composition
        .service::<Arc<dyn CardSource>>("card.source/1")
        .unwrap();
    let artifact = source
        .fetch("https://public.example/card", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(Card::import(&artifact.bytes).unwrap().name(), "Lumen");
    drop(source);
    drop(composition);
    registry.eject_owner("extension").await.unwrap();
}

#[tokio::test]
async fn external_protocol_failures_roll_back_activation() {
    let mut registry = PluginRegistry::default();
    let plugin = ExternalSourcePlugin::new(
        manifest(),
        definition("IFS= read -r line\nprintf 'plain log text\\n'\n".into()),
    );
    assert_eq!(
        registry
            .inject(vec![Arc::new(plugin)], "extension", &BTreeMap::new())
            .await
            .unwrap_err()
            .code,
        "invalid_plugin_frame"
    );
    assert!(registry.inspect().is_empty());
    let mut bad = manifest();
    bad.provides = vec!["provider/1".into()];
    let plugin = ExternalSourcePlugin::new(bad, definition(handshake()));
    assert_eq!(
        registry
            .inject(vec![Arc::new(plugin)], "extension", &BTreeMap::new())
            .await
            .unwrap_err()
            .code,
        "unsupported_plugin_service"
    );
    assert!(registry.inspect().is_empty());
}

#[tokio::test]
async fn a_nonreplying_external_source_times_out_and_ends_its_process() {
    let script = format!(
        "{}IFS= read -r line\nwhile IFS= read -r line; do :; done\n",
        handshake()
    );
    let mut registry = PluginRegistry::default();
    registry
        .inject(
            vec![Arc::new(ExternalSourcePlugin::new(
                manifest(),
                definition(script),
            ))],
            "extension",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    let composition = registry.freeze().unwrap();
    let source = composition
        .service::<Arc<dyn CardSource>>("card.source/1")
        .unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(3),
        source.fetch("https://public.example/card", CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code, "plugin_timeout");
    let error = source
        .fetch("https://public.example/card", CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code, "plugin_not_active");
}

struct ReplayFetcher {
    bytes: Vec<u8>,
    media_type: String,
}
#[async_trait]
impl ArtifactFetcher for ReplayFetcher {
    async fn fetch(&self, url: &str, _: CancellationToken) -> Result<Artifact> {
        Ok(Artifact {
            bytes: self.bytes.clone(),
            provenance: Provenance {
                source: "fixture".into(),
                requested_url: url.into(),
                final_url: url.into(),
                fetched_at_ms: 0,
                media_type: self.media_type.clone(),
                content_hash: rustytavern::digest(&self.bytes),
                creator: None,
                license: None,
            },
        })
    }
}
struct FetchPlugin {
    manifest: PluginManifest,
    fetcher: Arc<dyn ArtifactFetcher>,
}
#[async_trait]
impl RuntimePlugin for FetchPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        context.register("host.fetch/1", Arc::new(self.fetcher.clone()))
    }
}

#[tokio::test]
async fn large_card_downloads_use_scoped_artifact_handles() {
    let raw = include_bytes!("fixtures/synthetic/dual.png");
    let mut bytes = raw[..raw.len() - 12].to_vec();
    let payload = vec![0; 2 * 1024 * 1024];
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(b"raNd");
    bytes.extend_from_slice(&payload);
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"raNd");
    crc.update(&payload);
    bytes.extend_from_slice(&crc.finalize().to_be_bytes());
    bytes.extend_from_slice(&raw[raw.len() - 12..]);
    let fetch = FetchPlugin {
        manifest: PluginManifest {
            id: "host-fetch".into(),
            provides: vec!["host.fetch/1".into()],
            ..Default::default()
        },
        fetcher: Arc::new(ReplayFetcher {
            bytes: bytes.clone(),
            media_type: "image/png".into(),
        }),
    };
    let callback=json!({"jsonrpc":"2.0","id":"fetch-one","method":"host.fetch","params":{"url":"https://public.example/card.png"}}).to_string();
    let script = format!(
        "{}IFS= read -r line\nprintf '%s\\n' {}\nIFS= read -r reply\nhandle=$(printf '%s' \"$reply\" | sed -n 's/.*\"artifact_id\":\"\\([^\"]*\\)\".*/\\1/p')\nprintf '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"artifact_id\":\"%s\"}}}}\\n' \"$handle\"\nIFS= read -r line\n",
        handshake(),
        quote(&callback)
    );
    let mut manifest = manifest();
    manifest.requires = vec!["host.fetch/1".into()];
    manifest
        .requested_capabilities
        .insert("network:https:public.example".into());
    let grants = BTreeMap::from([(
        "fixture".into(),
        BTreeSet::from(["network:https:public.example".into()]),
    )]);
    let mut registry = PluginRegistry::default();
    registry
        .inject(
            vec![
                Arc::new(fetch),
                Arc::new(ExternalSourcePlugin::new(manifest, definition(script))),
            ],
            "extension",
            &grants,
        )
        .await
        .unwrap();
    let composition = registry.freeze().unwrap();
    let source = composition
        .service::<Arc<dyn CardSource>>("card.source/1")
        .unwrap();
    let artifact = source
        .fetch("https://public.example/card", CancellationToken::new())
        .await
        .unwrap();
    assert!(artifact.bytes.len() > 1024 * 1024);
    assert_eq!(artifact.bytes, bytes);
    assert_eq!(Card::import(&artifact.bytes).unwrap().name(), "Lumen");
}

#[tokio::test]
async fn host_callback_responses_share_the_invocation_output_budget() {
    let fetch = FetchPlugin {
        manifest: PluginManifest {
            id: "host-fetch".into(),
            provides: vec!["host.fetch/1".into()],
            ..Default::default()
        },
        fetcher: Arc::new(ReplayFetcher {
            bytes: include_bytes!("fixtures/synthetic/dual.png").to_vec(),
            media_type: "x".repeat(1_000_000),
        }),
    };
    let callback = json!({"jsonrpc":"2.0","id":"fetch","method":"host.fetch","params":{"url":"https://public.example/card.png"}});
    let script = format!(
        "{}IFS= read -r line\nfor n in 1 2 3 4 5 6 7 8; do\nprintf '%s\\n' {}\nIFS= read -r line\ndone\nIFS= read -r line",
        handshake(),
        quote(&callback.to_string())
    );
    let mut manifest = manifest();
    manifest.requires = vec!["host.fetch/1".into()];
    manifest
        .requested_capabilities
        .insert("network:https:public.example".into());
    let grants = BTreeMap::from([("fixture".into(), manifest.requested_capabilities.clone())]);
    let mut process = definition(script);
    process.timeout_ms = 30_000;
    let mut registry = PluginRegistry::default();
    registry
        .inject(
            vec![
                Arc::new(fetch),
                Arc::new(ExternalSourcePlugin::new(manifest, process)),
            ],
            "extension",
            &grants,
        )
        .await
        .unwrap();
    let lease = registry.freeze().unwrap();
    let source = lease
        .service::<Arc<dyn CardSource>>("card.source/1")
        .unwrap();
    let error = source
        .fetch(
            &format!("https://public.example/{}", "x".repeat(1_000_000)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "plugin_queue_limit");
}
