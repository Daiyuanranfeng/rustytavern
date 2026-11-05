use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use rustytavern_card::{Card, MAX_INPUT_BYTES};
use rustytavern_core::{Error, Result, digest, now_ms};

pub use rustytavern_core::{Artifact, Provenance};

#[async_trait]
pub trait ArtifactFetcher: Send + Sync {
    async fn fetch(&self, url: &str, cancellation: CancellationToken) -> Result<Artifact>;
}

pub struct PublicFetcher {
    grants: BTreeSet<String>,
    pub max_bytes: usize,
    pub timeout: Duration,
    pub max_redirects: usize,
}
impl PublicFetcher {
    pub fn new(hosts: impl IntoIterator<Item = String>) -> Self {
        Self {
            grants: hosts
                .into_iter()
                .map(|host| format!("network:https:{host}"))
                .collect(),
            max_bytes: MAX_INPUT_BYTES,
            timeout: Duration::from_secs(30),
            max_redirects: 3,
        }
    }
    pub fn check_url(&self, value: &str) -> Result<Url> {
        let url =
            Url::parse(value).map_err(Error::context("source_target_denied", "URL is invalid."))?;
        let host = url
            .host_str()
            .ok_or_else(|| Error::new("source_target_denied", "The card URL has no host."))?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.port().is_some_and(|p| p != 443)
            || !self.grants.contains(&format!("network:https:{host}"))
        {
            return Err(Error::new(
                "source_target_denied",
                "The source has no grant for this HTTPS target.",
            ));
        }
        if let Ok(address) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            if !public_address(address) {
                return Err(Error::new(
                    "source_target_denied",
                    "The card source resolves to a restricted address.",
                ));
            }
        }
        Ok(url)
    }
    pub async fn fetch(&self, value: &str, cancellation: CancellationToken) -> Result<Artifact> {
        let operation = self.fetch_inner(value, &cancellation);
        tokio::select! {biased;_=cancellation.cancelled()=>Err(Error::new("cancelled","The caller cancelled the source request.")),result=tokio::time::timeout(self.timeout,operation)=>result.map_err(Error::context("source_timeout", "The card download timed out."))?}
    }
    async fn fetch_inner(&self, value: &str, cancellation: &CancellationToken) -> Result<Artifact> {
        let mut url = self.check_url(value)?;
        for redirect in 0..=self.max_redirects {
            let host = url
                .host_str()
                .ok_or_else(|| Error::new("source_target_denied", "The source URL has no host."))?;
            let addresses = tokio::net::lookup_host((host, 443))
                .await
                .map_err(Error::context("source_network", "Source failed."))?
                .collect::<Vec<SocketAddr>>();
            if addresses.is_empty() || addresses.iter().any(|a| !public_address(a.ip())) {
                return Err(Error::new(
                    "source_target_denied",
                    "The source host has a restricted network address.",
                ));
            }
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .resolve_to_addrs(host, &addresses)
                .timeout(self.timeout)
                .build()
                .map_err(Error::context("source_network", "Source failed."))?;
            let response = client
                .get(url.clone())
                .header("User-Agent", "RustyTavern/0.1")
                .send()
                .await
                .map_err(Error::context("source_network", "Source failed."))?;
            if response.status().is_redirection() {
                if redirect == self.max_redirects {
                    return Err(Error::new(
                        "source_redirect_limit",
                        "The source exceeds the redirect limit.",
                    ));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|s| s.to_str().ok())
                    .ok_or_else(|| {
                        Error::new("source_network", "The source redirect has no location.")
                    })?;
                let target = url
                    .join(location)
                    .map_err(Error::context("source_target_denied", "URL is invalid."))?;
                url = self.check_url(target.as_str())?;
                continue;
            }
            if matches!(response.status().as_u16(), 401 | 403) {
                return Err(Error::new(
                    "source_auth_required",
                    "The source requires access that this adapter does not provide.",
                ));
            }
            if !response.status().is_success() {
                return Err(Error::new(
                    "source_http",
                    format!("The source returned HTTP {}.", response.status().as_u16()),
                ));
            }
            if response
                .content_length()
                .is_some_and(|n| n > self.max_bytes as u64)
            {
                return Err(Error::new(
                    "source_size_limit",
                    "The source artifact exceeds its byte limit.",
                ));
            }
            let media_type = response
                .headers()
                .get("content-type")
                .and_then(|s| s.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_owned();
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = tokio::select! {biased;_=cancellation.cancelled()=>return Err(Error::new("cancelled","The caller cancelled the source request.")),chunk=stream.next()=>chunk}
            {
                let chunk = chunk.map_err(Error::context("source_network", "Source failed."))?;
                if bytes.len().saturating_add(chunk.len()) > self.max_bytes {
                    return Err(Error::new(
                        "source_size_limit",
                        "The source artifact exceeds its byte limit.",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(Artifact {
                provenance: Provenance {
                    source: String::new(),
                    requested_url: value.to_owned(),
                    final_url: url.to_string(),
                    fetched_at_ms: now_ms(),
                    media_type,
                    content_hash: digest(&bytes),
                    creator: None,
                    license: None,
                },
                bytes,
            });
        }
        Err(Error::new(
            "source_redirect_limit",
            "The source exceeds the redirect limit.",
        ))
    }
}

#[async_trait]
impl ArtifactFetcher for PublicFetcher {
    async fn fetch(&self, url: &str, cancellation: CancellationToken) -> Result<Artifact> {
        PublicFetcher::fetch(self, url, cancellation).await
    }
}

pub fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_multicast()
                && !ip.is_unspecified()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0)
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 192 && b == 0 && c == 2)
                && !(a == 198 && b == 51 && c == 100)
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_address(IpAddr::V4(v4));
            }
            let parts = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (parts[0] & 0xe000) != 0x2000
                || parts[0] == 0x2001 && parts[1] == 0x0db8)
        }
    }
}

