# Library, plugin, and extension examples

The examples show both runtime plugins and product extensions.
The examples use authored character data and local fixtures.
The [plugin examples](plugins/README.md) contain the native and process implementations.

| Example | Design | Start guide |
| --- | --- | --- |
| Decoder and driver replacement | The application binds native decoder, driver, and fixture provider services. | [Feature services](plugins/feature-services.md) |
| Persistent state | The plugin upgrades its namespace and preserves older revisions. | [State migration](plugins/state-migration.md) |
| Native prompt plugin | The application injects and removes compiled Rust code. | [Native plugin](plugins/native-plugin.md) |
| Asset extension | The package contains a template and defaults without plugins. | [Harbor assets](extensions/harbor-assets/README.md) |
| Multiple plugins | One package owns two prompt contributions. | [Lighthouse notes](extensions/lighthouse-notes/README.md) |
| Prompt edits | One plugin edits a named section before context selection. | [Lighthouse edits](extensions/lighthouse-edits/README.md) |
| External source | The package starts a separate Rust executable through stdio. | [Inline source](extensions/inline-source/README.md) |

The [plugin guide](../../../docs/developer.md#runtime-plugins) defines native interfaces and lifecycle.
The [extension guide](../../../docs/developer.md#product-extensions) defines packaging and commands.
The [protocol guide](../../../docs/developer.md#external-card-source-protocol) defines process messages.
The [bundled extensions](../bundled/README.md) show website adapters.

```sh
cargo build --locked --examples
cargo run --locked --example native_plugin
cargo test --locked --test plugins --test extensions --test external
```

These commands make no paid model request.

## Library conversation

The [conversation example](library/conversation.rs) uses an existing session and its pinned provider.
The caller selects the data directory, session, and message.
The example returns the durable reply and usage.
The example supports cancellation and an optional submission key.

The example has its own executable identity.
The caller uses `--adopt` to create a configuration revision for its services.
The library retains the previous configuration and transcript.

```sh
cargo run --locked -p rustytavern --example conversation -- \
  --data-dir ./data --session SESSION_ID --message "Hello." --adopt
```

The selected provider determines whether this command makes a paid request.
The ordinary Cargo tests do not send a model request through this example.
The private verification workspace owns fixture assertions and paid integration checks.
