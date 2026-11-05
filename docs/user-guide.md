# User guide

## Interactive terminal

The `rustytavern` binary opens an interactive conversation by default. The terminal provides a transcript, a message composer, and a status line.
Slash commands open forms and selectors. The forms do not require command flags.
The existing command surface remains available for scripts.

### User tasks

| Command | Task |
| --- | --- |
| `/setup` | The user configures a model provider. |
| `/model` | The user selects or edits a provider profile. |
| `/character` | The user selects or imports a character. |
| `/worldbook` | The user imports, exports, or binds worldbooks. |
| `/persona` | The user selects or creates a persona. |
| `/new` | The user starts a conversation with the selected records. |
| `/sessions` | The user resumes a saved conversation. |
| `/config` | The user inspects product configuration. |
| `/plugins` | The user inspects plugins and binds services. |
| `/extensions` | The user installs, enables, disables, or removes packages. |
| `/prompt` | The user inspects the complete request for a proposed message. |
| `/adopt` | The user explicitly adopts changed configuration. |
| `/retry` | The user retries or regenerates the current turn. |
| `/branch` | The user selects a conversation head. |
| `/export` | The user exports the active conversation. |
| `/help` | The user reads the command and key reference. |
| `/quit` | The user closes the terminal. |

The composer sends ordinary text to the active conversation. The command palette completes slash commands.
Enter submits text or a form. Alt+Enter inserts a newline.
Tab changes the active form field. Escape closes a dialog.
Page Up and Page Down scroll the transcript. Ctrl+C cancels active generation.
Ctrl+D closes an idle terminal. An exit during generation requests cancellation and waits for durable completion.

### Ownership

The CLI owns rendering, input, forms, and selection preferences in `terminal.json`. The library owns durable records and product operations.
Slash commands call typed operations without shell conversion. The database remains the history authority.
The display uses a bounded transcript window and marks truncated text. Exports retain full history.
The request form can write the complete request to a file. The terminal permits one active operation and retains runtime leases.
The terminal never grants capabilities automatically. The setup form stores a credential variable name only.

## CLI reference

The binary name is `rustytavern`. The binary opens an interactive conversation when the caller supplies no subcommand.
The terminal requires an interactive input and output device. The remaining sections define the script interface.
The global `--data-dir PATH` option selects the data directory. The default order is `RUSTYTAVERN_DATA_DIR`, `XDG_DATA_HOME/rustytavern`, and the platform data directory under HOME.
The application stores `config.json` and `sessions.sqlite3` in that directory. The configuration retains unknown product settings as explicit extension settings.
Provider profile updates preserve those settings.

The CLI prints metadata as JSON. The `--json` flag makes this intent explicit on inspection commands.
The CLI prints structured errors to stderr. The chat command prints reply text unless the caller selects `--events`.
A message accepts `--message TEXT` or `--message-file PATH`. A message-file value of `-` reads stdin.

### Character archives

The `card import PATH` command accepts JSON, PNG, and CHARX cards. The `card export CHARACTER_ID --output PATH` command returns original bytes.
The `--normalized` option exports card JSON. The `--patch PATH` option applies a JSON merge patch and retains the input container format.
The `card asset CHARACTER_ID --uri URI --output PATH` command writes one embedded CHARX asset. The command requires an exact `embeded://` URI.
The command validates the asset before it creates the output file.

### Provider profiles

