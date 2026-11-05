use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub type AgentProviderStream = Pin<Box<dyn Stream<Item = Result<AgentProviderEvent>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentProviderEvent {
    Text { text: String },
    Usage { usage: Value },
    ToolCalls { calls: Vec<ToolCall> },
    Failure { error: Error },
    Complete,
}
impl AgentProviderEvent {
    pub fn into_text(self) -> Result<ProviderEvent> {
        Ok(match self {
            Self::Text { text } => ProviderEvent::Text { text },
            Self::Usage { usage } => ProviderEvent::Usage { usage },
            Self::Failure { error } => ProviderEvent::Failure { error },
            Self::Complete => ProviderEvent::Complete,
            Self::ToolCalls { .. } => return Err(invalid("The text contract rejects tool calls.")),
        })
    }
}
impl From<ProviderEvent> for AgentProviderEvent {
    fn from(event: ProviderEvent) -> Self {
        match event {
            ProviderEvent::Text { text } => Self::Text { text },
            ProviderEvent::Usage { usage } => Self::Usage { usage },
            ProviderEvent::Failure { error } => Self::Failure { error },
            ProviderEvent::Complete => Self::Complete,
        }
    }
}
fn invalid(message: &str) -> Error {
    Error::new("invalid_provider_tool", message)
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_graphic())
}
fn schema_valid(schema: &Value, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "enum"
                | "description"
                | "title"
        )
    }) {
        return false;
    }
    if !matches!(
        schema["type"].as_str(),
        Some("object" | "array" | "string" | "number" | "integer" | "boolean" | "null")
    ) {
        return false;
    }
    if object.get("description").is_some_and(|v| !v.is_string())
        || object.get("title").is_some_and(|v| !v.is_string())
    {
        return false;
    }
    if let Some(properties) = object.get("properties") {
        let Some(properties) = properties.as_object() else {
            return false;
        };
        if schema["type"] != "object"
            || properties.len() > 128
            || properties.values().any(|v| !schema_valid(v, depth + 1))
        {
            return false;
        }
    }
    if let Some(required) = object.get("required") {
        let Some(required) = required.as_array() else {
            return false;
        };
        let mut seen = BTreeSet::new();
        for key in required {
            let Some(key) = key.as_str() else {
                return false;
            };
            if !seen.insert(key) || schema["properties"].get(key).is_none() {
                return false;
            }
        }
    }
    if object
        .get("additionalProperties")
        .is_some_and(|v| !v.is_boolean())
    {
        return false;
    }
    if let Some(items) = object.get("items")
        && (schema["type"] != "array" || !schema_valid(items, depth + 1))
    {
        return false;
    }
    if object
        .get("enum")
        .is_some_and(|v| v.as_array().is_none_or(|a| a.is_empty()))
    {
        return false;
    }
    true
}
fn arguments_valid(schema: &Value, value: &Value) -> bool {
    if schema
        .get("enum")
        .is_some_and(|items| !items.as_array().is_some_and(|a| a.contains(value)))
    {
        return false;
    }
    match schema["type"].as_str() {
        Some("object") => value.as_object().is_some_and(|object| {
            schema["required"].as_array().is_none_or(|keys| {
                keys.iter()
                    .all(|key| key.as_str().is_some_and(|key| object.contains_key(key)))
            }) && object
                .iter()
                .all(|(key, value)| match schema["properties"].get(key) {
                    Some(property) => arguments_valid(property, value),
                    None => schema["additionalProperties"] != false,
                })
        }),
        Some("array") => value.as_array().is_some_and(|items| {
            schema
                .get("items")
                .is_none_or(|schema| items.iter().all(|value| arguments_valid(schema, value)))
        }),
        Some("string") => value.is_string(),
        Some("number") => value.is_number(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        _ => false,
    }
}
struct Budget(usize);
impl std::io::Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("The request exceeds its byte limit."))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn prepare(
    mut request: PreparedRequest,
    agent: &AgentRequest,
) -> Result<PreparedRequest> {
    if agent.tools.is_empty() || agent.tools.len() > 64 || agent.continuation.len() > 64 {
        return Err(invalid("The tool request exceeds its count bounds."));
    }
    serde_json::to_writer(Budget(8 * 1048576), agent)
        .map_err(|_| invalid("The tool request exceeds its byte limit."))?;
    let mut names = BTreeSet::new();
    for tool in &agent.tools {
        if !name(&tool.name)
            || !names.insert(&tool.name)
            || tool.description.len() > 16384
            || tool.parameters["type"] != "object"
            || !schema_valid(&tool.parameters, 0)
            || serde_json::to_vec(&tool.parameters)?.len() > 65536
        {
            return Err(invalid("The tool definition is invalid."));
        }
    }
    let protocol = request.protocol;
    let tools = agent.tools.iter().map(|t| match protocol {
        ProviderKind::OpenAi => json!({"type":"function","name":t.name,"description":t.description,"parameters":t.parameters,"strict":false}),
        ProviderKind::Anthropic => json!({"name":t.name,"description":t.description,"input_schema":t.parameters}),
        ProviderKind::Gemini => json!({"name":t.name,"description":t.description,"parametersJsonSchema":t.parameters}),
        _ => json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}}),
    });
    request.body["tools"] = match protocol {
        ProviderKind::Gemini => json!([{"functionDeclarations":tools.collect::<Vec<_>>()}]),
        _ => json!(tools.collect::<Vec<_>>()),
    };
    let field = match protocol {
        ProviderKind::OpenAi => "input",
        ProviderKind::Gemini => "contents",
        _ => "messages",
    };
    let messages = request.body[field]
        .as_array_mut()
        .ok_or_else(|| invalid("The request has no message array."))?;
    let mut ids = BTreeSet::new();
    for turn in &agent.continuation {
        if turn.calls.is_empty()
            || turn.calls.len() > 32
            || turn.results.len() != turn.calls.len()
            || turn.text.len() > 1048576
        {
            return Err(invalid(
                "The continuation has invalid call or result bounds.",
            ));
        }
        let mut expected = BTreeSet::new();
        for call in &turn.calls {
            if !id(&call.id)
                || !ids.insert(&call.id)
                || !names.contains(&call.name)
                || !call.arguments.is_object()
                || serde_json::to_vec(&call.arguments)?.len() > 65536
                || !agent
                    .tools
                    .iter()
                    .find(|tool| tool.name == call.name)
                    .is_some_and(|tool| arguments_valid(&tool.parameters, &call.arguments))
            {
                return Err(invalid("The continuation call is invalid."));
            }
            expected.insert(call.id.as_str());
        }
        for result in &turn.results {
            if !expected.remove(result.call_id.as_str()) || result.content.len() > 1048576 {
                return Err(invalid("The tool result has no unique matching call."));
            }
        }
        messages.extend(encode_turn(protocol, turn));
    }
    if serde_json::to_writer(Budget(8 * 1048576), &request.body).is_err() {
        return Err(invalid("The tool request exceeds its byte limit."));
    }
    request.contract_id = protocol
        .capabilities()
        .contract_id
        .replace("text-v1", "agent-v1");
    Ok(request)
}

