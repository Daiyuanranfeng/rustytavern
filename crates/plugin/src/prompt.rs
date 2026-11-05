use async_trait::async_trait;
use rustytavern_conversation::prompt::PromptPlan;
pub use rustytavern_conversation::prompt::phases::*;
use rustytavern_core::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub struct HookInput<'a> {
    pub snapshot: &'a Value,
    pub state: &'a Value,
}
pub struct TransformInput<'a> {
    pub input: HookInput<'a>,
    pub enrichment: &'a Value,
    pub plan: &'a PromptPlan,
}

/// A hook obtains granted input and returns explicit prompt edits.
#[async_trait]
pub trait PromptHook: Send + Sync {
    fn policy(&self) -> HookPolicy {
        HookPolicy::default()
    }
    async fn enrich(
        &self,
        _input: &HookInput<'_>,
        _cancellation: CancellationToken,
    ) -> Result<EnrichmentOutput> {
        Ok(EnrichmentOutput::default())
    }
    fn transform(&self, _input: &TransformInput<'_>) -> Result<Vec<SectionEdit>> {
        Ok(Vec::new())
    }
}
