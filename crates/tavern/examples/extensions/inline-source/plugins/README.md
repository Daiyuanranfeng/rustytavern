# Inline source plugin

The plugin ID is `example.source.inline`.
The plugin provides `card.source.example/1` through the `card.source/1` process family.
The executable path is `{package}/source-plugin`.
The source accepts HTTPS URLs on `example.com`.
The source returns a validated inline card through JSON-RPC.
The source accepts cancellation notifications without a response.
The source exits when the host sends `shutdown`.

The plugin requests no capabilities.
The plugin does not use the host fetch callback.
The accepted host list controls URL recognition only.
The extension still requires explicit process trust.

The [complete Rust source](../../../../../plugin/examples/source_plugin.rs) implements the handshake and source operation.
The [package README](../README.md) supplies build and installation commands.
The [protocol guide](../../../../../../docs/developer.md#external-card-source-protocol) specifies framing, limits, callbacks, and errors.
An author can replace the response logic while retaining that protocol interface.

## Lifecycle and verification

The extension manager activates this plugin when the package becomes enabled.
The registry keeps the plugin while another owner or operation needs it.
The registry releases its resources after its final owner and operation release it.
The [plugin guide](../../../../../../docs/developer.md#runtime-plugins) defines the complete lifecycle.

```sh
cargo test --locked --test external --test process_lifecycle --test extensions
```

The tests check handshake failures, timeouts, bounded transport, and artifact scope.
The Unix process tests verify graceful exit and forced child cleanup.
