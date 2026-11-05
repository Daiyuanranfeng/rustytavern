# Agent driver plugin

The plugin registers `Arc<dyn ConversationDriver>` as `driver/1`.
The plugin ID is `org.rustytavern.driver.agent`.
The factory name is `driver.agent`.
The driver policy ID is `agent/1`.

## Tools

The `web_fetch` tool reads one public HTTPS page.
The scoped host fetcher checks destination addresses and redirects.
The tool removes HTML scripts, styles, and tags.

The `skill` tool reads one skill from `DATA_DIR/skills`.
Each skill directory contains a `SKILL.md` file with `name` and `description` front matter.
The tool description lists each skill name and description.
The model reads the full skill only when it needs it.
The optional `path` argument reads another file inside the skill directory.

The MCP tools come from `DATA_DIR/mcp.json`.
The file uses the common `mcpServers` format.
Each server has `command`, `args`, and `env`, or one `url`.
A server with `"disabled": true` stays off.
The plugin connects at activation through the official `rmcp` SDK.
The plugin removes JSON Schema keywords that the provider contract does not accept.
The plugin closes each connection when the registry releases the plugin.

## Limits

The `max_steps` configuration limits tool steps. The default is 8 and the maximum is 64.
The driver truncates each tool result to 64 KiB.
A failed tool call returns its error text to the model.
The OpenRouter and custom protocols support tool calls.
The other protocols reject an agent request.
If the grants produce no tool, the driver acts as the single-reply driver.

## Verification

```sh
cargo test --locked -p rustytavern-cli --test agent
```
