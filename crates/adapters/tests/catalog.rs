use rustytavern_adapters::provider::{
    CredentialResolver, ProviderConfig, ProviderKind,
    catalog::{CatalogClient, CatalogFilter, CatalogLimits},
};
use rustytavern_core::{Error, Result};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

struct Credentials;
impl CredentialResolver for Credentials {
    fn resolve(&self, reference: &str) -> Result<String> {
        assert_eq!(reference, "test-reference");
        Ok("test-secret".into())
    }
}
fn config(kind: ProviderKind) -> ProviderConfig {
    ProviderConfig {
        kind,
        api_key_env: Some("test-reference".into()),
        timeout_seconds: 2,
        ..Default::default()
    }
}
async fn server(responses: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/models", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let capture = requests.clone();
    tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let size = socket.read(&mut request).await.unwrap();
            capture
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request[..size]).into_owned());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, requests)
}
fn response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
async fn fetch(
    kind: ProviderKind,
    body: &str,
) -> Result<rustytavern_adapters::provider::catalog::ModelCatalog> {
    let (url, _) = server(vec![response(body)]).await;
    CatalogClient::default()
        .fetch(
            &config(kind),
            Some(&url),
            &Credentials,
            CancellationToken::new(),
        )
        .await
}

#[tokio::test]
async fn openrouter_maps_advertisements_and_preserves_extensions() {
    let body = r#"{"future_page":3,"data":[{"id":"vendor/model","name":"Model","architecture":{"input_modalities":["text","future-input"],"output_modalities":["image"]},"supported_parameters":["tools","future-control"],"context_length":123,"top_provider":{"max_completion_tokens":12,"future":true},"pricing":{"prompt":"0.001","future":"9"},"extension":{"value":42}},{"id":"unknown"}]}"#;
    let catalog = fetch(ProviderKind::OpenRouter, body).await.unwrap();
    let model = &catalog.models[0];
    assert_eq!(model.advertised.context_length, Some(123));
    assert_eq!(model.advertised.output_token_limit, Some(12));
    assert_eq!(
        model.advertised.input_modalities.as_ref().unwrap()[1],
        "future-input"
    );
    assert_eq!(model.raw["extension"]["value"], 42);
    assert_eq!(catalog.raw_pages[0]["future_page"], 3);
    assert!(catalog.models[1].advertised.input_modalities.is_none());
    let filter = CatalogFilter {
        supported_parameter: Some("tools".into()),
        ..Default::default()
    };
    assert_eq!(catalog.filter(&filter).count(), 1);
    assert!(catalog.provenance.endpoint.starts_with("http://127.0.0.1:"));
}

