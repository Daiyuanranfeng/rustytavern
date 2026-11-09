//! The module holds the run policy of the agent driver as one Rig hook.
//! - The step limit stops a tool round beyond `max_steps` before its tools run.
//! - The approval list holds a tool call until the user decides.
//! - A truncated reply without tool calls repeats with a doubled output cap.
//! - The token budget stops the run before the next model call.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent,
    HookContext, InvalidToolCallAction, InvalidToolCallContext, InvalidToolCallReason,
    ModelTurnAction, ModelTurnFinished, OutcomeAction, OutcomeEvent, RequestPatch,
};
use rig_core::{completion::FinishReason, effect::EffectKind, message::AssistantContent};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{EventSink, RuntimeEvent};

/// The run policy of the agent driver. The `[plugins.agent]` table supplies it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentPolicy {
    /// The limit of tool rounds for one reply.
    pub max_steps: usize,
    /// The tools that wait for user approval. The name `*` selects every tool.
    pub confirm_tools: BTreeSet<String>,
    /// The number of repeats of a truncated reply. Zero disables the repeat.
    pub retry_truncated: usize,
    /// The token budget of one reply. Zero disables the budget.
    pub max_tokens: u64,
}
impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            max_steps: super::agent::DEFAULT_MAX_STEPS,
            confirm_tools: BTreeSet::new(),
            retry_truncated: 0,
            max_tokens: 0,
        }
    }
}
impl From<usize> for AgentPolicy {
    fn from(max_steps: usize) -> Self {
        Self {
            max_steps,
            ..Default::default()
        }
    }
}

/// The approval state of one generation.
#[derive(Default)]
pub(super) struct Approvals {
    /// The open requests in arrival order: the call ID, the answer channel, and the call text.
    pending: Mutex<Vec<(String, oneshot::Sender<bool>, String)>>,
    /// The time that the run waited for the user. The driver deadline excludes it.
    waited: Mutex<Duration>,
}
impl Approvals {
    pub(super) fn waiting(&self) -> bool {
        self.pending.lock().is_ok_and(|pending| !pending.is_empty())
    }
    pub(super) fn waited(&self) -> Duration {
        self.waited.lock().map_or(Duration::ZERO, |waited| *waited)
    }
}

/// The handle answers the approval requests of one generation.
#[derive(Clone, Default)]
pub struct Approver(pub(super) Arc<Approvals>);
impl Approver {
    /// The method answers one `tool_approval` event. It returns false for an unknown call ID.
    pub fn approve(&self, call_id: &str, allow: bool) -> bool {
        let sender = self.0.pending.lock().ok().and_then(|mut pending| {
            let index = pending.iter().position(|(id, _, _)| id == call_id)?;
            Some(pending.remove(index).1)
        });
        sender.is_some_and(|sender| sender.send(allow).is_ok())
    }
    /// The method lists the open requests in arrival order: the call ID and the call text.
    pub fn pending(&self) -> Vec<(String, String)> {
        let pending = self.0.pending.lock();
        pending.map_or_else(
            |_| Vec::new(),
            |pending| {
                pending
                    .iter()
                    .map(|(id, _, text)| (id.clone(), text.clone()))
                    .collect()
            },
        )
    }
}

/// The run state in the Rig scratchpad.
#[derive(Clone, Default)]
struct Spent {
    tokens: u64,
    rounds: usize,
    repeats: usize,
    cap: Option<u64>,
}

/// The function returns whether a model turn calls tools.
fn calls_tools(event: &ModelTurnFinished<'_>) -> bool {
    event
        .content
        .iter()
        .any(|content| matches!(content, AssistantContent::ToolCall(_)))
}

#[derive(Clone)]
pub(super) struct PolicyHook {
    pub(super) policy: AgentPolicy,
    pub(super) sink: EventSink,
    pub(super) generation_id: String,
    pub(super) approvals: Arc<Approvals>,
    pub(super) cancellation: CancellationToken,
}

impl PolicyHook {
    /// The method emits a `tool_approval` event and waits for the answer.
    /// The event waits for room in the queue, because a lost approval stalls the run.
    async fn approval(&self, call_id: String, name: &str, args: &str) -> bool {
        let (sender, receiver) = oneshot::channel();
        let text = format!("{name} {args}");
        if let Ok(mut pending) = self.approvals.pending.lock() {
            pending.push((call_id.clone(), sender, text.clone()));
        }
        let started = Instant::now();
        let mut event = RuntimeEvent::new("tool_approval", &self.generation_id);
        event.text = Some(text);
        event.call_id = Some(call_id);
        self.sink.deliver(event).await;
        let answer = tokio::select! {
            _ = self.cancellation.cancelled() => false,
            answer = receiver => answer.unwrap_or(false),
        };
        if let Ok(mut waited) = self.approvals.waited.lock() {
            *waited += started.elapsed();
        }
        answer
    }
}

