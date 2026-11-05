# RisuRealm extension

The RisuRealm extension adds a card source and the `risurealm-import` command.
The extension owns one [source plugin](plugins/README.md).
The adapter downloads a CCv3 PNG through the common host fetcher.
The card importer preserves the downloaded bytes.

## Interface

The package ID is `org.rustytavern.extension.risurealm`.
The command uses `card.source.risurealm/1` with the `fetch` operation.
The command requires a JSON `url` string.
The adapter accepts `https://realm.risuai.net/character/CHARACTER_ID`.
The package supplies `card_source: risurealm` as an optional library default.

```sh
cargo run --locked -- --data-dir ./data extension install --builtin risurealm
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.risurealm --grant network:https:realm.risuai.net
cargo run --locked -- --data-dir ./data card fetch https://realm.risuai.net/character/CHARACTER_ID --source risurealm
cargo run --locked -- --data-dir ./data extension run risurealm-import --url https://realm.risuai.net/character/CHARACTER_ID
cargo run --locked -- --data-dir ./data extension disable org.rustytavern.extension.risurealm
```

Replace `CHARACTER_ID` with the website character ID.
The adapter accepts hexadecimal digits and hyphens in an ID of 24 to 128 characters.
The fetch commands contact the website.

## Implementation

The [extension manifest](extension.json) declares the import command.
The [plugin manifest](plugins/source.json) selects the `source.risurealm` factory.
An author can copy this package to add another PNG card source.
The author must implement `CardSource` and register its native factory.
The [extension guide](../../../../docs/developer.md#product-extensions) defines the package interface.
The [plugin guide](../../../../docs/developer.md#runtime-plugins) defines the runtime interface.
