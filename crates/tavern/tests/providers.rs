use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};

use futures_util::StreamExt;
use rustytavern::{
    card::Card,
    prompt::{ConservativeCounter, PromptInput, PromptOptions},
    provider::{
        CredentialResolver, HttpProvider, ProviderAdapter, ProviderConfig, ProviderEvent,
        ProviderKind, SseDecoder, StreamNormalizer,
    },
    session::{Persona, Role},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn plan() -> rustytavern::prompt::PromptPlan {
    let card = Card::import(include_bytes!("fixtures/synthetic/card-v3.json")).unwrap();
    let persona = Persona::new("Alex".into(), "Alex charts the coast.".into()).unwrap();
    rustytavern::prompt::compile(&PromptInput {
        card: &card,
        persona: &persona,
        history: &[],
        message: "Hello.",
        options: &PromptOptions::default(),
        accept_reduced: false,
        counter: &ConservativeCounter,
    })
    .unwrap()
}

#[test]
fn system_message_merge_preserves_text_roles_and_logical_sections() {
    let mut plan = plan();
    let prototype = plan.sections[0].clone();
    plan.sections = [
        (Role::System, " α "),
        (Role::System, "β\n"),
        (Role::User, "Question."),
        (Role::System, "After."),
        (Role::Assistant, "Reply."),
        (Role::System, "Last."),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (role, text))| rustytavern::prompt::Section {
        id: index.to_string(),
        role,
        text: text.into(),
        ..prototype.clone()
    })
    .collect();
    let original = serde_json::to_value(&plan).unwrap();
    for kind in [ProviderKind::Custom, ProviderKind::OpenRouter] {
        let config = ProviderConfig {
            kind,
            endpoint: Some("http://127.0.0.1:1".into()),
            merge_system_messages: true,
            ..Default::default()
        };
        let request = HttpProvider.prepare(&plan, "fixture", &config).unwrap();
        assert_eq!(
            request.body["messages"],
            json!([
                {"role":"system","content":" α \n\nβ\n"},
                {"role":"user","content":"Question."},
                {"role":"system","content":"After."},
                {"role":"assistant","content":"Reply."},
                {"role":"system","content":"Last."}
            ])
        );
        assert_eq!(request.transformations.len(), 1);
    }
    assert_eq!(serde_json::to_value(&plan).unwrap(), original);
    let request = HttpProvider
        .prepare(&plan, "fixture", &ProviderConfig::default())
        .unwrap();
    assert_eq!(request.body["messages"].as_array().unwrap().len(), 6);
}

#[test]
fn system_merge_defaults_preserve_legacy_configuration_and_protocol_guards() {
    let legacy: Value =
        serde_json::from_str(include_str!("fixtures/synthetic/provider-config-v1.json")).unwrap();
    let mut config: ProviderConfig = serde_json::from_value(legacy.clone()).unwrap();
    assert!(!config.merge_system_messages);
    assert_eq!(serde_json::to_value(&config).unwrap(), legacy);
    config.merge_system_messages = true;
    assert_eq!(
        serde_json::to_value(&config).unwrap()["merge_system_messages"],
        true
    );
    for kind in [
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
    ] {
        config.kind = kind;
        assert_eq!(
            HttpProvider
                .prepare(&plan(), "fixture", &config)
                .unwrap_err()
                .code,
            "unsupported_provider_parameter"
        );
    }
}

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

#[test]
fn sse_decoder_accepts_every_fragment_boundary_and_multiline_data() {
    let bytes =
        ": keepalive\r\ndata: {\"word\":\"港\"}\r\n\r\ndata: first\ndata: second\n\n".as_bytes();
    for fragments in (0..=bytes.len())
        .map(|split| vec![&bytes[..split], &bytes[split..]])
        .chain(std::iter::once(bytes.chunks(1).collect()))
    {
        let mut decoder = SseDecoder::default();
        let frames: Vec<_> = fragments
            .into_iter()
            .flat_map(|fragment| decoder.push(fragment).unwrap())
            .collect();
        assert_eq!(frames, ["{\"word\":\"港\"}", "first\nsecond"]);
    }
}