/// The function maps one agent turn to the message terms of the protocol.
/// OpenAI uses `function_call` items. Anthropic uses `tool_use` blocks. Gemini uses `functionCall` parts.
fn encode_turn(protocol: ProviderKind, turn: &AgentTurn) -> Vec<Value> {
    let name = |id: &str| {
        turn.calls
            .iter()
            .find(|c| c.id == id)
            .map_or("", |c| c.name.as_str())
    };
    let text = (!turn.text.is_empty()).then_some(&turn.text);
    match protocol {
        ProviderKind::OpenAi => {
            let message = text.map(|t| json!({"role":"assistant","content":t}));
            let calls = turn.calls.iter().map(|c| json!({"type":"function_call","call_id":c.id,"name":c.name,"arguments":c.arguments.to_string()}));
            let results = turn.results.iter().map(
                |r| json!({"type":"function_call_output","call_id":r.call_id,"output":r.content}),
            );
            message.into_iter().chain(calls).chain(results).collect()
        }
        ProviderKind::Anthropic => {
            let text = text.map(|t| json!({"type":"text","text":t}));
            let calls = turn
                .calls
                .iter()
                .map(|c| json!({"type":"tool_use","id":c.id,"name":c.name,"input":c.arguments}));
            let results = turn
                .results
                .iter()
                .map(|r| json!({"type":"tool_result","tool_use_id":r.call_id,"content":r.content}))
                .collect::<Vec<_>>();
            vec![
                json!({"role":"assistant","content":text.into_iter().chain(calls).collect::<Vec<_>>()}),
                json!({"role":"user","content":results}),
            ]
        }
        ProviderKind::Gemini => {
            let text = text.map(|t| json!({"text":t}));
            let calls = turn.calls.iter().map(|c| {
                let mut part = json!({"functionCall":{"name":c.name,"args":c.arguments}});
                if let Some(signature) = &c.signature {
                    part["thoughtSignature"] = json!(signature);
                }
                part
            });
            let results = turn.results.iter().map(|r| json!({"functionResponse":{"name":name(&r.call_id),"response":{"result":r.content}}})).collect::<Vec<_>>();
            vec![
                json!({"role":"model","parts":text.into_iter().chain(calls).collect::<Vec<_>>()}),
                json!({"role":"user","parts":results}),
            ]
        }
        _ => {
            let calls = turn.calls.iter().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments.to_string()}})).collect::<Vec<_>>();
            std::iter::once(json!({"role":"assistant","content":turn.text,"tool_calls":calls}))
                .chain(
                    turn.results.iter().map(
                        |r| json!({"role":"tool","tool_call_id":r.call_id,"content":r.content}),
                    ),
                )
                .collect()
        }
    }
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
    signature: Option<String>,
}
/// Gemini can omit call IDs. The counter makes unique IDs for one process.
static GEMINI_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub struct AgentStreamNormalizer {
    text: StreamNormalizer,
    agent_mode: bool,
    calls: BTreeMap<usize, PartialCall>,
    finish: Option<String>,
    definitions: Option<BTreeMap<String, Value>>,
    pub completed: bool,
}
impl AgentStreamNormalizer {
    pub fn new(kind: ProviderKind, agent_mode: bool) -> Self {
        Self {
            text: StreamNormalizer::new(kind),
            agent_mode,
            calls: BTreeMap::new(),
            finish: None,
            definitions: None,
            completed: false,
        }
    }
    pub(super) fn with_request(mut self, request: &PreparedRequest) -> Result<Self> {
        if self.agent_mode {
            let tools = match request.protocol {
                ProviderKind::Gemini => &request.body["tools"][0]["functionDeclarations"],
                _ => &request.body["tools"],
            };
            let tools = tools
                .as_array()
                .ok_or_else(|| invalid("The request has no tools array."))?;
            let mut definitions = BTreeMap::new();
            for tool in tools {
                let tool = tool.get("function").unwrap_or(tool);
                let name = tool["name"]
                    .as_str()
                    .ok_or_else(|| invalid("The request has no tool name."))?;
                let schema = match request.protocol {
                    ProviderKind::Anthropic => &tool["input_schema"],
                    ProviderKind::Gemini => &tool["parametersJsonSchema"],
                    _ => &tool["parameters"],
                };
                if !schema_valid(schema, 0)
                    || definitions.insert(name.into(), schema.clone()).is_some()
                {
                    return Err(invalid("The request tool schema is invalid."));
                }
            }
            self.definitions = Some(definitions);
        }
        Ok(self)
    }
    pub fn frame(&mut self, frame: &str) -> Result<Vec<AgentProviderEvent>> {
        if !self.agent_mode {
            let events = self.text.frame(frame)?;
            self.completed = self.text.completed;
            return Ok(events.into_iter().map(Into::into).collect());
        }
        if self.completed {
            return Err(invalid("The stream has data after completion."));
        }
        if !matches!(
            self.text.kind,
            ProviderKind::Custom | ProviderKind::OpenRouter
        ) {
            return self.native_frame(frame);
        }
        if frame == "[DONE]" {
            if self.finish.as_deref() == Some("tool_calls") {
                let calls = self.take_calls()?;
                if calls.is_empty() {
                    return Err(invalid("The tool finish has no calls."));
                }
                self.completed = true;
                return Ok(vec![
                    AgentProviderEvent::ToolCalls { calls },
                    AgentProviderEvent::Complete,
                ]);
            }
            if !self.calls.is_empty() {
                return Err(invalid("The calls have no tool finish reason."));
            }
            let events = self.text.frame(frame)?;
            self.completed = self.text.completed;
            return Ok(events.into_iter().map(Into::into).collect());
        }
        let mut value: Value = serde_json::from_str(frame)
            .map_err(Error::context("invalid_provider_stream", "Frame failed."))?;
        if value.get("error").is_some() {
            let events = self.text.frame(frame)?;
            self.completed = self.text.completed;
            return Ok(events.into_iter().map(Into::into).collect());
        }
        if let Some(choices) = value["choices"].as_array_mut() {
            if choices.len() > 1 {
                return Err(invalid("The stream has multiple choices."));
            }
            if let Some(choice) = choices.first_mut() {
                if choice["index"].as_u64().is_some_and(|i| i != 0) {
                    return Err(invalid("The choice index is invalid."));
                }
                if ["audio", "function_call", "refusal"]
                    .iter()
                    .any(|key| !choice["delta"][key].is_null())
                {
                    return Err(invalid("The stream contains unsupported output."));
                }
                // OpenRouter repeats the finish reason and the role in its usage chunk.
                let has_delta = ["content", "tool_calls"]
                    .map(|k| &choice["delta"][k])
                    .iter()
                    .any(|v| !v.is_null() && *v != "" && *v != &json!([]));
                let other = choice["finish_reason"]
                    .as_str()
                    .is_some_and(|r| Some(r) != self.finish.as_deref());
                if self.finish.is_some() && (has_delta || other) {
                    return Err(invalid("The stream has output after its finish reason."));
                }
                if let Some(deltas) = choice["delta"].get("tool_calls").cloned() {
                    for delta in deltas
                        .as_array()
                        .ok_or_else(|| invalid("The tool calls must be an array."))?
                    {
                        let index = delta["index"]
                            .as_u64()
                            .filter(|i| *i < 32)
                            .ok_or_else(|| invalid("The tool call index exceeds its bounds."))?
                            as usize;
                        if delta.get("type").is_some_and(|t| t != "function") {
                            return Err(invalid("The tool type is invalid."));
                        }
                        let call = self.calls.entry(index).or_default();
                        if let Some(value) = delta.get("id") {
                            let value = value
                                .as_str()
                                .ok_or_else(|| invalid("The tool ID must be text."))?;
                            if !call.id.is_empty() {
                                return Err(invalid("The tool ID occurs more than once."));
                            }
                            call.id = value.into();
                        }
                        for (field, target, limit) in [
                            ("name", &mut call.name, 64),
                            ("arguments", &mut call.arguments, 65536),
                        ] {
                            if let Some(value) = delta["function"].get(field) {
                                target.push_str(
                                    value
                                        .as_str()
                                        .ok_or_else(|| invalid("The tool delta must be text."))?,
                                );
                                if target.len() > limit {
                                    return Err(invalid("The tool delta exceeds its byte limit."));
                                }
                            }
                        }
                    }
                    choice["delta"]
                        .as_object_mut()
                        .ok_or_else(|| invalid("The delta must be an object."))?
                        .remove("tool_calls");
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    self.finish = Some(reason.into());
                    if reason == "tool_calls" {
                        choice["finish_reason"] = json!("stop");
                    } else if !self.calls.is_empty() {
                        return Err(invalid("The calls have an invalid finish reason."));
                    }
                }
            }
        }
        let events = self.text.frame(&value.to_string())?;
        self.completed = self.text.completed;
        Ok(events.into_iter().map(Into::into).collect())
    }
    /// The method validates the collected calls and clears them.
    fn take_calls(&mut self) -> Result<Vec<ToolCall>> {
        let mut ids = BTreeSet::new();
        let mut calls = Vec::new();
        for call in std::mem::take(&mut self.calls).into_values() {
            let arguments = match call.arguments.as_str() {
                "" => json!({}),
                text => serde_json::from_str(text)
                    .map_err(|_| invalid("The tool arguments contain invalid JSON."))?,
            };
            if !id(&call.id)
                || !name(&call.name)
                || !ids.insert(call.id.clone())
                || !arguments.is_object()
                || self.definitions.as_ref().is_some_and(|tools| {
                    !tools
                        .get(&call.name)
                        .is_some_and(|schema| arguments_valid(schema, &arguments))
                })
            {
                return Err(invalid("The completed tool call is invalid."));
            }
            calls.push(ToolCall {
                id: call.id,
                name: call.name,
                arguments,
                signature: call.signature,
            });
        }
        Ok(calls)
    }
    /// The method returns the call at an output index. A new index adds a call.
    fn call(&mut self, index: &Value, create: bool) -> Result<&mut PartialCall> {
        let index = index
            .as_u64()
            .filter(|i| *i < 256)
            .ok_or_else(|| invalid("The tool call index exceeds its bounds."))?
            as usize;
        let exists = self.calls.contains_key(&index);
        if exists == create || (create && self.calls.len() >= 32) {
            return Err(invalid("The tool call order is invalid."));
        }
        Ok(self.calls.entry(index).or_default())
    }
    fn check_limits(&self) -> Result<Vec<AgentProviderEvent>> {
        if self
            .calls
            .values()
            .any(|c| c.name.len() > 64 || c.arguments.len() > 65536)
        {
            return Err(invalid("The tool delta exceeds its byte limit."));
        }
        Ok(Vec::new())
    }
    /// The method reads the native tool terms of OpenAI, Anthropic, and Gemini.
    /// The text normalizer reads the other events.
    fn native_frame(&mut self, frame: &str) -> Result<Vec<AgentProviderEvent>> {
        let mut value: Value = serde_json::from_str(frame)
            .map_err(Error::context("invalid_provider_stream", "Frame failed."))?;
        let text = |value: &Value, key: &str| -> Result<String> {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid("The tool field must be text."))
        };
        match self.text.kind {
            ProviderKind::OpenAi => match value["type"].as_str() {
                Some(kind @ ("response.output_item.added" | "response.output_item.done"))
                    if value["item"]["type"] == "function_call" =>
                {
                    let item = &value["item"];
                    let call = self.call(&value["output_index"], kind.ends_with("added"))?;
                    call.id = text(item, "call_id")?;
                    call.name = text(item, "name")?;
                    call.arguments = text(item, "arguments")?;
                    return self.check_limits();
                }
                Some("response.function_call_arguments.delta") => {
                    let delta = text(&value, "delta")?;
                    self.call(&value["output_index"], false)?
                        .arguments
                        .push_str(&delta);
                    return self.check_limits();
                }
                _ => {}
            },
            ProviderKind::Anthropic => match value["type"].as_str() {
                Some("content_block_start") if value["content_block"]["type"] == "tool_use" => {
                    let block = &value["content_block"];
                    let (id, name) = (text(block, "id")?, text(block, "name")?);
                    let call = self.call(&value["index"], true)?;
                    (call.id, call.name) = (id, name);
                    return self.check_limits();
                }
                Some("content_block_delta") if value["delta"]["type"] == "input_json_delta" => {
                    let delta = text(&value["delta"], "partial_json")?;
                    self.call(&value["index"], false)?
                        .arguments
                        .push_str(&delta);
                    return self.check_limits();
                }
                Some("message_delta") => match value["delta"]["stop_reason"].as_str() {
                    Some("tool_use") if self.calls.is_empty() => {
                        return Err(invalid("The tool finish has no calls."));
                    }
                    Some("tool_use") => value["delta"]["stop_reason"] = json!("end_turn"),
                    Some("end_turn" | "stop_sequence") if !self.calls.is_empty() => {
                        return Err(invalid("The calls have an invalid finish reason."));
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {
                if let Some(parts) = value["candidates"][0]["content"]["parts"].as_array_mut() {
                    let mut kept = Vec::new();
                    for part in std::mem::take(parts) {
                        let Some(call) = part.get("functionCall") else {
                            kept.push(part);
                            continue;
                        };
                        let index = json!(self.calls.keys().next_back().map_or(0, |i| i + 1));
                        let id = match call["id"].as_str() {
                            Some(id) => id.to_owned(),
                            None => format!(
                                "gemini_{}",
                                GEMINI_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            ),
                        };
                        let (name, arguments) = (text(call, "name")?, call["args"].to_string());
                        let signature = part
                            .get("thoughtSignature")
                            .map(|_| text(&part, "thoughtSignature"))
                            .transpose()?;
                        let target = self.call(&index, true)?;
                        (target.id, target.name, target.signature) = (id, name, signature);
                        target.arguments = if call["args"].is_null() {
                            String::new()
                        } else {
                            arguments
                        };
                    }
                    *parts = kept;
                }
            }
        }
        self.check_limits()?;
        let mut events: Vec<AgentProviderEvent> = self
            .text
            .frame(&value.to_string())?
            .into_iter()
            .map(Into::into)
            .collect();
        self.completed = self.text.completed;
        if matches!(events.last(), Some(AgentProviderEvent::Complete)) {
            let calls = self.take_calls()?;
            if !calls.is_empty() {
                events.insert(events.len() - 1, AgentProviderEvent::ToolCalls { calls });
            }
        }
        Ok(events)
    }
}
