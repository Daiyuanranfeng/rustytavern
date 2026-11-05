use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    thread,
};

fn run(directory: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap()
}

fn cli(directory: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&run(directory, args)).unwrap()
}

fn sse(delta: Value, finish: Value) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}

/// The server answers each request in order and returns the request bodies.
fn serve(answers: Vec<String>) -> (String, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/chat", listener.local_addr().unwrap());
    let task = thread::spawn(move || {
        let mut bodies = Vec::new();
        for answer in answers {
            let (mut stream, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buffer = [0u8; 4096];
            let split = loop {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buffer[..n]);
                if let Some(i) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&data[..split]).to_lowercase();
            let length = head
                .lines()
                .find_map(|s| s.strip_prefix("content-length:"))
                .map(|s| s.trim().parse::<usize>().unwrap())
                .unwrap();
            while data.len() - split < length {
                let n = stream.read(&mut buffer).unwrap();
                data.extend_from_slice(&buffer[..n]);
            }
            bodies.push(serde_json::from_slice(&data[split..split + length]).unwrap());
            let body = format!("{answer}data: [DONE]\n\n");
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        bodies
    });
    (endpoint, task)
}

/// The function prepares a data directory with the agent extension and one session.
fn session(directory: &Path, endpoint: &str, grants: &[&str]) -> String {
    cli(
        directory,
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
            endpoint,
        ],
    );
    let card = cli(
        directory,
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
        directory,
        &[
            "persona",
            "create",
            "--name",
            "Alex",
            "--description",
            "Alex reads maps.",
        ],
    );
    cli(directory, &["extension", "install", "--builtin", "agent"]);
    let mut enable = vec!["extension", "enable", "org.rustytavern.extension.agent"];
    for grant in [
        "session.read:current",
        "session.write:current",
        "provider.generate",
    ]
    .iter()
    .chain(grants)
    {
        enable.extend(["--grant", grant]);
    }
    cli(directory, &enable);
    let session = cli(
        directory,
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
    session["id"].as_str().unwrap().to_owned()
}

fn chat(directory: &Path, id: &str) -> Vec<Value> {
    run(
        directory,
        &["chat", id, "--message", "Greet me.", "--events"],
    )
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect()
}

#[test]
fn the_agent_reads_a_skill_and_returns_the_result_to_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let skill = directory.path().join("skills/greet");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: greet\ndescription: Greet a traveler.\n---\nSay hello in French.\n",
    )
    .unwrap();
    let call = json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"skill","arguments":"{\"name\":\"greet\"}"}}]});
    let (endpoint, server) = serve(vec![
        sse(call, Value::Null) + &sse(json!({}), json!("tool_calls")),
        sse(json!({"content":"Bonjour."}), json!("stop")),
    ]);
    let id = session(directory.path(), &endpoint, &["skills.read"]);
    let events = chat(directory.path(), &id);
    let bodies = server.join().unwrap();

    let tools = bodies[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert!(
        tools[0]["function"]["description"]
            .as_str()
            .unwrap()
            .contains("- greet: Greet a traveler.")
    );
    let messages = bodies[1]["messages"].as_array().unwrap();
    let result = messages.last().unwrap();
    assert_eq!(result["role"], "tool");
    assert_eq!(result["tool_call_id"], "call_1");
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("Say hello in French.")
    );
    let kinds = events
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"tool_call") && kinds.contains(&"tool_result"));
    assert_eq!(events.last().unwrap()["type"], "completed");
    assert_eq!(events.last().unwrap()["text"], "Bonjour.");
}

#[test]
fn the_agent_without_tool_grants_sends_a_plain_request() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, server) = serve(vec![sse(json!({"content":"Hello."}), json!("stop"))]);
    let id = session(directory.path(), &endpoint, &[]);
    let events = chat(directory.path(), &id);
    let bodies = server.join().unwrap();
    assert!(bodies[0].get("tools").is_none());
    assert_eq!(events.last().unwrap()["text"], "Hello.");
}

#[test]
fn the_agent_calls_an_mcp_tool_with_a_reduced_schema() {
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let server = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_echo.py");
    std::fs::write(
        directory.path().join("mcp.json"),
        json!({"mcpServers":{"echo":{"command":"python3","args":[server]}}}).to_string(),
    )
    .unwrap();
    let call = json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"echo__say","arguments":"{\"text\":\"hi\"}"}}]});
    let (endpoint, provider) = serve(vec![
        sse(call, Value::Null) + &sse(json!({}), json!("tool_calls")),
        sse(json!({"content":"Done."}), json!("stop")),
    ]);
    let id = session(directory.path(), &endpoint, &["mcp.connect"]);
    let events = chat(directory.path(), &id);
    let bodies = provider.join().unwrap();

    let schema = &bodies[0]["tools"][0]["function"]["parameters"];
    assert_eq!(
        schema,
        &json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
    );
    let result = bodies[1]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(result["content"], "echo: hi");
    assert_eq!(events.last().unwrap()["text"], "Done.");
}
