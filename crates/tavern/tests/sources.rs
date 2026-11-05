use async_trait::async_trait;
use rustytavern::{
    Result,
    card::Card,
    source::{
        Artifact, ArtifactFetcher, CardSource, ChubSource, Provenance, PublicFetcher,
        RisuRealmSource, public_address,
    },
};
use serde_json::json;
use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct ReplayFetcher {
    bytes: Vec<u8>,
    urls: Mutex<Vec<String>>,
}
#[async_trait]
impl ArtifactFetcher for ReplayFetcher {
    async fn fetch(&self, url: &str, _: CancellationToken) -> Result<Artifact> {
        self.urls.lock().unwrap().push(url.into());
        Ok(Artifact {
            bytes: self.bytes.clone(),
            provenance: Provenance {
                source: String::new(),
                requested_url: url.into(),
                final_url: url.into(),
                fetched_at_ms: 0,
                media_type: "application/octet-stream".into(),
                content_hash: rustytavern::digest(&self.bytes),
                creator: None,
                license: None,
            },
        })
    }
}
#[test]
fn public_source_policy_rejects_private_addresses_and_ungranted_origins() {
    for value in [
        "127.0.0.1",
        "10.0.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "fc00::1",
        "fe80::1",
        "2001:db8::1",
    ] {
        assert!(!public_address(value.parse::<IpAddr>().unwrap()), "{value}");
    }
    for value in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
        assert!(public_address(value.parse().unwrap()));
    }
    let fetcher = PublicFetcher::new(["public.example".into(), "127.0.0.1".into()]);
    for value in [
        "http://public.example/a",
        "https://public.example.evil/a",
        "https://user:pass@public.example/a",
        "https://public.example:444/a",
        "https://127.0.0.1/a",
        "file:///tmp/card.png",
    ] {
        assert_eq!(
            fetcher.check_url(value).unwrap_err().code,
            "source_target_denied"
        );
    }
    assert!(fetcher.check_url("https://public.example/a").is_ok());
}
#[test]
fn source_url_recognition_requires_an_exact_website_identity() {
    assert!(ChubSource::resolve("https://chub.ai/characters/author/character").is_ok());
    assert!(ChubSource::resolve("https://chub.ai/author/character").is_ok());
    assert!(
        ChubSource::resolve("https://chub.ai.evil.example/characters/author/character").is_err()
    );
    assert!(
        RisuRealmSource::resolve(
            "https://realm.risuai.net/character/3ca54c71-6efe-46a2-b9d0-4f62df23d712"
        )
        .is_ok()
    );
    assert!(
        RisuRealmSource::resolve(
            "https://realm.risuai.net.evil.example/character/3ca54c71-6efe-46a2-b9d0-4f62df23d712"
        )
        .is_err()
    );
}
#[tokio::test]
async fn chub_metadata_replay_uses_the_common_card_importer() {
    let metadata = json!({"node":{"definition":{"name":"Lumen","personality":"Lumen keeps a lighthouse.","tavern_personality":"Calm.","scenario":"A visitor arrives.","first_message":"Hello.","example_dialogs":"","extensions":{"sentinel":{"keep":7}}},"topics":["harbor"]}});
    let fetcher = Arc::new(ReplayFetcher {
        bytes: serde_json::to_vec(&metadata).unwrap(),
        urls: Mutex::new(Vec::new()),
    });
    let source = ChubSource::new(fetcher.clone());
    let url = "https://chub.ai/characters/author/lumen";
    let artifact = source.fetch(url, CancellationToken::new()).await.unwrap();
    let card = Card::import(&artifact.bytes).unwrap();
    assert_eq!(card.name(), "Lumen");
    assert_eq!(card.data()["extensions"]["sentinel"]["keep"], 7);
    assert_eq!(artifact.provenance.creator.as_deref(), Some("author"));
    let hash = rustytavern::digest(&artifact.bytes);
    assert_eq!(artifact.provenance.content_hash, hash);
    let endpoint = "https://api.chub.ai/api/characters/author/lumen?full=true";
    assert_eq!(fetcher.urls.lock().unwrap().as_slice(), [endpoint]);
}
#[tokio::test]
async fn risurealm_png_replay_preserves_original_download_bytes() {
    let bytes = include_bytes!("fixtures/synthetic/dual.png").to_vec();
    let fetcher = Arc::new(ReplayFetcher {
        bytes: bytes.clone(),
        urls: Mutex::new(Vec::new()),
    });
    let source = RisuRealmSource::new(fetcher.clone());
    let url = "https://realm.risuai.net/character/3ca54c71-6efe-46a2-b9d0-4f62df23d712";
    let artifact = source.fetch(url, CancellationToken::new()).await.unwrap();
    let card = Card::import(&artifact.bytes).unwrap();
    assert_eq!(card.export_original(), bytes);
    let p = &artifact.provenance;
    assert_eq!(p.requested_url, url);
    assert_eq!(p.source, "risurealm");
    assert_eq!(p.creator.as_deref(), Some("RustyTavern synthetic fixture"));
    assert!(p.license.is_none());
}
