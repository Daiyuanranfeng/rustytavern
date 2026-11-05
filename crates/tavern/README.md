# RustyTavern product library

The crate assembles the character runtime and product features.

## Interface

The crate retains the public `rustytavern` library paths.
The `app` module defines extension packages, factories, commands, and application setup.
The `config` module defines provider profiles and the data directory.
The crate re-exports runtime, plugin, prompt, format, provider, source, and storage interfaces.

## Design

The product enables the SQLite and process features of its component dependencies.
The runtime remains independent of product packaging.
A plugin changes runtime behavior.
An extension packages product features and can include plugins.
The crate keeps those concepts in separate modules.

## Packages and examples

The `bundled` directory contains the standard website extensions.
The `examples/extensions` directory contains example product packages.
The `examples/library` directory contains the conversation example.
The `examples/plugins` directory contains integration examples and author guides.
The [example index](examples/README.md) defines their use.
The [extension guide](../../docs/developer.md#product-extensions) defines the package interface.

## Verification

```sh
cargo test --locked -p rustytavern
cargo run --locked -p rustytavern --example feature_services
```
