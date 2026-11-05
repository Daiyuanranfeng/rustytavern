use futures_util::StreamExt;
use rustytavern::provider::*;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};
use tokio_util::sync::CancellationToken;
struct TestCredentials;
impl CredentialResolver for TestCredentials {
    fn resolve(&self, _: &str) -> rustytavern::Result<String> {
        Ok("test-key".into())
    }
}

fn prepare_agent(
    config: &ProviderConfig,
    agent: &AgentRequest,
) -> rustytavern::Result<PreparedRequest> {
    HttpProvider.prepare_agent(
        HttpProvider.prepare(&Default::default(), "fixture", config)?,
        agent,
    )
}

fn mock(body: String, status: &str) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let (send, receive) = mpsc::channel();
    let task = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        let split;
        loop {
            let n = stream.read(&mut chunk).unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&chunk[..n]);
            if let Some(index) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                split = index + 4;
                break;
            }
        }
        let headers = String::from_utf8_lossy(&request[..split]).to_lowercase();
        let length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .map(|s| s.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while request.len() - split < length {
            let n = stream.read(&mut chunk).unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&chunk[..n]);
        }
        let value = serde_json::from_slice(&request[split..split + length]).unwrap();
        send.send(value).unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    (format!("http://{address}/generation"), receive, task)
}