#[async_trait]
pub trait CardSource: Send + Sync {
    fn id(&self) -> &str;
    fn recognize(&self, url: &str) -> bool;
    async fn fetch(&self, url: &str, cancellation: CancellationToken) -> Result<Artifact>;
}

pub struct ChubSource {
    fetcher: Arc<dyn ArtifactFetcher>,
}
impl ChubSource {
    pub fn new(fetcher: Arc<dyn ArtifactFetcher>) -> Self {
        Self { fetcher }
    }
    pub fn resolve(url: &str) -> Result<(String, String, String)> {
        let url =
            Url::parse(url).map_err(Error::context("invalid_source_url", "URL is invalid."))?;
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("chub.ai" | "www.chub.ai" | "characterhub.org" | "www.characterhub.org")
            )
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::new(
                "invalid_source_url",
                "The URL does not identify a supported Chub character.",
            ));
        }
        let mut parts = url
            .path_segments()
            .ok_or_else(|| Error::new("invalid_source_url", "The Chub URL has no path."))?
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if parts.first() == Some(&"characters") {
            parts.remove(0);
        }
        if parts.len() != 2
            || parts.iter().any(|p| {
                !p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        {
            return Err(Error::new(
                "invalid_source_url",
                "The Chub URL requires a creator and character name.",
            ));
        }
        Ok((
            format!(
                "https://api.chub.ai/api/characters/{}/{}?full=true",
                parts[0], parts[1]
            ),
            parts[0].into(),
            parts[1].into(),
        ))
    }
    pub fn decode_metadata(metadata: &Value, creator: &str) -> Result<Vec<u8>> {
        let definition = &metadata["node"]["definition"];
        if !definition.is_object() {
            return Err(Error::new(
                "invalid_source_schema",
                "The Chub response has no character definition.",
            ));
        }
        let text = |field: &str| definition[field].as_str().unwrap_or_default();
        let mut data = json!({"name":text("name"),"description":text("personality"),"personality":text("tavern_personality"),"scenario":text("scenario"),"first_mes":text("first_message"),"mes_example":text("example_dialogs"),"creator_notes":text("description"),"system_prompt":text("system_prompt"),"post_history_instructions":text("post_history_instructions"),"creator":creator,"character_version":"","alternate_greetings":definition["alternate_greetings"].as_array().cloned().unwrap_or_default(),"tags":metadata["node"]["topics"].as_array().cloned().unwrap_or_default(),"extensions":definition["extensions"].as_object().cloned().unwrap_or_default()});
        if definition["embedded_lorebook"].is_object() {
            data["character_book"] = definition["embedded_lorebook"].clone();
        }
        let bytes =
            serde_json::to_vec(&json!({"spec":"chara_card_v2","spec_version":"2.0","data":data}))?;
        Card::import(&bytes)?;
        Ok(bytes)
    }
}
#[async_trait]
impl CardSource for ChubSource {
    fn id(&self) -> &str {
        "chub"
    }
    fn recognize(&self, url: &str) -> bool {
        Self::resolve(url).is_ok()
    }
    async fn fetch(&self, url: &str, cancellation: CancellationToken) -> Result<Artifact> {
        let (endpoint, creator, _) = Self::resolve(url)?;
        let mut artifact = self.fetcher.fetch(&endpoint, cancellation).await?;
        let metadata: Value = serde_json::from_slice(&artifact.bytes)
            .map_err(Error::context("invalid_source_schema", "JSON is invalid."))?;
        artifact.bytes = Self::decode_metadata(&metadata, &creator)?;
        artifact.provenance.source = self.id().into();
        artifact.provenance.requested_url = url.into();
        artifact.provenance.creator = Some(creator);
        artifact.provenance.content_hash = digest(&artifact.bytes);
        artifact.provenance.media_type = "application/json".into();
        Ok(artifact)
    }
}

