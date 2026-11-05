//! The `mcp` plugin connects its configured servers through the official `rmcp` SDK.

use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};

use async_trait::async_trait;
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
    service::Peer,
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::definition;
use crate::{
    Error, Result,
    agent::{AgentTool, ToolSet},
    plugin::PluginContext,
    provider::ToolDefinition,
};

/// The `servers` configuration maps each server name to its connection.
pub(crate) async fn activate(context: &mut PluginContext, configuration: &Value) -> Result<()> {
    context.authorize("mcp.connect")?;
    let servers: BTreeMap<String, McpServer> = match &configuration["servers"] {
        Value::Null => BTreeMap::new(),
        servers => serde_json::from_value(servers.clone())?,
    };
    let tools = connect(context, servers).await?;
    context.register("tools.mcp/1", Arc::new(tools))
}

/// Each server has `command`, `args`, and `env`, or one `url`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpServer {
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    url: Option<String>,
    #[serde(default)]
    disabled: bool,
}

async fn connect(
    context: &mut PluginContext,
    servers: BTreeMap<String, McpServer>,
) -> Result<ToolSet> {
    let mut tools: ToolSet = Vec::new();
    for (server, config) in servers.into_iter().filter(|(_, c)| !c.disabled) {
        let failed = || Error::new("mcp_connect", format!("The MCP server {server} failed."));
        let start = async {
            match (config.command, config.url) {
                (Some(command), None) => {
                    let mut command = tokio::process::Command::new(command);
                    command.args(config.args).envs(config.env);
                    let (transport, _) = TokioChildProcess::builder(command)
                        .stderr(Stdio::null())
                        .spawn()
                        .map_err(|_| failed())?;
                    ().serve(transport).await.map_err(|_| failed())
                }
                (None, Some(url)) => {
                    ().serve(StreamableHttpClientTransport::from_uri(url))
                        .await
                        .map_err(|_| failed())
                }
                _ => Err(Error::new(
                    "mcp_connect",
                    format!("The MCP server {server} requires a command or a URL."),
                )),
            }
        };
        let service = tokio::time::timeout(Duration::from_secs(30), start)
            .await
            .map_err(|_| failed())??;
        let listed = service.list_all_tools().await.map_err(|_| failed())?;
        let peer = service.peer().clone();
        context.own(move || drop(service));
        for tool in listed {
            let mut name = format!("{server}__{}", tool.name)
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            name.truncate(64);
            let schema = Value::Object(tool.input_schema.as_ref().clone());
            tools.push(Arc::new(McpTool {
                peer: peer.clone(),
                remote: tool.name.into_owned(),
                definition: definition(
                    &name,
                    tool.description.as_deref().unwrap_or_default(),
                    sanitize(&schema, 0).unwrap_or_else(|| json!({"type":"object"})),
                ),
            }));
        }
    }
    Ok(tools)
}

/// The function keeps the JSON Schema subset that the provider contract accepts.
/// The function drops each property that the subset cannot express.
fn sanitize(schema: &Value, depth: usize) -> Option<Value> {
    let object = schema.as_object().filter(|_| depth <= 16)?;
    let kind = match object.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|k| *k != "null")?,
        None if object.contains_key("properties") => "object",
        _ => return None,
    };
    if !matches!(
        kind,
        "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
    ) {
        return None;
    }
    let mut out = Map::from_iter([("type".to_owned(), json!(kind))]);
    for key in ["description", "title"] {
        if let Some(value) = object.get(key).filter(|v| v.is_string()) {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(values) = object
        .get("enum")
        .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
    {
        out.insert("enum".into(), values.clone());
    }
    if kind == "object" {
        let properties = object
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .take(128)
            .filter_map(|(key, value)| Some((key.clone(), sanitize(value, depth + 1)?)))
            .collect::<Map<_, _>>();
        let required = object
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|key| properties.contains_key(*key))
            .collect::<std::collections::BTreeSet<_>>();
        out.insert("properties".into(), Value::Object(properties));
        out.insert("required".into(), json!(required));
    }
    if let Some(items) = object.get("items").filter(|_| kind == "array")
        && let Some(items) = sanitize(items, depth + 1)
    {
        out.insert("items".into(), items);
    }
    Some(Value::Object(out))
}

struct McpTool {
    peer: Peer<RoleClient>,
    remote: String,
    definition: ToolDefinition,
}
#[async_trait]
impl AgentTool for McpTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String> {
        let arguments = arguments.as_object().cloned().unwrap_or_default();
        let request = CallToolRequestParams::new(self.remote.clone()).with_arguments(arguments);
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Error::new("cancelled", "The caller cancelled the tool call.")),
            result = self.peer.call_tool(request) => result.map_err(Error::context("mcp_call", "The MCP tool call failed."))?,
        };
        let mut text = Vec::new();
        for block in serde_json::to_value(&result.content)?
            .as_array()
            .into_iter()
            .flatten()
        {
            match block["text"].as_str() {
                Some(value) => text.push(value.to_owned()),
                None => text.push(format!("[The tool omits {} content.]", block["type"])),
            }
        }
        if text.is_empty()
            && let Some(value) = &result.structured_content
        {
            text.push(value.to_string());
        }
        if result.is_error == Some(true) {
            return Err(Error::new("mcp_tool_error", text.join("\n")));
        }
        Ok(text.join("\n"))
    }
}
