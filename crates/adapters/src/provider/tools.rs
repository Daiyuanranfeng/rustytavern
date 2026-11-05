//! The module checks tool definitions, continuations, and returned calls.
//! The provider contract accepts a small subset of JSON Schema.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn invalid(message: &str) -> Error {
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
/// The function adds the tools and the continuation to a prepared chat request.
pub(super) fn attach(
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
    let mut body = request.chat()?;
    body.tools = agent.tools.clone();
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
        body.messages.push(ChatMessage::Assistant {
            content: turn.text.clone(),
            tool_calls: turn.calls.clone(),
        });
        body.messages
            .extend(turn.results.iter().map(|r| ChatMessage::Tool {
                tool_call_id: r.call_id.clone(),
                content: r.content.clone(),
            }));
    }
    request.body = serde_json::to_value(body)?;
    if serde_json::to_writer(Budget(8 * 1048576), &request.body).is_err() {
        return Err(invalid("The tool request exceeds its byte limit."));
    }
    request.contract_id = request
        .protocol
        .capabilities()
        .contract_id
        .replace("text-v1", "agent-v1");
    Ok(request)
}

/// The function checks the completed calls against the tool schemas of the request.
pub(super) fn check_calls(tools: &[ToolDefinition], calls: Vec<ToolCall>) -> Result<Vec<ToolCall>> {
    let definitions: BTreeMap<_, _> = tools.iter().map(|t| (&t.name, &t.parameters)).collect();
    let mut ids = BTreeSet::new();
    for call in &calls {
        if !id(&call.id)
            || !name(&call.name)
            || !ids.insert(call.id.as_str())
            || serde_json::to_vec(&call.arguments)?.len() > 65536
            || !definitions
                .get(&call.name)
                .is_some_and(|schema| arguments_valid(schema, &call.arguments))
        {
            return Err(invalid("The completed tool call is invalid."));
        }
    }
    if calls.len() > 32 {
        return Err(invalid("The tool calls exceed their count bound."));
    }
    Ok(calls)
}
