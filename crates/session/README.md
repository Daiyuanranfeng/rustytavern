# Sessions, prompts, and storage

The crate owns conversation records, pure prompt compilation, and durable storage.

## Interface

- The crate root exports session records and native export records.
- The `provider` module defines provider settings and capabilities.
- The `prompt` module defines prompt profiles, context selection, contributions, and saved enrichment.
- The `store` module defines `StoreBackend` and exports `SqliteStore`.

## Design

Prompt compilation performs no network or database operation.
The storage contract stays independent of the SQLite implementation.
The SQLite module owns transactions, migrations, fencing, and the [SQL schema](src/store/sqlite/schema.sql). The current schema version is 5.
The runtime crate owns generation execution.

## Verification

```sh
cargo test --locked -p rustytavern --test foundation --test prompt_phases --test native_exports --test plugin_state
```

The [worldbook guide](../../docs/user-guide.md#worldbooks) defines archive storage, session bindings, and prompt profile support.
