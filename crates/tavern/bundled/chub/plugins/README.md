# Chub source plugin

The plugin converts Chub metadata into a validated character-card artifact.
The plugin ID is `org.rustytavern.source.chub`.
The factory name is `source.chub`.
The plugin registers `Arc<dyn CardSource>` as `card.source.chub/1`.
The registry stores that trait value inside an outer `Arc`.

The `recognize` method checks supported Chub URLs.
The asynchronous `fetch` method receives a URL and a cancellation token.
The method returns `Artifact` bytes with `Provenance`.
The adapter requires `network:https:api.chub.ai`.
The scoped host fetcher checks destination addresses and redirects.

The [source module](../../../../adapters/src/source/mod.rs) contains the implementation.
The [source tests](../../../tests/sources.rs) verify metadata conversion with local responses.
An author can follow this interface to implement another website adapter.
The [standard plugin reference](../../../../../docs/developer.md#standard-plugins) describes service registration.
The [plugin guide](../../../../../docs/developer.md#runtime-plugins) describes lifecycle and cleanup.

## Lifecycle and verification

The extension manager activates this plugin when the package becomes enabled.
The registry keeps the plugin while another owner or operation needs it.
The registry releases its resources after its final owner and operation release it.
The [plugin guide](../../../../../docs/developer.md#runtime-plugins) defines the complete lifecycle.

```sh
cargo test --locked --test sources --test extensions
```

The source tests check metadata conversion with local responses.
The extension tests check denied grants and package restoration.
These tests make no website request.
The tests do not prove that a public website endpoint remains available.
