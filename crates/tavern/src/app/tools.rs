//! The tool plugins register `tools.NAME/1` services for the agent driver.
//! Each plugin requires its own grant.

use std::{collections::BTreeMap, path::Path, process::Stdio, sync::Arc, time::Duration};

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

use super::{checked_file, valid_id};
use crate::{
    Error, Result,
    agent::{AgentTool, ToolSet},
    plugin::PluginContext,
    prompt::{Contribution, Retention},
    provider::ToolDefinition,
    source::PublicFetcher,
};
use rustytavern_core::{Role, bounded_read};

const MAX_SKILLS: usize = 128;
const MAX_SKILL_BYTES: usize = 256 * 1024;
const MAX_DESCRIPTION_BYTES: usize = 16_384;

/// The `tools.web` plugin registers `web_fetch` for its granted HTTPS hosts.
pub(super) fn web(context: &mut PluginContext) -> Result<()> {
    let hosts = context
        .grants()
        .iter()
        .filter_map(|g| g.strip_prefix("network:https:").map(str::to_owned))
        .collect::<Vec<_>>();
    if hosts.is_empty() {
        return Err(Error::new(
            "capability_denied",
            "The web tools require a network:https grant.",
        ));
    }
    let mut fetcher = PublicFetcher::new(hosts);
    fetcher.max_bytes = 2 * 1024 * 1024;
    let tools: ToolSet = vec![Arc::new(WebFetch(fetcher))];
    context.register("tools.web/1", Arc::new(tools))
}

/// The `tools.skills` plugin registers `skill` and adds the skill catalog to the prompt.
pub(super) fn skills(context: &mut PluginContext, data_dir: &Path) -> Result<()> {
    context.authorize("skills.read")?;
    let skills = Skills::load(&data_dir.join("skills"))?;
    let mut tools: ToolSet = Vec::new();
    if !skills.0.is_empty() {
        context.contribute(Contribution {
            plugin_id: String::new(),
            section_id: "skills".into(),
            slot: "before_history".into(),
            priority: 0,
            role: Role::System,
            text: skills.catalog(),
            provenance: Value::Null,
            retention: Retention::Optional,
        })?;
        tools.push(Arc::new(skills));
    }
    context.register("tools.skills/1", Arc::new(tools))
}

/// The `tools.mcp` plugin registers the tools of each server in `mcp.json`.
pub(super) async fn mcp(context: &mut PluginContext, data_dir: &Path) -> Result<()> {
    context.authorize("mcp.connect")?;
    let tools = connect(context, &data_dir.join("mcp.json")).await?;
    context.register("tools.mcp/1", Arc::new(tools))
}

fn definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    let mut description = description.to_owned();
    if description.len() > MAX_DESCRIPTION_BYTES {
        let mut end = MAX_DESCRIPTION_BYTES;
        while !description.is_char_boundary(end) {
            end -= 1;
        }
        description.truncate(end);
    }
    ToolDefinition {
        name: name.into(),
        description,
        parameters,
    }
}

struct WebFetch(PublicFetcher);
#[async_trait]
impl AgentTool for WebFetch {
    fn definition(&self) -> ToolDefinition {
        definition(
            "web_fetch",
            "Fetch one public HTTPS page. The tool returns the page as plain text.",
            json!({"type":"object","properties":{"url":{"type":"string","description":"The HTTPS URL."}},"required":["url"],"additionalProperties":false}),
        )
    }
    async fn call(&self, arguments: Value, cancellation: CancellationToken) -> Result<String> {
        let url = arguments["url"].as_str().unwrap_or_default();
        let artifact = self.0.fetch(url, cancellation).await?;
        let media = artifact.provenance.media_type.to_ascii_lowercase();
        let body = String::from_utf8_lossy(&artifact.bytes);
        let text = if media.starts_with("text/html") {
            html_text(&body)
        } else if media.starts_with("text/") || media.contains("json") || media.contains("xml") {
            body.into_owned()
        } else {
            format!("The tool does not read the media type {media}.")
        };
        Ok(format!("URL: {}\n\n{text}", artifact.provenance.final_url))
    }
}