#[test]
fn openrouter_duplicate_finish_reason_does_not_complete_twice() {
    let mut decoder = StreamNormalizer::new(ProviderKind::OpenRouter);
    let first = decoder
        .frame(r#"{"choices":[{"delta":{"content":"Hi"},"finish_reason":"stop"}]}"#)
        .unwrap();
    assert!(matches!(&first[0],ProviderEvent::Text{text} if text=="Hi"));
    let usage=decoder.frame(r#"{"choices":[{"delta":{"content":""},"finish_reason":"stop"}],"usage":{"total_tokens":9}}"#).unwrap();
    assert!(matches!(&usage[0],ProviderEvent::Usage{usage} if usage["total_tokens"]==9));
    assert!(matches!(
        decoder.frame("[DONE]").unwrap()[0],
        ProviderEvent::Complete
    ));
    assert!(decoder.frame("[DONE]").unwrap().is_empty());
}

#[test]
fn vendor_requests_preserve_prompt_text_and_declare_transformations() {
    let plan = plan();
    for kind in [
        ProviderKind::OpenRouter,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
        ProviderKind::Custom,
    ] {
        let config = ProviderConfig {
            kind,
            endpoint: Some("https://example.com/generation".into()),
            ..Default::default()
        };
        let request = HttpProvider
            .prepare(&plan, "fixture-model", &config)
            .unwrap();
        let text = serde_json::to_string(&request.body).unwrap();
        assert!(text.contains("Lumen") && text.contains("Alex") && text.contains("Hello."));
        assert!(!text.contains("test-key"));
        assert_eq!(request.protocol, kind);
        match kind {
            ProviderKind::Anthropic | ProviderKind::Gemini => {
                assert!(!request.transformations.is_empty())
            }
            _ => assert!(request.transformations.is_empty()),
        }
    }
}

#[tokio::test]
async fn vendor_streams_and_failures_run_through_real_http() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/provider/http-streams.json")).unwrap();
    for case in cases {
        let (endpoint, received, task) = mock(
            case["body"].as_str().unwrap().to_owned(),
            case["status"].as_str().unwrap(),
        );
        let config = ProviderConfig {
            kind: serde_json::from_value(case["kind"].clone()).unwrap(),
            endpoint: Some(endpoint),
            ..Default::default()
        };
        let request = HttpProvider
            .prepare(&plan(), "fixture-model", &config)
            .unwrap();
        let events = match HttpProvider
            .open_stream(request, &TestCredentials, CancellationToken::new())
            .await
        {
            Ok(stream) => stream.collect::<Vec<_>>().await,
            Err(error) => vec![Err(error)],
        };
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                Ok(ProviderEvent::Text { text }) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let errors: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Err(error) | Ok(ProviderEvent::Failure { error }) => {
                    assert!(!error.message.contains("PRIVATE_SENTINEL"));
                    Some(error.code.as_str())
                }
                _ => None,
            })
            .collect();
        if let Some(expected) = case.get("usage") {
            assert!(events.iter().any(
                |event| matches!(event, Ok(ProviderEvent::Usage { usage }) if usage == expected)
            ));
            let last = events.last().unwrap();
            assert!(matches!(last, Ok(ProviderEvent::Failure { .. })));
        }
        assert_eq!(text, case["text"].as_str().unwrap(), "{}", case["id"]);
        assert_eq!(json!(errors), case["errors"], "{}", case["id"]);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Ok(ProviderEvent::Complete)))
                .count(),
            usize::from(errors.is_empty()),
            "{}",
            case["id"]
        );
        assert!(received.recv().unwrap().is_object());
        task.join().unwrap();
    }
}

#[test]
fn errors_keep_internal_causes_out_of_public_records() {
    use std::error::Error as _;
    let error = rustytavern::Error::from(std::io::Error::other("PRIVATE_SENTINEL"));
    assert_eq!(error.source().unwrap().to_string(), "PRIVATE_SENTINEL");
    let serialized = serde_json::to_string(&error).unwrap();
    assert!(!serialized.contains("PRIVATE_SENTINEL"));
    assert!(!error.to_string().contains("PRIVATE_SENTINEL"));
    let restored: rustytavern::Error = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored.code, "io");
    assert!(restored.source().is_none());
}

