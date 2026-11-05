# MCP tool plugin

The plugin ID is `org.rustytavern.tools.mcp`.
The factory `tools.mcp` reads `DATA_DIR/mcp.json` at activation.
The file uses the common `mcpServers` format.
Each server has `command`, `args`, and `env`, or one `url`.
A server with `"disabled": true` stays off.

The plugin connects through the official `rmcp` SDK.
The plugin registers each server tool as `SERVER__TOOL` in `tools.mcp/1`.
The plugin removes JSON Schema keywords that the provider contract does not accept.
The plugin closes each connection when the registry releases the plugin.
A failed server connection stops the plugin activation.
