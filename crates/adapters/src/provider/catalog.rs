//! The catalog reports provider advertisements. It does not verify model behavior.

use super::{CredentialResolver, ProviderConfig, ProviderKind};
use rustytavern_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Debug, Clone)]
pub struct CatalogLimits {
    pub max_bytes: usize,
    pub max_models: usize,
    pub max_pages: usize,
}
impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_models: 20_000,
            max_pages: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogEvidence {
    AdvertisedUnverified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProvenance {
    pub protocol: ProviderKind,
    pub endpoint: String,
    pub fetched_at_unix_seconds: u64,
    pub evidence: CatalogEvidence,
}

/// A missing field means that the catalog does not supply this information.
/// The raw model retains unknown values and provider extensions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdvertisedCapabilities {
    pub input_modalities: Option<Vec<String>>,
    pub output_modalities: Option<Vec<String>>,
    pub supported_parameters: Option<Vec<String>>,
    pub generation_methods: Option<Vec<String>>,
    pub context_length: Option<u64>,
    pub output_token_limit: Option<u64>,
    pub pricing: Option<Value>,
    pub top_provider: Option<Value>,
    pub native_capabilities: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    pub id: String,
    pub name: Option<String>,
    pub advertised: AdvertisedCapabilities,
    pub raw: Value,
}

#[derive(Debug, Clone, Default)]
pub struct CatalogFilter {
    pub input_modality: Option<String>,
    pub output_modality: Option<String>,
    pub supported_parameter: Option<String>,
}
impl CatalogFilter {
    pub fn matches(&self, model: &ModelMetadata) -> bool {
        let caps = &model.advertised;
        [
            (&self.input_modality, &caps.input_modalities),
            (&self.output_modality, &caps.output_modalities),
            (&self.supported_parameter, &caps.supported_parameters),
        ]
        .into_iter()
        .all(|(query, values)| {
            query
                .as_ref()
                .is_none_or(|query| values.as_ref().is_some_and(|values| values.contains(query)))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub provenance: CatalogProvenance,
    pub models: Vec<ModelMetadata>,
    pub raw_pages: Vec<Value>,
}
impl ModelCatalog {
    pub fn filter<'a>(
        &'a self,
        filter: &'a CatalogFilter,
    ) -> impl Iterator<Item = &'a ModelMetadata> {
        self.models.iter().filter(|model| filter.matches(model))
    }
}

#[derive(Debug, Clone, Default)]
pub struct CatalogClient {
    pub limits: CatalogLimits,
}
impl CatalogClient {
    /// The endpoint override names a catalog URL. The generation endpoint is not used.
    pub async fn fetch(
        &self,
        config: &ProviderConfig,
        endpoint: Option<&str>,
        credentials: &dyn CredentialResolver,
        cancellation: CancellationToken,
    ) -> Result<ModelCatalog> {
        if config.timeout_seconds == 0
            || self.limits.max_bytes == 0
            || self.limits.max_models == 0
            || self.limits.max_pages == 0
        {
            return Err(Error::new(
                "configuration",
                "The catalog limits must be positive.",
            ));
        }
        let default = match config.kind {
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1/models?output_modalities=all",
            ProviderKind::OpenAi => "https://api.openai.com/v1/models",
            ProviderKind::Anthropic => "https://api.anthropic.com/v1/models",
            ProviderKind::Gemini => "https://generativelanguage.googleapis.com/v1beta/models",
            ProviderKind::Custom => endpoint.ok_or_else(|| {
                Error::new(
                    "configuration",
                    "A custom catalog requires an explicit endpoint.",
                )
            })?,
        };
        let endpoint = endpoint.unwrap_or(default);
        let url = validate_endpoint(endpoint)?;
        let work = self.fetch_pages(config, url, credentials);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(Error::new("cancelled", "The caller cancelled the catalog request.")),
            result = tokio::time::timeout(Duration::from_secs(config.timeout_seconds), work) => {
                result.map_err(|_| Error::new("provider_timeout", "The catalog request exceeded its time limit."))?
            }
        }
    }

