#![cfg(unix)]

use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};

use rustytavern::{
    external::{ExternalSourcePlugin, ProcessDefinition},
    plugin::{CompositionLease, PluginManifest, PluginRegistry},
    source::CardSource,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const URL: &str = "https://example.com/card";

async fn start(
    directory: &TempDir,
    body: &str,
    timeout_ms: u64,
) -> (PluginRegistry, CompositionLease) {
    let card: Value =
        serde_json::from_slice(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    for id in [2, 3] {
        std::fs::write(
            directory.path().join(format!("reply-{id}")),
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":id,"result":{"card":card}})
            ),
        )
        .unwrap();
    }
    let script = format!(
        r#"printf '%s\n' "$$" > "$1/pid"
IFS= read -r line
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"host_protocol":"1","plugin_id":"lifecycle","service_families":["card.source/1"]}}}}'
{body}
"#
    );
    let plugin = ExternalSourcePlugin::new(
        PluginManifest {
            id: "lifecycle".into(),
            provides: vec!["card.source/1".into()],
            ..Default::default()
        },
        ProcessDefinition {
            executable: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                script,
                "lifecycle".into(),
                directory.path().to_str().unwrap().into(),
            ],
            timeout_ms,
            ..Default::default()
        },
    );
    let mut registry = PluginRegistry::default();
    registry
        .inject(vec![Arc::new(plugin)], "test", &BTreeMap::new())
        .await
        .unwrap();
    let lease = registry.freeze().unwrap();
    (registry, lease)
}

async fn started(directory: &TempDir) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !directory.path().join("started").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