pub struct RisuRealmSource {
    fetcher: Arc<dyn ArtifactFetcher>,
}
impl RisuRealmSource {
    pub fn new(fetcher: Arc<dyn ArtifactFetcher>) -> Self {
        Self { fetcher }
    }
    pub fn resolve(url: &str) -> Result<String> {
        let url =
            Url::parse(url).map_err(Error::context("invalid_source_url", "URL is invalid."))?;
        let parts = url
            .path_segments()
            .ok_or_else(|| Error::new("invalid_source_url", "The RisuRealm URL has no path."))?
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if url.scheme() != "https"
            || url.host_str() != Some("realm.risuai.net")
            || !url.username().is_empty()
            || url.password().is_some()
            || parts.len() != 2
            || parts[0] != "character"
            || parts[1].len() < 24
            || parts[1].len() > 128
            || !parts[1].bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            return Err(Error::new(
                "invalid_source_url",
                "The URL does not identify a supported RisuRealm character.",
            ));
        }
        Ok(format!(
            "https://realm.risuai.net/api/v1/download/png-v3/{}?non_commercial=true",
            parts[1]
        ))
    }
}
#[async_trait]
impl CardSource for RisuRealmSource {
    fn id(&self) -> &str {
        "risurealm"
    }
    fn recognize(&self, url: &str) -> bool {
        Self::resolve(url).is_ok()
    }
    async fn fetch(&self, url: &str, cancellation: CancellationToken) -> Result<Artifact> {
        let endpoint = Self::resolve(url)?;
        let mut artifact = self.fetcher.fetch(&endpoint, cancellation).await?;
        let card = Card::import(&artifact.bytes)?;
        let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let data = card.data();
        let p = &mut artifact.provenance;
        p.source = self.id().into();
        p.requested_url = url.into();
        p.creator = p.creator.take().or(text(&data["creator"]));
        let license = text(&data["extensions"]["risuai"]["license"]);
        p.license = p.license.take().or(license);
        Ok(artifact)
    }
}