fn config(endpoint: String) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::Custom,
        endpoint: Some(endpoint),
        api_key_env: None,
        ..Default::default()
    }
}
fn agent() -> AgentRequest {
    AgentRequest {
        tools: vec![ToolDefinition {
            name: "lookup".into(),
            description: "The tool reads a value.".into(),
            parameters: json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}),
        }],
        continuation: vec![],
    }
}
fn chunk(delta: Value, reason: Value) -> String {
    json!({"choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string()
}
fn frames() -> Vec<String> {
    vec![
        chunk(
            json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"key\":"}},{"index":1,"id":"call_2","type":"function","function":{"name":"lookup","arguments":"{"}}]}),
            Value::Null,
        ),
        chunk(
            json!({"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}},{"index":1,"function":{"arguments":"\"key\":\"b\"}"}}]}),
            Value::Null,
        ),
        chunk(json!({}), json!("tool_calls")),
        json!({"choices":[],"usage":{"total_tokens":10}}).to_string(),
        "[DONE]".into(),
    ]
}
#[tokio::test]
async fn http_stream_emits_complete_calls_and_preserves_continuation_ids() {
    let body = frames()
        .iter()
        .map(|f| format!("data: {f}\n\n"))
        .collect::<String>();
    let (endpoint, received, server) = mock(body, "200 OK");
    let request = prepare_agent(&config(endpoint), &agent()).unwrap();
    let mut stream = HttpProvider
        .open_agent_stream(request, &TestCredentials, CancellationToken::new())
        .await
        .unwrap();
    let mut calls = vec![];
    let mut complete = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            AgentProviderEvent::ToolCalls { calls: value } => calls = value,
            AgentProviderEvent::Complete => complete = true,
            _ => {}
        }
    }
    assert!(complete);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "lookup");
    assert_eq!(calls[1].arguments, json!({"key":"b"}));
    assert_eq!(
        received.recv().unwrap()["tools"][0]["function"]["name"],
        "lookup"
    );
    server.join().unwrap();
    let mut agent = agent();
    agent.continuation.push(AgentTurn {
        text: String::new(),
        results: calls
            .iter()
            .map(|c| ToolResult {
                call_id: c.id.clone(),
                content: "found".into(),
            })
            .collect(),
        calls,
    });
    let request = prepare_agent(&config("http://127.0.0.1:1".into()), &agent).unwrap();
    assert_eq!(request.body["messages"][1]["tool_call_id"], "call_1");
    assert_eq!(request.body["messages"][2]["tool_call_id"], "call_2");
}
/// The function streams the frames through a mock server and returns the events and the wire body.
async fn stream(
    kind: ProviderKind,
    agent: &AgentRequest,
    frames: &[String],
) -> (Vec<rustytavern::Result<AgentProviderEvent>>, Value) {
    // Anthropic names each event in the SSE event field.
    let body = frames
        .iter()
        .map(|f| {
            match serde_json::from_str::<Value>(f)
                .ok()
                .and_then(|v| v["type"].as_str().map(str::to_owned))
            {
                Some(kind) => format!("event: {kind}\ndata: {f}\n\n"),
                None => format!("data: {f}\n\n"),
            }
        })
        .collect();
    let (endpoint, received, server) = mock(body, "200 OK");
    let mut config = config(endpoint);
    config.kind = kind;
    config.api_key_env = Some("KEY".into());
    let request = prepare_agent(&config, agent).unwrap();
    let events = match HttpProvider
        .open_agent_stream(request, &TestCredentials, CancellationToken::new())
        .await
    {
        Ok(stream) => stream.collect().await,
        Err(error) => vec![Err(error)],
    };
    let wire = received.recv().unwrap();
    server.join().unwrap();
    (events, wire)
}
fn failure(events: &[rustytavern::Result<AgentProviderEvent>]) -> Option<&str> {
    events.iter().find_map(|event| match event {
        Err(error) | Ok(AgentProviderEvent::Failure { error }) => Some(error.code.as_str()),
        _ => None,
    })
}
#[tokio::test]
async fn http_rejects_bad_calls_and_streams_without_a_finish() {
    for delta in [
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{"}}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"[]"}}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{\"key\":\"x\"}"}},{"index":1,"id":"a","function":{"name":"lookup","arguments":"{\"key\":\"y\"}"}}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"x".repeat(65537)}}]}),
    ] {
        let frames = [
            chunk(delta, Value::Null),
            chunk(json!({}), json!("tool_calls")),
            "[DONE]".into(),
        ];
        let (events, _) = stream(ProviderKind::Custom, &agent(), &frames).await;
        assert_eq!(failure(&events), Some("invalid_provider_tool"));
    }
    let (events, _) = stream(
        ProviderKind::Custom,
        &agent(),
        &[frames()[0].clone(), "[DONE]".into()],
    )
    .await;
    assert_eq!(failure(&events), Some("incomplete_stream"));
}
fn turn(result_id: &str) -> AgentTurn {
    AgentTurn {
        text: String::new(),
        calls: vec![ToolCall {
            id: "a".into(),
            name: "lookup".into(),
            arguments: json!({"key":"x"}),
            signature: Some("sig".into()),
        }],
        results: vec![ToolResult {
            call_id: result_id.into(),
            content: "found".into(),
        }],
    }
}
#[test]
fn agent_requests_keep_one_chat_shape_and_check_results() {
    let mut request = agent();
    request.continuation.push(turn("a"));
    for (kind, contract) in [
        (ProviderKind::OpenAi, "openai.responses.agent-v1"),
        (ProviderKind::Anthropic, "anthropic.messages.agent-v1"),
        (ProviderKind::Gemini, "gemini.generate_content.agent-v1"),
    ] {
        let mut config = config("http://127.0.0.1:1".into());
        config.kind = kind;
        let request = prepare_agent(&config, &request).unwrap();
        assert_eq!(request.contract_id, contract);
        assert_eq!(request.body["tools"][0]["name"], "lookup");
        let messages = request.body["messages"].as_array().unwrap();
        assert_eq!(
            messages[messages.len() - 2]["tool_calls"][0]["signature"],
            "sig"
        );
        assert_eq!(messages[messages.len() - 1]["tool_call_id"], "a");
    }
    let mut agent = agent();
    agent.continuation.push(turn("b"));
    let config = config("http://127.0.0.1:1".into());
    assert!(prepare_agent(&config, &agent).is_err());
}
fn native_frames(kind: ProviderKind) -> Vec<Value> {
    match kind {
        ProviderKind::OpenAi => vec![
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_1","delta":"{\"key\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_1","delta":"\"a\"}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{\"key\":\"a\"}"}}),
            json!({"type":"response.completed","response":{"id":"resp_1","model":"fixture","status":"completed","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}),
        ],
        ProviderKind::Anthropic => vec![
            json!({"type":"message_start","message":{"usage":{"input_tokens":3}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Look."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"key\":\"a\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ],
        _ => vec![
            json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"key":"a"}},"thoughtSignature":"sig"}]}}]}),
            json!({"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":3}}),
        ],
    }
}
#[tokio::test]
async fn http_native_streams_emit_tool_calls_and_send_native_continuations() {
    let mut request = agent();
    request.continuation.push(turn("a"));
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
    ] {
        let frames = native_frames(kind)
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>();
        let (events, wire) = stream(kind, &request, &frames).await;
        let events = events.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        let [
            ..,
            AgentProviderEvent::ToolCalls { calls },
            AgentProviderEvent::Complete,
        ] = &events[..]
        else {
            panic!("The {kind:?} stream must end with calls: {events:?}");
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup");
        assert_eq!(calls[0].arguments, json!({"key":"a"}));
        assert!(!calls[0].id.is_empty());
        let wire = wire.to_string();
        for term in match kind {
            ProviderKind::OpenAi => ["function_call", "function_call_output", "call_id"],
            ProviderKind::Anthropic => ["tool_use", "tool_result", "input_schema"],
            _ => ["functionCall", "functionResponse", "functionDeclarations"],
        } {
            assert!(
                wire.contains(term),
                "The {kind:?} request lacks {term}: {wire}"
            );
        }
    }
}
#[tokio::test]
async fn http_tool_finish_without_calls_fails() {
    let frames = [
        json!({"type":"message_start","message":{"usage":{"input_tokens":3}}}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]
    .map(|f| f.to_string());
    let (events, _) = stream(ProviderKind::Anthropic, &agent(), &frames).await;
    assert_eq!(failure(&events), Some("unsupported_provider_output"));
}
#[tokio::test]
async fn http_missing_terminal_error_and_cancellation_fail() {
    for (frames, code) in [
        (vec![frames()[0].clone()], "incomplete_stream"),
        (
            vec!["{\"error\":{\"code\":\"rate_limit_exceeded\"}}".into()],
            "provider_rate_limit",
        ),
    ] {
        let (events, _) = stream(ProviderKind::Custom, &agent(), &frames).await;
        assert_eq!(failure(&events), Some(code));
    }
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let config = config("http://127.0.0.1:1".into());
    let request = prepare_agent(&config, &agent()).unwrap();
    let error = HttpProvider
        .open_agent_stream(request, &TestCredentials, cancellation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "cancelled");
}

#[tokio::test]
async fn http_rejects_unknown_tools_and_invalid_argument_values() {
    for (name, arguments) in [
        ("other", "{\"key\":\"x\"}"),
        ("lookup", "{\"key\":2}"),
        ("lookup", "{}"),
    ] {
        let frames = [
            chunk(
                json!({"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":name,"arguments":arguments}}]}),
                Value::Null,
            ),
            chunk(json!({}), json!("tool_calls")),
            "[DONE]".into(),
        ];
        let (events, _) = stream(ProviderKind::Custom, &agent(), &frames).await;
        assert_eq!(failure(&events), Some("invalid_provider_tool"));
    }
}

