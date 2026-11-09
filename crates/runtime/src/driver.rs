use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use rig_agent::{agent::MultiTurnStreamItem, agent::StreamingResult};
use rig_core::streaming::{Item, StreamEvent};

use super::{
    EventSink, Runtime, RuntimeEvent,
    policy::{AgentPolicy, PolicyHook},
};
use crate::{
    Error, Result,
    provider::{self, PreparedRequest},
    session::Generation,
    store::now_ms,
};

pub const MAX_DRIVER_EVENTS: usize = 65_536;
pub const DRIVER_CAPABILITIES: [&str; 3] = [
    "session.read:current",
    "session.write:current",
    "provider.generate",
];

#[async_trait]
pub trait ConversationDriver: Send + Sync {
    fn id(&self) -> &str;
    async fn run(&self, context: &mut DriverContext) -> Result<()>;
}

/// One event of the agent run that a driver acts on.
/// The host runs the tools and records the usage itself.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Text {
        text: String,
    },
    /// The model streams its thinking text. The driver does not add it to the reply.
    Reasoning {
        text: String,
    },
    Complete,
}

type StreamSlot = Arc<Mutex<Option<StreamingResult>>>;
pub(super) struct DriverControl {
    cancellation: CancellationToken,
    stream: StreamSlot,
}
impl Drop for DriverControl {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Ok(mut stream) = self.stream.lock() {
            stream.take();
        }
    }
}

