# Agent driver plugin

The plugin registers `Arc<dyn ConversationDriver>` as `driver/1`.
The plugin ID is `org.rustytavern.driver.agent`.
The factory name is `driver.agent`.
The driver policy ID is `agent/1`.

## Tool registry

Each tool plugin registers a `ToolSet` as a `tools.NAME/1` service.
The host collects every composed tool set and rejects a duplicate tool name.
The host adds the tool schemas to the saved request, so `prompt inspect --request` shows them.
The host runs each tool call and emits `tool_call` and `tool_result` events.
The host truncates each tool result to 64 KiB.
A failed tool call returns its error text to the model.

## Limits

The `max_steps` configuration limits tool steps. The default is 8 and the maximum is 64.
The OpenRouter and custom protocols support tool calls.
The other protocols reject a request with tools.
Without composed tools, the driver sends the saved text request.

## Verification

```sh
cargo test --locked -p rustytavern-cli --test agent
```