#[test]
fn invalid_schemas_fail() {
    for schema in [
        json!({"type":"object","properties":{"x":{"type":"bad"}}}),
        json!({"type":"object","required":["missing"]}),
        json!({"type":"object","patternProperties":{}}),
    ] {
        let mut agent = agent();
        agent.tools[0].parameters = schema;
        let config = config("http://127.0.0.1:1".into());
        assert!(prepare_agent(&config, &agent).is_err());
    }
}

#[tokio::test]
async fn http_text_ignores_provider_metadata_and_the_openrouter_usage_chunk() {
    let end = json!({"content":"","role":"assistant","reasoning":null});
    let usage = json!({"choices":[{"index":0,"delta":end,"finish_reason":"stop"}],"usage":{"total_tokens":9}});
    let frames = [
        chunk(
            json!({"role":"assistant","content":"Hi","refusal":null,"reasoning":"Plan.","reasoning_details":[]}),
            Value::Null,
        ),
        chunk(end.clone(), json!("stop")),
        usage.to_string(),
        "[DONE]".into(),
    ];
    let (events, _) = stream(ProviderKind::OpenRouter, &agent(), &frames).await;
    let events = events.into_iter().map(Result::unwrap).collect::<Vec<_>>();
    assert!(
        matches!(
            &events[..],
            [
                AgentProviderEvent::Text { text },
                AgentProviderEvent::Usage { usage },
                AgentProviderEvent::Complete
            ] if text == "Hi" && usage["total_tokens"] == 9
        ),
        "{events:?}"
    );
    let frames = [
        chunk(
            json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{\"key\":\"x\"}"}}]}),
            Value::Null,
        ),
        chunk(end.clone(), json!("tool_calls")),
        usage.to_string().replace("\"stop\"", "\"tool_calls\""),
        "[DONE]".into(),
    ];
    let (events, _) = stream(ProviderKind::OpenRouter, &agent(), &frames).await;
    assert!(
        matches!(events.last(), Some(Ok(AgentProviderEvent::Complete))),
        "{events:?}"
    );
}

#[tokio::test]
async fn http_cancellation_discards_pending_calls() {
    for read_text in [false, true] {
        let text = chunk(json!({"content":"before"}), Value::Null);
        let body = std::iter::once(text)
            .chain(frames())
            .map(|f| format!("data: {f}\n\n"))
            .collect();
        let (endpoint, received, server) = mock(body, "200 OK");
        let cancellation = CancellationToken::new();
        let config = config(endpoint);
        let request = prepare_agent(&config, &agent()).unwrap();
        let mut stream = HttpProvider
            .open_agent_stream(request, &TestCredentials, cancellation.clone())
            .await
            .unwrap();
        if read_text {
            assert!(matches!(
                stream.next().await.unwrap().unwrap(),
                AgentProviderEvent::Text { .. }
            ));
        }
        cancellation.cancel();
        assert_eq!(stream.next().await.unwrap().unwrap_err().code, "cancelled");
        assert!(stream.next().await.is_none());
        received.recv().unwrap();
        server.join().unwrap();
    }
}

#[test]
fn continuation_budget_precedes_request_cloning() {
    let mut agent = agent();
    agent.continuation = (0..9)
        .map(|index| AgentTurn {
            text: "x".repeat(1048576),
            calls: vec![ToolCall {
                id: index.to_string(),
                name: "lookup".into(),
                arguments: json!({"key":"x"}),
                signature: None,
            }],
            results: vec![ToolResult {
                call_id: index.to_string(),
                content: "found".into(),
            }],
        })
        .collect();
    let config = config("http://127.0.0.1:1".into());
    let error = prepare_agent(&config, &agent).unwrap_err();
    assert_eq!(error.code, "invalid_provider_tool");
    assert_eq!(error.message, "The tool request exceeds its byte limit.");
}
