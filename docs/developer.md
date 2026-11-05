# Developer guide

This guide serves library embedders, plugin developers, extension authors, and contributors.
Each reader uses the same host contracts for durable state, permissions, cancellation, and configuration adoption.

## Reader paths

| Reader | Starting points | Development work |
| --- | --- | --- |
| Library embedder | The [architecture](architecture.md#current-implementation) and [library configuration interface](user-guide.md#library-interface). | The developer assembles services, creates sessions, calls typed operations, and supplies optional adapters. |
| Plugin developer | The [native interface](#native-interface) and [standard services](#essential-plugins). | The developer implements behavior, declares grants and dependencies, registers services, and verifies cleanup. |
| Extension author | The [extension interface](#extensions). | The author changes the application appearance. An extension cannot change agent behavior or agent context. |
| Contributor | The [workspace boundaries](architecture.md#workspace-boundaries) and [root checks](../README.md#build-and-check). | The contributor changes the owning crate, preserves public contracts, and runs checks for the change. |

The current native API remains experimental. A roadmap requirement does not create an implemented service contract.
The [product roadmap](product.md#required-target) identifies required features beyond the current interfaces.

## Runtime plugins

A plugin changes agent behavior or adds content to the agent context. Drivers, providers, tools, MCP servers, skills, card sources, and prompt contributions are plugins.
A user can [install a third-party plugin](#installed-plugins). An [extension](#extensions) changes only the application appearance.
The [plugin crate](../crates/plugins/README.md) holds every essential plugin.

The native API remains experimental in version `0.1.0`. The host does not load Rust dynamic libraries.
An embedded application compiles native plugins into its executable. An external process uses the [stdio protocol](developer.md#external-card-source-protocol).
The current process interface supports card sources only.

### Design

The registry owns each activation through an owner ID. The plugin manager uses `plugin:PLUGIN_ID` as an owner ID.
The essential plugins use the `core-product` owner. The registry removes an instance after its last owner leaves.
The registry rejects removal when a remaining plugin requires its services.

The registry resolves dependencies before activation. The registry rejects missing services, dependency cycles, and ambiguous providers.
A binding selects one plugin ID for a service name. The registry rolls back a failed activation batch.
The registry releases owned resources during rollback and removal.

A composition lease freezes services and contributions for an operation. The registry rejects changes with `plugin_change_deferred` while a lease exists.
The registry does not queue these changes. The caller can retry after the operation releases its lease.
Existing sessions require an explicit configuration revision before they use a changed composition.

### Native interface

The [plugin module](../crates/plugin/src/lib.rs) defines these interfaces.

| Interface | Contract |
| --- | --- |
| `RuntimePlugin::manifest` | The method returns a stable `PluginManifest`. |
| `RuntimePlugin::activate` | The method registers services and contributions through `PluginContext`. |
| `RuntimePlugin::deactivate` | The method stops resources that require asynchronous cleanup. |
| `PluginContext::register` | The method registers one declared service as an `Arc<T>`. |
| `PluginContext::dependency` | The method obtains a declared dependency with its exact Rust type. |
| `PluginContext::contribute` | The method adds one declared prompt section. |
| `PluginContext::authorize` | The method checks one effective capability grant. |
| `PluginContext::own` | The method registers synchronous cleanup in reverse registration order. |
| `PluginContext::state_read` | The method reads a key from the plugin namespace. |
| `PluginContext::state_write` | The method appends a state revision after a schema check. |
| `PluginContext::state_migrate` | The method upgrades owner state through a pure conversion and atomic commit. |
| `PluginRegistry::inject` | The method activates a batch under one owner. |
| `PluginRegistry::eject_owner` | The method removes an owner and unused plugins. |
| `PluginRegistry::freeze` | The method returns an immutable composition lease. |

The registry stores type-erased service values. The producer and consumer must use the same concrete Rust type.
The standard trait services use an outer `Arc` around an `Arc<dyn Trait>`. An activation method registers a profile in this form.

```rust
let profile: Arc<dyn PromptProfile> = Arc::new(CoreProfile);
context.register("profile/1", Arc::new(profile))?;
```

The consumer obtains this value with `service::<Arc<dyn PromptProfile>>("profile/1")`. The [standard plugin reference](developer.md#essential-plugins) lists the supported service types.

### Manifest

| Field | Meaning |
| --- | --- |
| `id` | The value identifies the plugin. The value permits ASCII letters, digits, `.`, `_`, and `-`. |
| `version` | The value uses semantic versioning. |
| `host_protocol` | The current value is `"1"`. |
| `provides` | The list declares service names with version suffixes such as `profile/1`. |
| `requires` | The list declares required services. |
| `requested_capabilities` | The set declares host operations that the plugin requests. |
| `allowed_slots` | The set declares prompt contribution slots. |
| `state_schema` | The positive integer identifies the plugin state schema. |
| `artifact_hash` | The host records the executable identity. |
| `configuration_hash` | The host records the implementation configuration identity. |

The plugin manager computes both hashes during activation. The author must not use a version string as a substitute for an artifact hash.
The host grants only the intersection of requested and approved capabilities. A native plugin remains trusted Rust code inside the host process.
Capability checks do not create an operating-system sandbox.

### Prompt contributions

The `core-v1` profile exposes four slots. The slots are `before_character`, `after_character`, `before_history`, and `after_history`.
Each slot requires `prompt.contribute:SLOT` in the request and the host grant. Each contribution has a unique `section_id`, a slot, a priority, a role, and text.
The host assigns `plugin_id` from the manifest. The profile accepts the `system` role only.
The host orders contributions by slot, priority, plugin ID, and section ID. The default `required` retention rule treats contribution text as required context.
An `optional` contribution can leave the request with an explicit budget omission. The `provenance` field retains source metadata.
The host supplies producer metadata when that field is absent. The profile returns an error when required context exceeds its budget.

The [edit plugin](../crates/tavern/examples/plugins/lighthouse-edits/README.md) uses that interface without external I/O.


### Implementation procedure

1. Choose a stable plugin ID and a supported interface.
2. Implement `RuntimePlugin` with a stable manifest.
3. Register only the services that the manifest declares.
4. Declare each dependency and capability before activation.
5. Register cleanup as soon as the plugin acquires a resource.
6. Return `rustytavern::Result` from fallible operations.
7. Use `Error::with_source` to retain an internal error cause.
8. Inject the plugin under an owner ID.
9. Verify its effect through the same runtime that the application uses.
10. Eject the owner and verify that the effect disappears.

The [native example](../crates/tavern/examples/plugins/native-plugin.md) contains a complete implementation. The [lighthouse plugin](../crates/tavern/examples/plugins/lighthouse-notes/README.md) uses the supplied contribution factory.
The [source process example](../crates/tavern/examples/plugins/inline-source/README.md) requires no host recompilation.

### Plugin documentation

Each plugin requires a README or a linked interface document. A shared document must identify each plugin.
The document must specify purpose, plugin ID, service, Rust type or protocol, inputs, outputs, configuration, grants, state, dependencies, cleanup, cancellation, examples, verification, limits, and trust. The author must update the document with each interface change.

## Essential plugins

The [plugin crate](../crates/plugins/README.md) holds every essential plugin. Each plugin family is one module.
The standard agent activates the essential plugins through `plugins::registry`. The owner ID is `core-product`.
The host grants each requested capability of an essential plugin. Every essential plugin is always on.
The `plugins.enabled` list in `config.toml` selects the installed plugins. Each `[plugins.ID]` table configures one plugin.

| Plugin ID | Service | Registered Rust type | Requested capabilities |
| --- | --- | --- | --- |
| `decoder` | `card.decoder/1` | `Arc<dyn CardDecoder>` | None. Always on. |
| `profile` | `profile/1` | `Arc<dyn PromptProfile>` | None. Always on. |
| `provider` | `provider/1` | `Arc<dyn ProviderAdapter>` | None. Always on. |
| `store` | `store/1` | `Arc<dyn StoreBackend>` | None. Always on. |
| `agent` | `driver/1` | `Arc<dyn ConversationDriver>` | The three driver capabilities |
| `web` | `tools.web/1` | `agent::ToolSet` | `network:https:*` |
| `skills` | `tools.skills/1` | `agent::ToolSet` | `skills.read`, `prompt.contribute:before_history` |
| `mcp` | `tools.mcp/1` | `agent::ToolSet` | `mcp.connect` |
| `chub` | `card.source.chub/1` | `Arc<dyn CardSource>` | `network:https:api.chub.ai` |
| `risurealm` | `card.source.risurealm/1` | `Arc<dyn CardSource>` | `network:https:realm.risuai.net` |

An installed plugin can also select the `prompt.contributions` and `prompt.edits` factories.
The registry wraps each table type in an outer `Arc`. The runtime selects decoder, driver, profile, and provider services from its composition lease.
The application selects the store when it creates the runtime. A store binding change does not move existing session records.
The provider adapter owns vendor stream decoding.

### Profile, provider, and store contracts

| Service | Required behavior |
| --- | --- |
| `PromptProfile::draft` | The pure method returns candidate sections before context selection. |
| `PromptProfile::select_context` | The method selects complete groups after authorized edits. |
| `PromptProfile::compile` | The default method combines drafting and selection for direct callers. |
| `ProviderAdapter::prepare` | The pure method returns a `PreparedRequest` without network I/O. |
| `ProviderAdapter::open_stream` | The method dispatches with a credential resolver and cancellation token. The stream emits normalized `ProviderEvent` values: text, thinking text, tool calls, usage, failure, and completion. |
| `ProviderAdapter::id` | The method identifies the native implementation. |
| `ProviderAdapter::validate_config` | The pure method checks settings before configuration persistence. |
| `StoreBackend` | The backend preserves atomic transitions, writer leases, fencing, namespace snapshots, checked writes, and migrations. |

A replacement profile must preserve the input snapshot and explain context omissions. A replacement provider must preserve credential exclusion and terminal stream semantics.
An embedded application can supply another backend during registry construction. The `store.sqlite` factory uses the application's open store.
The factory does not open a second database from plugin configuration.

### Contribution factory

The `prompt.contributions` factory registers static prompt contributions. The factory does not require a service in `provides`.
Its configuration contains a `contributions` array. The [lighthouse plugin](../crates/tavern/examples/plugins/lighthouse-notes/README.md) provides two complete manifests.
An author can use this factory for prompt text without new Rust code.

### Edit factory

The `prompt.edits` factory registers a pure `PromptHook` service. Its configuration contains a hook policy and explicit section edits.
The [lighthouse edit plugin](../crates/tavern/examples/plugins/lighthouse-edits/README.md) supplies a complete manifest.

### Source factories

The `source.chub` factory provides `card.source.chub/1`. The `source.risurealm` factory provides `card.source.risurealm/1`.
Both services register `Arc<dyn CardSource>` through an outer `Arc`. The [source module](../crates/adapters/src/source/mod.rs) defines `id`, `recognize`, and asynchronous `fetch`.
The `fetch` method returns an `Artifact` with bytes and `Provenance`. The common card importer checks the artifact hash and card format.

The [plugin crate](../crates/plugins/README.md) lists their host grants. The [source tests](../crates/tavern/tests/sources.rs) use recorded or synthetic responses.
Those tests do not establish continued availability of the public website endpoints.

## Decoder and driver services

The default product supplies decoder and driver plugins. An embedding application can replace either service through the existing registry.
The [complete example](../crates/tavern/examples/plugins/feature-services.md) replaces both services without a runtime fork.

### Card decoder

The [card module](../crates/card/src/lib.rs) defines `CardDecoder`, `DecodedCard`, and `DecoderRecord`. The native service name is `card.decoder/1`.
The registered type is an outer `Arc` around `Arc<dyn CardDecoder>`.

| Method or field | Contract |
| --- | --- |
| `CardDecoder::id` | The method returns the stable decoding policy ID. |
| `CardDecoder::decode` | The pure method interprets bounded bytes and returns `DecodedCard`. |
| `DecodedCard::document` | The value contains a normalized CCv2 or CCv3 envelope. |
| `DecodedCard::format` | The value identifies the original container format. |
| Envelope metadata | The values identify a selected embedded envelope and conflicting envelopes. |

The decoder owns container parsing. The common importer owns normalized validation, identities, diagnostics, original bytes, and provenance checks.
The host limits original input to 32 MiB and normalized JSON to 8 MiB. The host also applies the standard field, string, and lore-entry limits.
A decoder cannot supply arbitrary executable behavior through a card. The host never executes card scripts during import.

The default `CcDecoder` policy is `cc-v2-v3/1`. That policy retains the JSON and PNG rules and raw-source revision IDs.
The decoder also reads bounded CHARX archives. An alternative policy must use its own ID.
The host hashes the original source, normalized fields, and alternative policy ID into its revision identity. Two alternative policies can therefore create different revisions from the same bytes.

A custom card stores its declared policy ID and normalized-data hash in `decoder`. The native export reader validates those records without running custom decoder code.
The reader checks internal data consistency. The record does not authenticate its author.
An unchanged export always returns the original bytes. An edited CHARX export retains the archive and its other entry payloads.
An edited JSON export returns normalized card JSON. A custom container writer needs a separate future export contract.

The library uses `Runtime::import_card` for local bytes. The library uses `Runtime::import_artifact` for bytes with provenance.
Website commands use the same selected decoder and common importer. The host decodes native plugin input on a blocking worker.
The worker retains the selected composition until it returns. The low-level `Card::import` and `Card::import_artifact` methods remain explicit default-parser shortcuts.

### Conversation driver

The [driver module](../crates/runtime/src/driver.rs) defines `ConversationDriver` and `DriverContext`. The native service name is `driver/1`.
The registered type is an outer `Arc` around `Arc<dyn ConversationDriver>`. The `agent::AgentDriver` policy is `agent/1`. It is the default driver and the only built-in driver.

| Interface | Contract |
| --- | --- |
| `ConversationDriver::id` | The method returns the stable driver policy ID. |
| `ConversationDriver::run` | The asynchronous method coordinates one reply through host commands. |
| `DriverContext::snapshot` | The method returns the immutable conversation view. |
| `DriverContext::request` | The method returns the sanitized saved request. |
| `DriverContext::open_step` | The method adds the composed tools and a continuation to the saved request and dispatches one step. |
| `DriverContext::next_event` | The method obtains the normalized events of the open step, including thinking text and tool calls. |
| `DriverContext::append_reasoning` | The method emits a `reasoning_delta` event. The draft does not keep the thinking text. |
| `DriverContext::call_tool` | The host runs one composed tool and emits `tool_call` and `tool_result` events. |
| `DriverContext::append_text` | The method persists text before the host emits its delta event. |
| `DriverContext::cancellation_token` | The method supplies the current phase cancellation token. |

The selected plugin must request and receive `session.read:current`, `session.write:current`, and `provider.generate`. The `DRIVER_CAPABILITIES` constant lists those names.
The standard composition grants them to its essential driver. An installed plugin receives no automatic grant.
The host rejects a selected driver with missing grants before session use. A directly configured native driver uses its embedding application's authority.

The conversation view excludes provider configuration, composition history, and plugin state. The saved request contains credential references only.
The driver receives no credential resolver or database handle. The driver cannot append text before dispatch.
A driver must call `open_step` before `next_event`. Each step uses the same saved request.
The `agent::AgentDriver` opens a new step after each completed step with tool calls. `ProviderAdapter::prepare_agent` extends the saved request for that step.
A tool plugin registers `agent::ToolSet` as a `tools.NAME/1` service. The runtime collects every composed tool set, like the `prompt.hook.` services.
The saved request always contains the composed tool schemas. The list can be empty.
The host records usage before it exposes an accounting event to the driver.

The driver can transform, buffer, or reject reply text. The host permits successful completion only after an actual provider completion event.
A driver cannot hide provider failure, premature closure, or a rejected draft write by returning success. The host owns terminal status and the final assistant transaction.
The host also owns writer leases, fencing, heartbeat renewal, and event delivery.

### Cancellation and limits

The host runs native driver work on a blocking worker with the application runtime handle. The configured `timeout_seconds` value bounds the driver phase.
The default value is 120 seconds. The host rejects a deadline outside the supported clock range.
The driver phase accepts at most 65536 normalized events. The store limits draft text to 8 MiB.

Cancellation and timeout close the host-owned provider stream. The host rejects later text and closes event delivery after the terminal event.
A blocked native worker retains its code and composition until its local work ends. The host does not forcibly stop native code inside its process.
The interface does not create an operating-system sandbox.

The current interface permits one provider request per generation. The host does not expose tools, retries, arbitrary requests, or direct terminal writes through this interface.
A later driver family must prove bounded multi-request behavior before it advertises those features. External stdio plugins still support `card.source/1` only.

## Prompt phases and replay

The runtime saves enrichment before it builds the model request. The host applies explicit section edits before context selection.
The [phase module](../crates/runtime/src/phases.rs) defines the native hook interface.

### Phase order

1. The host captures the card, persona, branch, options, provider profile, and granted plugin state revisions.
2. The host creates a prepared generation with that input snapshot.
3. Each granted enrichment hook obtains its input.
4. The host saves each result before it invokes the next hook.
5. The profile builds a draft with all candidate sections.
6. The host adds static and enriched contributions in declared slot order.
7. Each authorized transform returns edits to named sections.
8. The profile selects context after those edits.
9. The adapter prepares the wire request.
10. The host checks the final request budget and saves the request before dispatch.

The host renews the writer lease throughout these phases. The host retains completed enrichment results after cancellation or failure.
The host does not modify the original submitted input snapshot. The host keeps enrichment progress in the generation payload.
The progress record uses `complete: false` until every enrichment hook finishes. The store rejects changes to a committed result and changes after phase completion.

### Native hook interface

A native plugin registers `Arc<dyn PromptHook>` through an outer `Arc`. The service name must follow `prompt.hook.NAME/1`.
The service owner supplies the manifest, grants, and artifact identity.

```rust
let hook: Arc<dyn PromptHook> = Arc::new(MyHook);
context.register("prompt.hook.memory/1", Arc::new(hook))?;
```

| Method | Contract |
| --- | --- |
| `policy` | The method returns priority, required status, and an enrichment deadline. |
| `enrich` | The asynchronous method returns saved data and prompt contributions. |
| `transform` | The pure method returns explicit `SectionEdit` values. |

The default policy uses priority `0`, required status, and a 30000 millisecond deadline. The host accepts enrichment deadlines from 1 to 120000 milliseconds.
The host orders hooks by priority, plugin ID, and service name. The host uses lower integer priorities first.
The host applies the deadline to enrichment. Enrichment runs on blocking workers with the application runtime handle.
A blocking native callback cannot prevent the supervisor from checking its deadline. The worker retains its plugin lease after cancellation or timeout until its local work ends.
Cancellation stops dispatch while a pure worker finishes its local work. The worker retains its plugin composition until that work ends.

Every hook requires `session.read:current`. The host invokes `enrich` only when the manifest requests `prompt.enrich`.
The host also requires an effective `prompt.enrich` grant for that call. A transform-only hook does not need that capability.
The [lighthouse edit example](../crates/tavern/examples/plugins/lighthouse-edits/README.md) uses a transform-only hook.

`HookInput::snapshot` contains the immutable conversation input. The host removes provider configuration, composition metadata, and plugin state from this view.
`HookInput::state` contains the plugin's own saved state revisions. The host supplies those revisions only with `state.read:PLUGIN_ID`.
The host does not expose another plugin's state through this interface. `TransformInput` also supplies the hook's saved enrichment data and the current prompt plan.

The author must keep `transform` free of external effects. The author must obtain external or random input through `enrich` and save that input in `data`.
The author can then use that saved data during pure compilation and replay. Trusted native code can still use operating-system APIs outside this interface.
The trait interface does not provide an operating-system sandbox.

### Contributions and edits

An `EnrichmentOutput` contains `data` and a `contributions` array. The host limits each result to 1 MiB.
The host limits the saved enrichment record to 8 MiB. Every dynamic contribution uses the same slot and grant checks as a static contribution.
The host assigns its producer ID from the service owner. The contribution can retain source metadata in `provenance`.
The host supplies plugin identity metadata when that field is absent.

The `retention` value can be `required` or `optional`. The default value is `required`.
Required contributions consume the mandatory context budget. Optional contributions can leave the request with an explicit budget omission.
The core selector considers optional contributions before lore, examples, and history. The final wire-budget pass removes history, examples, lore, and remaining optional contributions in that order.

Each `SectionEdit` contains `section_id`, `mode`, and `text`. The supported modes are `prepend`, `append`, and `replace`.
Each target requires `prompt.transform:SECTION_ID`. The target must already exist in the draft.
The host rejects transcript and current-user targets even with an explicit grant. The host rejects two replacements for the same section.
The host limits one hook to 128 edits. The host limits each edited section to 1 MiB.
The edits change the prompt only. The edits do not change the stored card or transcript.

### Failures and trace

A required hook failure blocks dispatch. An optional enrichment failure omits the hook and records a diagnostic.
An optional transform failure omits its edit batch but retains successful enrichment and contributions. Competing replacements always fail.
The saved `phase_trace` records profile, composition, enrichment hash, contributions, edits, and before/after hashes. An extended profile receives an effective name with a hash suffix.

### Offline inspection and replay

`Runtime::inspect_request` prepares without dispatch or enrichment. The method returns `enrichment_required` when an active hook needs saved input.
Transform-only hooks permit direct inspection. `Runtime::replay` recompiles the saved snapshot and enrichment without credentials or external calls.
Replay compares the plan, phase trace, and request. A difference returns `nondeterministic_prompt`.
A different composition, profile, or absent saved enrichment blocks replay. Old generations remain readable and exportable.

```sh
cargo run --locked -- --home ./home prompt replay GENERATION_ID
```

### Profile interface

The experimental `PromptProfile` interface now requires `draft`. The method must return candidate sections before context selection.
The `select_context` method selects complete groups after contributions and edits. The default `compile` method combines those two operations for direct profile callers.
An existing profile must move its section construction into `draft`. The profile must preserve the real input options and leave final selection to `select_context`.

The [core profile](../crates/conversation/src/prompt/mod.rs) supplies the reference implementation. The [phase tests](../crates/tavern/tests/prompt_phases.rs) show complete native hook implementations and host registration.
The tests cover saved state, cancellation, deadlines, denied grants, optional failures, edit conflicts, and nondeterministic replay.

## Plugin-state schemas and migration

The host stores isolated plugin state as immutable key revisions. The host validates the stored schema before a plugin reads or writes that state.
The manifest field `state_schema` declares the schema that the plugin can read. The value must be a positive integer.

### Data model

`PluginStateSnapshot` contains `schema` and `entries`. Each entry uses `{"revision":N,"value":JSON}`.
The SQLite store reads the schema and entries within one read transaction. A namespace without a schema marker uses schema 1.
This rule preserves existing state.

The host stores a schema marker in `@state-schema/PLUGIN_ID`. The marker key is `schema`.
The payload is the schema number. A plugin ID cannot contain `@` or `/`.
A plugin cannot claim that host namespace through its manifest. The database uses schema version 5.
The marker uses the existing `plugin_state` table.

### Native host commands

| Command | Interface and result |
| --- | --- |
| `PluginContext::state_read` | The method requires the owner's read grant. The method returns the latest value or `None`. |
| `PluginContext::state_write` | The method requires the owner's write grant and namespace. The method appends one revision after a schema check. |
| `PluginContext::state_migrate` | The method requires both owner grants. The method returns a `PluginStateSnapshot` after an atomic upgrade. |

The grants are `state.read:PLUGIN_ID` and `state.write:PLUGIN_ID`. The host checks each grant against the manifest request and the caller's approval.
The write command compares the stored schema within its write transaction. An unreadable or mismatched schema returns `unsupported_plugin_state_schema`.
The runtime also validates the schema before it captures granted state for a generation. The saved plugin composition retains the expected schema.
The hook receives the existing entry format.

### Migration interface

The callback receives `&PluginStateSnapshot`. The callback returns `Result<BTreeMap<String, serde_json::Value>>`.
The map contains replacements and added keys. The host retains keys that the callback omits.
The callback must preserve unknown fields within any replacement when its data format requires them.

```rust
context.state_migrate(store, |before| {
    if before.schema != 1 {
        return Err(rustytavern::Error::new(
            "unsupported_plugin_state_schema",
            "This migration requires state schema 1.",
        ));
    }
    let count = before.entries["counter"]["value"].as_i64().ok_or_else(|| {
        rustytavern::Error::new("invalid_plugin_state", "The counter must be an integer.")
    })?;
    Ok(std::collections::BTreeMap::from([
        ("counter".into(), serde_json::json!({"count": count})),
    ]))
})?;
```

The [complete example](../crates/tavern/examples/plugins/state-migration.md) supplies the plugin, grants, store, and executable. The example manifest declares state schema 2.
An author can validate several known source schemas in one callback. The host rejects a downgrade.
The host skips the callback when the stored schema already matches the manifest.

The callback runs outside the database lock. The callback must perform a pure conversion.
The callback must return an error when its input is unreadable. The host catches a callback panic before it writes migration data.
The store then compares the complete old snapshot within an immediate write transaction. A concurrent state change returns `configuration_conflict`.
The caller can inspect the new state before it retries. The host does not retry the callback automatically.

The transaction appends every replacement and the schema marker together. An SQL failure rolls back all of those rows.
The store retains all earlier revisions after success. A completed migration remains committed after a later activation failure.
The registry owns service activation rollback separately. A plugin author must account for that operation boundary.

### Storage backend interface

The [store module](../crates/store/src/lib.rs) defines three schema-aware operations.

| Method | Required behavior |
| --- | --- |
| `plugin_state_snapshot` | The method returns one consistent schema and entry view. |
| `plugin_state_write` | The method validates the expected schema and appends one value atomically. |
| `plugin_state_migrate` | The method compares the complete expected snapshot and commits the upgrade atomically. |

A replacement backend must implement these operations. The backend must reject stale snapshots and non-increasing migration targets.
The raw `state_*` methods remain trusted host storage operations. An application must use the checked context commands for plugin access.
Native Rust code still shares process authority. The host interface does not provide an operating-system sandbox.

### Plugins, snapshots, and exports

A native factory can migrate state during activation. The host registers the factory before plugin restoration or activation.
The external card-source protocol has no migration callback. A changed composition requires explicit session adoption.
Migration does not rewrite generation snapshots. Replay uses saved entries and pinned composition.
New generation rejects an unreadable current schema. Version 4 session exports retain namespace state and opaque schema-marker histories.
Uninstallation retains that state.

## Installed plugins

A user installs a third-party plugin from a local directory. The directory contains one `plugin.json` file and the files that the plugin uses.
The `PluginManager` owns installation and enabled state. The `PluginRegistry` owns runtime plugin instances.
The manager records each installed plugin as a `[plugins.ID]` table in `config.toml` with `path`, `grants`, and `trust_processes`.
The caller saves `PluginManager::settings` into `config.toml` after a change.

Installation reads `plugin.json` from its path and activates the plugin. Installation does not copy the directory or run a script.
Activation validates command services before the manager records the plugin.
An activation failure removes the newly activated plugin and restores the previous service bindings.
The next application process restores the enabled installed plugins from `config.toml`.
An enabled plugin does not change an existing session's saved composition automatically. The user adopts that change through `session configure`.

### Plugin manifest

The `plugin.json` file holds the runtime manifest, one `implementation` object, and optional `commands`.
The [application module](../crates/tavern/src/app/mod.rs) defines `PluginDefinition`. A native factory uses this shape.

```json
{
  "id": "example.note",
  "version": "0.1.0",
  "host_protocol": "1",
  "requested_capabilities": ["prompt.contribute:before_character"],
  "allowed_slots": ["before_character"],
  "implementation": {
    "kind": "builtin",
    "name": "prompt.contributions",
    "configuration": {
      "contributions": [{
        "plugin_id": "",
        "section_id": "setting",
        "slot": "before_character",
        "priority": 0,
        "role": "system",
        "text": "The harbor has a chart room."
      }]
    }
  }
}
```

The [essential plugin reference](developer.md#essential-plugins) lists the compiled factory names. An embedded application can add a factory through `register_factory`.
The `NativeFactory` receives a `PluginDefinition`. The factory returns `Result<Arc<dyn RuntimePlugin>>`.
The application must register its factories before `restore` or `enable`. The stock CLI does not load arbitrary native code from a plugin directory.
The [installed plugin tests](../crates/tavern/tests/installed_plugins.rs) show a custom native factory.

A process plugin uses `"kind": "process"` and a `process` object. A `{plugin}/` prefix selects a file inside the plugin directory.
The host requires `--trust-processes` before it starts the executable. The host does not provide an operating-system sandbox.

A `{plugin}/` path must stay inside the plugin directory. The manager rejects symbolic links and parent traversal in that path.

### Commands

A command names a declared runtime service and an operation. The host accepts `Arc<dyn CommandService>` for a general native command.
The `call` method receives the operation, JSON arguments, and a cancellation token. The method returns JSON through `rustytavern::Result`.
A card-source command uses `Arc<dyn CardSource>` with the `fetch` operation. The host validates and stores the returned card through the common importer.
The CLI runs a command through `plugin run NAME --args JSON`. The CLI also accepts a root alias such as `example-import --url URL`.

### Implementation procedure

1. Copy the [contribution example](../crates/tavern/examples/plugins/lighthouse-notes/README.md) or the [process example](../crates/tavern/examples/plugins/inline-source/README.md).
2. Set a unique plugin ID and a semantic version.
3. Add a README that explains the design, interface, grants, and commands.
4. Install the plugin directory.
5. Enable the plugin with only its required grants.
6. Verify its runtime effect and commands.
7. Restart the CLI and verify restoration.
8. Disable the plugin and verify removal of its effect.

```sh
cargo run --locked -- --home ./home plugin install crates/tavern/examples/plugins/lighthouse-notes --grant prompt.contribute:before_character --grant prompt.contribute:after_character
cargo run --locked -- --home ./home plugin inspect lighthouse-notes
cargo run --locked -- --home ./home plugin disable lighthouse-notes
cargo run --locked -- --home ./home plugin uninstall lighthouse-notes
```

The manager rejects an update while the installed plugin remains active. Disable the plugin before installation of its replacement.
Uninstallation removes the plugin table from `config.toml`. Uninstallation keeps user plugin state.
A plugin update requires explicit session adoption before another generation.
The plugin manifest accepts a positive `state_schema`. A compiled native factory can call the host migration command during activation.

### Example plugins

| Plugin | Purpose |
| --- | --- |
| [Lighthouse notes](../crates/tavern/examples/plugins/lighthouse-notes/README.md) | The plugin adds two static prompt contributions. |
| [Lighthouse edits](../crates/tavern/examples/plugins/lighthouse-edits/README.md) | The plugin adds a pure prompt hook with explicit section edits. |
| [Inline source](../crates/tavern/examples/plugins/inline-source/README.md) | The plugin runs an independent Rust process and declares a command. |

The author must test failure rollback, restart, disable, uninstall, and denied capabilities.

## Extensions

An extension changes how an application looks. An extension cannot change agent behavior or agent context.
A plugin does that work. An extension cannot contain plugins, commands, or grants.
The [extension module](../crates/tavern/src/extension.rs) defines `Extension` and `theme`.

Each `[extensions.NAME]` table in `config.toml` is one enabled extension. Remove the table to remove the extension.

```toml
[extensions.rose]
description = "Pink user bubbles."
user_bubble = "#ff5f87"
assistant_bubble = "magenta"
```

| Field | Meaning |
| --- | --- |
| `description` | The optional text describes the extension. |
| `user_bubble` | The optional color sets the user bubble. |
| `assistant_bubble` | The optional color sets the character bubble. |

A color is a name, such as `magenta`, or a hex value, such as `#5f87ff`. The terminal keeps its default for an invalid color.
The configuration rejects unknown fields. `theme` merges the extensions in name order. A later name overrides an earlier name.
The terminal reads the theme when it starts. The `extension list` command prints the extensions.
The [configuration tests](../crates/tavern/tests/configuration.rs) cover themes and rejected fields.

## External card-source protocol

An external plugin runs as a trusted child process. The process communicates through JSON-RPC 2.0 on standard input and standard output.
The current host supports the `card.source/1` family only. The host does not support external prompt profiles, providers, or stores yet.
The [Rust example](../crates/plugin/examples/source_plugin.rs) implements the complete inline-card path.

### Process definition

The installed plugin manifest selects its executable with this object.

```json
{
  "implementation": {
    "kind": "process",
    "process": {
      "executable": "{plugin}/source-plugin",
      "args": [],
      "accepted_hosts": ["example.com"],
      "timeout_ms": 30000
    }
  }
}
```

The executable must use an absolute path or a `{plugin}/` path. The host resolves `{plugin}/` arguments to installed files.
The author must preserve executable permissions on the installed binary. The timeout must fall between 1 and 120000 milliseconds.
The host clears the child environment except for `PATH`. The host discards child standard error in the current implementation.
The plugin must write only protocol frames to standard output. The plugin must not depend on a particular working directory.

The `accepted_hosts` list controls source URL recognition. An empty list accepts any HTTPS host for recognition.
The list does not grant host network access. The host fetch callback requires a separate `network:https:HOST` capability.
The process can perform its own operating-system operations because the host supplies no sandbox.

### Framing and limits

Each frame contains one UTF-8 JSON object and ends with a newline. Each frame can contain at most 1 MiB, including its newline.
Each invocation can receive at most 8 MiB of response and callback frames. The request and host callback responses share a separate 8 MiB output budget.
The host checks that output budget before it writes each frame. Each invocation can issue at most 64 host callbacks.
Each invocation can hold at most eight fetched artifacts. The fetched artifact total cannot exceed 32 MiB.
The client permits at most 64 outstanding calls and 8 MiB of queued parameters. The host serializes exchanges with one child process.

Each response must copy the corresponding request `id` exactly. The host uses increasing numeric IDs.
The plugin must not assume that an ID always has a particular value. A host callback can use its own string or unsigned numeric ID.
The host rejects malformed frames, mismatched IDs, and premature output closure. The host stops the child after an active invocation fails, times out, or receives cancellation.
Cancellation of a queued invocation does not stop the active invocation or child. The host does not retry an invocation automatically.

### Cancellation and shutdown

Each request carries its numeric ID in the JSON-RPC envelope. Each request includes an absolute Unix deadline in `params.deadline_ms`.
The deadline includes time in the invocation queue. The source operation also includes a unique `params.scope` for artifact ownership.
A source operation has no session scope because card import can precede session creation.

The host sends this notification when it stops an active invocation.

```json
{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":2,"reason":"cancelled"}}
```

The notification identifies the original request. The `reason` contains the host error code, such as `cancelled` or `plugin_timeout`.
The plugin must stop that request's work and release its resources. The plugin must not reply to the notification.
The host rejects all later results for that invocation. An implementation with long operations must keep its request reader responsive to cancellation.
The Rust example completes its local source operation synchronously.

The host then sends a shutdown request. The host also sends this request during normal plugin removal.

```json
{"jsonrpc":"2.0","id":3,"method":"shutdown","params":{}}
```

The plugin must release its resources and exit. The host accepts process exit without a shutdown response.
The host closes its input pipe after it sends shutdown. The host permits at most two seconds for notification, shutdown, and process exit.
The host then terminates an unresponsive child and waits for process cleanup. The forced cleanup wait has a separate two-second limit.
A failed pipe write can cause earlier forced termination. The host terminates the configured child only.
The plugin remains responsible for any descendant processes that it creates.

### Handshake

The host sends this request before any source operation.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"host_protocol":"1","plugin_id":"example.source.inline","service_families":["card.source/1"],"deadline_ms":2000000030000}}
```

The plugin returns its matching identity and supported family.

```json
{"jsonrpc":"2.0","id":1,"result":{"host_protocol":"1","plugin_id":"example.source.inline","service_families":["card.source/1"]}}
```

The host rejects a mismatched identity, protocol, or family. The plugin cannot call host services during initialization.

### Source request

The host sends a URL and an invocation scope.

```json
{"jsonrpc":"2.0","id":2,"method":"card.source.fetch","params":{"url":"https://example.com/lumen","scope":"invocation-EXAMPLE","deadline_ms":2000000030000}}
```

The plugin can return a complete card JSON object.

```json
{"jsonrpc":"2.0","id":2,"result":{"card":{"spec":"chara_card_v2","spec_version":"2.0","data":{"name":"Lumen","description":"Lumen keeps the lighthouse.","personality":"Calm.","scenario":"A harbor.","first_mes":"Hello.","mes_example":""}}}}
```

The common importer validates the card. The host supplies provenance and a content hash for the inline result.
The caller stores the validated card through the same path as a local import.

### Host fetch callback

The plugin can ask the host to fetch a card artifact. The plugin must request the destination host capability in its manifest.
The user must grant that capability at activation.

```json
{"jsonrpc":"2.0","id":"download-1","method":"host.fetch","params":{"url":"https://example.com/card.png"}}
```

The host checks HTTPS, destination grants, DNS results, and redirect destinations. The host returns an opaque artifact handle without its bytes.

```json
{"jsonrpc":"2.0","id":"download-1","result":{"artifact_id":"artifact_EXAMPLE","sha256":"SHA256_HEX","bytes":1024,"media_type":"image/png"}}
```

The plugin returns the handle as the source result.

```json
{"jsonrpc":"2.0","id":2,"result":{"artifact_id":"artifact_EXAMPLE"}}
```

The handle remains valid only within the current invocation. The host rejects a handle from another invocation.
The host validates the downloaded card before it accepts the result. The callback does not expose a general response-body API to the child.
A source that must transform remote metadata needs its own trusted process I/O or a native adapter.

### Errors and implementation

The plugin returns an ordinary JSON-RPC error for an unsupported method.

```json
{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"The method does not exist."}}
```

The host reports a safe `plugin_error` to the application. The host does not expose arbitrary child error text through the public error.
The host uses stable error codes for transport failures and invalid artifacts.

1. Implement newline framing and bounded input reads.
2. Implement `initialize` and `card.source.fetch`.
3. Copy each request ID into its response.
4. Return an inline card or an invocation-scoped artifact handle.
5. Add the executable to a [plugin directory](developer.md#installed-plugins).
6. Test handshake failure, malformed frames, cancellation, and timeout.

The [inline-source README](../crates/tavern/examples/plugins/inline-source/README.md) supplies build and CLI commands. The [external tests](../crates/tavern/tests/external.rs) exercise protocol failures and limits.
The [process lifecycle tests](../crates/tavern/tests/process_lifecycle.rs) verify cooperative shutdown, late-result rejection, queue limits, and forced child cleanup on Unix.
