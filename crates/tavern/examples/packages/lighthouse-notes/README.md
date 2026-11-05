# Lighthouse note package

This package owns two static prompt plugins.
The package ID is `example.lighthouse-notes`.
The [manifest](manifest.json) names both plugin manifests.
The package does not add commands or assets.

## Design and interface

The `example.lighthouse.world` plugin adds setting text before the character.
The `example.lighthouse.voice` plugin adds reply guidance after the character.
Both plugins use the compiled `prompt.contributions` factory.
The [plugin README](plugins/README.md) defines their configuration and grants.
Disabling the package removes both contributions through their shared package owner.

```sh
cargo run --locked -- --data-dir ./data package install crates/tavern/examples/packages/lighthouse-notes
cargo run --locked -- --data-dir ./data package enable example.lighthouse-notes --grant prompt.contribute:before_character --grant prompt.contribute:after_character
cargo run --locked -- --data-dir ./data plugin inspect
cargo run --locked -- --data-dir ./data session configure SESSION_ID
cargo run --locked -- --data-dir ./data prompt inspect SESSION_ID --message "Hello." --json
cargo run --locked -- --data-dir ./data package disable example.lighthouse-notes
cargo run --locked -- --data-dir ./data session configure SESSION_ID
cargo run --locked -- --data-dir ./data prompt inspect SESSION_ID --message "Hello." --json
```

Use an existing session ID from `session new`.
Prompt inspection makes no model request.
The enabled prompt contains `plugin:example.lighthouse.world:world` and `plugin:example.lighthouse.voice:voice` sections.
The disabled prompt contains neither section.

## Implementation

1. Copy this directory and change the package and plugin IDs.
2. Edit the contribution text in each plugin manifest.
3. Keep each slot consistent with its requested capability and allowed slot.
4. Add each new plugin manifest to `manifest.json`.
5. Test prompt inspection before enable, after enable, and after disable.

The [package guide](../../../../../docs/developer.md#plugin-packages) defines package ownership.
The [plugin guide](../../../../../docs/developer.md#runtime-plugins) defines contribution ordering and lifecycle.
