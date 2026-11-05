# Essential plugins

The crate holds every essential runtime plugin.
A plugin changes agent behavior or adds content to the agent context.
Each plugin family is one module.

## Interface

`essential` returns the plugins of the standard agent.
`registry` activates them under the `core-product` owner and grants their requested capabilities.
`Builtin` runs one plugin from its factory name. A package manifest can select the same factory names.
`Environment` supplies the store and the data directory.

| Module | Factory | Service | Behavior |
| --- | --- | --- | --- |
| `lib` | `decoder.cc-v2-v3`, `profile.core-v1`, `provider.http`, `store.sqlite` | Core services | The plugins decode cards, compile prompts, call models, and store records. |
| `drivers` | `driver.agent` | `driver/1` | The loop calls the model and runs requested tools until a reply has no tool calls. |
| `drivers` | `driver.single-reply` | `driver/1` | The driver sends one text request. The standard agent does not activate it. |
| `tools::web` | `tools.web` | `tools.web/1` | The `web_fetch` tool reads one public HTTPS page as plain text. |
| `tools::skills` | `tools.skills` | `tools.skills/1` | The plugin adds the skill catalog to the prompt and registers the `skill` tool. |
| `tools::mcp` | `tools.mcp` | `tools.mcp/1` | The plugin connects each server in `DATA_DIR/mcp.json` and registers its tools. |
| `sources` | `source.chub`, `source.risurealm` | `card.source.NAME/1` | The sources fetch character cards from their granted host. |
| `prompts` | `prompt.contributions`, `prompt.edits` | Prompt slots and hooks | The plugins add static prompt text or static section edits. |

## Agent and tools

Each tool plugin registers a `ToolSet` as a `tools.NAME/1` service.
The runtime collects every composed tool set and rejects a duplicate tool name.
The runtime adds the tool schemas to the saved request, so `prompt inspect --request` shows them.
The host runs each tool call and emits `tool_call` and `tool_result` events.
The host truncates each tool result to 64 KiB. A failed tool call returns its error text to the model.

The agent `max_steps` configuration limits tool steps. The default is 8 and the maximum is 64.
The OpenRouter and custom protocols support tool calls. Other protocols receive the text request.

The web fetcher checks destination addresses and redirects. The tool removes HTML scripts, styles, and tags.

Each skill is `DATA_DIR/skills/NAME/SKILL.md` with `name` and `description` front matter.
The catalog goes to the `before_history` prompt slot. The model reads a full skill only when it needs it.
The optional `path` argument reads another file inside the skill directory.

The MCP file uses the common `mcpServers` format. Each server has `command`, `args`, and `env`, or one `url`.
A server with `"disabled": true` stays off. The plugin connects through the official `rmcp` SDK at activation.
The plugin names each tool `SERVER__TOOL` and removes JSON Schema keywords that the provider contract does not accept.
A failed server connection stops the plugin activation. The plugin closes each connection when the registry releases it.

## Configuration

The `plugins.disabled` setting in `config.json` lists the essential plugin IDs that stay off.

```json
{"schema_version": 1, "plugins": {"disabled": ["org.rustytavern.tools.mcp"]}}
```

A session pins its composition. A changed plugin set requires `session configure` before the next reply.

## Features

The default `mcp` feature includes the MCP plugin and the `rmcp` dependency.

## Verification

```sh
cargo test --locked -p rustytavern-cli --test agent
cargo test --locked -p rustytavern --test packages --test sources
```
