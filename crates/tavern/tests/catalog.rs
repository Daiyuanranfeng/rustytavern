use rustytavern::provider::{
    CredentialResolver, ProviderConfig, ProviderKind,
    catalog::{CatalogFilter, ModelCatalog, fetch},
};
use rustytavern_core::Result;
use std::sync::{Arc, Mutex};
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
/// The function lists one protocol from a local server and returns the catalog and the requests.
async fn list(kind: ProviderKind, responses: Vec<String>) -> (Result<ModelCatalog>, Vec<String>) {
    let (url, requests) = server(responses).await;
    let catalog = fetch(
        &config(kind),
        Some(&url),
        &Credentials,
        CancellationToken::new(),
    )
    .await;
    let requests = requests.lock().unwrap().clone();
    (catalog, requests)
}

#[tokio::test]
async fn openrouter_listing_checks_the_key_and_takes_facts_from_the_rig_catalog() {
    let models = r#"{"data":[{"id":"anthropic/claude-haiku-5.5","name":"Haiku","created":1700000000},{"id":"example/unknown"}]}"#;
    let key = r#"{"data":{"label":"test"}}"#;
    let (catalog, requests) = list(
        ProviderKind::OpenRouter,
        vec![response(key), response(models)],
    )
    .await;
    let catalog = catalog.unwrap();
    assert!(requests[0].contains("/key") && requests[1].contains("/models"));
    assert!(
        requests
            .iter()
            .all(|r| r.to_lowercase().contains("bearer test-secret"))
    );
    let haiku = &catalog.models[0].advertised;
    assert_eq!(haiku.context_length, Some(1_000_000));
    assert!(haiku.pricing.is_some() && haiku.input_modalities.is_some());
    let filter = CatalogFilter {
        supported_parameter: Some("tools".into()),
        ..Default::default()
    };
    let tools: Vec<_> = catalog
        .models
        .iter()
        .filter(|m| filter.matches(m))
        .collect();
    assert_eq!(tools.len(), 1);
    assert!(catalog.models[1].advertised.pricing.is_none());
}

#[tokio::test]
async fn native_listings_use_the_protocol_authentication() {
    for (kind, body, header) in [
        (
            ProviderKind::Anthropic,
            r#"{"data":[{"id":"claude-opus-4-5","display_name":"Opus","type":"model"}],"has_more":false}"#,
            "x-api-key: test-secret",
        ),
        (
            ProviderKind::Gemini,
            r#"{"models":[{"name":"models/gemini-2.5-flash","displayName":"Flash"}]}"#,
            "test-secret",
        ),
        (
            ProviderKind::OpenAi,
            r#"{"data":[{"id":"gpt-5.2","object":"model"}]}"#,
            "bearer test-secret",
        ),
    ] {
        let (catalog, requests) = list(kind, vec![response(body)]).await;
        let catalog = catalog.unwrap_or_else(|error| panic!("{kind:?}: {error:?}"));
        assert_eq!(catalog.models.len(), 1, "{kind:?}");
        assert!(
            requests[0].to_lowercase().contains(header),
            "{kind:?}: {}",
            requests[0]
        );
    }
}

#[tokio::test]
async fn failures_map_to_provider_codes_and_a_custom_catalog_needs_an_endpoint() {
    let denied = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
    let (catalog, _) = list(ProviderKind::OpenAi, vec![denied.into()]).await;
    assert_eq!(catalog.unwrap_err().code, "provider_auth");
    let custom = config(ProviderKind::Custom);
    let error = fetch(&custom, None, &Credentials, CancellationToken::new()).await;
    assert_eq!(error.unwrap_err().code, "configuration");
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (url, _) = server(vec![]).await;
    let error = fetch(
        &config(ProviderKind::OpenAi),
        Some(&url),
        &Credentials,
        cancellation,
    )
    .await;
    assert_eq!(error.unwrap_err().code, "cancelled");
}

#[tokio::test]
async fn a_custom_endpoint_gets_the_facts_of_a_known_model_and_output_modalities() {
    let body = r#"{"data":[{"id":"claude-opus-4-5"},{"id":"local/unknown-model"}]}"#;
    let (catalog, _) = list(ProviderKind::Custom, vec![response(body)]).await;
    let catalog = catalog.unwrap();
    let known = &catalog.models[0].advertised;
    assert!(known.context_length.is_some() && known.pricing.is_some());
    assert_eq!(
        known.output_modalities.as_deref(),
        Some(&["text".to_owned()][..])
    );
    let filter = CatalogFilter {
        output_modality: Some("text".into()),
        ..Default::default()
    };
    let text: Vec<_> = catalog
        .models
        .iter()
        .filter(|m| filter.matches(m))
        .collect();
    assert_eq!(text.len(), 1);
}

#[tokio::test]
async fn a_keyless_custom_endpoint_sends_no_key_and_vendors_never_borrow_facts() {
    let (url, requests) = server(vec![response(r#"{"data":[{"id":"local-model"}]}"#)]).await;
    let keyless = ProviderConfig {
        api_key_env: None,
        ..config(ProviderKind::Custom)
    };
    fetch(&keyless, Some(&url), &Credentials, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        !requests.lock().unwrap()[0]
            .to_lowercase()
            .contains("authorization")
    );
    // An OpenRouter ID from another vendor gets no facts from a similar catalog ID.
    let body = r#"{"data":[{"id":"someone/claude-opus-4-5"}]}"#;
    let (catalog, _) = list(
        ProviderKind::OpenRouter,
        vec![response("{}"), response(body)],
    )
    .await;
    assert!(catalog.unwrap().models[0].advertised.pricing.is_none());
}
