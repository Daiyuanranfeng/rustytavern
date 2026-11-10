# AGENTS.md

This file gives instructions to AI coding agents. Human contributors read [CONTRIBUTING.md](CONTRIBUTING.md).

## Project

RustyTavern is a Rust library and terminal app for persistent character conversations.
It has no web server and no web frontend.
The [product roadmap](docs/product.md) defines the target features.

## Commands

Run these commands from the repository root. Use `--locked` on every Cargo command.

| Task | Command |
| --- | --- |
| Build | `cargo build --locked --workspace` |
| Test all | `cargo test --locked --workspace --all-targets` |
| Test one crate | `cargo test --locked -p rustytavern` |
| Format | `cargo fmt --all` |
| Lint | `cargo clippy --locked --workspace --all-targets -- -D warnings` |
| Dependency policy | `cargo deny check` |
| Run the terminal app | `cargo run --locked -p rustytavern-cli -- --home ./home` |

Run the format, lint, and test commands before you finish a task.
Do not run the ignored `live_providers` test. It sends paid requests.

## Layout

| Path | Content |
| --- | --- |
| `crates/core` | `rustytavern-core`: errors, identities, card and worldbook formats. No workspace dependency. |
| `crates/session` | `rustytavern-session`: sessions, prompt compilation, SQLite storage. |
| `crates/runtime` | `rustytavern-runtime`: Rig providers, the agent runtime, plugin contracts, prompt phases. |
| `crates/tavern` | `rustytavern`: the assembled library, configuration, extensions, and essential plugins. |
| `crates/cli` | `rustytavern-cli`: the `rustytavern` binary, the Ratatui terminal, and the script commands. |
| `docs/` | User, developer, architecture, compatibility, and product guides. |
| `docs/schemas/` | JSON schemas for native exports. |
| `docs/assets/` | TikZ figure sources and rendered figures. |

## Rules

- Keep the dependency direction: core, then session, then runtime, then tavern, then cli. A lower crate must not import a higher crate.
- Keep terminal code in `crates/cli`. The library has no terminal dependency.
- Change the crate that owns the behavior. Do not duplicate logic across crates.
- Do not change a data format or the database schema without a new schema version and a migration.
- Do not read API keys from environment variables in product code. Keys live in `config.toml`.
- Do not log, print, or export an API key.
- Tests must not need network access or an API key. Use the local endpoints and cassettes in the existing tests.
- Declare new dependencies in the root `Cargo.toml` under `[workspace.dependencies]`.
- Do not copy code from SillyTavern, RisuAI, or another project. The project uses Apache-2.0.
- Do not add an abstraction that the current task does not need.

## Documentation

- Update the crate README and the matching guide in `docs/` with each behavior change.
- Write documentation, code comments, and commit messages in Simplified Technical English (ASD-STE100): short sentences, one idea in each sentence, active voice, simple tenses, one term for each concept.
- Use Conventional Commits for commit messages and pull request titles, for example `feat(terminal): add a zoom key`.