#[test]
fn adapter_capabilities_and_parameter_types_match_the_wire_contract() {
    for kind in [
        ProviderKind::OpenRouter,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
        ProviderKind::Custom,
    ] {
        let capabilities = kind.capabilities();
        assert_eq!(capabilities.input_types, ["text"]);
        assert!(!capabilities.tools && !capabilities.reasoning_controls);
        for (key, value) in [
            ("temperature", json!("warm")),
            ("temperature", json!(-0.1)),
            ("temperature", json!(2.1)),
            ("top_p", json!(true)),
            ("top_p", json!(1.1)),
            ("stop", json!([7])),
            ("stop", json!([])),
            ("stop", json!("")),
            ("tools", json!([])),
        ] {
            let config = ProviderConfig {
                kind,
                endpoint: Some("https://example.com/generate".into()),
                parameters: [(key.into(), value)].into(),
                ..Default::default()
            };
            let expected = if !capabilities.parameters.contains(&key) {
                "unsupported_provider_parameter"
            } else {
                "invalid_provider_parameter"
            };
            assert_eq!(
                HttpProvider
                    .prepare(&plan(), "fixture", &config)
                    .unwrap_err()
                    .code,
                expected
            );
        }
        let config = ProviderConfig {
            kind,
            endpoint: Some("https://example.com/generate".into()),
            parameters: [
                ("temperature".into(), json!(0.5)),
                ("top_p".into(), json!(0.9)),
            ]
            .into(),
            ..Default::default()
        };
        assert!(HttpProvider.prepare(&plan(), "fixture", &config).is_ok());
        if kind != ProviderKind::OpenAi {
            let config = ProviderConfig {
                parameters: [("stop".into(), json!("END"))].into(),
                ..config
            };
            let body = HttpProvider
                .prepare(&plan(), "fixture", &config)
                .unwrap()
                .body;
            match kind {
                ProviderKind::Gemini => {
                    assert_eq!(body["generationConfig"]["stopSequences"], json!(["END"]))
                }
                ProviderKind::Anthropic => assert_eq!(body["stop_sequences"], json!(["END"])),
                _ => assert_eq!(body["stop"], "END"),
            }
        }
    }
}

#[test]
fn endpoint_credentials_fail_before_a_request_can_be_saved() {
    for endpoint in [
        "https://user:password@example.com/chat",
        "https://example.com/chat?key=private-value",
        "https://example.com/chat?api_key=private-value",
        "https://example.com/chat?Access-Token=private-value",
        "https://example.com/chat#private-value",
    ] {
        let config = ProviderConfig {
            endpoint: Some(endpoint.into()),
            ..Default::default()
        };
        let error = HttpProvider
            .prepare(&plan(), "fixture", &config)
            .unwrap_err();
        assert_eq!(error.code, "configuration");
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("private-value")
        );
    }
    let config = ProviderConfig {
        kind: ProviderKind::Gemini,
        ..Default::default()
    };
    assert!(
        HttpProvider
            .prepare(&plan(), "fixture", &config)
            .unwrap()
            .endpoint
            .ends_with("?alt=sse")
    );
}

#[test]
fn openrouter_routing_is_explicit_and_preserves_requested_parameter_support() {
    use rustytavern::provider::OpenRouterRouting;
    let config = ProviderConfig::default();
    let request = HttpProvider.prepare(&plan(), "fixture", &config).unwrap();
    assert_eq!(
        request.body["provider"],
        json!({"allow_fallbacks":false,"require_parameters":true})
    );
    let config = ProviderConfig {
        routing: Some(OpenRouterRouting {
            order: vec!["openai".into()],
            ..Default::default()
        }),
        ..config
    };
    assert_eq!(
        HttpProvider
            .prepare(&plan(), "fixture", &config)
            .unwrap()
            .body["provider"]["order"],
        json!(["openai"])
    );
    let config = ProviderConfig {
        kind: ProviderKind::Anthropic,
        ..config
    };
    assert_eq!(
        HttpProvider
            .prepare(&plan(), "fixture", &config)
            .unwrap_err()
            .code,
        "unsupported_provider_parameter"
    );
}