The `config provider` command writes one named profile. The `config show` command prints the effective configuration with enabled extension defaults.
Explicit configuration values override those defaults. The save operation writes explicit provider settings only.
The profile stores a provider kind, model ID, endpoint, credential environment-variable name, context limit, and output limit. A custom provider requires an explicit full endpoint URL.
A custom provider can omit credentials for a local model. The native provider kinds are `openrouter`, `openai`, `anthropic`, `gemini`, and `custom`.
The adapters use Chat Completions, Responses, Messages, or native Gemini SSE as appropriate. The `config capabilities` command prints the supported adapter subset for each kind.
The `config provider --parameters JSON` option supplies validated sampling settings. The `--routing JSON` option supplies explicit OpenRouter provider preferences.
The [provider contracts](compatibility.md#provider-contracts) define these settings and their model restrictions. Provider profile updates affect a session only after `session configure` adopts them.

The adapter rejects unsupported requested parameters. The adapter does not switch a model or vendor automatically.
The adapter does not retry a paid request automatically. The runtime records a failed generation when a stream closes without terminal success.

### Generation events

The `--events` option prints NDJSON. Each event contains `type` and `generation_id`.

| Type | Meaning |
| --- | --- |
| `prepared` | The store contains the submitted input and prepared generation. |
| `text_delta` | The store contains the acknowledged draft text. |
| `usage` | The adapter reports provider accounting data. |
| `completed` | The store contains the committed assistant node. |
| `failed` | The generation ends with a typed error. |
| `cancelled` | The caller cancels generation. |
| `interrupted` | Recovery identifies an expired unfinished writer. |

A text event contains `text`. A usage event contains `usage`.
A terminal event can contain the full draft in `text`. A failed event contains `error.code` and `error.message`.
The optional `missed_events` field signals an observer gap. The caller should use the durable final record after a gap.
Ctrl+C requests cancellation and waits for terminal persistence. The runtime saves usage before it emits a usage event.
Failure, cancellation, and recovery retain saved usage.

### Prompt inspection

The `prompt inspect SESSION_ID --message TEXT --json` command compiles the selected provider request. The command resolves the session provider profile without reading its credential value.
The command uses the same final request-body budget as generation. The plan includes `serialized_input_units` with the conservative request byte count.
The plan records `serialized_request_budget` omissions when wire overhead removes context. The command makes no model request.
The `--request` option returns the complete compiled request. The result contains `plan`, `prepared`, and `phase_trace`.
The `prepared.body` field contains the exact JSON body for dispatch. The result includes the endpoint, protocol, credential reference, and transformation trace.
The result excludes authentication headers and credential values. The default output contains the prompt plan only.
The command returns `enrichment_required` when an active hook needs external input. The `prompt replay GENERATION_ID` command uses the generation's saved enrichment and state revisions.
Replay runs pure compilation and compares the result with the saved request. Replay makes no enrichment or model request.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The operation completes. |
| `1` | A provider, source, transport, or I/O operation fails. |
| `2` | Input, configuration, or an unsupported format blocks the operation. |
| `3` | Compatibility or context constraints block generation. |
| `4` | A session conflict, missing record, or storage invariant blocks the operation. |
| `130` | The caller cancels generation. |

### Plugins and extensions

The `plugin inspect [PLUGIN_ID]` command lists standard plugins and declared package plugins. The command does not start external processes.
The `plugin bind SERVICE PLUGIN_ID` command saves an explicit service binding. The `preset inspect tavern --resolved` command resolves the enabled composition.
The resolved inspection can start enabled trusted processes.

The `extension install PATH` command copies and hashes a local package. The `extension install --builtin chub` and `--builtin risurealm` forms install bundled sources.
Installation does not enable the package. The `extension enable ID` command checks dependencies and activates plugins.
Each `--grant CAPABILITY` option approves one requested host capability. The `--trust-processes` option permits package executables.
The `extension list` and `extension inspect ID` commands read package metadata. The `extension disable ID` command removes the package features.
The `extension uninstall ID` command removes a disabled catalog entry. Uninstallation keeps plugin state and cached package files.

The `extension asset ID PATH --output DESTINATION` command writes a declared asset. The `extension run NAME --args JSON` command calls an enabled extension command.
The command also accepts `--args-file PATH` or `--url URL`. The root alias form accepts `NAME --url URL` or `NAME --args JSON`.
The `card fetch URL --source NAME` command imports through an enabled source plugin. Source imports retain provenance in the stored card record.
Ctrl+C cancels a source or extension operation.

The [extension guide](developer.md#product-extensions) includes complete manifest and implementation instructions. The [example index](../crates/tavern/examples/README.md) links executable examples.
The command help lists current commands and flags.

### Session and persona exports

The [native export interface](#native-records-and-exports) defines session and persona exports. The `session show SESSION_ID` command includes generation summaries and retained drafts.

## Session configuration revisions

A session uses saved configuration revisions. The host requires an explicit revision before generation or prompt inspection.
The host does not substitute current plugins when a saved composition differs.

### CLI interface

The `session new` command saves revision `1` with the initial session. The command saves the provider settings and resolved composition atomically.
Later `chat`, `session retry`, `session regenerate`, and `prompt inspect` commands use those saved provider settings. A change to `config.json` does not change an existing session.

```sh
cargo run --locked -- --data-dir ./data session configure SESSION_ID
cargo run --locked -- --data-dir ./data session configure SESSION_ID --provider openrouter --model openai/gpt-4.1-nano
cargo run --locked -- --data-dir ./data session configure SESSION_ID --context-limit 16384 --output-limit 512 --expected-version VERSION
```

The `session configure` command adopts the current enabled composition. The command reloads the selected provider profile.
The command uses that profile's model and limits unless an option overrides them. The command retains the previous model when the profile has no model.
The optional `--expected-version` value checks the session writer version. The command rejects a stale version and an active generation.

The `session rebind` command appends a revision for a character or persona change. The command retains the existing provider settings and composition.
The command does not rewrite greetings, transcript nodes, or generation snapshots.

The `session show` command exposes current and previous revisions in `session.composition.session_configurations`. The `configuration_schema` value identifies nested format version `1`.
The separate session `version` protects all writer operations. The configuration `revision` identifies a configuration change only.
The CLI returns exit status `2` for missing, invalid, unsupported, or oversized configuration input. The CLI returns exit status `4` for writer conflicts and unavailable or changed compositions.

### Library interface

The caller creates or configures a session through the runtime.

```rust
let session = runtime.create_session(&target, Some("Hello."), provider_config.clone()).await?;
let saved = runtime.provider_config(&session.id).await?;
let handle = runtime.start(&session.id, "Hello.", None, saved).await?;
let mut target = store.session(&session.id)?;
target.model = "another-model".into();
let updated = runtime.configure_session(&target, provider_config).await?;
```

The host checks `target.version` and references before atomic persistence. The host retains earlier revisions.
Direct store creation leaves a session unconfigured until `Runtime::configure_session` succeeds. An unconfigured session permits reads and exports but returns `session_configuration_required` for generation.

### Record and lock

The [session module](../crates/conversation/src/session.rs) defines `SessionConfiguration`. The [configuration schema](schemas/session-configuration-v1.schema.json) defines its serialized fields.

| Field | Content |
| --- | --- |
| `revision`, `identity` | The record stores its consecutive number and SHA-256 content hash. |
| `character_id`, `persona_id` | The record pins imported revision IDs. |
| `provider`, `model` | The record identifies the provider profile and selected model. |
| `provider_config` | The record retains settings and credential references. |
| `options`, `accept_reduced` | The record retains prompt settings and compatibility acceptance. |
| `profile` | The record identifies the selected base prompt profile. |
| `composition` | The record retains the exact service and product lock. |

The composition records plugin manifests, artifacts, configuration hashes, grants, bindings, and contributions. The product composition also records extension versions, package hashes, and asset hashes.
The `service_identity` value identifies the registry snapshot. The full `identity` also includes the product metadata and native implementations.
The native metadata records the executable hash, provider, profile, decoder, and driver implementation IDs. The default provider implementation ID uses its Rust type name.
The `ProviderAdapter::id` method can supply another stable implementation ID. The `validate_config` method must remain pure.
A custom adapter must override that method when its parameters differ from the HTTP adapter.

A different executable can require explicit adoption. This rule also applies when a CLI continues a session that a separate example executable created.
The host never resolves credential values while it saves a revision. Trusted native code remains responsible for its declared configuration and hidden mutable state.
An executable hash does not isolate native code.

### Failures and limits

| Error code | Meaning |
| --- | --- |
| `session_configuration_required` | The session has no saved configuration. |
| `session_configuration_changed` | The caller supplies different provider settings. |
| `pinned_composition_unavailable` | Current services or product metadata differ from the saved lock. |
| `composition_changed` | An embedded application must resolve its changed product composition again. |
| `session_conflict` | A stale writer version or active generation blocks the update. |
| `invalid_session_configuration` | A content hash, revision, limit, or current binding is invalid. |
| `unsupported_session_configuration` | The nested configuration schema is unsupported. |
| `session_configuration_limit` | The retained history exceeds a resource limit. |

The host permits at most 128 revisions and 8 MiB of serialized configuration history. The host returns an error before it commits a change that exceeds either limit.
The host retains previous records after a rejected change. The host does not remove old revisions automatically.

The native session export retains every configuration record. The export also archives each referenced card and persona, including revisions without generations.
The database uses marker `5` for the current storage contract. The database upgrade retains unconfigured legacy sessions.
An unsupported or damaged configuration blocks mutation through the configuration command. The stored transcript remains readable.

### Worldbook selections

Each configuration pins an ordered `worldbook_ids` list. The decoder treats an absent list as empty.
The encoder omits empty lists to retain older configuration hashes.

## Worldbooks

The library imports portable character books, `lorebook_v3` envelopes, and native SillyTavern World Info files. The format crate retains original bytes and unknown JSON fields.
The store uses the full SHA-256 hash of those bytes as the archive ID. A changed file creates a new ID.
The store does not replace an existing archive.

### Terminal controls

The `/worldbook` menu provides import, export, Add, and Remove actions. Each binding action appends a configuration revision.
The action rejects a stale version or an active generation.

### Script commands

```sh
rustytavern worldbook import ./harbor.json
rustytavern worldbook list
rustytavern worldbook inspect WORLDBOOK_ID
rustytavern worldbook export WORLDBOOK_ID --output ./harbor-copy.json
rustytavern worldbook bind SESSION_ID --book WORLDBOOK_ID --expected-version VERSION
rustytavern worldbook bind SESSION_ID
```

The export command writes the exact original bytes. The bind command replaces the full ordered selection.
A bind command without `--book` clears the selection. The command retains the saved provider settings and plugin composition.
The command does not adopt changed product settings.

### Worldbooks: Library interface

The caller imports bytes with `WorldBook::import` and saves them with `StoreBackend::save_worldbook`. The caller sets `Session::worldbook_ids` before creation or submits a new configuration revision.
Custom stores implement `save_worldbook` and `worldbook`. Default methods reject unsupported storage operations.
The selection permits at most 16 unique archives and 8 MiB of original bytes. Each generation snapshots selected IDs and documents for replay.
Old generations retain their selections. Native exports archive every referenced book.

### Prompt profiles

The `core-v1` profile uses portable books. The profile scans the submitted message and saved history.
The profile applies each book's `scan_depth`. The profile retains the embedded book first.
The profile then uses standalone books in selection order. Each book sorts its matches by `insertion_order` and source position.
The profile prefixes standalone lore IDs with the archive ID. The profile applies the shared lore and context budgets after prompt hooks.
The profile expands `{{char}}` and `{{user}}` in lore text.

The profile reports unsupported portable rules through diagnostics. Recursive scanning and per-book `token_budget` require explicit acceptance of reduced behavior.
The shared `lore_budget` remains the execution limit. Native World Info execution remains unsupported in `core-v1`.
The runtime rejects that execution even when the session accepts reduced behavior. The format and store still retain those archives for exact export.
A replacement profile implements `PromptProfile::draft_with_worldbooks` to execute other rules. The default trait method rejects a nonempty selection.

The [worldbook tests](../crates/tavern/tests/worldbooks.rs) check requests, replay, restart, limits, export validation, and migration. The [database figure](architecture.md#db-model) shows SQL and logical references.

## Native records and exports

The library owns native serialization and validation. The CLI reads arguments and writes the resulting bytes.

| Record | Version | Authority |
| --- | --- | --- |
| SQLite database | `5` | The database stores the marker in `PRAGMA user_version`. |
| Application configuration | `1` | `config.json` stores `schema_version`. |
| Runtime input snapshot | `2` | Each generation stores an immutable snapshot. |
| Enrichment progress | `1` | Each generation stores its saved hook results. |
| Native session export | `4` | The export contains revision archives and conversation records. |
| Native persona export | `1` | The export contains one content-addressed persona. |
| Session configuration | `1` | The session composition retains immutable configuration revisions. |

### Session export

```sh
cargo run --locked -- --data-dir ./data session export SESSION_ID --output session.json
cargo run --locked -- --data-dir ./data session export SESSION_ID --output session-traces.json --include-traces
```

The default export sets `includes_prompt_traces` to `false`. The exporter sets each generation's `request` to `null` in that mode.
The `--include-traces` flag retains the compiled plan, prepared request, and phase trace. Both modes retain conversation text, input snapshots, saved enrichment, and plugin state.
The flag controls compiled request traces only.

The [version 3 session schema](schemas/session-v3.schema.json) documents the historical version 3 format. That schema does not describe version 4 worldbook archives.
The version 4 reader and writer use the Rust types and runtime validation. The [session module](../crates/conversation/src/session.rs) defines the corresponding Rust types.

| Field | Content |
| --- | --- |
| `session` | The record contains current bindings, options, selected head, and version. |
| `characters` | The archive contains the current card and every referenced earlier card revision. |
| `personas` | The archive contains the current persona and every referenced earlier persona revision. |
| `messages` | The array contains the complete parent tree in insertion order. |
| `generations` | The array contains statuses, drafts, snapshots, enrichment, usage, errors, and acceptance links. |
| `plugin_state` | The array contains opaque state revisions for referenced plugin namespaces. |

The exporter reads these records in one SQLite read transaction. The exporter uses card and persona IDs from the session, configuration revisions, and generation snapshots.
The exporter derives state namespaces from the recorded compositions and pinned state inputs. The exporter retains every stored revision in those namespaces.
The exporter does not include unrelated state namespaces. The exporter does not interpret a plugin's state payload.
Unknown nested state values survive the export.

Each card archive contains its source hash, document, import metadata, provenance, and original bytes. A custom card also retains its decoder policy ID and normalized-data hash.
The reader validates custom records without loading decoder code. The JSON writer stores original bytes as base64 text.
The reader also accepts legacy arrays of integers from 0 to 255. The byte decoder enforces the card input limit without an intermediate numeric JSON tree.
An unchanged card export still returns those exact bytes. The character and persona archives use ID order.
The message and generation arrays use insertion order. State entries use namespace, key, and revision order.

### Library byte interface

```rust
let export = store.export_session(session_id, false)?;
let bytes = export.encode()?;
let decoded = SessionExport::decode(&bytes)?;
```

The encoder validates before serialization. The decoder checks the version, identities, references, trace settings, and accepted draft links.
The reader accepts versions 2, 3, and 4. The writer uses version 4.
Version 3 adds custom decoder records and containers. Version 4 adds worldbook archives.
Version 1 lacks complete revision archives and remains unsupported. The product supplies no native session-import CLI command.

### Persona export

```sh
cargo run --locked -- --data-dir ./data persona export PERSONA_ID --output persona.json
```

The [persona schema](schemas/persona-v1.schema.json) defines the envelope. The `PersonaExport` type contains `schema_version: 1` and `persona`.
The persona record contains its ID, name, and description. The decoder verifies the ID against the name and description.

### Explicit partial-reply acceptance

```sh
cargo run --locked -- --data-dir ./data session show SESSION_ID --json
cargo run --locked -- --data-dir ./data session accept-draft SESSION_ID GENERATION_ID
```

The show command lists generation IDs, statuses, drafts, usage, and acceptance links. The acceptance command requires a nonempty failed, cancelled, or interrupted draft.
The selected branch must end at that generation's user node. The store rejects acceptance while another writer owns the session.
The optional `--expected-version VERSION` argument rejects a stale caller view.

Acceptance creates an assistant message under the original user node. Acceptance selects that message as the session head.
The generation retains its original status and draft. The `accepted_message` field links the generation to the new message.
The next prompt includes the accepted message through normal history selection. An unaccepted draft remains outside message history.
A repeated acceptance returns the existing message with a current session version. The command does not change a completed generation into a partial reply.

### Database migration and credentials

The store creates schema 5 and upgrades versions 1 through 4 atomically. The upgrade retains rows and adds the worldbook archive table.
Decoder defaults supply absent enrichment and acceptance fields. Legacy sessions require explicit configuration before generation.
A failed upgrade rolls back. A future marker blocks writes.
Profiles and snapshots store credential references only. The provider resolves credential values during dispatch.
The [export tests](../crates/tavern/tests/native_exports.rs) check credential exclusion in both trace modes.

### Plugin schema markers

The exporter includes `@state-schema/PLUGIN_ID` revisions for each selected plugin namespace. The marker key is `schema`.
The exporter preserves unknown marker payloads as opaque JSON.

### Worldbook archives

Version 4 adds `worldbooks`. The decoder validates archive bytes, IDs, documents, and selected references.
Versions 2 and 3 remain readable without worldbooks.


## Model discovery

```sh
rustytavern model discover PROVIDER --input text --output text --parameter tools
rustytavern model discover PROVIDER --endpoint URL
rustytavern model select PROVIDER MODEL
```

Each discovery filter is optional. The `/models` command opens a provider chooser, a discovery form, and a model chooser.
Selection updates the provider profile. An existing session still requires explicit configuration adoption.
The [catalog contract](compatibility.md#model-catalog) distinguishes advertised capabilities from verified behavior. The integrated CLI and terminal gate remains pending.
