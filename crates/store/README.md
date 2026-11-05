# Storage

The crate defines atomic storage contracts and an optional SQLite adapter.

## Interface

`StoreBackend` defines sessions, generations, snapshots, recovery, and plugin state.
The `sqlite` feature exports `SqliteStore`.
The default feature set contains no database adapter.

## Design

The storage contract stays independent of a database implementation.
The optional SQLite module owns transactions, migrations, fencing, and the SQL schema.
The schema resides in [schema.sql](src/sqlite/schema.sql).
The adapter owns SQL migrations.
The current database schema version is 5.

## Dependencies

The default crate depends on core values, cards, conversation records, and serialization libraries.
The `sqlite` feature adds `rusqlite` and its core error conversion.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) defines the graph.

## Verification

```sh
cargo check --locked -p rustytavern-store --no-default-features
cargo test --locked -p rustytavern --test foundation --test native_exports --test plugin_state
```

The [worldbook guide](../../docs/user-guide.md#worldbooks) defines archive storage, session bindings, and prompt profile support.