#[tokio::test]
async fn native_catalogs_preserve_unknown_capabilities() {
    let catalog = fetch(
        ProviderKind::OpenAi,
        r#"{"data":[{"id":"gpt-test","owned_by":"openai"}]}"#,
    )
    .await
    .unwrap();
    assert!(catalog.models[0].advertised.supported_parameters.is_none());
    let catalog = fetch(ProviderKind::Anthropic, r#"{"data":[{"id":"claude-test","display_name":"Claude","capabilities":{"future":true}}],"has_more":false}"#).await.unwrap();
    assert_eq!(
        catalog.models[0]
            .advertised
            .native_capabilities
            .as_ref()
            .unwrap()["future"],
        true
    );
    assert!(catalog.models[0].advertised.input_modalities.is_none());
}

#[tokio::test]
async fn pagination_uses_same_endpoint_and_protocol_auth() {
    for (kind, first, second, cursor, header) in [
        (
            ProviderKind::Anthropic,
            r#"{"data":[{"id":"one"}],"has_more":true,"last_id":"one"}"#,
            r#"{"data":[{"id":"two"}],"has_more":false}"#,
            "after_id=one",
            "x-api-key: test-secret",
        ),
        (
            ProviderKind::Gemini,
            r#"{"models":[{"name":"models/one","inputTokenLimit":10,"supportedGenerationMethods":["generateContent"]}],"nextPageToken":"next?&"}"#,
            r#"{"models":[{"name":"models/two"}]}"#,
            "pageToken=next%3F%26",
            "x-goog-api-key: test-secret",
        ),
    ] {
        let (url, requests) = server(vec![response(first), response(second)]).await;
        let catalog = CatalogClient::default()
            .fetch(
                &config(kind),
                Some(&url),
                &Credentials,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(catalog.models.len(), 2);
        let requests = requests.lock().unwrap();
        assert!(requests[1].contains(cursor));
        assert!(
            requests
                .iter()
                .all(|r| r.contains(header) && !r.contains("authorization:"))
        );
    }
}

#[tokio::test]
async fn malformed_catalogs_fail_without_remote_text() {
    let oversized = serde_json::json!({"data":[{"id":"x".repeat(1025)}]}).to_string();
    for body in [
        oversized.as_str(),
        r#"{"data":[{"id":"line\nbreak"}]}"#,
        r#"{"data":[{"id":42}]}"#,
        r#"{"data":[{"id":"a","supported_parameters":[7]}]}"#,
        r#"{"data":[{"id":"a"},{"id":"a"}]}"#,
        "remote-secret",
    ] {
        let error = fetch(ProviderKind::OpenRouter, body).await.unwrap_err();
        assert_eq!(error.code, "invalid_model_catalog");
        assert!(!format!("{error:?}").contains("remote-secret"));
    }
}

#[tokio::test]
async fn byte_model_and_page_limits_fail() {
    for limits in [
        CatalogLimits {
            max_bytes: 4,
            ..Default::default()
        },
        CatalogLimits {
            max_models: 1,
            ..Default::default()
        },
    ] {
        let (url, _) = server(vec![response(r#"{"data":[{"id":"one"},{"id":"two"}]}"#)]).await;
        let error = CatalogClient { limits }
            .fetch(
                &config(ProviderKind::OpenRouter),
                Some(&url),
                &Credentials,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "catalog_limit");
    }
    let (url, _) = server(vec![response(
        r#"{"models":[{"name":"models/one"}],"nextPageToken":"next"}"#,
    )])
    .await;
    let error = CatalogClient {
        limits: CatalogLimits {
            max_pages: 1,
            ..Default::default()
        },
    }
    .fetch(
        &config(ProviderKind::Gemini),
        Some(&url),
        &Credentials,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "catalog_limit");
}

#[tokio::test]
async fn redirects_do_not_send_credentials_to_another_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}/leak", listener.local_addr().unwrap());
    let (url, requests) = server(vec![format!(
        "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n"
    )])
    .await;
    let error = CatalogClient::default()
        .fetch(
            &config(ProviderKind::OpenRouter),
            Some(&url),
            &Credentials,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "provider_http");
    assert!(requests.lock().unwrap()[0].contains("authorization: Bearer test-secret"));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn timeout_and_cancellation_cover_headers_and_body() {
    for body_started in [false, true] {
        for cancel in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/models", listener.local_addr().unwrap());
            let cancellation = CancellationToken::new();
            let signal = cancellation.clone();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 8192];
                let received = socket.read(&mut buffer).await.unwrap();
                assert!(received > 0);
                if body_started {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                        .await
                        .unwrap();
                }
                if cancel {
                    signal.cancel();
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            });
            let mut config = config(ProviderKind::OpenRouter);
            config.timeout_seconds = 1;
            let error = CatalogClient::default()
                .fetch(&config, Some(&url), &Credentials, cancellation)
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                if cancel {
                    "cancelled"
                } else {
                    "provider_timeout"
                }
            );
        }
    }
}

#[tokio::test]
async fn endpoint_validation_precedes_credential_resolution() {
    struct NoCredentials;
    impl CredentialResolver for NoCredentials {
        fn resolve(&self, _: &str) -> Result<String> {
            Err(Error::new("unexpected", "The resolver must not run."))
        }
    }
    for endpoint in [
        "ftp://example.com/models",
        "https://user:pass@example.com/models",
        "https://example.com/models?api_key=secret",
        "https://example.com/models#fragment",
        "http://example.com/models",
    ] {
        let error = CatalogClient::default()
            .fetch(
                &config(ProviderKind::OpenAi),
                Some(endpoint),
                &NoCredentials,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "configuration");
    }
    let (url, requests) = server(vec![response(r#"{"data":[]}"#)]).await;
    let config = ProviderConfig {
        api_key_env: None,
        endpoint: Some("https://generation.invalid/messages".into()),
        ..Default::default()
    };
    CatalogClient::default()
        .fetch(
            &config,
            Some(&url),
            &NoCredentials,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!requests.lock().unwrap()[0].contains("authorization"));
}

#[tokio::test]
async fn streamed_byte_limits_and_pagination_cycles_fail() {
    let wire = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n";
    let (url, _) = server(vec![wire.into()]).await;
    let error = CatalogClient {
        limits: CatalogLimits {
            max_bytes: 8,
            ..Default::default()
        },
    }
    .fetch(
        &config(ProviderKind::OpenRouter),
        Some(&url),
        &Credentials,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "catalog_limit");
    let (url, _) = server(vec![
        response(r#"{"models":[{"name":"models/one"}],"nextPageToken":"repeat"}"#),
        response(r#"{"models":[{"name":"models/two"}],"nextPageToken":"repeat"}"#),
    ])
    .await;
    let error = CatalogClient::default()
        .fetch(
            &config(ProviderKind::Gemini),
            Some(&url),
            &Credentials,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid_model_catalog");
}
