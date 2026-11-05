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
            json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"look","arguments":"{\"key\":"}},{"index":1,"id":"call_2","type":"function","function":{"name":"lookup","arguments":"{"}}]}),
            Value::Null,
        ),
        chunk(
            json!({"tool_calls":[{"index":0,"function":{"name":"up","arguments":"\"a\"}"}},{"index":1,"function":{"arguments":"\"key\":\"b\"}"}}]}),
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
#[test]
fn normalizer_holds_calls_until_done_and_rejects_bad_calls() {
    let mut normalizer = AgentStreamNormalizer::new(ProviderKind::Custom, true);
    for frame in &frames()[..4] {
        assert!(
            !normalizer
                .frame(frame)
                .unwrap()
                .iter()
                .any(|e| matches!(e, AgentProviderEvent::ToolCalls { .. }))
        );
    }
    assert!(matches!(
        normalizer.frame("[DONE]").unwrap()[0],
        AgentProviderEvent::ToolCalls { .. }
    ));
    for delta in [
        json!({"tool_calls":[{"index":32}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{"}}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"[]"}}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{}"}},{"index":1,"id":"a","function":{"name":"lookup","arguments":"{}"}}]}),
        json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"x".repeat(65537)}}]}),
    ] {
        let mut n = AgentStreamNormalizer::new(ProviderKind::Custom, true);
        let result = n
            .frame(&chunk(delta, Value::Null))
            .and_then(|_| n.frame(&chunk(json!({}), json!("tool_calls"))))
            .and_then(|_| n.frame("[DONE]"));
        assert!(result.is_err());
    }
    let mut n = AgentStreamNormalizer::new(ProviderKind::Custom, true);
    n.frame(&frames()[0]).unwrap();
    assert!(n.frame("[DONE]").is_err());
    let mut n = AgentStreamNormalizer::new(ProviderKind::Custom, true);
    n.frame(&chunk(json!({}), json!("stop"))).unwrap();
    assert!(n.frame(&frames()[0]).is_err());
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
fn native_protocols_encode_tools_and_continuations() {
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
        let body = &request.body;
        match kind {
            ProviderKind::OpenAi => {
                assert_eq!(body["tools"][0]["name"], "lookup");
                assert_eq!(body["tools"][0]["strict"], false);
                let input = body["input"].as_array().unwrap();
                assert_eq!(input[input.len() - 2]["type"], "function_call");
                assert_eq!(input[input.len() - 1]["call_id"], "a");
                assert_eq!(input[input.len() - 1]["output"], "found");
            }
            ProviderKind::Anthropic => {
                assert!(body["tools"][0]["input_schema"].is_object());
                let messages = body["messages"].as_array().unwrap();
                let call = &messages[messages.len() - 2]["content"][0];
                assert_eq!(call["type"], "tool_use");
                assert_eq!(call["input"], json!({"key":"x"}));
                let result = &messages[messages.len() - 1]["content"][0];
                assert_eq!(result["tool_use_id"], "a");
            }
            _ => {
                assert_eq!(
                    body["tools"][0]["functionDeclarations"][0]["name"],
                    "lookup"
                );
                let contents = body["contents"].as_array().unwrap();
                let call = &contents[contents.len() - 2]["parts"][0];
                assert_eq!(call["functionCall"]["args"], json!({"key":"x"}));
                assert_eq!(call["thoughtSignature"], "sig");
                let result = &contents[contents.len() - 1]["parts"][0]["functionResponse"];
                assert_eq!(result["name"], "lookup");
                assert_eq!(result["response"]["result"], "found");
            }
        }
    }
    let mut agent = agent();
    agent.continuation.push(turn("b"));
    let config = config("http://127.0.0.1:1".into());
    assert!(prepare_agent(&config, &agent).is_err());
}
fn native_frames(kind: ProviderKind) -> Vec<Value> {
    match kind {
        ProviderKind::OpenAi => vec![
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"key\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"\"a\"}"}),
            json!({"type":"response.completed","response":{"status":"completed","usage":{"total_tokens":3}}}),
        ],
        ProviderKind::Anthropic => vec![
            json!({"type":"message_start","message":{"usage":{"input_tokens":3}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Look."}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"key\":\"a\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ],
        _ => vec![
            json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"key":"a"}},"thoughtSignature":"sig"}]}}]}),
            json!({"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":3}}),
        ],
    }
}
#[tokio::test]
async fn http_native_streams_emit_tool_calls() {
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
    ] {
        let body = native_frames(kind)
            .iter()
            .map(|f| format!("data: {f}\n\n"))
            .collect();
        let (endpoint, received, server) = mock(body, "200 OK");
        let mut config = config(endpoint);
        config.kind = kind;
        config.api_key_env = Some("KEY".into());
        let request = prepare_agent(&config, &agent()).unwrap();
        let mut stream = HttpProvider
            .open_agent_stream(request, &TestCredentials, CancellationToken::new())
            .await
            .unwrap();
        let mut events = vec![];
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
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
        if kind == ProviderKind::Gemini {
            assert!(calls[0].id.starts_with("gemini_"));
            assert_eq!(calls[0].signature.as_deref(), Some("sig"));
        } else {
            assert_eq!(calls[0].id, "call_1");
        }
        received.recv().unwrap();
        server.join().unwrap();
    }
}
#[test]
fn native_streams_reject_bad_calls() {
    let mut n = AgentStreamNormalizer::new(ProviderKind::Anthropic, true);
    let stop = json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}});
    assert!(n.frame(&stop.to_string()).is_err());
    let mut n = AgentStreamNormalizer::new(ProviderKind::Anthropic, true);
    let frames = native_frames(ProviderKind::Anthropic);
    n.frame(&frames[2].to_string()).unwrap();
    let end = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}});
    assert!(n.frame(&end.to_string()).is_err());
    let mut n = AgentStreamNormalizer::new(ProviderKind::OpenAi, true);
    let delta = native_frames(ProviderKind::OpenAi)[1].to_string();
    assert!(n.frame(&delta).is_err());
    let mut n = AgentStreamNormalizer::new(ProviderKind::OpenAi, true);
    let added = native_frames(ProviderKind::OpenAi)[0].to_string();
    n.frame(&added).unwrap();
    assert!(n.frame(&added).is_err());
}
#[tokio::test]
async fn http_missing_terminal_error_and_cancellation_fail() {
    for (body, code) in [
        (format!("data: {}\n\n", frames()[0]), "incomplete_stream"),
        (
            "data: {\"error\":{\"code\":\"rate_limit_exceeded\"}}\n\n".into(),
            "provider_rate_limit",
        ),
    ] {
        let (endpoint, received, server) = mock(body, "200 OK");
        let request = prepare_agent(&config(endpoint), &agent()).unwrap();
        let mut stream = HttpProvider
            .open_agent_stream(request, &TestCredentials, CancellationToken::new())
            .await
            .unwrap();
        let event = stream.next().await.unwrap();
        match event {
            Err(error) | Ok(AgentProviderEvent::Failure { error }) => assert_eq!(error.code, code),
            _ => panic!("The stream must fail."),
        }
        received.recv().unwrap();
        server.join().unwrap();
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
        let body = [
            chunk(json!({"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":name,"arguments":arguments}}]}), Value::Null),
            chunk(json!({}), json!("tool_calls")), "[DONE]".into(),
        ].iter().map(|f| format!("data: {f}\n\n")).collect();
        let (endpoint, received, server) = mock(body, "200 OK");
        let request = prepare_agent(&config(endpoint), &agent()).unwrap();
        let mut stream = HttpProvider
            .open_agent_stream(request, &TestCredentials, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().code,
            "invalid_provider_tool"
        );
        received.recv().unwrap();
        server.join().unwrap();
    }
}

