# Persistent state plugin example

The example upgrades a native plugin from state schema 1 to schema 2.
The [complete source](../../../plugin/examples/state_migration.rs) uses the public library API.
The example makes no model or website request.

## Design and interface

The plugin ID is `example.state-counter`.
The plugin implements `RuntimePlugin`.
The plugin requests `state.read:example.state-counter` and `state.write:example.state-counter`.
The plugin supplies no service or prompt contribution.
The host grants both state capabilities explicitly.

Schema 1 stores an integer under `counter`.
Schema 2 stores an object with a `count` field under the same key.
The migration returns that one replacement.
The host retains the omitted `opaque` key and all earlier revisions.
The host stores the schema marker under `@state-schema/example.state-counter`.
The example also creates a session so that the product CLI can export those records.

## Build and use

The caller must select a fresh data directory.
The failed migration must run before the successful upgrade.

```sh
cargo build --locked --example state_migration
rustytavern_state_dir=$(mktemp -d)
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" seed
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" fail
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" inspect
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" upgrade
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" upgrade
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" old-write
cargo run --locked --example state_migration -- --data-dir "$rustytavern_state_dir" inspect
```

The `fail` operation exits with code 1 and `migration_fixture`.
The following inspection still reports schema 1 and counter value 7.
The first `upgrade` reports schema 2 and `counter.value: {"count":7}`.
The second `upgrade` returns the same state without another migration revision.
The `old-write` operation exits with code 2 and `unsupported_plugin_state_schema`.
The last inspection retains the schema 2 values.

The seed output contains one session ID.
The caller can use that ID with the ordinary CLI.

```sh
cargo run --locked -- --data-dir "$rustytavern_state_dir" session export SESSION_ID --output state-session.json
```

The export contains the original counter, the upgraded counter, the opaque value, and the schema marker.
The stock CLI can export this session without the example plugin code.

## Lifecycle and implementation

The example injects one native plugin under the `example` owner.
The example removes that owner after the operation succeeds.
The store retains state after removal and process exit.
The host skips the callback when the stored schema already matches the manifest.

An author can copy `Counter` and choose another plugin ID.
The author must declare a positive `state_schema` and both state grants.
The author must validate each source schema before the callback converts its data.
The callback must return replacements and added keys.
The callback must retain unknown fields within each replacement when its format requires them.
The host retains keys that the callback omits.
The [state guide](../../../../docs/developer.md#plugin-state-schemas-and-migration) defines atomicity and concurrent updates.
The [plugin guide](../../../../docs/developer.md#runtime-plugins) defines native activation and grants.
The [extension guide](../../../../docs/developer.md#product-extensions) explains how an embedded host exposes the plugin through a compiled factory.

## Verification

```sh
cargo test --locked --test plugin_state
```

The tests check grants, rollback, panic, concurrent updates, competing upgrades, restart, replay, and opaque exports.
