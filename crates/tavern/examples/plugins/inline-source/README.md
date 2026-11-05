# Inline source plugin

The plugin runs a separate Rust card-source process.
The process returns the authored Lumen card for an accepted example URL.
The process makes no network request or model request.
The plugin ID is `inline-source`.

## Interface

The [plugin manifest](plugin.json) selects `{plugin}/source-plugin` and declares the `example-import` command.
The plugin provides `card.source.example/1` through the `card.source/1` process family.
The command calls that service with the `fetch` operation.
The source accepts HTTPS URLs on `example.com`. The accepted host list controls URL recognition only.
The plugin requests no capabilities. The host still requires explicit process trust.
The [Rust source](../../../../plugin/examples/source_plugin.rs) implements JSON-RPC over stdio.

```sh
cargo build --locked --example source_plugin
rustytavern_example_dir=$(mktemp -d)
cp -R crates/tavern/examples/plugins/inline-source/. "$rustytavern_example_dir/"
cp target/debug/examples/source_plugin "$rustytavern_example_dir/source-plugin"
cargo run --locked -- --home ./home plugin install "$rustytavern_example_dir" --trust-processes
cargo run --locked -- --home ./home plugin run example-import --url https://example.com/lumen
cargo run --locked -- --home ./home card fetch https://example.com/lumen --source example
cargo run --locked -- --home ./home plugin disable inline-source
cargo run --locked -- --home ./home plugin uninstall inline-source
rm -rf "$rustytavern_example_dir"
```

The host clears the child environment except for `PATH`. The host does not provide an operating-system sandbox.
The executable must match the host platform.

## Implementation

1. Copy the Rust source into a separate binary project.
2. Keep bounded newline framing and request ID copying.
3. Replace the inline response with your source behavior.
4. Update the plugin ID, accepted hosts, and command name.
5. Build the executable and copy it into the plugin directory before installation.
6. Test invalid handshake, timeout, cancellation, and malformed responses.

The [protocol guide](../../../../../docs/developer.md#external-card-source-protocol) defines every message.

```sh
cargo test --locked --test external --test process_lifecycle
```
