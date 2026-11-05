# Harbor asset extension

This extension provides an asset and a configuration default without a plugin.
The package ID is `example.harbor-assets`.
The [manifest](extension.json) declares `templates/harbor.txt` with its SHA-256 hash.
The package supplies `example.location: harbor` as a library default.
The package does not insert its template into a prompt automatically.

## Interface and use

The application reads the declared asset through `ExtensionManager::asset`.
The application reads defaults through `ExtensionManager::configuration`.
An explicit `example.location` value overrides the package default.

```sh
cargo run --locked -- --data-dir ./data extension install crates/tavern/examples/extensions/harbor-assets
cargo run --locked -- --data-dir ./data extension enable example.harbor-assets
cargo run --locked -- --data-dir ./data extension asset example.harbor-assets templates/harbor.txt --output ./harbor.txt
cargo run --locked -- --data-dir ./data extension disable example.harbor-assets
cargo run --locked -- --data-dir ./data extension uninstall example.harbor-assets
```

The commands require no grants, process trust, or model credentials.
The manager refuses an asset read after disable.

## Implementation

1. Copy this directory and choose a new package ID.
2. Replace or add assets under relative paths.
3. Update each asset hash with `shasum -a 256 PATH`.
4. Declare application defaults in `configuration_defaults`.
5. Connect those defaults to the feature in the host application.
6. Verify installation, restart, asset access, and disable.

The [extension guide](../../../../../docs/developer.md#product-extensions) defines the complete package contract.
