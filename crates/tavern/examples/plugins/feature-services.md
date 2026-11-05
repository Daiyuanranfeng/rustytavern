# Native decoder and driver example

The example replaces bundled services through explicit bindings.
The [complete source](feature_services.rs) builds in the existing Cargo package.
The example uses an in-memory store and a local fixture provider.
The example makes no network request or paid request.

## Design and interface

The `Features` plugin provides `card.decoder/1`, `driver/1`, and `provider/1`.
The `PrefixDecoder` implementation accepts `RT:` followed by a newline and a standard card.
The `UppercaseDriver` implementation converts reply text to uppercase.
The fixture provider returns one fixed reply.
The host records the normalized card and original bytes.
The host also validates driver completion and persists reply text.

The plugin requests the three capabilities in `DRIVER_CAPABILITIES`.
The embedding application grants those capabilities explicitly.
The application binds each replaced service to `example.features`.
The remaining bundled profile and store services keep their original bindings.

```sh
cargo run --locked --example feature_services
```

The example prints these fields after its assertions pass.

```json
{"decoder":"prefix-json/1","original_bytes_preserved":true,"paid_requests":0,"reply":"HELLO, ALEX"}
```

## Implementation

1. The author must copy the source into another example or embedding application.
2. The author must choose unique plugin and policy IDs.
3. The author must implement the two source traits.
4. The author must register each trait value through an outer `Arc`.
5. The author must declare the required services and capabilities.
6. The application must grant the driver capabilities and select explicit bindings.
7. The author must test malformed cards, denied grants, failure, cancellation, and removal.

The [service guide](../../../../docs/developer.md#decoder-and-driver-services) defines every host method and limit.
The [plugin guide](../../../../docs/developer.md#runtime-plugins) defines lifecycle and registration.
The stock CLI does not load this Rust implementation from a package dynamically.
An embedding application can register a native factory for an extension to select it.
