# Lighthouse prompt plugins

These two plugins show a configurable native factory without new Rust code.

| Manifest | Plugin ID | Slot | Required grant |
| --- | --- | --- | --- |
| [world.json](world.json) | `example.lighthouse.world` | `before_character` | `prompt.contribute:before_character` |
| [voice.json](voice.json) | `example.lighthouse.voice` | `after_character` | `prompt.contribute:after_character` |

Each manifest selects `prompt.contributions` through `implementation.name`.
Each configuration contains a `contributions` array.
Each contribution defines `section_id`, `slot`, `priority`, `role`, and `text`.
The example uses an empty `plugin_id` because the host assigns that value.
The core profile accepts the `system` role in these slots.
The plugins provide no typed services.
The plugins require no network access.

An author can add another contribution with a unique section ID.
The author must declare and request any new slot before activation.
The host checks the matching grant during activation.
The host treats the resulting text as required prompt context.

The [package README](../README.md) supplies executable CLI commands.
The [plugin guide](../../../../../../docs/developer.md#runtime-plugins) defines the native lifecycle.
The [standard plugin reference](../../../../../../docs/developer.md#standard-plugins) defines the factory.

## Lifecycle and verification

The extension manager activates this plugin when the package becomes enabled.
The registry keeps the plugin while another owner or operation needs it.
The registry releases its resources after its final owner and operation release it.
The [plugin guide](../../../../../../docs/developer.md#runtime-plugins) defines the complete lifecycle.

```sh
cargo test --locked --test plugins --test extensions
```

The tests check contribution order, denied grants, activation rollback, and shared ownership.
