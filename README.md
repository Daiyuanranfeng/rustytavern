# RustyTavern

RustyTavern provides a Rust library and CLI for persistent character conversations.
The project targets a local character-agent harness.
The project has no web backend or frontend.

The implementation is in progress.
The current CLI supports cards, worldbooks, personas, prompt inspection, streaming, sessions, alternatives, and branches.
The product supports native runtime plugins and external card-source processes.
A plugin changes agent behavior or adds content to the agent context.
The essential plugins include the agent loop, web fetch, skills, MCP tools, and the Chub and RisuRealm sources.
The CLI installs, enables, inspects, and removes third-party plugins.
An extension changes only the application appearance, such as the bubble colors.
The project does not claim full SillyTavern or RisuAI parity.

The [workspace guide](docs/architecture.md#workspace-boundaries) defines the component crates and their dependency direction.

## Build and check

The `rust-toolchain.toml` file pins Rust 1.99.1.
The declared minimum is Rust 1.99.
The build uses the resolved Cargo.lock file.

```sh
cargo build --locked --workspace
cargo test --locked --workspace --all-targets
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

The tests use local model endpoints.
The tests do not require an API key.
The tests do not make paid model calls.

## Start a conversation

The binary opens an interactive terminal without a subcommand.

```sh
cargo run --locked -p rustytavern-cli
```

The `/provider` command connects a model provider. The `/model` command selects a model.
The `/character` and `/persona` commands select identities.
The `/new` command starts a chat. The `/settings` command opens the other controls.
The [terminal guide](docs/user-guide.md#interactive-terminal) defines commands, keys, and persistence.

## Script commands

The default home directory is `~/.rustytavern`. The examples use `./home` instead.
The home directory holds `config.toml` for every setting and `rustytavern.db` for durable records.
The `--api-key KEY` option stores the key in `config.toml`. RustyTavern does not read keys from environment variables.
The [configuration guide](docs/user-guide.md#configuration-file) shows the complete file.

```sh
cargo run -- --home ./home config provider \
  --name openrouter --kind openrouter \
  --model openai/gpt-4.1-nano --output-limit 256 --api-key sk-or-...
cargo run -- --home ./home card import crates/tavern/examples/lumen.json --json
cargo run -- --home ./home persona create \
  --name Alex --description "Alex charts the coast."
```

The import and persona commands print their IDs as JSON.
The next command uses those IDs.

```sh
rustytavern --home ./home session new \
  --character CHARACTER_ID --persona PERSONA_ID --provider openrouter
rustytavern --home ./home prompt inspect SESSION_ID \
  --message "Hello." --request
rustytavern --home ./home chat SESSION_ID \
  --message "Hello." --events --idempotency-key first-message
rustytavern --home ./home session show SESSION_ID --json
```

The last commands require the built binary on PATH.
The same commands work through `cargo run --`.
A later process can continue the same session.
The caller should reuse an idempotency key only for the same submission.

## Inspect, branch, and export

```sh
rustytavern --home ./home session regenerate SESSION_ID --events
rustytavern --home ./home session branch SESSION_ID --at MESSAGE_ID
rustytavern --home ./home session edit SESSION_ID \
  --at MESSAGE_ID --message "Another direction."
rustytavern --home ./home session export SESSION_ID --output session.json
rustytavern --home ./home card export CHARACTER_ID --output original.png
```

An original card export returns the exact imported bytes.
The output filename does not convert the card format.
An edited export accepts an explicit JSON merge patch through `--patch PATH`.
The export keeps unknown JSON values unless the patch changes them.
The native session export retains branches, generations, drafts, revision archives, and plugin state.
The `--include-traces` flag adds compiled request snapshots.
The [native format guide](docs/user-guide.md#native-records-and-exports) defines session and persona exports.

## Documentation

- The [product guide](docs/product.md) defines required features and current support.
- The [architecture guide](docs/architecture.md) defines components, host ownership, dependencies, and figures.
- The [user guide](docs/user-guide.md) defines terminal commands, scripts, configuration, worldbooks, and exports.
- The [developer guide](docs/developer.md) defines plugins, extensions, prompt hooks, state, and process frames.
- The [compatibility guide](docs/compatibility.md) defines formats, providers, reference comparisons, and evidence.

Each [component](docs/architecture.md#current-implementation) retains its README.
The [plugin crate](crates/plugins/README.md) and each [example plugin](crates/tavern/examples/README.md) retain their interface documents.

## License and source use

The repository remains private.
The project has no distribution license yet.
The investigation reads pinned SillyTavern and RisuAI sources.
The implementation does not copy their source files into the tracked repository.
The project makes no clean-room claim.
Research copies, comparison captures, and temporary plans remain in ignored `.rustify/`.

The [worldbook guide](docs/user-guide.md#worldbooks) defines archive import, export, session bindings, and profile limits.
