use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    thread,
};

fn cli(directory: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_rustytavern"))
        .arg("--data-dir")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn cli_import_prompt_chat_resume_regenerate_branch_and_export_work() {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/chat", listener.local_addr().unwrap());
    let task = thread::spawn(move || {
        let mut bodies = Vec::new();
        for answer in ["Welcome, Alex.", "Another reply."] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            let split;
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buf[..n]);
                if let Some(i) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                    split = i + 4;
                    break;
                }
            }
            let head = String::from_utf8_lossy(&data[..split]).to_lowercase();
            let length = head
                .lines()
                .find_map(|s| {
                    s.strip_prefix("content-length:")
                        .map(|s| s.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            while data.len() - split < length {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buf[..n]);
            }
            bodies.push(serde_json::from_slice::<Value>(&data[split..split + length]).unwrap());
            let body = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"content":answer},"finish_reason":"stop"}],"usage":{"total_tokens":9}})
            );
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
        bodies
    });
    cli(
        directory.path(),
        &[
            "config",
            "provider",
            "--name",
            "mock",
            "--kind",
            "custom",
            "--model",
            "fixture",
            "--endpoint",
            &endpoint,
            "--merge-system-messages",
        ],
    );
    let card = cli(
        directory.path(),
        &[
            "card",
            "import",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../tavern/tests/fixtures/synthetic/card-v3.json"
            ),
            "--json",
        ],
    );
    let persona = cli(
        directory.path(),
        &[
            "persona",
            "create",
            "--name",
            "Alex",
            "--description",
            "Alex charts the coast.",
        ],
    );
    cli(
        directory.path(),
        &[
            "extension",
            "install",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../tavern/examples/extensions/lighthouse-edits"
            ),
        ],
    );
    cli(
        directory.path(),
        &[
            "extension",
            "enable",
            "example.lighthouse-edits",
            "--grant",
            "session.read:current",
            "--grant",
            "prompt.transform:description",
        ],
    );
    let session = cli(
        directory.path(),
        &[
            "session",
            "new",
            "--character",
            card["id"].as_str().unwrap(),
            "--persona",
            persona["id"].as_str().unwrap(),
            "--provider",
            "mock",
        ],
    );
    let id = session["id"].as_str().unwrap();
    let plan = cli(
        directory.path(),
        &["prompt", "inspect", id, "--message", "Hello.", "--json"],
    );
    assert!(
        plan["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "current_user" && s["text"] == "Hello.")
    );
    let inspected = cli(
        directory.path(),
        &["prompt", "inspect", id, "--message", "Hello.", "--request"],
    );
    assert_eq!(inspected["plan"], plan);
    assert_eq!(
        inspected["prepared"]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "system")
            .count(),
        1
    );
    let output = Command::new(env!("CARGO_BIN_EXE_rustytavern"))
        .arg("--data-dir")
        .arg(directory.path())
        .args([
            "chat",
            id,
            "--message",
            "Hello.",
            "--events",
            "--idempotency-key",
            "once",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.first().unwrap()["type"], "prepared");
    assert_eq!(events.last().unwrap()["type"], "completed");
    let resumed = cli(directory.path(), &["session", "show", id, "--json"]);
    assert_eq!(resumed["history"].as_array().unwrap().len(), 3);
    let output = Command::new(env!("CARGO_BIN_EXE_rustytavern"))
        .arg("--data-dir")
        .arg(directory.path())
        .args(["session", "regenerate", id])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Another reply."));
    let shown = cli(directory.path(), &["session", "show", id]);
    let user = shown["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "user")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    cli(
        directory.path(),
        &[
            "session",
            "edit",
            id,
            "--at",
            user,
            "--message",
            "Another direction.",
        ],
    );
    let export_path = directory.path().join("export.json");
    cli(
        directory.path(),
        &[
            "session",
            "export",
            id,
            "--output",
            export_path.to_str().unwrap(),
            "--include-traces",
        ],
    );
    let export: Value = serde_json::from_slice(&std::fs::read(export_path).unwrap()).unwrap();
    assert_eq!(export["generations"].as_array().unwrap().len(), 2);
    assert!(
        export["generations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["request"]["prepared"]["body"].is_object())
    );
    let generation = &export["generations"][0];
    assert_eq!(inspected["prepared"], generation["request"]["prepared"]);
    let replay = cli(
        directory.path(),
        &["prompt", "replay", generation["id"].as_str().unwrap()],
    );
    assert_eq!(
        replay["prepared"]["body"],
        generation["request"]["prepared"]["body"]
    );
    assert_eq!(replay["phase_trace"]["edits"].as_array().unwrap().len(), 1);
    assert!(
        replay["plan"]["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "description"
                && s["text"].as_str().unwrap().contains("lamp shines blue"))
    );
    let requests = task.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(inspected["prepared"]["body"], requests[0]);
    assert_eq!(
        requests[0]["messages"].as_array().unwrap().last().unwrap()["content"],
        "Hello."
    );
    assert_eq!(
        requests[1]["messages"].as_array().unwrap().last().unwrap()["content"],
        "Hello."
    );
}
