# Library and plugin examples

The examples show the library and runtime plugins.
The examples use authored character data and local fixtures.
The [plugin examples](plugins/README.md) contain the native and process implementations.

| Example | Design | Start guide |
| --- | --- | --- |
| Decoder and driver replacement | The application binds native decoder, driver, and fixture provider services. | [Feature services](plugins/feature-services.md) |
| Persistent state | The plugin upgrades its namespace and preserves older revisions. | [State migration](plugins/state-migration.md) |
| Native prompt plugin | The application injects and removes compiled Rust code. | [Native plugin](plugins/native-plugin.md) |
| Prompt contributions | One installable plugin adds two prompt contributions. | [Lighthouse notes](plugins/lighthouse-notes/README.md) |
| Prompt edits | One plugin edits a named section before context selection. | [Lighthouse edits](plugins/lighthouse-edits/README.md) |
| External source | The plugin starts a separate Rust executable through stdio. | [Inline source](plugins/inline-source/README.md) |

The [plugin guide](../../../docs/developer.md#runtime-plugins) defines native interfaces and lifecycle.
The [installed plugin guide](../../../docs/developer.md#installed-plugins) defines plugin directories and commands.
The [protocol guide](../../../docs/developer.md#external-card-source-protocol) defines process messages.
The [plugin crate](../../plugins/README.md) holds the essential plugins and website adapters.

```sh
cargo build --locked --examples
cargo run --locked --example native_plugin
cargo test --locked --test plugins --test installed_plugins --test external
```

These commands make no paid model request.

## Library conversation

The [conversation example](library/conversation.rs) uses an existing session and its pinned provider.
The caller selects the home directory, session, and message.
The example returns the durable reply and usage.
The example supports cancellation and an optional submission key.

The example has its own executable identity.
The caller uses `--adopt` to create a configuration revision for its services.
The library retains the previous configuration and transcript.

```sh
cargo run --locked -p rustytavern --example conversation -- \
  --home ./home --session SESSION_ID --message "Hello." --adopt
```

The selected provider determines whether this command makes a paid request.
The ordinary Cargo tests do not send a model request through this example.
The private verification workspace owns fixture assertions and paid integration checks.
