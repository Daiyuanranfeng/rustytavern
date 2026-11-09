use futures_util::StreamExt;
use rig_agent::agent::MultiTurnStreamItem;
use rig_core::{
    message::ToolName,
    streaming::{Item, StreamEvent},
    tool::{DynamicTool, ToolOutput},
};
use rustytavern::provider::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

struct TestCredentials;
impl CredentialResolver for TestCredentials {
    fn resolve(&self, _: &str) -> rustytavern::Result<String> {
        Ok("test-key".into())
    }
}

/// The server answers one connection for each body and sends each request body back.
fn mock(bodies: Vec<String>) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (send, receive) = mpsc::channel();
    let task = thread::spawn(move || {
        for body in bodies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            let split = loop {
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&chunk[..n]);
                if let Some(index) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..split]).to_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while request.len() - split < length {
                let n = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..n]);
            }
            send.send(serde_json::from_slice(&request[split..split + length]).unwrap())
                .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (format!("http://{address}"), receive, task)
}

fn sse(frames: &[Value]) -> String {
    frames.iter().map(|f| format!("data: {f}\n\n")).collect()
}
fn chunk(delta: Value, reason: Value) -> Value {
    json!({"choices":[{"index":0,"delta":delta,"finish_reason":reason}]})
}

/// The function returns a tool call reply and a text reply in the native frames of the protocol.
fn native_frames(kind: ProviderKind) -> [String; 2] {
    match kind {
        ProviderKind::OpenAi => [
            sse(&[
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_1","delta":"{\"key\":\"a\"}"}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{\"key\":\"a\"}"}}),
                json!({"type":"response.completed","response":{"id":"resp_1","model":"fixture","status":"completed","output":[{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{\"key\":\"a\"}"}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}),
            ]),
            sse(&[
                json!({"type":"response.output_text.delta","delta":"Done."}),
                json!({"type":"response.completed","response":{"id":"resp_2","model":"fixture","status":"completed","usage":{"total_tokens":3}}}),
            ]),
        ],
        ProviderKind::Anthropic => {
            let start = json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"fixture","content":[],"usage":{"input_tokens":3,"output_tokens":0}}});
            let event = |frames: &[Value]| {
                frames
                    .iter()
                    .map(|f| format!("event: {}\ndata: {f}\n\n", f["type"].as_str().unwrap()))
                    .collect::<String>()
            };
            [
                event(&[
                    start.clone(),
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"lookup","input":{}}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"key\":\"a\"}"}}),
                    json!({"type":"content_block_stop","index":0}),
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
                    json!({"type":"message_stop"}),
                ]),
                event(&[
                    start,
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}}),
                    json!({"type":"content_block_stop","index":0}),
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                    json!({"type":"message_stop"}),
                ]),
            ]
        }
        ProviderKind::Gemini => [
            sse(&[
                json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"key":"a"}},"thoughtSignature":"sig"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"totalTokenCount":3}}),
            ]),
            sse(&[
                json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Done."}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"totalTokenCount":3}}),
            ]),
        ],
        _ => [
            sse(&[
                chunk(
                    json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"key\":\"a\"}"}}]}),
                    Value::Null,
                ),
                chunk(json!({}), json!("tool_calls")),
            ]) + "data: [DONE]\n\n",
            sse(&[
                chunk(json!({"content":"Done."}), Value::Null),
                chunk(json!({}), json!("stop")),
            ]) + "data: [DONE]\n\n",
        ],
    }
}

fn request(kind: ProviderKind, endpoint: String) -> PreparedRequest {
    let config = ProviderConfig {
        kind,
        endpoint: Some(endpoint),
        api_key_env: Some("config:test".into()),
        ..Default::default()
    };
    let plan = rustytavern::prompt::PromptPlan {
        sections: vec![rustytavern::prompt::Section {
            id: "message".into(),
            role: rustytavern::session::Role::User,
            text: "Look up a.".into(),
            source: "test".into(),
            mandatory: true,
            units: 10,
        }],
        output_reserve: 64,
        ..Default::default()
    };
    let lookup = ToolDefinition {
        name: "lookup".into(),
        description: "The tool reads a value.".into(),
        parameters: json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}),
    };
    let prepared = HttpProvider.prepare(&plan, "fixture", &config).unwrap();
    HttpProvider.prepare_agent(prepared, &[lookup]).unwrap()
}

/// The tool records each call and returns a fixed value.
fn lookup(calls: Arc<Mutex<Vec<Value>>>) -> DynamicTool {
    DynamicTool::new(
        ToolName::new("lookup").unwrap(),
        "The tool reads a value.",
        json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}),
        move |arguments| {
            calls.lock().unwrap().push(arguments);
            Box::pin(async { Ok(ToolOutput::text("VALUE_A")) })
        },
    )
}

#[tokio::test]
async fn http_tool_rounds_send_native_tools_and_continuations() {
    for kind in [
        ProviderKind::OpenRouter,
        ProviderKind::Custom,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
    ] {
        let (endpoint, received, server) = mock(native_frames(kind).into());
        let request = request(kind, endpoint);
        let model = HttpProvider.model(&request, &TestCredentials).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut stream = agent(&request, model, vec![lookup(calls.clone())], 2, 0, true)
            .unwrap()
            .stream();
        let mut text = String::new();
        while let Some(item) = stream.next().await {
            if let MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                text: delta,
                ..
            })) = item.unwrap_or_else(|error| panic!("{kind:?}: {error}"))
            {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "Done.", "{kind:?}");
        assert_eq!(*calls.lock().unwrap(), [json!({"key":"a"})], "{kind:?}");
        let (first, second) = (received.recv().unwrap(), received.recv().unwrap());
        let (first, second) = (first.to_string(), second.to_string());
        let (declaration, search, result) = match kind {
            ProviderKind::OpenAi => ("\"lookup\"", "web_search", "function_call_output"),
            ProviderKind::Anthropic => ("input_schema", "web_search_20250305", "tool_result"),
            ProviderKind::Gemini => ("functionDeclarations", "google_search", "functionResponse"),
            ProviderKind::OpenRouter => ("\"lookup\"", "openrouter:web_search", "tool_call_id"),
            ProviderKind::Custom => ("\"lookup\"", "\"lookup\"", "tool_call_id"),
        };
        assert!(
            first.contains(declaration) && first.contains(search),
            "{kind:?}: {first}"
        );
        assert!(
            second.contains(result) && second.contains("VALUE_A"),
            "{kind:?}: {second}"
        );
        if kind == ProviderKind::Custom {
            assert!(!first.contains("web_search"), "{first}");
        }
        server.join().unwrap();
    }
}

#[tokio::test]
async fn http_text_keeps_reasoning_apart() {
    let body = sse(&[
        chunk(
            json!({"role":"assistant","content":"Hi","reasoning":"Plan."}),
            Value::Null,
        ),
        chunk(json!({}), json!("stop")),
    ]) + "data: [DONE]\n\n";
    let (endpoint, _received, server) = mock(vec![body]);
    let request = request(ProviderKind::OpenRouter, endpoint);
    let model = HttpProvider.model(&request, &TestCredentials).unwrap();
    let mut stream = agent(&request, model, Vec::new(), 1, 0, true)
        .unwrap()
        .stream();
    let (mut text, mut thinking) = (String::new(), String::new());
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                text: delta,
                ..
            })) => text.push_str(&delta),
            MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Reasoning {
                text: delta,
                ..
            })) => thinking.push_str(&delta),
            _ => {}
        }
    }
    assert_eq!((text.as_str(), thinking.as_str()), ("Hi", "Plan."));
    server.join().unwrap();
}
