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
    let request = HttpProvider
        .prepare_agent(&Default::default(), "fixture", &config(endpoint), &agent())
        .unwrap();
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
    let request = HttpProvider
        .prepare_agent(
            &Default::default(),
            "fixture",
            &config("http://127.0.0.1:1".into()),
            &agent,
        )
        .unwrap();
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
#[test]
fn native_protocols_and_duplicate_results_fail() {
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
    ] {
        let mut config = config("http://127.0.0.1:1".into());
        config.kind = kind;
        assert_eq!(
            HttpProvider
                .prepare_agent(&Default::default(), "fixture", &config, &agent())
                .unwrap_err()
                .code,
            "unsupported_provider_tools"
        );
    }
    let mut agent = agent();
    agent.continuation.push(AgentTurn {
        text: String::new(),
        calls: vec![ToolCall {
            id: "a".into(),
            name: "lookup".into(),
            arguments: json!({"key":"x"}),
        }],
        results: vec![ToolResult {
            call_id: "b".into(),
            content: "found".into(),
        }],
    });
    let config = config("http://127.0.0.1:1".into());
    assert!(
        HttpProvider
            .prepare_agent(&Default::default(), "fixture", &config, &agent)
            .is_err()
    );
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
        let request = HttpProvider
            .prepare_agent(&Default::default(), "fixture", &config(endpoint), &agent())
            .unwrap();
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
    let request = HttpProvider
        .prepare_agent(&Default::default(), "fixture", &config, &agent())
        .unwrap();
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
        let request = HttpProvider
            .prepare_agent(&Default::default(), "fixture", &config(endpoint), &agent())
            .unwrap();
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
        assert!(
            HttpProvider
                .prepare_agent(&Default::default(), "fixture", &config, &agent)
                .is_err()
        );
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
        let request = HttpProvider
            .prepare_agent(&Default::default(), "fixture", &config, &agent())
            .unwrap();
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
            }],
            results: vec![ToolResult {
                call_id: index.to_string(),
                content: "found".into(),
            }],
        })
        .collect();
    let config = config("http://127.0.0.1:1".into());
    let error = HttpProvider
        .prepare_agent(&Default::default(), "fixture", &config, &agent)
        .unwrap_err();
    assert_eq!(error.code, "invalid_provider_tool");
    assert_eq!(error.message, "The tool request exceeds its byte limit.");
}
