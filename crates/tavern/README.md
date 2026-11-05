# RustyTavern product library

The crate assembles the character runtime and product features.

## Interface

The crate retains the public `rustytavern` library paths.
The `app` module defines plugin packages, factories, commands, and application setup.
The `extension` module defines application extensions.
The `config` module defines provider profiles, plugin settings, and the data directory.
The `plugins` path re-exports the [plugin crate](../plugins/README.md).
The crate re-exports runtime, plugin, prompt, format, provider, source, and storage interfaces.

## Design

The product enables the SQLite and process features of its component dependencies.
The runtime remains independent of product packaging.
A plugin changes agent behavior or adds content to the agent context.
A package delivers third-party plugins.
An extension changes only the application appearance.
The crate keeps those concepts in separate modules.

## Packages and examples

The `examples/packages` directory contains example plugin packages.
The `examples/library` directory contains the conversation example.
The `examples/plugins` directory contains integration examples and author guides.
The [example index](examples/README.md) defines their use.
The [package guide](../../docs/developer.md#plugin-packages) defines the package interface.

## Verification

```sh
cargo test --locked -p rustytavern
cargo run --locked -p rustytavern --example feature_services
```
