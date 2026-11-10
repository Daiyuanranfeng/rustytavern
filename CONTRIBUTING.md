# Contributing to RustyTavern

Thank you for your help. This guide explains how to set up the project, change it, and send a pull request.

## Before you start

- Search the [issues](https://github.com/SebastianElvis/rustytavern/issues) before you open a new one.
- Open an issue before a large change. Agree on the design first. A large change without an agreed design can be rejected.
- Report a security problem as described in [SECURITY.md](SECURITY.md). Do not open a public issue for it.

## Set up

1. Install [rustup](https://rustup.rs/).
   The `rust-toolchain.toml` file selects Rust 1.99.1 with `rustfmt` and `clippy`.
2. Clone the repository:

   ```sh
   git clone https://github.com/SebastianElvis/rustytavern.git
   cd rustytavern
   ```

3. Build the workspace:

   ```sh
   cargo build --locked --workspace
   ```

4. Start the terminal app with a separate home directory:

   ```sh
   cargo run --locked -p rustytavern-cli -- --home ./home
   ```

   Git ignores `/data/` and `/target/`. Do not commit your home directory.

## Run the checks

Run all four checks before you push. The CI runs only the format and lint checks.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo deny check
```

Install `cargo-deny` with `cargo install --locked cargo-deny`.
The `deny.toml` file defines the allowed dependency licenses and the advisory policy.

The tests use local model endpoints and recorded HTTP cassettes.
The tests do not need an API key. The tests do not make paid model calls.

The live provider test is ignored by default. It sends paid requests to OpenRouter.
Run it only when you change a provider adapter:

```sh
OPENROUTER_API_KEY=... cargo test -p rustytavern --test live_providers -- --ignored --nocapture
```

## Know the workspace

The workspace has five crates. A lower crate must not depend on a higher crate.

```text
rustytavern-core  <-  rustytavern-session  <-  rustytavern-runtime  <-  rustytavern  <-  rustytavern-cli
```

- Change the crate that owns the behavior.
- Keep terminal code in `rustytavern-cli`. The library has no terminal dependency.
- Keep the public data formats and the database schema stable. A format change needs a new schema version in `docs/schemas/`.
- Read the [architecture guide](docs/architecture.md#workspace-boundaries) for the full rules.
- Read the [developer guide](docs/developer.md#implementation-procedure) before you write a plugin.

## Write code

- Add a test for each bug fix and each new behavior.
- Do not add a dependency without a reason in the pull request. Prefer a dependency that the workspace already uses.
- Declare each dependency once in the root `Cargo.toml` under `[workspace.dependencies]`.
- Do not read secrets from environment variables in product code. API keys live in `config.toml`.
- Do not log or export an API key.

## Write documentation

- Update the crate README and the matching guide in `docs/` when you change behavior.
- Write documentation and code comments in [Simplified Technical English](https://www.asd-ste100.org/):
  - Use short sentences with one idea each.
  - Use the active voice and simple tenses.
  - Use one term for each concept.
- Edit the TikZ sources in `docs/assets/` to change a figure. Build the figures with `make -C docs/assets`.
  The build requires `latexmk` and `pdftocairo`.

## Commit

Use [Conventional Commits](https://www.conventionalcommits.org/) for each commit message:

```text
feat(terminal): edit a settings field in $EDITOR
fix(agent): stop the tool loop after a cancellation
docs(readme): add the install steps
```

Common types are `feat`, `fix`, `docs`, `refactor`, `test`, `ci`, and `chore`.
Common scopes are `agent`, `chat`, `terminal`, `prompt`, `memory`, `cli`, `core`, and `session`.

## Open a pull request

1. Fork the repository and create a branch from `main`.
2. Make one logical change in each pull request.
3. Run all checks.
4. Fill in the pull request template.
5. Add an entry under `Unreleased` in [CHANGELOG.md](CHANGELOG.md) for a user-visible change.

A maintainer reviews each pull request. The maintainer squashes the commits when the pull request merges.
The pull request title becomes the commit message, so use the Conventional Commits format for it.

## License

RustyTavern uses the [Apache License, Version 2.0](LICENSE).
Section 5 of the license applies to each contribution.
You agree that your contribution uses the same license when you submit it.
Do not submit code that you cannot license under Apache-2.0.
Do not copy code from SillyTavern, RisuAI, or another project with an incompatible license.
