# Agent extension

The agent extension replaces the single-reply driver with a tool loop.
The extension owns one [driver plugin](plugins/README.md).
The loop follows the minimal pi-agent-core design.
The loop calls the model, runs the requested tools, and stops at a reply without tool calls.

## Interface

The package ID is `org.rustytavern.extension.agent`.
The package binds `driver/1` to `org.rustytavern.driver.agent`.
Each grant adds one tool source.

| Grant | Tool | Source |
| --- | --- | --- |
| `network:https:*` | `web_fetch` | Public HTTPS pages |
| `skills.read` | `skill` | `DATA_DIR/skills/NAME/SKILL.md` in the Agent Skills format |
| `mcp.connect` | `SERVER__TOOL` | Servers in `DATA_DIR/mcp.json` |

The driver also requires `session.read:current`, `session.write:current`, and `provider.generate`.

```sh
cargo run --locked -- --data-dir ./data extension install --builtin agent
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.agent \
  --grant session.read:current --grant session.write:current --grant provider.generate \
  --grant 'network:https:*' --grant skills.read --grant mcp.connect
```

A session pins its driver.
An earlier session requires an explicit configuration change before it uses the agent.
The `chat --events` command prints a `tool_result` event for each tool call.

## Implementation

The [extension manifest](extension.json) declares the binding.
The [plugin manifest](plugins/driver.json) selects the compiled `driver.agent` factory.
The [extension guide](../../../../docs/developer.md#product-extensions) explains the package lifecycle.
