# Inline source extension

This extension runs a separate Rust card-source process.
The process returns the authored Lumen card for an accepted example URL.
The process makes no network request or model request.
The package ID is `example.inline-source`.

## Design and interface

The [extension manifest](extension.json) declares `example-import`.
The command calls `card.source.example/1` with the `fetch` operation.
The [plugin manifest](plugins/source.json) selects `{package}/source-plugin`.
The [plugin README](plugins/README.md) defines the process boundary.
The [Rust source](../../../../plugin/examples/source_plugin.rs) implements JSON-RPC over stdio.
The host validates and stores the returned card with provenance.

## Build and use

Run these commands from the repository root on macOS or Linux.
The temporary directory holds the platform-specific binary.

```sh
cargo build --locked --example source_plugin
rustytavern_example_dir=$(mktemp -d)
cp -R crates/tavern/examples/extensions/inline-source/. "$rustytavern_example_dir/"
cp target/debug/examples/source_plugin "$rustytavern_example_dir/source-plugin"
cargo run --locked -- --data-dir ./data extension install "$rustytavern_example_dir"
cargo run --locked -- --data-dir ./data extension enable example.inline-source --trust-processes
cargo run --locked -- --data-dir ./data extension run example-import --url https://example.com/lumen
cargo run --locked -- --data-dir ./data card fetch https://example.com/lumen --source example
cargo run --locked -- --data-dir ./data extension disable example.inline-source
cargo run --locked -- --data-dir ./data extension uninstall example.inline-source
rm -rf "$rustytavern_example_dir"
```

The host requires process trust even though this example uses no network grant.
The host clears the child environment except for `PATH`.
The host does not provide an operating-system sandbox.
The package executable must match the host platform.
The Windows build requires an executable filename and manifest path for that platform.

## Implementation

1. Copy the Rust source into a separate binary project.
2. Keep bounded newline framing and request ID copying.
3. Replace the inline response with your source behavior.
4. Update the plugin ID, accepted hosts, package ID, and command name.
5. Add requested capabilities only for host callbacks that the plugin uses.
6. Build the executable and copy it into the package before installation.
7. Test invalid handshake, timeout, cancellation, and malformed responses.

The [protocol guide](../../../../../docs/developer.md#external-card-source-protocol) defines every supported message.
The [extension guide](../../../../../docs/developer.md#product-extensions) defines package activation and restart.
