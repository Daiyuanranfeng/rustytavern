# Lighthouse setting plugin

The plugin ID is `example.lighthouse.setting`.
The plugin provides `prompt.hook.lighthouse/1`.
The implementation selects the compiled `prompt.edits` factory.
The [manifest](setting.json) contains its complete configuration.

The configuration contains `policy` and `edits`.
Each edit declares a target section, mode, and text.
This plugin appends text to `description`.
The plugin requests `session.read:current` and `prompt.transform:description`.
The host rejects an ungranted or missing target.
The host rejects transcript edits and competing replacements.

An author can change the target and its matching grant to create another pure edit plugin.
An author must implement `PromptHook` in Rust when the feature needs dynamic enrichment or custom edit logic.
The [phase guide](../../../../../../docs/developer.md#prompt-phases-and-replay) defines that native interface.
The [package README](../README.md) supplies complete CLI commands.

## Lifecycle and verification

The extension manager activates this plugin when the package becomes enabled.
The registry keeps the plugin while another owner or operation needs it.
The registry releases its resources after its final owner and operation release it.
The [plugin guide](../../../../../../docs/developer.md#runtime-plugins) defines the complete lifecycle.

```sh
cargo test --locked --test prompt_phases --test cli
```

The tests check edit grants, missing targets, edit conflicts, cancellation, and replay.
