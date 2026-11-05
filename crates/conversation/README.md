# Conversation records and prompts

The crate owns conversation records and pure prompt compilation.

## Interface

The crate exports session records and native export records.
The `provider` module defines provider settings and capabilities.
The `prompt` module defines prompt profiles, context selection, contributions, and saved enrichment.

## Design

The crate performs no network or database operation.
The modules share one package because prompts consume the same conversation records.
The runtime owns generation execution.

## Dependencies

The crate depends on core values, card formats, and serialization libraries.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) defines the dependency direction.

## Verification

```sh
cargo test --locked -p rustytavern --test foundation --test prompt_phases --test native_exports
```

The [worldbook guide](../../docs/user-guide.md#worldbooks) defines archive storage, session bindings, and prompt profile support.
