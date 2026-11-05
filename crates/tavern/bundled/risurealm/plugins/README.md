# RisuRealm source plugin

The plugin obtains PNG card artifacts from RisuRealm.
The plugin ID is `org.rustytavern.source.risurealm`.
The factory name is `source.risurealm`.
The plugin registers `Arc<dyn CardSource>` as `card.source.risurealm/1`.
The registry stores that trait value inside an outer `Arc`.

The `recognize` method checks the website and character ID.
The asynchronous `fetch` method receives a URL and a cancellation token.
The method returns the original PNG bytes in an `Artifact` with `Provenance`.
The adapter requires `network:https:realm.risuai.net`.
The common importer checks card envelopes and PNG structure.

The [source module](../../../../adapters/src/source/mod.rs) contains the implementation.
The [source tests](../../../tests/sources.rs) verify PNG preservation with local responses.
An author can use this adapter as an example for another direct card download.
The [standard plugin reference](../../../../../docs/developer.md#standard-plugins) describes typed registration.
The [plugin guide](../../../../../docs/developer.md#runtime-plugins) describes lifecycle and grants.

## Lifecycle and verification

The extension manager activates this plugin when the package becomes enabled.
The registry keeps the plugin while another owner or operation needs it.
The registry releases its resources after its final owner and operation release it.
The [plugin guide](../../../../../docs/developer.md#runtime-plugins) defines the complete lifecycle.

```sh
cargo test --locked --test sources --test extensions
```

The source tests check exact PNG preservation with local responses.
The extension tests check denied grants and package restoration.
These tests make no website request.
The tests do not prove that a public website endpoint remains available.
