# Generation runtime

The crate coordinates durable character generations.

## Interface

`Runtime`, `GenerationHandle`, `CompiledRequest`, runtime events, and `driver::ConversationDriver`.

## Design

The runtime freezes inputs and plugin composition. The runtime saves the request before dispatch. The runtime saves drafts before delivery and settles each generation.

## Dependencies

The crate depends on plugin contracts, model providers, prompt compilation, and storage contracts. The product extension layer depends on this crate.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) records the complete graph.

## Verification

The repository root supports this command.

```sh
cargo test --locked -p rustytavern-runtime
```
