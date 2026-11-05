# Essential plugins

The crate holds every essential runtime plugin.
A plugin changes agent behavior or adds content to the agent context.
Each plugin family is one module.

## Interface

`essential` returns the plugins for the enabled IDs. The core services are always on.
`registry` activates them under the `core-product` owner and grants their requested capabilities.
`DEFAULT_ENABLED` lists the plugins that are on when `config.toml` has no `plugins.enabled` list.
`Builtin` runs one plugin from its factory name. An installed plugin can select the same factory names.
`Environment` supplies the store. Each plugin receives its `[plugins.ID]` table as configuration.

| Plugin ID | Module | Service | Behavior |
| --- | --- | --- | --- |
| `decoder`, `profile`, `provider`, `store` | `lib` | Core services | The plugins decode cards, compile prompts, call models, and store records. They are always on. |
| `agent` | `drivers` | `driver/1` | The loop calls the model and runs requested tools until a reply has no tool calls. |
| `single-reply` | `drivers` | `driver/1` | The driver sends one text request. It answers when `agent` is off. |
| `web` | `tools::web` | `tools.web/1` | The `web_fetch` tool reads one public HTTPS page as plain text. |
| `skills` | `tools::skills` | `tools.skills/1` | The plugin adds the skill catalog to the prompt and registers the `skill` tool. |
| `mcp` | `tools::mcp` | `tools.mcp/1` | The plugin connects each configured server and registers its tools. |
| `chub`, `risurealm` | `sources` | `card.source.NAME/1` | The sources fetch character cards from their granted host. |

The `prompts` module supplies the `prompt.contributions` and `prompt.edits` factories for installed plugins.

## Agent and tools

Each tool plugin registers a `ToolSet` as a `tools.NAME/1` service.
The runtime collects every composed tool set and rejects a duplicate tool name.
The runtime adds the tool schemas to the saved request, so `prompt inspect --request` shows them.
The host runs each tool call and emits `tool_call` and `tool_result` events.
The host truncates each tool result to 64 KiB. A failed tool call returns its error text to the model.

The `[plugins.agent]` table has `max_steps`. It limits tool steps. The default is 8 and the maximum is 64.
The OpenRouter and custom protocols support tool calls. Other protocols receive the text request.

The web fetcher checks destination addresses and redirects. The tool removes HTML scripts, styles, and tags.

The `[plugins.skills]` table has `paths`. Each path holds `NAME/SKILL.md` skills with `name` and `description` front matter. The default path is `HOME/skills`.
The catalog goes to the `before_history` prompt slot. The model reads a full skill only when it needs it.
The optional `path` argument reads another file inside the skill directory.

Each `[plugins.mcp.servers.NAME]` table has `command`, `args`, and `env`, or one `url`.
A server with `disabled = true` stays off. The plugin connects through the official `rmcp` SDK at activation.
The plugin names each tool `SERVER__TOOL` and removes JSON Schema keywords that the provider contract does not accept.
A failed server connection stops the plugin activation. The plugin closes each connection when the registry releases it.

## Configuration

```toml
[plugins]
enabled = ["agent", "web", "skills", "mcp"]

[plugins.agent]
max_steps = 8

[plugins.mcp.servers.docs]
url = "https://example.com/mcp"
```

A plugin configuration change changes the plugin configuration hash. A session pins its composition, so it requires `session configure` before the next reply.

## Features

The default `mcp` feature includes the MCP plugin and the `rmcp` dependency.

## Verification

```sh
cargo test --locked -p rustytavern-cli --test agent
cargo test --locked -p rustytavern --test installed_plugins --test sources
```
