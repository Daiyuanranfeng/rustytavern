# Network adapters

The crate owns model provider adapters and character source adapters.

## Interface

The `provider` module exports provider contracts, prepared requests, HTTP adapters, and stream events.
The `source` module exports public artifact fetching and Chub and RisuRealm adapters.

## Design

Both modules own outbound network boundaries.
The provider module validates model requests and normalizes streams.
The source module validates URL grants, addresses, redirects, and artifact limits.
The modules retain separate policies.
The crate performs no session persistence or plugin registration.

## Dependencies

The crate depends on core values, cards, conversation records, and HTTP libraries.
The [workspace guide](../../docs/architecture.md#workspace-boundaries) defines the complete graph.

## Verification

```sh
cargo test --locked -p rustytavern --test providers --test sources
```