#[test]
fn schemas_mixed_output_and_duplicate_ids_fail() {
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
    for delta in [
        json!({"audio":{}}),
        json!({"function_call":{}}),
        json!({"refusal":"No"}),
    ] {
        let mut n = AgentStreamNormalizer::new(ProviderKind::Custom, true);
        assert!(n.frame(&chunk(delta, Value::Null)).is_err());
    }
    let mut n = AgentStreamNormalizer::new(ProviderKind::Custom, true);
    n.frame(&frames()[0]).unwrap();
    assert!(
        n.frame(&chunk(
            json!({"tool_calls":[{"index":0,"id":"other"}]}),
            Value::Null
        ))
        .is_err()
    );
}

#[test]
fn normalizer_ignores_provider_metadata_in_text_deltas() {
    let mut n = AgentStreamNormalizer::new(ProviderKind::Custom, true);
    let delta = json!({"role":"assistant","content":"Hi","refusal":null,"reasoning":"Plan.","reasoning_details":[]});
    assert!(
        n.frame(&chunk(delta, Value::Null))
            .unwrap()
            .iter()
            .any(|e| matches!(e, AgentProviderEvent::Text { text } if text == "Hi"))
    );
    n.frame(&chunk(json!({}), json!("stop"))).unwrap();
    assert!(matches!(
        n.frame("[DONE]").unwrap()[..],
        [AgentProviderEvent::Complete]
    ));
}

#[test]
fn normalizer_accepts_the_openrouter_usage_chunk_after_the_finish_reason() {
    for (delta, reason) in [
        (json!({"content":"Hi"}), "stop"),
        (
            json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{}"}}]}),
            "tool_calls",
        ),
    ] {
        let mut n = AgentStreamNormalizer::new(ProviderKind::OpenRouter, true);
        n.frame(&chunk(delta, Value::Null)).unwrap();
        let end = json!({"content":"","role":"assistant","reasoning":null});
        n.frame(&chunk(end.clone(), json!(reason))).unwrap();
        let usage = json!({"choices":[{"index":0,"delta":end,"finish_reason":reason}],"usage":{"total_tokens":9}});
        assert!(matches!(
            n.frame(&usage.to_string()).unwrap()[..],
            [AgentProviderEvent::Usage { .. }]
        ));
        assert!(matches!(
            n.frame("[DONE]").unwrap().last(),
            Some(AgentProviderEvent::Complete)
        ));
    }
    let mut n = AgentStreamNormalizer::new(ProviderKind::OpenRouter, true);
    n.frame(&chunk(json!({}), json!("stop"))).unwrap();
    assert!(n.frame(&chunk(json!({}), json!("length"))).is_err());
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
