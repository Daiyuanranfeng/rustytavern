# Runtime plugins

The crate defines native plugin contracts and optional process support.

## Interface

`RuntimePlugin`, `PluginContext`, and `PluginRegistry` define activation and composition.
The `prompt` module defines prompt hooks.
The `process` feature exports the external card-source host in the `process` module.
The default feature set contains no external process adapter.

## Design

The native registry owns grants, dependencies, service bindings, and composition leases.
The process module owns child lifetime, cancellation, JSON-RPC, and scoped artifacts.
The crate has no runtime or product package dependency.
The [plugin guide](../../docs/developer.md#runtime-plugins) defines author interfaces.
The [protocol guide](../../docs/developer.md#external-card-source-protocol) defines process messages.

## Examples

The `examples` directory contains native, state migration, and external source examples.

```sh
cargo run --locked -p rustytavern-plugin --example native_plugin
cargo build --locked -p rustytavern-plugin --example source_plugin
```

## Verification

```sh
cargo check --locked -p rustytavern-plugin --no-default-features
cargo test --locked -p rustytavern --test plugins --test plugin_state --test external --test process_lifecycle
```
