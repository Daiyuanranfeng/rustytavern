use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    thread,
};

fn run(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustytavern"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap()
}
fn cli(data: &Path, args: &[&str]) -> Value {
    let output = run(data, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn catalog_filters_and_selection_keep_generation_settings_and_saved_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/catalog", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
        }
        let body = json!({"data":[
            {"id":"text-model","architecture":{"input_modalities":["text"],"output_modalities":["text"]},"supported_parameters":["temperature"]},
            {"id":"vision-model","architecture":{"input_modalities":["text","image"],"output_modalities":["text"]},"supported_parameters":["tools"],"future":{"keep":42}}
        ]}).to_string();
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        String::from_utf8(request).unwrap()
    });
    cli(
        data,
        &[
            "config",
            "provider",
            "--name",
            "local",
            "--kind",
            "custom",
            "--model",
            "old-model",
            "--endpoint",
            "http://127.0.0.1:1/chat",
        ],
    );
    let card = cli(
        data,
        &[
            "card",
            "import",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../tavern/examples/lumen.json"),
        ],
    );
    let persona = cli(
        data,
        &[
            "persona",
            "create",
            "--name",
            "Alex",
            "--description",
            "A visitor.",
        ],
    );
    let session = cli(
        data,
        &[
            "session",
            "new",
            "--character",
            card["id"].as_str().unwrap(),
            "--persona",
            persona["id"].as_str().unwrap(),
            "--provider",
            "local",
        ],
    );
    let catalog = cli(
        data,
        &[
            "model",
            "discover",
            "local",
            "--endpoint",
            &endpoint,
            "--input",
            "image",
            "--output",
            "text",
            "--parameter",
            "tools",
        ],
    );
    assert_eq!(catalog["models"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["models"][0]["id"], "vision-model");
    assert_eq!(catalog["models"][0]["raw"]["future"]["keep"], 42);
    assert_eq!(catalog["provenance"]["evidence"], "advertised_unverified");
    assert!(
        server
            .join()
            .unwrap()
            .starts_with("GET /catalog HTTP/1.1\r\n")
    );
    cli(data, &["model", "select", "local", "vision-model"]);
    let saved = cli(data, &["config", "show"]);
    assert_eq!(saved["providers"]["local"]["model"], "vision-model");
    assert_eq!(
        saved["providers"]["local"]["endpoint"],
        "http://127.0.0.1:1/chat"
    );
    let retained = cli(data, &["session", "show", session["id"].as_str().unwrap()]);
    assert_eq!(retained["session"]["model"], "old-model");
    let rejected = run(
        data,
        &[
            "model",
            "discover",
            "local",
            "--endpoint",
            "http://example.com/models",
        ],
    );
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("Authorization"));
}
#[test]
fn model_selection_normalizes_gemini_resource_ids_and_rejects_invalid_models() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    cli(
        data,
        &[
            "config",
            "provider",
            "--name",
            "gemini",
            "--kind",
            "gemini",
            "--model",
            "old-model",
        ],
    );
    cli(data, &["model", "select", "gemini", "models/new-model"]);
    assert_eq!(
        cli(data, &["config", "show"])["providers"]["gemini"]["model"],
        "new-model"
    );
    assert!(
        !run(data, &["model", "select", "gemini", ""])
            .status
            .success()
    );
    assert_eq!(
        cli(data, &["config", "show"])["providers"]["gemini"]["model"],
        "new-model"
    );
}
