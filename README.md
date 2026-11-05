# RustyTavern

RustyTavern provides a Rust library and CLI for persistent character conversations.
The project targets a local character-agent harness.
The project has no web backend or frontend.

The implementation is in progress.
The current CLI supports cards, worldbooks, personas, prompt inspection, streaming, sessions, alternatives, and branches.
The product supports native runtime plugins and external card-source processes.
The CLI installs, enables, inspects, and removes extension packages.
The product supplies optional Chub and RisuRealm source extensions.
The project does not claim full SillyTavern or RisuAI parity.

The [workspace guide](docs/architecture.md#workspace-boundaries) defines the component crates and their dependency direction.

## Build and check

The current development environment uses Rust 1.94.
The declared minimum is Rust 1.85.
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

The `/setup` command opens the model provider form.
The `/character` and `/persona` commands select identities.
The `/new` command starts a conversation.
The `/plugins` and `/extensions` commands open product controls.
The [terminal guide](docs/user-guide.md#interactive-terminal) defines commands, keys, and persistence.

## Script commands

The examples use `./data` for durable records.
The OpenRouter adapter reads `OPENROUTER_API_KEY` from the process environment.
The configuration stores the variable name.
The configuration does not store the key value.

```sh
cargo run -- --data-dir ./data config provider \
  --name openrouter --kind openrouter \
  --model openai/gpt-4.1-nano --output-limit 256
cargo run -- --data-dir ./data card import crates/tavern/examples/lumen.json --json
cargo run -- --data-dir ./data persona create \
  --name Alex --description "Alex charts the coast."
```

The import and persona commands print their IDs as JSON.
The next command uses those IDs.

```sh
rustytavern --data-dir ./data session new \
  --character CHARACTER_ID --persona PERSONA_ID --provider openrouter
rustytavern --data-dir ./data prompt inspect SESSION_ID \
  --message "Hello." --request
rustytavern --data-dir ./data chat SESSION_ID \
  --message "Hello." --events --idempotency-key first-message
rustytavern --data-dir ./data session show SESSION_ID --json
```

The last commands require the built binary on PATH.
The same commands work through `cargo run --`.
A later process can continue the same session.
The caller should reuse an idempotency key only for the same submission.

## Inspect, branch, and export

```sh
rustytavern --data-dir ./data session regenerate SESSION_ID --events
rustytavern --data-dir ./data session branch SESSION_ID --at MESSAGE_ID
rustytavern --data-dir ./data session edit SESSION_ID \
  --at MESSAGE_ID --message "Another direction."
rustytavern --data-dir ./data session export SESSION_ID --output session.json
rustytavern --data-dir ./data card export CHARACTER_ID --output original.png
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
Each [bundled extension](crates/tavern/bundled/README.md) and [example package](crates/tavern/examples/README.md) retains its interface document.

## License and source use

The repository remains private.
The project has no distribution license yet.
The investigation reads pinned SillyTavern and RisuAI sources.
The implementation does not copy their source files into the tracked repository.
The project makes no clean-room claim.
Research copies, comparison captures, and temporary plans remain in ignored `.rustify/`.

The [worldbook guide](docs/user-guide.md#worldbooks) defines archive import, export, session bindings, and profile limits.
