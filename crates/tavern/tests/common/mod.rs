//! The module scripts a Rig model for the runtime tests.
#![allow(dead_code)]

use rig_core::{
    driver::{Exchange, Model, Opened, Opening, Transport},
    error::ProviderError,
    test_utils::{MockFrame, MockScript, MockStreamEvent},
};
use rustytavern::{
    Result,
    prompt::PromptPlan,
    provider::{
        CompletionModel, CredentialResolver, HttpProvider, PreparedRequest, ProviderAdapter,
        ProviderConfig,
    },
};
use serde_json::Value;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

/// One step of a scripted reply.
#[derive(Clone)]
pub enum Step {
    Text(String),
    Delay(Duration),
    Fail(String),
    /// The reply ends without its final frame.
    Cut,
    /// The model calls a tool with an ID, a name, and arguments.
    Call(&'static str, &'static str, Value),
    /// The provider answers with an HTTP error status before any output.
    Status(u16),
    /// The reply ends with the finish reason.
    Finish(rig_core::completion::FinishReason),
}

pub fn text(text: &str) -> Step {
    Step::Text(text.into())
}

/// The provider answers request N with turn N. It answers later requests with its steps.
/// A held reply never ends. Otherwise the reply ends with the usage.
#[derive(Clone, Default)]
pub struct Script {
    pub turns: Vec<Vec<Step>>,
    pub steps: Vec<Step>,
    pub hold: bool,
    pub total_tokens: Option<u64>,
    pub calls: Arc<AtomicUsize>,
    pub requests: Arc<Mutex<Vec<Value>>>,
    /// The debug text of each Rig request that the model receives.
    pub sent: Arc<Mutex<Vec<String>>>,
    pub closed: Arc<AtomicBool>,
}

impl Script {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            ..Default::default()
        }
    }
    pub fn held(mut self) -> Self {
        self.hold = true;
        self
    }
    pub fn arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl ProviderAdapter for Script {
    fn prepare(
        &self,
        plan: &PromptPlan,
        model: &str,
        config: &ProviderConfig,
    ) -> Result<PreparedRequest> {
        HttpProvider.prepare(plan, model, config)
    }
    fn model(
        &self,
        request: &PreparedRequest,
        _: &dyn CredentialResolver,
    ) -> Result<CompletionModel> {
        self.requests.lock().unwrap().push(request.body.clone());
        Ok(Model::new(MockScript::default(), self.clone()).into())
    }
}

/// The guard records that the runtime dropped the reply.
struct Probe(Arc<AtomicBool>);
impl Drop for Probe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Transport<MockScript> for Script {
    fn send(
        &self,
        request: rig_core::completion::CompletionRequest,
        _: Exchange,
    ) -> Opening<MockFrame> {
        self.sent.lock().unwrap().push(format!("{request:?}"));
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut steps = self.turns.get(call).unwrap_or(&self.steps).clone();
        let probe = Probe(self.closed.clone());
        let (hold, total) = (self.hold, self.total_tokens.unwrap_or_default());
        steps.reverse();
        let frames = futures_util::stream::unfold(
            (steps, probe, false),
            move |(mut steps, probe, end)| async move {
                if end {
                    return None;
                }
                let frame = match steps.pop() {
                    Some(Step::Text(text)) => Ok(MockFrame::Event(MockStreamEvent::text(text))),
                    Some(Step::Delay(delay)) => {
                        tokio::time::sleep(delay).await;
                        return Some((
                            Ok(MockFrame::Event(MockStreamEvent::text(""))),
                            (steps, probe, false),
                        ));
                    }
                    Some(Step::Fail(message)) => {
                        return Some((Err(ProviderError::Provider(message)), (steps, probe, true)));
                    }
                    Some(Step::Cut) => return None,
                    Some(Step::Status(status)) => {
                        let status = http::StatusCode::from_u16(status).unwrap();
                        let error = ProviderError::from_http_response(status, "{}");
                        return Some((Err(error), (steps, probe, true)));
                    }
                    Some(Step::Call(id, name, arguments)) => Ok(MockFrame::Event(
                        MockStreamEvent::tool_call(id, name, arguments),
                    )),
                    Some(Step::Finish(reason)) => {
                        let mut end = rig_core::test_utils::mock_final_with_total_tokens(total);
                        end.reason = Some(reason);
                        let end = MockStreamEvent::FinalResponse(end);
                        return Some((Ok(MockFrame::Event(end)), (steps, probe, true)));
                    }
                    None if hold => std::future::pending().await,
                    None => {
                        let end = MockStreamEvent::final_response_with_total_tokens(total);
                        return Some((Ok(MockFrame::Event(end)), (steps, probe, true)));
                    }
                };
                Some((frame, (steps, probe, false)))
            },
        );
        Opening::ready(Opened::new(frames))
    }
}
