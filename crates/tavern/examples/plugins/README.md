# Plugin examples

These examples implement runtime plugins.
Cargo compiles each example as a separate executable.
The [library examples](../README.md#library-conversation) show application use.
The [extension examples](../extensions) show product packages.

| Example | Interface | Guide |
| --- | --- | --- |
| `native_plugin` | The plugin contributes prompt sections. | [Native plugin](native-plugin.md) |
| `feature_services` | The plugins replace decoder, driver, and provider services. | [Feature services](feature-services.md) |
| `state_migration` | The plugin upgrades its persistent state. | [State migration](state-migration.md) |
| `source_plugin` | The plugin implements the external card-source protocol. | [Source package](../extensions/inline-source/README.md) |

The example names remain unchanged:

```sh
cargo run --locked --example native_plugin
cargo run --locked --example feature_services
cargo build --locked --example source_plugin
```

These commands make no paid model request.
