# Generation runtime

The crate runs durable character generations with the [Rig](https://crates.io/crates/rig-agent) agent loop.

## Interface

- The crate root exports `Runtime`, `GenerationHandle`, `CompiledRequest`, and runtime events.
- The `agent` module exports `AgentDriver` and the `AgentTool` contract. The `driver` module exports `ConversationDriver` and `AgentEvent`.
- The `provider` module exports provider contracts, prepared requests, Rig model construction, the model catalog, and Rig error mapping.
- The `source` module exports public artifact fetching and the Chub and RisuRealm adapters.
- The `plugin` module exports `RuntimePlugin`, `PluginContext`, `PluginRegistry`, and prompt hooks. The `plugin::process` module hosts external card sources.
- The `phases` module defines the prompt hook phases.
- The `memory` module keeps the chat summary and the facts, and builds their prompt sections.

## Design

The runtime freezes inputs and plugin composition. The runtime saves the request before dispatch.
The runtime saves drafts before delivery and settles each generation.
Rig sends each request, parses each stream, and runs the tool loop. The runtime keeps tool composition, durable drafts, usage records, leases, and error codes.
The source module validates URL grants, addresses, redirects, and artifact limits.
The plugin registry owns grants, dependencies, service bindings, and composition leases.
The [plugin guide](../../docs/developer.md#runtime-plugins) defines author interfaces.
The [protocol guide](../../docs/developer.md#external-card-source-protocol) defines process messages.

## Verification

```sh
cargo test --locked -p rustytavern --test runtime --test agent_loop --test providers --test tool_provider --test sources --test plugins
```
