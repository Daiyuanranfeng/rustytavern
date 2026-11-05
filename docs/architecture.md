# Architecture

## Current implementation

RustyTavern uses a virtual workspace with nine crates. Each crate owns a cohesive component and its README.
Modules separate related implementation areas within each component. The CLI supplies the interactive terminal and script commands.
The product library assembles the runtime and extension packages. The runtime uses plugin, conversation, adapter, and storage interfaces.

![Runtime architecture](assets/runtime-architecture.svg)

| Directory | Package | Responsibility |
| --- | --- | --- |
| [`core`](../crates/core/README.md) | `rustytavern-core` | Shared errors, identities, personas, messages, and artifact records. |
| [`card`](../crates/card/README.md) | `rustytavern-card` | Character cards, worldbooks, containers, and format diagnostics. |
| [`conversation`](../crates/conversation/README.md) | `rustytavern-conversation` | Session records, provider settings, native exports, and pure prompt compilation. |
| [`store`](../crates/store/README.md) | `rustytavern-store` | Atomic storage contracts and the optional SQLite adapter. |
| [`plugin`](../crates/plugin/README.md) | `rustytavern-plugin` | Native plugin contracts and the optional external process host. |
| [`adapters`](../crates/adapters/README.md) | `rustytavern-adapters` | Model provider adapters and public character source adapters. |
| [`runtime`](../crates/runtime/README.md) | `rustytavern-runtime` | Generation control, durable events, prompt hooks, and cancellation. |
| [`tavern`](../crates/tavern/README.md) | `rustytavern` | Product assembly, extension packages, configuration, and public library exports. |
| [`cli`](../crates/cli/README.md) | `rustytavern-cli` | The interactive terminal and script interface. |

