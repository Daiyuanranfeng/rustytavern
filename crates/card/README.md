# Character card formats

The crate imports and exports character cards.

## Interface

`Card`, `CardDecoder`, `CcDecoder`, typed character records, typed worldbook records, and format diagnostics.

## Design

The importer retains the original bytes and unknown JSON values. The decoder enforces envelope precedence and input bounds.
The CHARX adapter retains embedded assets and unknown archive files.

## Dependencies

The crate depends on core values, serialization libraries, and the `zip` library.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) records the complete graph.

## Formats and example

The [format guide](../../docs/compatibility.md#character-cards-and-worldbooks) defines character and worldbook structures.
The [example](examples/formats.rs) creates a character card with an embedded book.

```sh
cargo run --locked -p rustytavern-card --example formats
```

## Verification

The repository root supports this command.

```sh
cargo test --locked -p rustytavern-card
```

The [worldbook guide](../../docs/user-guide.md#worldbooks) defines archive storage, session bindings, and prompt profile support.