/// The function removes scripts, styles, and tags from HTML.
/// ASCII lowercase keeps byte offsets, so one index serves both strings.
fn html_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut text = String::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find('<').map(|i| at + i) {
        text.push_str(&html[at..start]);
        let close = if lower[start..].starts_with("<script") {
            "</script>"
        } else if lower[start..].starts_with("<style") {
            "</style>"
        } else {
            ">"
        };
        at = lower[start..]
            .find(close)
            .map_or(html.len(), |i| start + i + close.len());
        text.push(' ');
    }
    text.push_str(&html[at..]);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Each skill uses the Agent Skills layout: `skills/<name>/SKILL.md`.
struct Skills(BTreeMap<String, (String, std::path::PathBuf)>);
impl Skills {
    fn load(root: &Path) -> Result<Self> {
        let mut skills = BTreeMap::new();
        if !root.is_dir() {
            return Ok(Self(skills));
        }
        for entry in std::fs::read_dir(root)? {
            let directory = entry?.path();
            let Some(name) = directory.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !valid_id(name) || !directory.join("SKILL.md").is_file() {
                continue;
            }
            let text = String::from_utf8(bounded_read(
                &checked_file(&directory, "SKILL.md")?,
                MAX_SKILL_BYTES,
            )?)
            .map_err(Error::context(
                "invalid_skill",
                "The skill is not UTF-8 text.",
            ))?;
            let field = |key: &str| {
                let header = text.strip_prefix("---")?.split("\n---").next()?;
                header.lines().find_map(|line| {
                    let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
                    Some(value.trim_matches(['"', '\'']).to_owned())
                })
            };
            let description = field("description").unwrap_or_default();
            let name = field("name").unwrap_or_else(|| name.to_owned());
            if valid_id(&name) && skills.len() < MAX_SKILLS {
                skills.insert(name, (description, directory));
            }
        }
        Ok(Self(skills))
    }
}
impl Skills {
    fn catalog(&self) -> String {
        let list = self
            .0
            .iter()
            .map(|(name, (description, _))| format!("- {name}: {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Skill catalog. A skill is a reusable procedure. Call the skill tool to read a skill before you do a task that it covers.\n{list}"
        )
    }
}
#[async_trait]
impl AgentTool for Skills {
    fn definition(&self) -> ToolDefinition {
        definition(
            "skill",
            "Read one skill from the skill catalog.",
            json!({"type":"object","properties":{"name":{"type":"string","enum":self.0.keys().collect::<Vec<_>>()},"path":{"type":"string","description":"A file inside the skill directory. Omit it to read SKILL.md."}},"required":["name"],"additionalProperties":false}),
        )
    }
    async fn call(&self, arguments: Value, _: CancellationToken) -> Result<String> {
        let (_, directory) = self
            .0
            .get(arguments["name"].as_str().unwrap_or_default())
            .ok_or_else(|| Error::new("not_found", "The skill does not exist."))?;
        let path = arguments["path"].as_str().unwrap_or("SKILL.md");
        let bytes = bounded_read(&checked_file(directory, path)?, MAX_SKILL_BYTES)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The file uses the common `mcp.json` format.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpFile {
    mcp_servers: BTreeMap<String, McpServer>,
}
#[derive(Deserialize)]
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

async fn connect(context: &mut PluginContext, path: &Path) -> Result<ToolSet> {
    let mut tools: ToolSet = Vec::new();
    if !path.is_file() {
        return Ok(tools);
    }
    let file: McpFile = serde_json::from_slice(&bounded_read(path, 1024 * 1024)?)?;
    for (server, config) in file.mcp_servers.into_iter().filter(|(_, c)| !c.disabled) {
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

#[cfg(test)]
mod tests {
    #[test]
    fn html_text_removes_scripts_styles_and_tags() {
        let html = "<p>A<SCRIPT>x<y</script> <style>b{}</style>B&amp;</p>";
        assert_eq!(super::html_text(html), "A B&amp;");
    }
}
