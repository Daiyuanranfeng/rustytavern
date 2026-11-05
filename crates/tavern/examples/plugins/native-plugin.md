# Native prompt plugin example

The example injects a Rust prompt plugin and then removes it.
The [complete source](../../../plugin/examples/native_plugin.rs) builds as a Cargo example.
The example uses a plugin registry and an empty prompt plan.
The example makes no model request.

## Design and interface

The `HarborNote` type implements `RuntimePlugin`.
The manifest identifies `example.native-note`.
The manifest requests `prompt.contribute:before_character`.
The `activate` method adds one system contribution through `PluginContext`.
The example reads that contribution from the frozen registry.

The example injects the plugin under the `example` owner with its grant.
The example applies the contribution to a copy of the original prompt.
The example removes the owner and applies the remaining contributions to another copy.
The assertions compare prompt hashes at those boundaries.

```sh
cargo run --locked --example native_plugin
```

The program prints this JSON after the assertions pass.

```json
{"ejection_restores_prompt":true,"model_requests":0,"plugin_changes_prompt":true}
```

## Implementation

1. Copy `native_plugin.rs` into another Cargo example or embedded application.
2. Change the plugin ID, section ID, and contribution text.
3. Keep the requested capability consistent with the declared slot.
4. Add service declarations only if the plugin provides or requires a service.
5. Register owned resources through `PluginContext::own` when the plugin acquires resources.
6. Verify activation failure and removal as described in the [plugin guide](../../../../docs/developer.md#runtime-plugins).

The stock CLI cannot load this Rust type from a package at runtime.
An embedded application can expose the type through a registered `NativeFactory`.
An independent process can use the [external source example](../extensions/inline-source/README.md).
