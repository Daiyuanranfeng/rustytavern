# RustyTavern product library

The crate assembles the character runtime and product features.

## Interface

The crate retains the public `rustytavern` library paths.
The `app` module installs third-party plugins and defines factories, commands, and application setup.
The `extension` module defines application extensions.
The `config` module defines `config.toml`, the home directory, and the API key resolver.
The `plugins` path re-exports the [plugin crate](../plugins/README.md).
The crate re-exports runtime, plugin, prompt, format, provider, source, and storage interfaces.

## Design

The product enables the SQLite and process features of its component dependencies.
The runtime remains independent of the product library.
A plugin changes agent behavior or adds content to the agent context.
An extension changes only the application appearance.
The crate keeps those concepts in separate modules.

## Examples

The `examples/library` directory contains the conversation example.
The `examples/plugins` directory contains installable plugins, integration examples, and author guides.
The [example index](examples/README.md) defines their use.
The [installed plugin guide](../../docs/developer.md#installed-plugins) defines the plugin directory interface.

## Verification

```sh
cargo test --locked -p rustytavern
cargo run --locked -p rustytavern --example feature_services
```
