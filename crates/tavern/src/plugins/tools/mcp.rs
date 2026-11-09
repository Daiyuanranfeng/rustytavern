//! The `mcp` plugin connects its configured servers through `rig-rmcp` and the official `rmcp` SDK.
//! Rig lists the tools, calls them, and maps their results. The plugin names each tool `SERVER__TOOL`.

use std::{collections::BTreeMap, process::Stdio, sync::Arc, time::Duration};

use rig_core::{message::ToolName, tool::DynamicTool};
use rig_rmcp::{
    McpConnection,
    rmcp::{
        ServiceExt,
        transport::{StreamableHttpClientTransport, TokioChildProcess},
    },
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    Error, Result,
    agent::{AgentTool, ToolSet},
    plugin::PluginContext,
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
        let connection = McpConnection::new(service);
        let listed = connection.tools().await.map_err(|_| failed())?;
        context.own(move || drop(connection));
        for tool in listed {
            let mut name = format!("{server}__{}", tool.name())
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
            let name = ToolName::new(name).map_err(|_| failed())?;
            let tool = DynamicTool::try_from(tool.with_name(name)).map_err(|_| failed())?;
            tools.push(Arc::new(tool) as Arc<dyn AgentTool>);
        }
    }
    Ok(tools)
}
