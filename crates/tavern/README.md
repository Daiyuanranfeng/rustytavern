# RustyTavern product library

The crate assembles the product: the application, configuration, extensions, and the bundled plugins.

## Interface

The crate keeps the public `rustytavern` library paths. It re-exports the core, session, and runtime crates.
The `app` module installs third-party plugins and defines factories, commands, and application setup.
The `extension` module defines application extensions.
The `config` module defines `config.toml`, the home directory, and the API key resolver.
The `plugins` module holds every essential plugin. A plugin changes agent behavior or adds content to the agent context.
An extension changes only the application appearance.

## Essential plugins

`essential` returns the essential plugins. Every essential plugin is always on.
`registry` activates them under the `core-product` owner and grants their requested capabilities.
`Builtin` runs one plugin from its factory name. An installed plugin can select the same factory names.
`Environment` supplies the store. Each plugin receives its `[plugins.ID]` table as configuration.

| Plugin ID | Module | Service | Behavior |
| --- | --- | --- | --- |
| `decoder`, `profile`, `provider`, `store` | `lib` | Core services | The plugins decode cards, compile prompts, call models, and store records. They are always on. |
| `agent` | `drivers` | `driver/1` | The loop calls the model and runs requested tools until a reply has no tool calls. It is the only conversation mode. |
| `web` | `tools::web` | `tools.web/1` | The `web_fetch` tool reads one public HTTPS page as plain text. The plugin adds the tool-use guide to the prompt. |
| `skills` | `tools::skills` | `tools.skills/1` | The plugin adds the skill catalog to the prompt and registers the `read_skill` tool. |
| `mcp` | `tools::mcp` | `tools.mcp/1` | The plugin connects each configured server and registers its tools. |
| `chub`, `risurealm` | `sources` | `card.source.NAME/1` | The sources fetch character cards from their granted host. |

The `prompts` module supplies the `prompt.contributions` and `prompt.edits` factories for installed plugins.


## Agent and tools

Each tool plugin registers a `ToolSet` as a `tools.NAME/1` service.
The runtime collects every composed tool set and rejects a duplicate tool name.
The runtime adds the tool schemas to the saved request, so `prompt inspect --request` shows them.
The Rig agent loop runs each tool call. The host emits `tool_call` and `tool_result` events.
The host truncates each tool result to 64 KiB. A failed tool call returns its error text to the model.

The `[plugins.agent]` table holds the run policy. One Rig hook applies it:
- `max_steps` limits tool rounds. The default is 32 and the maximum is 64.
- `confirm_tools` lists the tools that wait for user approval.
- `retry_truncated` repeats a truncated reply with a doubled output cap.
- `max_tokens` sets a token budget for one reply.

Rig runs up to four tool calls of one turn at the same time.
Every built-in protocol supports tool calls. Rig maps the tool calls and results to the provider terms:

| Protocol | Tool definition | Call | Result |
| --- | --- | --- | --- |
| OpenRouter, custom | `tools[].function` | `tool_calls` | `role: "tool"` message |
| OpenAI Responses | `tools[]` with `strict: false` | `function_call` item | `function_call_output` item |
| Anthropic Messages | `tools[].input_schema` | `tool_use` block | `tool_result` block |
| Gemini | `functionDeclarations[].parametersJsonSchema` | `functionCall` part | `functionResponse` part |

Rig returns the Gemini `thoughtSignature` of each call in the next request.

The web fetcher checks destination addresses and redirects. The tool removes HTML scripts, styles, and tags.

The `[plugins.skills]` table has `paths`. Each path holds `NAME/SKILL.md` skills with `name` and `description` front matter. The default path is `HOME/skills`.
The catalog goes to the `before_history` prompt slot. The model reads a full skill only when it needs it.
The optional `path` argument reads another file inside the skill directory.

Each `[plugins.mcp.servers.NAME]` table has `command`, `args`, and `env`, or one `url`.
A server with `disabled = true` stays off. The plugin connects through the official `rmcp` SDK at activation. `rig-rmcp` lists the tools, calls them, and maps their results.
The plugin names each tool `SERVER__TOOL`. Rig sends each tool schema as the server declares it and adapts it to the protocol, for example for Anthropic.
A failed server connection stops the plugin activation. The plugin closes each connection when the registry releases it.

## Configuration

```toml
[plugins.agent]
max_steps = 8
confirm_tools = ["echo__say"]

[plugins.mcp.servers.docs]
url = "https://example.com/mcp"
```

A plugin configuration change changes the plugin configuration hash. A session pins its composition, so it requires `session configure` before the next reply.

## Features

The default `mcp` feature includes the MCP plugin and the `rmcp` dependency.

## Examples

The `examples/library` directory contains the conversation example.
The `examples/plugin` directory contains the native plugin, state migration, and external source examples.
The `examples/plugins` directory contains installable plugins, integration examples, and author guides.
The [example index](examples/README.md) defines their use.

## Verification

```sh
cargo test --locked -p rustytavern
cargo test --locked -p rustytavern-cli --test agent
cargo run --locked -p rustytavern --example feature_services
cargo run --locked -p rustytavern --example native_plugin
```
