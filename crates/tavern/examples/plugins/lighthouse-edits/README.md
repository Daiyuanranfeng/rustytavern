# Lighthouse edit plugin

The plugin adds a pure prompt transform.
The plugin ID is `example.lighthouse-edits`.
The [plugin manifest](plugin.json) selects the compiled `prompt.edits` factory.
The plugin appends a blue-lamp setting to the prompt description.
The plugin does not change the stored character card.

## Interface

The plugin provides `prompt.hook.lighthouse/1`.
The configuration contains `policy` and `edits`. Each edit declares a target section, mode, and text.
The plugin requests `session.read:current` and `prompt.transform:description`.
The host rejects an ungranted or missing target, transcript edits, and competing replacements.
The host calls the transform after it builds the draft. The host then selects context and checks the request budget.

```sh
cargo run --locked -- --data-dir ./data plugin install crates/tavern/examples/plugins/lighthouse-edits
cargo run --locked -- --data-dir ./data plugin enable example.lighthouse-edits --grant session.read:current --grant prompt.transform:description
cargo run --locked -- --data-dir ./data session configure SESSION_ID
cargo run --locked -- --data-dir ./data prompt inspect SESSION_ID --message "Hello." --json
cargo run --locked -- --data-dir ./data plugin disable example.lighthouse-edits
```

The enabled plan contains the blue-lamp text. The commands make no model request.

## Implementation

1. Copy this directory and choose a new plugin ID.
2. Declare a namespaced `prompt.hook.NAME/1` service.
3. Configure the edits in `plugin.json`.
4. Request `session.read:current` and each target capability.
5. Test prompt inspection, removal, and `prompt replay`.

Implement `PromptHook` in Rust when the feature needs dynamic enrichment or custom edit logic.
The [phase guide](../../../../../docs/developer.md#prompt-phases-and-replay) defines that interface.

```sh
cargo test --locked --test prompt_phases
```
