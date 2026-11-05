# Chub extension

The Chub extension adds a card source and the `chub-import` command.
The extension owns one [source plugin](plugins/README.md).
The package does not change the model provider or the conversation runtime.

## Interface

The package ID is `org.rustytavern.extension.chub`.
The command uses `card.source.chub/1` with the `fetch` operation.
The command requires a JSON `url` string.
The adapter accepts supported Chub character URLs.
The adapter maps website metadata to a CCv2 card.
The common importer validates and stores the card with provenance.
The package supplies `card_source: chub` as an optional library default.

```sh
cargo run --locked -- --data-dir ./data extension install --builtin chub
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.chub --grant network:https:api.chub.ai
cargo run --locked -- --data-dir ./data card fetch https://chub.ai/characters/CREATOR/CHARACTER --source chub
cargo run --locked -- --data-dir ./data extension run chub-import --url https://chub.ai/characters/CREATOR/CHARACTER
cargo run --locked -- --data-dir ./data extension disable org.rustytavern.extension.chub
```

Replace `CREATOR/CHARACTER` with a real character path.
The fetch commands contact the website.

## Implementation

The [extension manifest](extension.json) binds the product command to the source service.
The [plugin manifest](plugins/source.json) selects the compiled `source.chub` factory.
An author can copy this package to add another native source factory.
The author must implement `CardSource` and register the factory in the host.
The [extension guide](../../../../docs/developer.md#product-extensions) explains the package lifecycle.
The [plugin guide](../../../../docs/developer.md#runtime-plugins) explains registration and grants.
