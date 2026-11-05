use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    Error, EventSink, Result, Runtime, RuntimeEvent,
    provider::{
        AgentRequest, AgentTurn, PreparedRequest, ProviderEvent, ProviderStream, ToolCall,
        ToolResult,
    },
    session::Generation,
    store::now_ms,
};

pub const MAX_DRIVER_EVENTS: usize = 65_536;
/// The host truncates each tool result to this byte count.
pub const MAX_TOOL_RESULT_BYTES: usize = 65_536;
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

type StreamSlot = Arc<Mutex<Option<ProviderStream>>>;
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
    /// The method dispatches one step with the composed tools and the continuation.
    pub async fn open_step(&mut self, continuation: &[AgentTurn]) -> Result<()> {
        let agent = self.runtime.agent_request(continuation.to_vec());
        let result = self.open(&agent).await;
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    pub async fn next_event(&mut self) -> Result<Option<ProviderEvent>> {
        let result = self.next_inner().await;
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
    async fn open(&mut self, agent: &AgentRequest) -> Result<()> {
        self.active()?;
        if self.opened && !self.complete {
            return Err(Error::new(
                "invalid_driver_command",
                "The driver cannot open a step before the previous step completes.",
            ));
        }
        let stream = self
            .runtime
            .dispatch(
                &self.generation,
                self.request.clone(),
                self.cancellation.clone(),
                agent,
            )
            .await?;
        *self
            .stream
            .lock()
            .map_err(|_| Error::new("driver_state", "The driver stream is unavailable."))? =
            Some(stream);
        self.opened = true;
        self.complete = false;
        Ok(())
    }
    async fn next_inner(&mut self) -> Result<Option<ProviderEvent>> {
        self.active()?;
        if self.complete {
            return Ok(None);
        }
        if self.events >= MAX_DRIVER_EVENTS {
            return Err(Error::new(
                "output_limit",
                "The driver phase exceeds its event limit.",
            ));
        }
        if !self.opened {
            return Err(Error::new(
                "invalid_driver_command",
                "The driver cannot read events before it opens a step.",
            ));
        }
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| Error::new("driver_state", "The driver stream is unavailable."))?
            .take()
            .ok_or_else(|| Error::new("driver_state", "The driver stream is closed."))?;
        let event = tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the driver phase.")),
            event = stream.next() => event.ok_or_else(|| Error::new("incomplete_stream", "The provider ended without a completion event."))??,
        };
        self.events += 1;
        match &event {
            ProviderEvent::Usage { usage } => self.record_usage(usage.clone()).await?,
            ProviderEvent::Complete => self.complete = true,
            ProviderEvent::Failure { error } => self.failure = Some(error.clone()),
            ProviderEvent::Text { .. }
            | ProviderEvent::Reasoning { .. }
            | ProviderEvent::ToolCalls { .. } => {}
        }
        if !self.complete && self.failure.is_none() {
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
                "The driver cannot append text before dispatch.",
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
        let mut event = RuntimeEvent::new("text_delta", &self.generation.id);
        event.text = Some(text);
        self.sink.emit(event);
        Ok(())
    }
    /// The method emits a `reasoning_delta` event. The draft does not keep the thinking text.
    pub fn append_reasoning(&self, text: &str) -> Result<()> {
        self.emit("reasoning_delta", text.to_owned())
    }
    /// The method emits `tool_call` without a result and `tool_result` with a result.
    /// The host runs one tool call and emits `tool_call` and `tool_result` events.
    /// A failed tool returns its error text to the model.
    pub async fn call_tool(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.emit("tool_call", format!("{} {}", call.name, call.arguments))?;
        let tool = self
            .runtime
            .tools
            .iter()
            .find(|tool| tool.definition().name == call.name)
            .cloned();
        let result = match tool {
            Some(tool) => {
                tool.call(call.arguments.clone(), self.cancellation.clone())
                    .await
            }
            None => Err(Error::new("unknown_tool", "The tool does not exist.")),
        };
        let mut content = match result {
            Ok(content) => content,
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(error) => format!("Error {}: {}", error.code, error.message),
        };
        if content.len() > MAX_TOOL_RESULT_BYTES {
            let mut end = MAX_TOOL_RESULT_BYTES;
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            content.truncate(end);
        }
        self.emit("tool_result", content.clone())?;
        Ok(ToolResult {
            call_id: call.id.clone(),
            content,
        })
    }
    fn emit(&self, kind: &str, text: String) -> Result<()> {
        self.active()?;
        let mut event = RuntimeEvent::new(kind, &self.generation.id);
        event.text = Some(text);
        self.sink.emit(event);
        Ok(())
    }
    async fn record_usage(&mut self, next: Value) -> Result<()> {
        if let (Some(Value::Object(current)), Value::Object(next)) = (self.usage.as_mut(), &next) {
            current.extend(next.iter().map(|(k, v)| (k.clone(), v.clone())));
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
