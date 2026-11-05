# Terminal interface

The crate supplies the `rustytavern` executable.

## Interface

The binary opens an interactive conversation by default.
Slash commands open configuration forms and selectors.
Script commands expose the same typed operations.
The [terminal guide](../../docs/user-guide.md#interactive-terminal) defines user tasks and keys.

## Design

The CLI owns terminal state and rendering.
The library owns product behavior and durable history.
The terminal and script modes share typed command handlers.
The terminal forwards cancellation and waits for durable completion.

`src/main.rs` defines startup and command records.
`src/batch.rs` adapts typed operations to script or captured output.
`src/terminal/` owns the event loop, forms, selections, and rendering.

The manifest declares one `[[bin]]` target.
The target name is `rustytavern`.
The virtual root manifest owns workspace configuration.

## Dependencies

The crate uses Ratatui for rendering.
The crate uses Crossterm for terminal events.
The crate uses `tui-textarea` for text editing.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) records the complete graph.

## Verification

The repository root supports this command.

```sh
cargo test --locked -p rustytavern-cli
cargo run --locked -p rustytavern-cli
```

The [worldbook guide](../../docs/user-guide.md#worldbooks) defines archive storage, session bindings, and prompt profile support.