impl AgentHook for PolicyHook {
    /// The method counts each tool round. A repeated turn does not reach this hook.
    async fn on_model_turn_finished(
        &self,
        context: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> ModelTurnAction {
        if calls_tools(&event) {
            context
                .scratchpad()
                .update(|spent: &mut Spent| spent.rounds += 1);
        }
        ModelTurnAction::continue_run()
    }

    async fn on_dispatch(&self, context: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        let EffectKind::ToolCall { name, args } = event.kind else {
            return DispatchAction::proceed();
        };
        let spent = context.scratchpad().get::<Spent>().unwrap_or_default();
        if spent.rounds > self.policy.max_steps {
            return DispatchAction::stop("tool_step_limit");
        }
        let confirm = &self.policy.confirm_tools;
        if confirm.contains(name) || confirm.contains("*") {
            let call_id = match event.call_id {
                Some(call_id) => call_id.to_string(),
                None => event.id.to_string(),
            };
            if !self.approval(call_id, name, args).await {
                return DispatchAction::skip("The user declined the tool call.");
            }
        }
        DispatchAction::proceed()
    }

    /// The method adds the tokens of each model call. A provider without a total reports its parts.
    async fn on_outcome(&self, context: &HookContext, event: OutcomeEvent<'_>) -> OutcomeAction {
        if let Some(response) = event.completion() {
            let usage = &response.usage;
            let parts =
                usage.input_tokens.unwrap_or_default() + usage.output_tokens.unwrap_or_default();
            let tokens = usage.total_tokens.unwrap_or(parts);
            context
                .scratchpad()
                .update(|spent: &mut Spent| spent.tokens += tokens);
        }
        OutcomeAction::proceed()
    }

    async fn on_completion_call(
        &self,
        context: &HookContext,
        _: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        let spent = context.scratchpad().get::<Spent>().unwrap_or_default();
        if self.policy.max_tokens > 0 && spent.tokens >= self.policy.max_tokens {
            return CompletionCallAction::stop("token_budget");
        }
        CompletionCallAction::continue_run()
    }
}

/// The hook decides the end of each model turn and answers invalid tool calls.
/// A truncated reply without tool calls repeats with a doubled output cap while repeats remain.
/// The cap stays within the output limit of the model and applies only to the repeated turn.
/// Otherwise a truncated reply stops with `output_limit` and a filtered reply with `provider_refusal`.
#[derive(Clone)]
pub(crate) struct FinishHook {
    pub(crate) retry_truncated: usize,
    /// The output limit of the model in the Rig catalog.
    pub(crate) ceiling: Option<u64>,
}

impl AgentHook for FinishHook {
    /// An invalid tool call returns its reason to the model as a skipped result.
    /// The model can then call the tool again or reply. The step limit bounds the repeats.
    async fn on_invalid_tool_call(
        &self,
        _: &HookContext,
        event: &InvalidToolCallContext,
    ) -> Option<InvalidToolCallAction> {
        let name = &event.tool_name;
        let reason = match &event.reason {
            InvalidToolCallReason::UnknownTool => format!(
                "The tool {name} does not exist. The tools are: {}.",
                event.available_tools.join(", ")
            ),
            InvalidToolCallReason::MalformedArguments { error } => format!(
                "The arguments of the tool {name} are invalid: {error}. Send one JSON object."
            ),
            _ => format!("The tool {name} is not allowed in this turn."),
        };
        Some(InvalidToolCallAction::skip(reason))
    }

    async fn on_completion_call(
        &self,
        context: &HookContext,
        _: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        match context
            .scratchpad()
            .get::<Spent>()
            .and_then(|spent| spent.cap)
        {
            Some(cap) => CompletionCallAction::patch(RequestPatch::new().max_tokens(cap)),
            None => CompletionCallAction::continue_run(),
        }
    }

    async fn on_model_turn_finished(
        &self,
        context: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> ModelTurnAction {
        let calls = calls_tools(&event);
        let length = event.finish_reason == Some(&FinishReason::Length);
        let repeat = context.scratchpad().update(|spent: &mut Spent| {
            let cap = event.max_tokens.map(|cap| {
                let doubled = cap.saturating_mul(2);
                self.ceiling.map_or(doubled, |ceiling| doubled.min(ceiling))
            });
            let grows = cap.is_some_and(|cap| Some(cap) > event.max_tokens);
            let repeat = length && !calls && grows && spent.repeats < self.retry_truncated;
            if repeat {
                spent.repeats += 1;
            }
            spent.cap = if repeat { cap } else { None };
            repeat
        });
        match event.finish_reason {
            _ if repeat => ModelTurnAction::repeat(),
            Some(FinishReason::ContentFilter) => ModelTurnAction::stop("provider_refusal"),
            Some(FinishReason::Length) => ModelTurnAction::stop("output_limit"),
            _ => ModelTurnAction::continue_run(),
        }
    }
}
