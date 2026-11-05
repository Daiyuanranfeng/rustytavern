# Lighthouse edit extension

This extension adds a pure prompt transform through one native plugin.
The package ID is `example.lighthouse-edits`.
The [manifest](extension.json) owns the [setting plugin](plugins/README.md).
The plugin adds a blue-lamp setting to the prompt description.
The plugin does not change the stored character card.

## Interface and use

The plugin provides `prompt.hook.lighthouse/1` through the `prompt.edits` factory.
The host calls its pure transform after it builds the draft.
The host then selects context and validates the final request budget.

```sh
cargo run --locked -- --data-dir ./data extension install crates/tavern/examples/extensions/lighthouse-edits
cargo run --locked -- --data-dir ./data extension enable example.lighthouse-edits --grant session.read:current --grant prompt.transform:description
cargo run --locked -- --data-dir ./data session configure SESSION_ID
cargo run --locked -- --data-dir ./data prompt inspect SESSION_ID --message "Hello." --json
cargo run --locked -- --data-dir ./data extension disable example.lighthouse-edits
```

The inspection uses an existing session with a description section.
The enabled plan contains the additional blue-lamp text.
The phase trace records the edit when the runtime saves or replays a generation.
The commands above make no model request.
An existing session requires another configuration revision after plugin removal.

## Implementation

1. You must copy this directory and choose new package and plugin IDs.
2. You must declare a namespaced `prompt.hook.NAME/1` service.
3. You must configure edits in the plugin manifest.
4. You must request `session.read:current` and each target capability.
5. You must test prompt inspection and removal.
6. You must compare a saved generation with `prompt replay` after a conversation.

The [phase guide](../../../../../docs/developer.md#prompt-phases-and-replay) defines hook and edit semantics.
The [extension guide](../../../../../docs/developer.md#product-extensions) defines package lifecycle rules.
