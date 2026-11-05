//! This example replaces bundled decoder, driver, and provider services.

use async_trait::async_trait;
use rustytavern::{
    Error, Result,
    app::default_registry,
    card::{CardDecoder, CcDecoder, DecodedCard},
    driver::{ConversationDriver, DRIVER_CAPABILITIES, DriverContext},
    plugin::{PluginContext, PluginManifest, RuntimePlugin},
    prompt::{PromptOptions, PromptPlan},
    provider::{
        CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter, ProviderConfig,
        ProviderEvent, ProviderStream,
    },
    runtime::Runtime,
    session::{Persona, Session},
    store::{SqliteStore, StoreBackend},
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;

struct PrefixDecoder;
impl CardDecoder for PrefixDecoder {
    fn id(&self) -> &str {
        "prefix-json/1"
    }
    fn decode(&self, bytes: &[u8]) -> Result<DecodedCard> {
        let bytes = bytes
            .strip_prefix(b"RT:\n")
            .ok_or_else(|| Error::new("invalid_prefix", "The card prefix is missing."))?;
        let mut decoded = CcDecoder.decode(bytes)?;
        decoded.format = "prefixed-json".into();
        Ok(decoded)
    }
}
struct UppercaseDriver;
#[async_trait]
impl ConversationDriver for UppercaseDriver {
    fn id(&self) -> &str {
        "uppercase-reply/1"
    }
    async fn run(&self, context: &mut DriverContext) -> Result<()> {
        while let Some(event) = context.next_event().await? {
            match event {
                ProviderEvent::Text { text } => context.append_text(&text.to_uppercase()).await?,
                ProviderEvent::Complete => return Ok(()),
                ProviderEvent::Failure { error } => return Err(error),
                ProviderEvent::Usage { .. } => {}
            }
        }
        Ok(())
    }
}
struct FixtureProvider;
#[async_trait]
impl ProviderAdapter for FixtureProvider {
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        HttpProvider.prepare(plan, model, config)
    }
    async fn open_stream(
        &self,
        _: PreparedRequest,
        _: &dyn CredentialResolver,
        _: CancellationToken,
    ) -> Result<ProviderStream> {
        Ok(Box::pin(futures_util::stream::iter([
            Ok(ProviderEvent::Text {
                text: "hello, Alex".into(),
            }),
            Ok(ProviderEvent::Complete),
        ])))
    }
}
struct Features(PluginManifest);
#[async_trait]
impl RuntimePlugin for Features {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    async fn activate(&self, context: &mut PluginContext) -> Result<()> {
        for capability in DRIVER_CAPABILITIES {
            context.authorize(capability)?;
        }
        let decoder: Arc<dyn CardDecoder> = Arc::new(PrefixDecoder);
        let driver: Arc<dyn ConversationDriver> = Arc::new(UppercaseDriver);
        let provider: Arc<dyn ProviderAdapter> = Arc::new(FixtureProvider);
        context.register("card.decoder/1", Arc::new(decoder))?;
        context.register("driver/1", Arc::new(driver))?;
        context.register("provider/1", Arc::new(provider))
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    let store = Arc::new(SqliteStore::memory()?);
    let registry = default_registry(store.clone()).await?;
    let capabilities: std::collections::BTreeSet<_> =
        DRIVER_CAPABILITIES.into_iter().map(str::to_owned).collect();
    let plugin = Features(PluginManifest {
        id: "example.features".into(),
        provides: vec![
            "card.decoder/1".into(),
            "driver/1".into(),
            "provider/1".into(),
        ],
        requested_capabilities: capabilities.clone(),
        artifact_hash: rustytavern::digest(include_bytes!("feature_services.rs")),
        configuration_hash: rustytavern::digest(b"prefix and uppercase"),
        ..Default::default()
    });
    let bindings = ["card.decoder/1", "driver/1", "provider/1"]
        .into_iter()
        .map(|name| (name.into(), "example.features".into()))
        .collect();
    registry
        .lock()
        .await
        .inject_bound(
            vec![Arc::new(plugin)],
            "example",
            &BTreeMap::from([("example.features".into(), capabilities)]),
            bindings,
        )
        .await?;
    let runtime = Runtime::new(store.clone()).with_plugins(registry);
    let raw = [b"RT:\n".as_slice(), include_bytes!("../lumen.json")].concat();
    let card = runtime.import_card(raw.clone()).await?;
    let persona = Persona::new("Alex".into(), "A visitor.".into())?;
    store.save_persona(&persona)?;
    let target = Session {
        id: String::new(),
        character_id: card.id.clone(),
        persona_id: persona.id,
        worldbook_ids: Vec::new(),
        provider: "fixture".into(),
        model: "fixture".into(),
        head: None,
        version: 0,
        accept_reduced: false,
        options: PromptOptions::default(),
        composition: json!({}),
    };
    let session = runtime
        .create_session(&target, None, ProviderConfig::default())
        .await?;
    let generation = runtime
        .start(
            &session.id,
            "Hello.",
            None,
            runtime.provider_config(&session.id).await?,
        )
        .await?
        .wait()
        .await?;
    assert_eq!(generation.draft, "HELLO, ALEX");
    assert_eq!(card.export_original(), raw);
    println!(
        "{}",
        json!({"decoder":card.decoder.unwrap().id,"reply":generation.draft,"original_bytes_preserved":true,"paid_requests":0})
    );
    Ok(())
}