pub struct DriverContext {
    runtime: Runtime,
    generation: Generation,
    request: PreparedRequest,
    cancellation: CancellationToken,
    sink: EventSink,
    stream: StreamSlot,
    opened: bool,
    complete: bool,
    failure: Option<Error>,
    events: usize,
    /// The draft lengths at the start of the current and the previous model turn.
    /// Rig reports a rejected turn before its retry, so a reset returns to the previous start.
    turn_start: usize,
    previous_start: usize,
    draft: usize,
    policy: AgentPolicy,
    /// The run sends the provider web search tool. A failed first model call can drop it once.
    searching: bool,
    /// The run finished at least one model call.
    answered: bool,
    usage: Option<Value>,
    view: Value,
}
impl DriverContext {
    pub(super) fn new(
        runtime: Runtime,
        generation: Generation,
        request: PreparedRequest,
        cancellation: CancellationToken,
        sink: EventSink,
    ) -> (Self, DriverControl) {
        let cancellation = cancellation.child_token();
        let stream = Arc::new(Mutex::new(None));
        let control = DriverControl {
            cancellation: cancellation.clone(),
            stream: stream.clone(),
        };
        let view = crate::phases::conversation_snapshot(&generation.snapshot);
        (
            Self {
                runtime,
                generation,
                request,
                cancellation,
                sink,
                stream,
                opened: false,
                complete: false,
                failure: None,
                events: 0,
                turn_start: 0,
                previous_start: 0,
                draft: 0,
                policy: AgentPolicy::default(),
                searching: true,
                answered: false,
                usage: None,
                view,
            },
            control,
        )
    }
    pub fn snapshot(&self) -> &Value {
        &self.view
    }
    pub fn request(&self) -> &PreparedRequest {
        &self.request
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    fn active(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Error::new(
                "cancelled",
                "The caller cancelled the driver phase.",
            ));
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        Ok(())
    }
    /// The method starts the Rig agent run with the composed tools and the run policy.
    /// A number selects the step limit with the default policy.
    pub async fn open(&mut self, policy: impl Into<AgentPolicy>) -> Result<()> {
        let result = self.open_inner(policy.into()).await;
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    pub async fn next_event(&mut self) -> Result<Option<AgentEvent>> {
        let result = self.next_inner().await;
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    async fn open_inner(&mut self, policy: AgentPolicy) -> Result<()> {
        self.active()?;
        if self.opened {
            return Err(Error::new(
                "invalid_driver_command",
                "The driver can open the agent run only once.",
            ));
        }
        self.runtime.renew(&self.generation).await?;
        self.policy = policy;
        let stream = self.start(true)?;
        *self
            .stream
            .lock()
            .map_err(|_| Error::new("driver_state", "The driver stream is unavailable."))? =
            Some(stream);
        self.opened = true;
        Ok(())
    }
    /// The method builds the Rig agent run with or without the provider web search tool.
    fn start(&self, web_search: bool) -> Result<StreamingResult> {
        let policy = self.policy.clone();
        let model = self
            .runtime
            .provider
            .model(&self.request, self.runtime.credentials.as_ref())?;
        let tools = self
            .runtime
            .tools
            .iter()
            .map(|tool| {
                super::agent::bridge(
                    tool.clone(),
                    self.sink.clone(),
                    self.generation.id.clone(),
                    self.cancellation.clone(),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let turns = policy
            .max_steps
            .saturating_add(1)
            .saturating_add(policy.retry_truncated);
        let hook = PolicyHook {
            policy,
            sink: self.sink.clone(),
            generation_id: self.generation.id.clone(),
            approvals: self.sink.1.clone(),
            cancellation: self.cancellation.clone(),
        };
        let retry = hook.policy.retry_truncated;
        let runner = provider::agent(&self.request, model, tools, turns, retry, web_search)?;
        Ok(runner.add_hook(hook).stream())
    }
    async fn next_inner(&mut self) -> Result<Option<AgentEvent>> {
        self.active()?;
        if self.complete {
            return Ok(None);
        }
        if !self.opened {
            return Err(Error::new(
                "invalid_driver_command",
                "The driver cannot read events before it opens the run.",
            ));
        }
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| Error::new("driver_state", "The driver stream is unavailable."))?
            .take()
            .ok_or_else(|| Error::new("driver_state", "The driver stream is closed."))?;
        let event = loop {
            if self.events >= MAX_DRIVER_EVENTS {
                return Err(Error::new(
                    "output_limit",
                    "The driver phase exceeds its event limit.",
                ));
            }
            let item = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the driver phase.")),
                item = stream.next() => item.ok_or_else(|| Error::new("incomplete_stream", "The provider ended without a completion event."))?,
            };
            let item = match item.map_err(provider::map_error) {
                Ok(item) => item,
                // Some OpenRouter endpoints fail with the web search tool. The first model call
                // repeats once without it when it fails before any output.
                Err(error)
                    if self.searching
                        && !self.answered
                        && self.draft == 0
                        && provider::search_fallback(&self.request, &error) =>
                {
                    self.searching = false;
                    stream = self.start(false)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            match item {
                MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                    text,
                    ..
                })) if !text.is_empty() => break AgentEvent::Text { text },
                MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Reasoning {
                    text,
                    ..
                })) if !text.is_empty() => break AgentEvent::Reasoning { text },
                MultiTurnStreamItem::CompletionCall(call) => {
                    self.answered = true;
                    self.record_usage(serde_json::to_value(call.usage)?).await?;
                    self.runtime.renew(&self.generation).await?;
                    self.previous_start = std::mem::replace(&mut self.turn_start, self.draft);
                }
                // Rig repeats a truncated turn. The text of that turn was provisional.
                MultiTurnStreamItem::ModelTurnRetried { .. } => self.reset_turn().await?,
                MultiTurnStreamItem::FinalResponse(_) => {
                    self.complete = true;
                    break AgentEvent::Complete;
                }
                _ => {}
            }
        };
        // The limit counts the text and thinking deltas that reach the driver.
        if !matches!(event, AgentEvent::Complete) {
            self.events += 1;
        }
        if !self.complete {
            *self
                .stream
                .lock()
                .map_err(|_| Error::new("driver_state", "The driver stream is unavailable."))? =
                Some(stream);
        }
        Ok(Some(event))
    }
    pub async fn append_text(&mut self, text: &str) -> Result<()> {
        self.active()?;
        if !self.opened {
            return Err(Error::new(
                "invalid_driver_command",
                "The driver cannot append text before it opens the run.",
            ));
        }
        let id = self.generation.id.clone();
        let token = self.generation.fencing_token;
        let text = text.to_owned();
        let draft = text.clone();
        let result = self
            .runtime
            .store_call(move |store| store.append_draft(&id, token, &draft, now_ms()))
            .await;
        if let Err(error) = result {
            self.failure = Some(error.clone());
            return Err(error);
        }
        self.active()?;
        self.draft += text.len();
        let mut event = RuntimeEvent::new("text_delta", &self.generation.id);
        event.text = Some(text);
        self.sink.emit(event);
        Ok(())
    }
    /// The method cuts the draft back to the start of the current turn.
    /// The `draft_reset` event carries the kept draft.
    async fn reset_turn(&mut self) -> Result<()> {
        let length = self.previous_start;
        let (id, token) = (self.generation.id.clone(), self.generation.fencing_token);
        let generation = self
            .runtime
            .store_call(move |store| store.truncate_draft(&id, token, length, now_ms()))
            .await?;
        (self.draft, self.turn_start) = (length, length);
        self.active()?;
        let mut event = RuntimeEvent::new("draft_reset", &self.generation.id);
        event.text = Some(generation.draft);
        self.sink.deliver(event).await;
        Ok(())
    }
    /// The method emits a `reasoning_delta` event. The draft does not keep the thinking text.
    pub fn append_reasoning(&self, text: &str) -> Result<()> {
        self.emit("reasoning_delta", text.to_owned())
    }
    fn emit(&self, kind: &str, text: String) -> Result<()> {
        self.active()?;
        let mut event = RuntimeEvent::new(kind, &self.generation.id);
        event.text = Some(text);
        self.sink.emit(event);
        Ok(())
    }
    async fn record_usage(&mut self, next: Value) -> Result<()> {
        // The usage of a reply is the sum of the usage of its model calls.
        if let (Some(Value::Object(current)), Value::Object(next)) = (self.usage.as_mut(), &next) {
            for (key, value) in next {
                let sum = match (current.get(key), value) {
                    (Some(Value::Number(a)), Value::Number(b)) => match (a.as_u64(), b.as_u64()) {
                        (Some(a), Some(b)) => Value::from(a.saturating_add(b)),
                        _ => Value::from(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0)),
                    },
                    _ => value.clone(),
                };
                current.insert(key.clone(), sum);
            }
        } else {
            self.usage = Some(next);
        }
        let id = self.generation.id.clone();
        let token = self.generation.fencing_token;
        let usage = self.usage.clone();
        self.runtime
            .store_call(move |store| store.record_usage(&id, token, usage, now_ms()))
            .await?;
        let mut event = RuntimeEvent::new("usage", &self.generation.id);
        event.usage = self.usage.clone();
        self.active()?;
        self.sink.emit(event);
        Ok(())
    }
    pub(super) fn finish(self, result: Result<()>) -> Result<()> {
        result?;
        self.active()?;
        if !self.complete {
            return Err(Error::new(
                "invalid_driver_completion",
                "The driver returned success before provider completion.",
            ));
        }
        Ok(())
    }
}