    async fn fetch_pages(
        &self,
        config: &ProviderConfig,
        endpoint: Url,
        credentials: &dyn CredentialResolver,
    ) -> Result<ModelCatalog> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::new("provider_transport", "The catalog client failed."))?;
        let key = config
            .api_key_env
            .as_deref()
            .map(|reference| credentials.resolve(reference))
            .transpose()?;
        if key.is_none() && !matches!(config.kind, ProviderKind::OpenRouter | ProviderKind::Custom)
        {
            return Err(Error::new(
                "configuration",
                "The catalog requires a credential reference.",
            ));
        }
        let mut catalog = ModelCatalog {
            provenance: CatalogProvenance {
                protocol: config.kind,
                endpoint: endpoint.to_string(),
                fetched_at_unix_seconds: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| {
                        Error::new("configuration", "The system clock precedes the Unix epoch.")
                    })?
                    .as_secs(),
                evidence: CatalogEvidence::AdvertisedUnverified,
            },
            models: Vec::new(),
            raw_pages: Vec::new(),
        };
        let mut url = endpoint.clone();
        let mut bytes = 0usize;
        let mut cursors = HashSet::new();
        let mut ids = HashSet::new();
        loop {
            if catalog.raw_pages.len() >= self.limits.max_pages {
                return Err(limit_error());
            }
            let mut request = client.get(url.clone()).header("Accept", "application/json");
            if let Some(key) = &key {
                request = match config.kind {
                    ProviderKind::Anthropic => request.header("x-api-key", key),
                    ProviderKind::Gemini => request.header("x-goog-api-key", key),
                    _ => request.bearer_auth(key),
                };
            }
            if config.kind == ProviderKind::Anthropic {
                request = request.header("anthropic-version", "2023-06-01");
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| Error::new("provider_transport", "The catalog request failed."))?;
            if !response.status().is_success() {
                return Err(Error::new(
                    "provider_http",
                    format!("The catalog returned HTTP {}.", response.status().as_u16()),
                ));
            }
            if response
                .content_length()
                .is_some_and(|length| length > self.limits.max_bytes.saturating_sub(bytes) as u64)
            {
                return Err(limit_error());
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| Error::new("provider_transport", "The catalog body failed."))?
            {
                bytes = bytes.checked_add(chunk.len()).ok_or_else(limit_error)?;
                if bytes > self.limits.max_bytes {
                    return Err(limit_error());
                }
                body.extend_from_slice(&chunk);
            }
            let page: Value = serde_json::from_slice(&body).map_err(|_| invalid_error())?;
            let field = if config.kind == ProviderKind::Gemini {
                "models"
            } else {
                "data"
            };
            let models = page
                .get(field)
                .and_then(Value::as_array)
                .ok_or_else(invalid_error)?;
            if models.len() > self.limits.max_models.saturating_sub(catalog.models.len()) {
                return Err(limit_error());
            }
            for raw in models {
                let model = map_model(config.kind, raw)?;
                if !ids.insert(model.id.clone()) {
                    return Err(invalid_error());
                }
                catalog.models.push(model);
            }
            let next = match config.kind {
                ProviderKind::Anthropic => match page.get("has_more").and_then(Value::as_bool) {
                    Some(true) => Some((
                        "after_id",
                        page.get("last_id")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .ok_or_else(invalid_error)?
                            .to_owned(),
                    )),
                    Some(false) => None,
                    _ => return Err(invalid_error()),
                },
                ProviderKind::Gemini => match page.get("nextPageToken") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(token)) if token.is_empty() => None,
                    Some(Value::String(token)) => Some(("pageToken", token.clone())),
                    _ => return Err(invalid_error()),
                },
                _ => None,
            };
            catalog.raw_pages.push(page);
            match next {
                None => break,
                Some((parameter, cursor)) => {
                    if !cursors.insert(cursor.clone()) {
                        return Err(invalid_error());
                    }
                    url = endpoint.clone();
                    let pairs = url
                        .query_pairs()
                        .filter(|(name, _)| name != parameter)
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect::<Vec<_>>();
                    url.set_query(None);
                    url.query_pairs_mut()
                        .extend_pairs(pairs)
                        .append_pair(parameter, &cursor);
                }
            }
        }
        Ok(catalog)
    }
}

