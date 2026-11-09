# Core values and card formats

The crate defines shared values, errors, and the character card and worldbook formats.

## Interface

- The crate root exports `Error`, `Result`, `Artifact`, `Provenance`, `Persona`, `Role`, `Message`, `digest`, and clock helpers.
- The `card` module exports `Card`, `CardDecoder`, `CcDecoder`, typed character records, typed worldbook records, and format diagnostics.

## Design

The card importer keeps the original bytes and unknown JSON values. The decoder enforces envelope precedence and input bounds.
The CHARX adapter keeps embedded assets and unknown archive files.
The optional `sqlite` feature supplies the SQLite error conversion.

## Formats and example

The [format guide](../../docs/compatibility.md#character-cards-and-worldbooks) defines character and worldbook structures.
The [example](examples/formats.rs) creates a character card with an embedded book.

```sh
cargo run --locked -p rustytavern-core --example formats
```

## Verification

```sh
cargo test --locked -p rustytavern-core
```
