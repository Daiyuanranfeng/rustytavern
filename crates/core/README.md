# Shared values and errors

The crate defines shared identities, artifact metadata, conversation values, and errors.

## Interface

`Error`, `Result`, `Artifact`, `Provenance`, `Persona`, `Role`, `Message`, `digest`, and clock helpers.

## Design

The optional `sqlite` feature supplies the SQLite error conversion. The SQLite adapter selects that feature.

## Dependencies

The crate supports the other component crates.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) records the complete graph.

## Verification

The repository root supports this command.

```sh
cargo test --locked -p rustytavern-core
```
