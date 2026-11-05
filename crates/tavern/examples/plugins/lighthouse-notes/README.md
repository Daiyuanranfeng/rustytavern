# Lighthouse note plugin

The plugin adds two static prompt contributions.
The plugin ID is `lighthouse-notes`.
The [plugin manifest](plugin.json) selects the compiled `prompt.contributions` factory.

## Interface

| Section | Slot | Required grant |
| --- | --- | --- |
| `world` | `before_character` | `prompt.contribute:before_character` |
| `voice` | `after_character` | `prompt.contribute:after_character` |

Each contribution defines `section_id`, `slot`, `priority`, `role`, and `text`.
The example uses an empty `plugin_id` because the host assigns that value.
The core profile accepts the `system` role in these slots.
The plugin provides no typed services and requires no network access.

```sh
cargo run --locked -- --data-dir ./data plugin install crates/tavern/examples/plugins/lighthouse-notes
cargo run --locked -- --data-dir ./data plugin enable lighthouse-notes --grant prompt.contribute:before_character --grant prompt.contribute:after_character
cargo run --locked -- --data-dir ./data session configure SESSION_ID
cargo run --locked -- --data-dir ./data prompt inspect SESSION_ID --message "Hello." --json
cargo run --locked -- --data-dir ./data plugin disable lighthouse-notes
```

Use an existing session ID from `session new`. Prompt inspection makes no model request.
The enabled prompt contains the `plugin:lighthouse-notes:world` and `plugin:lighthouse-notes:voice` sections.

## Implementation

1. Copy this directory and change the plugin ID.
2. Edit the contribution text in `plugin.json`.
3. Keep each slot consistent with its requested capability and allowed slot.
4. Test prompt inspection before enable, after enable, and after disable.

The [plugin guide](../../../../../docs/developer.md#runtime-plugins) defines contribution order and lifecycle.

```sh
cargo test --locked --test plugins --test installed_plugins
```