fn validate_endpoint(endpoint: &str) -> Result<Url> {
    let url = Url::parse(endpoint)
        .map_err(|_| Error::new("configuration", "The catalog endpoint is invalid."))?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query_pairs().any(|(name, _)| {
            matches!(
                name.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
                "key"
                    | "apikey"
                    | "token"
                    | "accesstoken"
                    | "authorization"
                    | "secret"
                    | "password"
                    | "signature"
                    | "credential"
            )
        })
    {
        return Err(Error::new(
            "configuration",
            "The catalog endpoint must be an HTTP URL without credentials or fragments.",
        ));
    }
    // The caller can use local HTTP servers for tests. Remote credentials require HTTPS.
    if url.scheme() == "http"
        && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    {
        return Err(Error::new(
            "configuration",
            "A remote catalog endpoint requires HTTPS.",
        ));
    }
    Ok(url)
}

fn map_model(kind: ProviderKind, raw: &Value) -> Result<ModelMetadata> {
    if !raw.is_object() {
        return Err(invalid_error());
    }
    let id_field = if kind == ProviderKind::Gemini {
        "name"
    } else {
        "id"
    };
    let id = raw[id_field].as_str().ok_or_else(invalid_error)?;
    if id.trim().is_empty() || id.len() > 1024 || id.chars().any(char::is_control) {
        return Err(invalid_error());
    }
    let mut advertised = AdvertisedCapabilities::default();
    let name = match kind {
        ProviderKind::OpenRouter | ProviderKind::Custom => {
            advertised.input_modalities = strings(&raw["architecture"], "input_modalities")?;
            advertised.output_modalities = strings(&raw["architecture"], "output_modalities")?;
            advertised.supported_parameters = strings(raw, "supported_parameters")?;
            advertised.context_length = integer(raw, "context_length")?;
            advertised.output_token_limit = integer(&raw["top_provider"], "max_completion_tokens")?;
            advertised.pricing = raw.get("pricing").filter(|v| !v.is_null()).cloned();
            advertised.top_provider = raw.get("top_provider").filter(|v| !v.is_null()).cloned();
            "name"
        }
        ProviderKind::Gemini => {
            advertised.context_length = integer(raw, "inputTokenLimit")?;
            advertised.output_token_limit = integer(raw, "outputTokenLimit")?;
            advertised.generation_methods = strings(raw, "supportedGenerationMethods")?;
            "displayName"
        }
        ProviderKind::Anthropic => {
            advertised.native_capabilities =
                raw.get("capabilities").filter(|v| !v.is_null()).cloned();
            "display_name"
        }
        ProviderKind::OpenAi => "name",
    };
    let name = match raw.get(name) {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.clone()),
        _ => return Err(invalid_error()),
    };
    Ok(ModelMetadata {
        id: id.to_owned(),
        name,
        advertised,
        raw: raw.clone(),
    })
}
fn strings(value: &Value, field: &str) -> Result<Option<Vec<String>>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid_error))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        _ => Err(invalid_error()),
    }
}
fn integer(value: &Value, field: &str) -> Result<Option<u64>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(invalid_error),
    }
}
fn invalid_error() -> Error {
    Error::new(
        "invalid_model_catalog",
        "The catalog response has an invalid structure.",
    )
}
fn limit_error() -> Error {
    Error::new("catalog_limit", "The catalog response exceeds its limit.")
}
