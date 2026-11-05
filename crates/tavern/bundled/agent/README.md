# Agent extension

The agent extension replaces the single-reply driver with a tool loop.
The extension owns one [driver plugin](plugins/README.md).
The loop follows the minimal pi-agent-core design.
The loop calls the model, runs the requested tools, and stops at a reply without tool calls.

## Interface

The package ID is `org.rustytavern.extension.agent`.
The package binds `driver/1` to `org.rustytavern.driver.agent`.
The extension adds no tool.
Tool extensions register tools on the shared plugin context.

| Extension | Tool | Grant |
| --- | --- | --- |
| [Web](../web/README.md) | `web_fetch` | `network:https:*` or `network:https:HOST` |
| [Skills](../skills/README.md) | `skill` | `skills.read` and `prompt.contribute:before_history` |
| [MCP](../mcp/README.md) | `SERVER__TOOL` | `mcp.connect` |

```sh
cargo run --locked -- --data-dir ./data extension install --builtin agent
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.agent \
  --grant session.read:current --grant session.write:current --grant provider.generate
```

A session pins its composition.
An earlier session requires an explicit configuration change before it uses the agent or a new tool.

## Implementation

The [extension manifest](extension.json) declares the binding.
The [plugin manifest](plugins/driver.json) selects the compiled `driver.agent` factory.
The [extension guide](../../../../docs/developer.md#product-extensions) explains the package lifecycle.