The dependency graph remains acyclic. The runtime has no product package dependency.
The default storage contract has no SQLite dependency. The default plugin registry has no process host dependency.
The stock product enables the SQLite and process features explicitly. The plugin and extension concepts retain separate interfaces.
The [workspace guide](architecture.md#workspace-boundaries) records dependency direction and feature choices.

### Runtime ordering

The [prompt phases](developer.md#phase-order) define preparation through request persistence. The host performs network I/O outside database transactions.
The host persists each draft delta before event delivery. The host commits the assistant node and terminal status atomically.
Each session permits one writer through a lease, fencing token, and expected version. Recovery interrupts expired work without restarting model requests.
Blocking workers run storage and compilation while the runtime renews the lease. A cancelled worker retains its composition until its local work ends.
A bounded observer queue cannot block durable completion. The caller reads durable results after a `missed_events` gap.

### Runtime replacement

The [registry contract](developer.md#design) defines dependencies, rollback, ownership, and leases. The runtime selects decoder, driver, profile, provider, and store services from the frozen registry.
A generation records that composition in its input snapshot. Each session pins its composition through an immutable [configuration revision](user-guide.md#session-configuration-revisions).
The user must adopt a changed composition explicitly.

### State and context

The store keeps the full message tree and one selected branch head. Regeneration adds an assistant sibling.
An edit adds a node without changing existing descendants. The compiler selects context without deleting history.
[Draft acceptance](user-guide.md#explicit-partial-reply-acceptance) adds a retained partial reply through an explicit message link.
[Native exports](user-guide.md#native-records-and-exports) retain the tree, revision archives, and opaque plugin state.
The counter estimates tokens from UTF-8 bytes, including the serialized request body. The budget reserves output capacity and a protocol margin.
The final budget pass removes history, examples, lore, and optional contributions in that order. The plan records each omission.
Required context overflow stops dispatch.
[Inspection and replay](developer.md#offline-inspection-and-replay) expose pure compilation.

### Product extensions

The [extension manager](developer.md#product-extensions) owns packages, commands, defaults, assets, and plugin ownership. The runtime registry enforces plugin grants and lifecycle rules.
The catalog uses versioned `plugin_state` records.

### Error handling

The shared error uses thiserror. The error retains its underlying cause in memory.
The public record contains a stable code and a safe message. Serialization omits the internal cause.
The CLI maps the same code to its exit status.

## Target runtime

The [product target](product.md#required-target) includes agent, media, audio, and discovery features. The current figures show implemented code only.
The current single-reply driver does not implement that full target. The core host must own persistence, permission checks, cancellation, and resource limits.
The core host must bound request counts, tool steps, deadlines, event counts, and stored artifacts. Plugins must implement behavior and adapters through declared services.
Extensions must package those plugins with commands, defaults, and assets. A plugin must not bypass durable host transitions through a replacement driver.
The host must save operation inputs and results before it exposes durable events. New service families must preserve configuration adoption and replay boundaries.

## Workspace boundaries

### Dependency direction

The conversation crate owns pure records and prompt compilation. The runtime depends on conversation, plugin, adapter, and storage interfaces.
The product library depends on the runtime. The runtime has no product library dependency.
The CLI depends on the assembled product library. The library has no terminal dependency.

The plugin registry keeps its process host behind the `process` feature. The storage contract keeps its SQLite adapter behind the `sqlite` feature.
The stock product enables both features. A standalone consumer can use either contract without its adapter.
The model and source adapters retain separate modules and network policies. The product library keeps extension packaging above runtime plugins.

| Package | Direct workspace dependencies |
| --- | --- |
| `rustytavern` | `rustytavern-adapters`, `rustytavern-card`, `rustytavern-conversation`, `rustytavern-core`, `rustytavern-plugin`, `rustytavern-runtime`, `rustytavern-store` |
| `rustytavern-adapters` | `rustytavern-card`, `rustytavern-conversation`, `rustytavern-core` |
| `rustytavern-card` | `rustytavern-core` |
| `rustytavern-cli` | `rustytavern` |
| `rustytavern-conversation` | `rustytavern-card`, `rustytavern-core` |
| `rustytavern-core` | The package has no workspace dependency. |
| `rustytavern-plugin` | `rustytavern-adapters`, `rustytavern-card`, `rustytavern-conversation`, `rustytavern-core`, `rustytavern-store` |
| `rustytavern-runtime` | `rustytavern-adapters`, `rustytavern-card`, `rustytavern-conversation`, `rustytavern-core`, `rustytavern-plugin`, `rustytavern-store` |
| `rustytavern-store` | `rustytavern-card`, `rustytavern-conversation`, `rustytavern-core` |

The plugin dependency on adapters applies to its optional process host. The default plugin registry excludes that dependency.

### Public library paths

The assembled `rustytavern` library retains its existing module paths. The `app` module owns extension packages and application setup.
The `config` module owns product configuration. The `session`, `prompt`, `plugin`, `external`, `provider`, `source`, `runtime`, and `store` paths expose the component interfaces.
The package consolidation changes internal Cargo package names. Direct component consumers must update their dependencies and imports.
The data formats and database schema remain unchanged.

### Optional adapters

```toml
[dependencies]
rustytavern-store = { path = "crates/store", features = ["sqlite"] }
rustytavern-plugin = { path = "crates/plugin", features = ["process"] }
```

A consumer omits those feature names when it needs contracts only. The native registry has no generation runtime dependency.
The runtime can compile without SQLite or external process support.

## Dependency reuse

The project reuses libraries for general infrastructure. The project keeps its runtime policy separate from those libraries.

| Function | Existing dependency | Project responsibility |
| --- | --- | --- |
| JSON and records | `serde`, `serde_json` | The importer enforces format policy and preserves unknown values. |
| HTTP and TLS | `reqwest` | The host enforces endpoint grants, address checks, limits, and cancellation. |
| SQLite | `rusqlite` | The store owns transactions, fencing, and the SQL schema. |
| Errors | `thiserror` | The public error excludes private causes and credentials. |
| Async work and cancellation | `tokio`, `tokio-util` | The runtime owns leases, cleanup, and durable completion. |
| CLI | `clap` | The CLI maps commands to library operations. |
| Terminal rendering | `ratatui`, `crossterm` | The CLI owns event handling, restoration, and conversation views. |
| Text editing | `tui-textarea` | The CLI owns form validation, slash routing, and input bounds. |
| Versions | `semver` | The plugin registry checks declared dependencies. |
| Binary metadata | `base64`, `crc32fast` | The card importer applies envelope precedence and byte limits. |
| CHARX archives | `zip` | The card importer checks metadata, paths, CRC values, and expansion limits. |

### Card-format candidate

The project has not added `chara_card` version `0.4.1`. That version requires a newer ZIP dependency than the Rust 1.85 baseline supports.
A replacement decoder must preserve original bytes, unknown fields, envelope rules, bounds, and diagnostics.

### ZIP adapter

The card crate uses `zip` version `7.3` with Deflate on Rust 1.85. The adapter reads bounded entries in memory and checks counts before index allocation.
The adapter rejects special files and never extracts files to disk. The project must reassess extraction advisories before it adds filesystem extraction.

### Further candidates

An event-stream replacement must pass fragmentation, limits, failures, and cancellation checks. A token counter must identify its model and tokenizer version.
The product must retain estimated-count labels for other models.

## Architecture figures

The figures show the current implementation. The product map includes the implemented extension package layer.

### Build

The renderer requires `latexmk` and `pdftocairo`.

```sh
make -C docs/assets
```

The sources remain editable TikZ files. The build creates PDF and SVG files.
The reviewed PNG files provide previews.

### Product map

![Product map](assets/product-map.svg)

[TikZ source](assets/product-map.tex) · [PDF](assets/product-map.pdf) · [SVG](assets/product-map.svg) · [PNG](assets/product-map.png)

### Runtime architecture

![Runtime architecture](assets/runtime-architecture.svg)

[TikZ source](assets/runtime-architecture.tex) · [PDF](assets/runtime-architecture.pdf) · [SVG](assets/runtime-architecture.svg) · [PNG](assets/runtime-architecture.png)

### Generation workflow

![Generation workflow](assets/generation-workflow.svg)

[TikZ source](assets/generation-workflow.tex) · [PDF](assets/generation-workflow.pdf) · [SVG](assets/generation-workflow.svg) · [PNG](assets/generation-workflow.png)

### DB model

![DB model](assets/database-model.svg)

[TikZ source](assets/database-model.tex) · [PDF](assets/database-model.pdf) · [SVG](assets/database-model.svg) · [PNG](assets/database-model.png)

The DB model uses solid arrows for SQL foreign keys. The DB model uses dashed arrows for logical references.
The library checks logical references before it changes a branch or binding. The diagram does not claim that SQLite enforces every logical reference.
