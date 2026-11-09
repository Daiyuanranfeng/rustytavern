//! The catalog lists the models of a provider through Rig. It does not verify model behavior.
//! Rig lists the models. The Rig model catalog adds the modalities, prices, and limits that a listing omits.

use super::{CredentialResolver, PreparedRequest, ProviderConfig, ProviderKind, client};
use rig_core::{
    catalog::{Catalog, Modalities, ModelSpec},
    model::{ModelInfo, ModelList},
    providers::{anthropic::AnthropicConfig, gemini::GeminiConfig, openai, registry::ProviderId},
};
use rustytavern_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

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

/// A missing field means that neither the listing nor the Rig catalog supplies this information.
/// The prices are USD for one million tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvertisedCapabilities {
    pub input_modalities: Option<Vec<String>>,
    pub output_modalities: Option<Vec<String>>,
    pub supported_parameters: Option<Vec<String>>,
    pub context_length: Option<u64>,
    pub output_token_limit: Option<u64>,
    pub pricing: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    pub id: String,
    pub name: Option<String>,
    pub advertised: AdvertisedCapabilities,
    /// The Rig listing entry of the model.
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
}

/// The catalog vendor of each protocol. A custom endpoint has no vendor.
fn vendor(kind: ProviderKind) -> Option<ProviderId> {
    ProviderId::catalog(match kind {
        ProviderKind::OpenRouter => "openrouter",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Gemini => "gcp.gemini",
        ProviderKind::Custom => return None,
    })
}

/// The function returns the names of the modalities.
fn names(modalities: Modalities) -> Vec<String> {
    [
        ("text", modalities.text),
        ("image", modalities.image),
        ("audio", modalities.audio),
        ("video", modalities.video),
        ("file", modalities.pdf),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(name, _)| name.to_owned())
    .collect()
}

/// The function finds the Rig catalog entry of a model in the vendor of the protocol.
/// A custom endpoint has no vendor, so it matches the model ID in any vendor.
pub(super) fn spec<'a>(
    catalog: &'a Catalog,
    kind: ProviderKind,
    id: &str,
) -> Option<&'a ModelSpec> {
    let id = id.strip_prefix("models/").unwrap_or(id);
    if kind != ProviderKind::Custom {
        let vendor = vendor(kind)?;
        return catalog.get(vendor, id).map(|resolved| resolved.spec);
    }
    let last = |id: &str| id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
    catalog.iter().find(|spec| last(&spec.id) == last(id))
}

/// The function joins the listing entry and the Rig catalog entry of one model.
fn metadata(info: ModelInfo, spec: Option<&ModelSpec>) -> Result<ModelMetadata> {
    let modalities = spec.map(|spec| names(spec.input));
    let parameters = spec.map(|spec| {
        let mut parameters = vec!["temperature".to_owned()];
        if spec.tools {
            parameters.push("tools".into());
        }
        if spec.reasoning != Default::default() {
            parameters.push("reasoning".into());
        }
        parameters
    });
    let limit = |listed: Option<u32>, cataloged: Option<u32>| listed.or(cataloged).map(u64::from);
    Ok(ModelMetadata {
        advertised: AdvertisedCapabilities {
            input_modalities: modalities,
            output_modalities: spec.and_then(|spec| spec.details.output).map(names),
            supported_parameters: parameters,
            context_length: limit(info.context_length, spec.and_then(|s| s.context_window)),
            output_token_limit: limit(
                info.max_output_tokens,
                spec.and_then(|s| s.max_output_tokens),
            ),
            pricing: spec
                .and_then(|spec| spec.pricing.as_ref())
                .map(|price| json!({"input": price.input, "output": price.output})),
        },
        raw: serde_json::to_value(&info)?,
        name: info.name,
        id: info.id,
    })
}

/// The function lists the models of a provider through Rig.
/// An endpoint override names the catalog URL. A custom provider requires it.
/// An OpenRouter key also passes the Rig key check, because the OpenRouter catalog is public.
pub async fn fetch(
    config: &ProviderConfig,
    endpoint: Option<&str>,
    credentials: &dyn CredentialResolver,
    cancellation: CancellationToken,
) -> Result<ModelCatalog> {
    if config.kind == ProviderKind::Custom && endpoint.is_none() {
        return Err(Error::new(
            "configuration",
            "A custom catalog requires an explicit endpoint.",
        ));
    }
    let mut config = config.clone();
    if let Some(endpoint) = endpoint {
        config.endpoint = Some(endpoint.trim_end_matches("/models").to_owned());
    }
    let request = super::request::prepare(&Default::default(), "catalog", &config)?;
    let work = list(&request, credentials);
    let list = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the catalog request.")),
        result = tokio::time::timeout(Duration::from_secs(config.timeout_seconds), work) => {
            result.map_err(|_| Error::new("provider_timeout", "The catalog request exceeded its time limit."))??
        }
    };
    let catalog = Catalog::builtin();
    let models = list
        .data
        .into_iter()
        .map(|info| {
            let spec = spec(catalog, config.kind, &info.id);
            metadata(info, spec)
        })
        .collect::<Result<_>>()?;
    Ok(ModelCatalog {
        provenance: CatalogProvenance {
            protocol: config.kind,
            endpoint: client::base_url(&request),
            fetched_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            evidence: CatalogEvidence::AdvertisedUnverified,
        },
        models,
    })
}

/// The function sends the Rig listing request of the protocol.
async fn list(
    request: &PreparedRequest,
    credentials: &dyn CredentialResolver,
) -> Result<ModelList> {
    let (key, http, base) = client::transport(request, credentials)?;
    let error = |error| client::map_error(rig_agent::completion::PromptError::Provider(error));
    match request.protocol {
        ProviderKind::OpenRouter | ProviderKind::Custom | ProviderKind::OpenAi => {
            let dialect = match request.protocol {
                ProviderKind::OpenRouter => &openai::wire::OPENROUTER,
                ProviderKind::Custom => &client::CUSTOM,
                _ => &openai::wire::OPENAI,
            };
            let client = openai::OpenAIConfig::with_key(dialect, key)
                .with_base_url(base)
                .connect(http);
            if request.protocol == ProviderKind::OpenRouter
                && request.credential_reference.is_some()
            {
                client.verify().await.map_err(error)?;
            }
            client.list_models().await.map_err(error)
        }
        ProviderKind::Anthropic => AnthropicConfig::new(key)
            .with_base_url(base)
            .connect(http)
            .list_models()
            .await
            .map_err(error),
        ProviderKind::Gemini => GeminiConfig::new(key)
            .with_base_url(base)
            .connect(http)
            .list_models()
            .await
            .map_err(error),
    }
}