fn ended(directory: &TempDir) {
    let pid = std::fs::read_to_string(directory.path().join("pid")).unwrap();
    assert!(
        !std::process::Command::new("/bin/sh")
            .args(["-c", "kill -0 \"$1\"", "probe", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn cancellation_sends_the_request_id_and_rejects_a_late_result() {
    let directory = TempDir::new().unwrap();
    let (_, lease) = start(
        &directory,
        r#"IFS= read -r line
printf '%s\n' "$line" > "$1/request"
touch "$1/started"
IFS= read -r line
printf '%s\n' "$line" > "$1/cancel"
cat "$1/reply-2"
IFS= read -r line
printf '%s\n' "$line" > "$1/shutdown""#,
        10_000,
    )
    .await;
    let source = lease
        .service::<Arc<dyn CardSource>>("card.source/1")
        .unwrap();
    let cancellation = CancellationToken::new();
    let task = tokio::spawn({
        let source = source.clone();
        let cancellation = cancellation.clone();
        async move { source.fetch(URL, cancellation).await }
    });
    started(&directory).await;
    cancellation.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.code, "cancelled");
    let read = |name| {
        serde_json::from_slice::<Value>(&std::fs::read(directory.path().join(name)).unwrap())
            .unwrap()
    };
    let request = read("request");
    assert!(request["params"]["deadline_ms"].as_i64().unwrap() > 0);
    assert!(
        request["params"]["scope"]
            .as_str()
            .unwrap()
            .starts_with("invocation-")
    );
    let cancel = read("cancel");
    assert_eq!(cancel["method"], "$/cancelRequest");
    assert_eq!(cancel["params"]["id"], request["id"]);
    assert_eq!(cancel["params"]["reason"], "cancelled");
    assert!(cancel.get("id").is_none());
    assert_eq!(read("shutdown")["method"], "shutdown");
    assert_eq!(
        source
            .fetch(URL, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        "plugin_not_active"
    );
    ended(&directory);
}

#[tokio::test]
async fn removal_sends_shutdown_and_reaps_the_child() {
    let directory = TempDir::new().unwrap();
    let (mut registry, lease) = start(
        &directory,
        r#"IFS= read -r line
printf '%s\n' "$line" > "$1/shutdown""#,
        1000,
    )
    .await;
    drop(lease);
    registry.eject_owner("test").await.unwrap();
    let request: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("shutdown")).unwrap()).unwrap();
    assert_eq!(request["method"], "shutdown");
    assert_eq!(request["id"], 2);
    ended(&directory);
}

#[tokio::test]
async fn deadline_termination_bounds_an_uncooperative_child() {
    for (body, padding) in [
        (
            "IFS= read -r line\ntouch \"$1/started\"\nexec /bin/sleep 30",
            0,
        ),
        ("touch \"$1/started\"\nexec /bin/sleep 30", 1_000_000),
    ] {
        let directory = TempDir::new().unwrap();
        let (_, lease) = start(&directory, body, 100).await;
        let source = lease
            .service::<Arc<dyn CardSource>>("card.source/1")
            .unwrap();
        let began = tokio::time::Instant::now();
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            source.fetch(
                &format!("{URL}{}", "x".repeat(padding)),
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.code, "plugin_timeout");
        assert!(began.elapsed() >= Duration::from_secs(2));
        assert!(directory.path().join("started").exists());
        ended(&directory);
    }
}

#[tokio::test]
async fn queued_cancellation_keeps_the_active_process_available() {
    let directory = TempDir::new().unwrap();
    let (_, lease) = start(
        &directory,
        r#"IFS= read -r line
touch "$1/started"
sleep 0.2
cat "$1/reply-2"
IFS= read -r line
cat "$1/reply-3"
IFS= read -r line"#,
        10_000,
    )
    .await;
    let source = lease
        .service::<Arc<dyn CardSource>>("card.source/1")
        .unwrap();
    let first = tokio::spawn({
        let source = source.clone();
        async move { source.fetch(URL, CancellationToken::new()).await }
    });
    started(&directory).await;
    let cancellation = CancellationToken::new();
    let mut queued = Box::pin(source.fetch(URL, cancellation.clone()));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    cancellation.cancel();
    assert_eq!(queued.await.unwrap_err().code, "cancelled");
    first.await.unwrap().unwrap();
    source.fetch(URL, CancellationToken::new()).await.unwrap();
}

#[tokio::test]
async fn transport_rejects_oversized_invalid_and_unmatched_frames() {
    for (body, code) in [
        ("exit 0", "plugin_process"),
        ("printf 'bad json\\n'", "invalid_plugin_frame"),
        ("printf '\\377\\n'", "invalid_plugin_frame"),
        (
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":999,\"result\":{}}'",
            "invalid_plugin_frame",
        ),
        (
            "head -c 1048577 /dev/zero | tr '\\000' x",
            "invalid_plugin_frame",
        ),
        (
            r#"i=0
while [ "$i" -lt 65 ]; do
printf '%s\n' '{"jsonrpc":"2.0","id":"callback","method":"unknown","params":{}}'
IFS= read -r line
i=$((i+1))
done
IFS= read -r line"#,
            "plugin_queue_limit",
        ),
        (
            r#"i=0
while [ "$i" -lt 9 ]; do
printf '%s' '{"jsonrpc":"2.0","id":"callback","method":"unknown","params":{"padding":"'
head -c 999000 /dev/zero | tr '\000' x
printf '%s\n' '"}}'
IFS= read -r line
i=$((i+1))
done
IFS= read -r line"#,
            "plugin_queue_limit",
        ),
    ] {
        let directory = TempDir::new().unwrap();
        let (_, lease) = start(&directory, &format!("IFS= read -r line\n{body}"), 10_000).await;
        let source = lease
            .service::<Arc<dyn CardSource>>("card.source/1")
            .unwrap();
        assert_eq!(
            source
                .fetch(URL, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            code
        );
        ended(&directory);
    }
}

#[tokio::test]
async fn invocation_queues_enforce_count_and_byte_limits() {
    for (count, padding) in [(63, 0), (7, 1_000_000)] {
        let directory = TempDir::new().unwrap();
        let (_, lease) = start(
            &directory,
            "IFS= read -r line\ntouch \"$1/started\"\nwhile IFS= read -r line; do :; done",
            30_000,
        )
        .await;
        let source = lease
            .service::<Arc<dyn CardSource>>("card.source/1")
            .unwrap();
        let url = format!("{URL}{}", "x".repeat(padding));
        let cancellation = CancellationToken::new();
        let first = tokio::spawn({
            let source = source.clone();
            let url = url.clone();
            let cancellation = cancellation.clone();
            async move { source.fetch(&url, cancellation).await }
        });
        started(&directory).await;
        let mut queued = (0..count)
            .map(|_| Box::pin(source.fetch(&url, cancellation.clone())))
            .collect::<Vec<_>>();
        for future in &mut queued {
            assert!(futures_util::poll!(future.as_mut()).is_pending());
        }
        assert_eq!(
            source
                .fetch(&url, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            "plugin_queue_limit"
        );
        drop(queued);
        cancellation.cancel();
        assert_eq!(first.await.unwrap().unwrap_err().code, "cancelled");
        ended(&directory);
    }
}
