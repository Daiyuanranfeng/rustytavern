# User guide

## Interactive terminal

The `rustytavern` binary opens an interactive conversation by default. The terminal writes the chat history into the native terminal scrollback.
A live area below the history shows the active reply, the message composer, and a status line. Dialogs use the alternate screen.
Slash commands open forms and selectors. The forms do not require command flags.
The existing command surface remains available for scripts.

### User tasks

| Command | Task |
| --- | --- |
| `/provider` | The user connects a provider with an API key or replaces its key. Several providers can stay connected. |
| `/model` | The user selects a model from the combined catalogs of all connected providers. The model selects its provider. |
| `/character` | The user selects a character, or previews and adds one from a file or web address. |
| `/persona` | The user selects or creates a persona. |
| `/new` | The user starts a chat and selects the first greeting. |
| `/chats` | The user resumes, renames, or deletes a saved chat. |
| `/retry` | The user regenerates the last reply. |
| `/messages` | The user selects a message of the open branch, then rewinds, edits, regenerates, branches, hides, or copies it. |
| `/continue` | The model extends the last reply. |
| `/impersonate` | The model writes the next user message into the composer. |
| `/memory` | The user reads, edits, or updates the memory of the open chat. |
| `/export` | The user exports the open chat to a file. |
| `/prompt` | The user inspects the next model request. |
| `/settings` | The user changes the model, the providers, the prompt and memory options, the agent options, the plugins, and the lorebooks of the open chat. |
| `/help` | The user reads the command and key reference. |
| `/quit` | The user closes the terminal. |

The names `/setup`, `/sessions`, `/models`, and `/branch` remain aliases. The `/settings` items also accept direct names: `/worldbook`, `/adopt`, `/profile`, `/plugins`, and `/config`.

### Settings

Each `/settings` row shows its current value. The rows are:

- **Model** opens the model list.
- **Model providers and keys** connects a provider or replaces its key.
- **Provider options** edits the limits and the sampling parameters of the selected provider.
- **Prompt and memory** edits the `[prompt]` table: the history limit, the memory interval, the limits, the budgets, the instructions, the roles, the author's note and its depth, and the prefill.
- **Import a SillyTavern preset** reads a preset file into the `[prompt]` table. The row shows the number of custom blocks.
- **Agent and tools** edits the `[plugins.agent]` table: the tool steps, the tools that wait for approval, the repeats of a cut-off reply, and the token budget.
- **Plugins** shows one row for each installed plugin. Select the row to turn the plugin on or off.
- **Memory of this chat**, **Lorebooks of this chat**, and **Apply the current settings to this chat** appear while a chat is open.
- **Show config.toml** shows the file with the keys hidden.

The prompt and agent forms save `config.toml`. Then they apply the settings to the open chat. The chat keeps its provider and model.

### Memory

The memory keeps the important parts of a long chat after the old messages leave the prompt.
The memory has two parts:

- The **summary** describes the story so far. Each chat has its own summary. A summary of another branch does not apply.
- The **facts** list lasting facts about the character, your persona, and their relationship. All chats of the same character and persona share the facts.

The model of the chat updates the memory after each `memory_interval` new messages. The default interval is 20. The value 0 turns the memory off.
An update reads the current memory and at most 48 KiB of new messages. A long chat catches up over several updates.
The prompt includes the summary and the facts before the history. They have priority over the history in the context budget.
The status line shows `The memory is updating.` during an update. Ctrl+C cancels the update.
`/memory` shows the coverage. It edits the summary and the facts, and it updates the memory at once.
A session export does not include the memory.

The composer sends ordinary text to the active conversation. A command popup opens when the composer starts with `/`.
Up and Down select a command in the popup. Tab completes the selected command. Enter runs it.
Enter submits text or a form. Alt+Enter inserts a newline.
Tab changes the active form field. Escape closes a dialog. Typing in a menu filters its items.
Ctrl+C cancels active generation.
Ctrl+D closes an idle terminal. An exit during generation requests cancellation and waits for durable completion.

The terminal scrolls and selects the chat history with its own controls. The terminal captures the mouse only while a dialog is open.
In a dialog, the mouse wheel scrolls menus and documents. A pointer over a menu item selects it. A click activates it.
A click on a form field moves the focus to it. The `[ Apply ]`, `[ Cancel ]`, and `[×]` targets accept clicks.

The history shows your messages on the right side. The history renders Markdown in all messages. Fenced code blocks with a known language get syntax colors.
Each message has a rounded bubble. Your message appears at once. The reply bubble shows animated dots until the first text arrives.
The reply then streams with a cursor at its end in the live area. A saved reply moves into the history. A failed reply shows its error in a red bubble.
The status line shows the character, the persona, the model, and the provider of the open chat. Without a chat, it shows the model for new chats.
A chat switch writes the chat name and its recent messages into the history. A retry or a branch change writes the messages after the changed point.

### Message actions

The scrollback belongs to the terminal, so a click on a message in it has no effect. Use the message list or the keys below.
A dim action bar under the last message shows the keys. The keys work while the composer is empty.

| Key | Action |
| --- | --- |
| ← and → | Show the previous or next alternative of the last message. The bar shows the position, for example `‹ 2/3 ›`. |
| → at the newest reply | Generate another reply. The earlier replies stay as alternatives. |
| ↑ | Edit the last message. The edit adds a new version. The original stays as an alternative. |
| Ctrl+R | Regenerate the last reply. After an edited user message, generate its reply. |
| Esc twice | Open the message list of the open branch, newest first. |

Select a message in the list with the keyboard or a click. The action menu of the message has these items:

- **Rewind to here** makes the message the end of the branch. Later messages stay in the tree.
- **Edit** adds a new version of the message.
- **Regenerate this reply** rewinds to the user message before a reply, then generates a new reply.
- **Alternatives** lists the versions of the message. Select one to show it.
- **Branch into a new chat** copies the branch up to the message into a new chat and opens it.
- **Hide from the prompt** keeps the message in the chat but removes it from the next requests. **Include in the prompt** reverses it.
- **Copy the text** sends the text to the clipboard with OSC 52. Some terminals block this sequence.

`/continue` sends the chat with an instruction to continue the last reply. The result is a new version of the reply with the new text at its end. The original reply stays as an alternative.
`/impersonate` sends the chat with an instruction to write the next message of your persona. The text goes into the composer. Edit it, then press Enter to send it. The chat stores no message until you send it.
`/chats` lists the chats with their names. The menu also renames or deletes the open chat. A deletion asks for confirmation and removes the chat with its history.

Steps before a reply have their own small gray bubbles: `✻` thinking, `↗` web calls, `⧉` MCP tools (`mcp__` names), and `⚙` other tools.
A step shows a spinner while it runs and `✓` after its result. Ctrl+O expands or collapses the steps of the active reply. The history keeps the steps collapsed.
The terminal builds steps from the `reasoning_delta`, `tool_call`, and `tool_result` runtime events. The event `text` holds the delta, the call name with its arguments, or the result.

A chat pins its provider settings and plugins. A changed setup stops the next reply. The terminal then offers `/adopt` and keeps the unsent message in the composer.

### First setup

The terminal opens the provider menu when no provider profile exists. The menu lists OpenRouter, Anthropic, Google Gemini, OpenAI, and a local or custom server.
The menu shows the connection status of each vendor. The key form for a vendor requires a pasted API key.
The form rejects a key whose prefix belongs to another vendor: `sk-or-` (OpenRouter), `sk-ant-` (Anthropic), `AIza` (Gemini), or `sk-` (OpenAI).
RustyTavern does not read API keys from environment variables.

The custom server form requires a server URL. The key is optional for that kind.
The form saves a profile named after the vendor. The terminal then loads the model catalog of that provider.
The catalog load tests the key. The OpenRouter catalog is public, so the terminal first sends the key to `https://openrouter.ai/api/v1/key`.
A rejected key shows `The provider rejected the API key.` instead of a model list. Use `/provider` to paste a new key.
The `/model` list loads all connected catalogs at the same time. Each row starts with its provider name. A failed catalog does not hide the other catalogs.
The list sorts models from the newest release to the oldest and shows each release date. A tab bar shows `★ Recommended`, `All`, and one tab for each provider.
Left and Right change the tab. A click on a tab also changes it.
A model selection while a chat is open offers `/adopt` for that chat.
The model list shows recommended roleplay models first, with context length and price. Typing filters the list.

### Characters

The `/character` menu adds a character from a file or from a web address.
The file browser starts in `~/Downloads`. It lists folders and PNG, JSON, and CHARX files. Type to filter the list.
The web address form accepts a `chub.ai` or `realm.risuai.net` address. The terminal selects the source from the address.
The form also accepts a dropped file path. The terminal removes quotes, shell escapes, and a leading `~/`.
The essential Chub and RisuRealm source plugins download the card.
A preview shows the card before the import. The store does not change until you press Enter.
The preview has four tabs: Overview, Greetings, Lorebook, and Prompts. The Overview tab shows the card picture.
The terminal uses the Kitty, iTerm2, or Sixel image protocol when available. Other terminals show colored half blocks.
A Chub card is JSON and has no picture.

### Ownership

The CLI owns rendering, input, and forms. The database keeps the selected chat, character, and persona. The library owns durable records and product operations.
Slash commands call typed operations without shell conversion. The database remains the history authority.
The history shows a bounded window of recent messages and marks truncated text. Exports retain full history.
The request form can write the complete request to a file. The terminal permits one active operation and retains runtime leases.
The terminal grants a source capability only after an explicit confirmation.
The setup form saves a pasted key as `api_key` in `config.toml`. A session records only the reference `config:NAME`.

## CLI reference

The binary name is `rustytavern`. The binary opens an interactive conversation when the caller supplies no subcommand.
The terminal requires an interactive input and output device. The remaining sections define the script interface.
The global `--home PATH` option selects the home directory. The default is `RUSTYTAVERN_HOME`, then `~/.rustytavern`.
The home directory holds two files: `config.toml` for every setting and `rustytavern.db` for cards, personas, sessions, and plugin state.
The application writes `config.toml` with owner-only permissions. A save keeps your comments and key order. An unknown setting is an error.

### Configuration file

```toml
default_provider = "openrouter"      # the provider for new chats

[providers.openrouter]
kind = "openrouter"
model = "openai/gpt-4.1-mini"
api_key = "sk-or-..."
context_limit = 131072                # the model limit
output_limit = 8192
parameters = { temperature = 0.8 }

[prompt]                              # optional: the prompt options for new chats
context_limit = 131072
output_limit = 8192
lore_budget = 32768
example_budget = 8192
history_limit = 0                     # 0 keeps every message that fits the context limit
memory_interval = 20                  # the new messages between memory updates; 0 turns the memory off
main_instruction = "You act as {{char}}."
post_history_instruction = "..."      # the text after the chat; other apps call it the jailbreak
post_history_role = "user"            # optional: system (default) or user
note = "..."                          # the author's note
note_depth = 4                        # optional: the chat messages after the note; no value puts it after the persona
note_role = "system"                  # optional: system, user, or assistant
prefill = "..."                       # optional: the start of each reply

[[prompt.blocks]]                     # optional: custom blocks in this order
name = "Style"
text = "{{char}} writes short replies."
role = "system"                       # system (default), user, or assistant
position = "depth"                    # before_character, after_character, before_history (default), after_history, or depth
depth = 2                             # the chat messages after the block; only the depth position uses it
enabled = true

[plugins]
enabled = ["lighthouse-notes"]       # installed plugins only

[plugins.agent]
max_steps = 32                       # the limit of tool rounds for one reply
confirm_tools = ["web_fetch"]        # the tools that wait for approval; "*" selects every tool
retry_truncated = 1                  # the repeats of a truncated reply; 0 disables them
max_tokens = 0                       # the token budget of one reply; 0 disables it

[plugins.skills]
paths = ["/Users/me/.rustytavern/skills"]

[plugins.mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_..." }

[plugins.lighthouse-notes]           # an installed plugin
path = "/path/to/lighthouse-notes"
grants = ["prompt.contribute:before_character", "prompt.contribute:after_character"]

[extensions.rose]
user_bubble = "#ff5f87"
assistant_bubble = "magenta"
```

| Key | Meaning |
| --- | --- |
| `default_provider` | The provider profile for new chats. |
| `providers.NAME` | One provider profile. `api_key` stores the key in this file. The limits are the model limits. |
| `prompt` | The prompt options for new chats. A chat uses the lower limit of `prompt` and its provider. `/adopt` applies the table to the open chat. `memory_interval` sets the memory updates. |
| `plugins.enabled` | The enabled installed plugin IDs. The essential plugins are always on. |
| `plugins.NAME` | The configuration of one plugin. A table with `path` is an installed plugin. |
| `extensions.NAME` | One enabled extension. |

The `config show` command prints the configuration and hides API keys and MCP environment values.

### Memory commands

```sh
rustytavern memory show SESSION_ID
rustytavern memory update SESSION_ID            # update now
rustytavern memory update SESSION_ID --if-due   # update after the memory interval only
rustytavern memory edit SESSION_ID --summary TEXT --facts $'Fact one.\nFact two.'
```

The `memory update` command uses the model and the provider settings of the session. The result holds `updated`, `memory`, and `pending`.
An empty `--summary` removes the summary. The `--facts` value has one fact on each line.

### Session settings

A session can change the `plugins` settings of `config.toml` for itself. The database stores these settings.
The key is a dotted `plugins` path. The value uses TOML syntax. An object value merges by key. Any other value replaces the old value.

```sh
rustytavern session set SESSION_ID plugins.enabled '["lighthouse-notes"]'
rustytavern session set SESSION_ID plugins.agent.max_steps 4
rustytavern session settings SESSION_ID
rustytavern session unset SESSION_ID plugins.enabled
```

The `session set` and `session unset` commands also adopt the new plugin composition for that session. The session keeps its provider settings.
The `session settings` command prints the session settings and the enabled plugins. Other sessions keep `config.toml`.
A session export does not include the session settings.

The CLI prints metadata as JSON. The `--json` flag makes this intent explicit on inspection commands.
The CLI prints structured errors to stderr. The chat command prints reply text unless the caller selects `--events`.
A message accepts `--message TEXT` or `--message-file PATH`. A message-file value of `-` reads stdin.

### Character archives

The `card import PATH` command accepts JSON, PNG, and CHARX cards. The `card export CHARACTER_ID --output PATH` command returns original bytes.
The `--normalized` option exports card JSON. The `--patch PATH` option applies a JSON merge patch and retains the input container format.
The `card asset CHARACTER_ID --uri URI --output PATH` command writes one embedded CHARX asset. The command requires an exact `embeded://` URI.
The command validates the asset before it creates the output file.

### Provider profiles

The `config provider` command writes one named profile. The `--api-key KEY` option stores the key in `config.toml`.
The profile stores a provider kind, model ID, endpoint, API key, context limit, and output limit. A custom provider requires an explicit endpoint: the API root or the full `/chat/completions` URL. Rig adds `/chat/completions` to an API root. An endpoint must not have a query string.
A custom provider without a key sends no `Authorization` header.
A custom provider can omit credentials for a local model. The native provider kinds are `openrouter`, `openai`, `anthropic`, `gemini`, and `custom`.
The adapters use Chat Completions, Responses, Messages, or native Gemini SSE as appropriate. The `config capabilities` command prints the supported adapter subset for each kind.
The `config provider --parameters JSON` option supplies validated settings: `temperature`, `top_p`, `stop`, `reasoning`, and `cache`. The `--routing JSON` option supplies explicit OpenRouter provider preferences.
The `reasoning` value is `off`, an effort (`minimal`, `low`, `medium`, `high`, `xhigh`, or `max`), or a token budget. The `cache` value is `none`, `short`, or `long`. Rig maps each setting to the protocol.
The [provider contracts](compatibility.md#provider-contracts) define these settings and their model restrictions. Provider profile updates affect a session only after `session configure` adopts them.

Rig checks each parameter against the protocol and its model catalog when the profile is saved. The adapter rejects a refused parameter with `unsupported_provider_parameter`. The adapter does not switch a model or vendor automatically.
The adapter retries a truncated reply only when `retry_truncated` is above zero. Each repeat doubles the output cap and replaces the partial text. The runtime records a failed generation when a stream closes without terminal success.

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

### Chat management

```sh
rustytavern session continue SESSION_ID [--events]
rustytavern session impersonate SESSION_ID [--events]
rustytavern session fork SESSION_ID --at MESSAGE_ID
rustytavern session rename SESSION_ID "Harbor night"
rustytavern session hide SESSION_ID --at MESSAGE_ID [--show]
rustytavern session delete SESSION_ID
```

The `continue` command requires a reply at the session head. The store adds the longer reply as a sibling of the original reply and selects it as the head.
The `impersonate` command prints the text of the next user message. The store adds no message. The generation record keeps the text as its draft.
Both commands store a generation with `mode` set to `continue` or `impersonate`. A reply generation omits the field.
The `fork` command copies the branch that ends at the message into a new session with the same configuration. The copies have no generation links.
The `rename` command sets the session `title`. An empty title removes it. A rename does not change the session version.
The `hide` command adds the message ID to the session `hidden` list. The prompt omits hidden messages. The `--show` flag removes the ID.
The `delete` command removes the session, its messages, and its generations. The store rejects a deletion or a hide while a generation owns the session.

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

A plugin changes agent behavior or adds content to the agent context.
An extension changes only the application appearance.

The standard agent includes the essential plugins: the agent loop, `web_fetch`, skills, MCP tools, and the card sources.
The provider runs its native web search: `openrouter:web_search` on OpenRouter, `web_search` on OpenAI and Anthropic, and Google Search on Gemini. A custom endpoint has no web search.
A web search can add a cost at the provider.
The essential plugins are always on. The agent loop is the only conversation mode.
The `plugins.enabled` list in `config.toml` selects the installed plugins.
The [plugins module](../crates/tavern/README.md) lists each plugin ID, grant, and file location.

| Setting | Purpose |
| --- | --- |
| `plugins.skills.paths` | Each directory holds `NAME/SKILL.md` skills. The default is `HOME/skills`. |
| `plugins.mcp.servers.NAME` | One MCP server with `command`, `args`, and `env`, or one `url`. |

The `chat --events` command prints a `tool_call` and a `tool_result` event for each tool call. The terminal shows each tool call as a step.
A tool in `confirm_tools` waits for approval. The runtime emits a `tool_approval` event with the call name, its arguments, and a `call_id`.
The terminal asks for the y key. Parallel calls wait together, and the terminal asks about each call in order. `GenerationHandle::approver` lists the calls that wait.
The `chat` command declines the call unless the global `--approve-tools` option is present. The model receives a declined call as a skipped result. The provider timeout does not count the time that a call waits for approval.
A call to an unknown tool, or a call with invalid arguments, returns its reason to the model as a skipped result. The model can then call the tool again or reply.
A repeated truncated reply emits a `draft_reset` event. Its `text` holds the kept draft. The `chat` command starts a new line with the kept draft.
The larger output cap applies only to the repeated turn and stays within the output limit of the model in the Rig catalog. A repeated turn does not count as a tool step.
The `usage` of a reply is the sum of its model calls. The token budget uses the total, or the input and output tokens when a provider reports no total.
The `max_tokens` budget stops the reply with `token_budget` before the next model call.

The `plugin install PATH` command checks a plugin directory with a `plugin.json` file, activates it, and records it in `config.toml`.
Each `--grant CAPABILITY` option approves one requested host capability. The `--trust-processes` option permits a plugin executable.
The `plugin enable ID` and `plugin disable ID` commands edit the `plugins.enabled` list.
The `plugin list` command lists essential and installed plugins. The `plugin inspect ID` command shows one plugin. Neither command starts external processes.
The `plugin uninstall ID` command removes the plugin table and keeps the plugin state.
The `plugin bind SERVICE PLUGIN_ID` command saves an explicit service binding. The `preset inspect tavern --resolved` command resolves the enabled composition.
The resolved inspection can start enabled trusted processes.

The `plugin run NAME --args JSON` command calls an enabled plugin command.
The command also accepts `--args-file PATH` or `--url URL`. The root alias form accepts `NAME --url URL` or `NAME --args JSON`.
The `card fetch URL --source NAME` command imports through a source plugin. Source imports retain provenance in the stored card record.
The `--preview` option prints the complete card and does not save it.
Ctrl+C cancels a source or plugin operation.
The [installed plugin guide](developer.md#installed-plugins) includes the manifest and implementation instructions. The [example index](../crates/tavern/examples/README.md) links executable examples.

Each `[extensions.NAME]` table in `config.toml` is one enabled extension. The `extension list` command prints them. The terminal reads them at start.
The [extension guide](developer.md#extensions) defines the theme fields.
The command help lists current commands and flags.

### Session and persona exports

The [native export interface](#native-records-and-exports) defines session and persona exports. The `session show SESSION_ID` command includes generation summaries and retained drafts.

## Session configuration revisions

A session uses saved configuration revisions. The host requires an explicit revision before generation or prompt inspection.
The host does not substitute current plugins when a saved composition differs.

### CLI interface

The `session new` command saves revision `1` with the initial session. The command saves the provider settings and resolved composition atomically.
Later `chat`, `session retry`, `session regenerate`, and `prompt inspect` commands use those saved provider settings. A change to `config.toml` does not change an existing session.

```sh
cargo run --locked -- --home ./home session configure SESSION_ID
cargo run --locked -- --home ./home session configure SESSION_ID --provider openrouter --model openai/gpt-4.1-nano
cargo run --locked -- --home ./home session configure SESSION_ID --context-limit 16384 --output-limit 512 --expected-version VERSION
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

The [session module](../crates/session/src/records.rs) defines `SessionConfiguration`. The [configuration schema](schemas/session-configuration-v1.schema.json) defines its serialized fields.

| Field | Content |
| --- | --- |
| `revision`, `identity` | The record stores its consecutive number and SHA-256 content hash. |
| `character_id`, `persona_id` | The record pins imported revision IDs. |
| `provider`, `model` | The record identifies the provider profile and selected model. |
| `provider_config` | The record retains settings and credential references. |
| `options`, `accept_reduced` | The record retains prompt settings and compatibility acceptance. |
| `profile` | The record identifies the selected base prompt profile. |
| `composition` | The record retains the exact service and product lock. |

The composition records plugin manifests, artifacts, configuration hashes, grants, bindings, and contributions. The product composition also records the product preset.
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

### Custom prompts

The `[prompt]` table holds the text that the user adds to every request. The request has this order:

1. The main instruction. The card `system_prompt` replaces it. `{{original}}` in the card text inserts the main instruction.
2. The `before_character` blocks, the lore, the character fields, the persona, and the author's note without a depth.
3. The `after_character` blocks, the examples, and the `before_history` blocks.
4. The chat and the new message. A `depth` block or a note with `note_depth` goes before the last N chat messages. Depth 0 puts it after the new message.
5. The `after_history` blocks.
6. The post-history instruction. The card `post_history_instructions` replaces it. `{{original}}` in the card text inserts the post-history instruction.
7. The prefill, as an assistant message.

Each block, the note, and the prefill expand `{{char}}` and `{{user}}`.

The post-history instruction is the jailbreak slot of SillyTavern, RisuAI, and Tavo. The app ships no jailbreak text. Paste your own text there.
Gemini and some Claude models move every system message to the start of the request. Set `post_history_role = "user"` to keep the text after the chat.

The prefill makes the model continue your text. A model that refuses a prefill, such as Claude 4.6 and later, gets the request without it. The reply does not show the prefill.

`rustytavern config preset FILE` reads a SillyTavern chat completion preset. Tavo and community presets use the same format.
The command replaces the main instruction, the post-history instruction and its role, the prefill, and the blocks.
The `main` prompt sets the main instruction. The `jailbreak` prompt sets the post-history instruction.
Each other prompt with text becomes a block. The markers before it set its position. An absolute prompt becomes a `depth` block.
The command reads the global prompt order. It keeps disabled prompts as disabled blocks.

## Native records and exports

The library owns native serialization and validation. The CLI reads arguments and writes the resulting bytes.

| Record | Version | Authority |
| --- | --- | --- |
| SQLite database | `5` | The database stores the marker in `PRAGMA user_version`. |
| Application configuration | None | `config.toml` has no version field. An unknown setting is an error. |
| Runtime input snapshot | `2` | Each generation stores an immutable snapshot. |
| Enrichment progress | `1` | Each generation stores its saved hook results. |
| Native session export | `4` | The export contains revision archives and conversation records. |
| Native persona export | `1` | The export contains one content-addressed persona. |
| Session configuration | `1` | The session composition retains immutable configuration revisions. |

### Session export

```sh
cargo run --locked -- --home ./home session export SESSION_ID --output session.json
cargo run --locked -- --home ./home session export SESSION_ID --output session-traces.json --include-traces
```

The default export sets `includes_prompt_traces` to `false`. The exporter sets each generation's `request` to `null` in that mode.
The `--include-traces` flag retains the compiled plan, prepared request, and phase trace. Both modes retain conversation text, input snapshots, saved enrichment, and plugin state.
The flag controls compiled request traces only.

The [version 3 session schema](schemas/session-v3.schema.json) documents the historical version 3 format. The [version 4 session schema](schemas/session-v4.schema.json) adds the worldbook archives.
The version 4 reader and writer use the Rust types and runtime validation. The [session module](../crates/session/src/records.rs) defines the corresponding Rust types.

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
cargo run --locked -- --home ./home persona export PERSONA_ID --output persona.json
```

The [persona schema](schemas/persona-v1.schema.json) defines the envelope. The `PersonaExport` type contains `schema_version: 1` and `persona`.
The persona record contains its ID, name, and description. The decoder verifies the ID against the name and description.

### Explicit partial-reply acceptance

```sh
cargo run --locked -- --home ./home session show SESSION_ID --json
cargo run --locked -- --home ./home session accept-draft SESSION_ID GENERATION_ID
```

The show command lists generation IDs, statuses, drafts, usage, and acceptance links. The acceptance command requires a nonempty failed, cancelled, or interrupted reply draft.
The command rejects a continuation or an impersonation draft.
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
Sessions and snapshots store credential references only. The provider resolves credential values during dispatch.
A `config:NAME` reference reads the `api_key` of that provider profile in `config.toml`. The application has no other credential source.
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

Each discovery filter is optional. Rig lists the models. The Rig model catalog supplies the modalities, parameters, prices, and limits. A custom endpoint gets the facts of a model that the catalog knows by ID. The `/model` command loads the catalogs of all connected providers and opens one model chooser.
Selection updates the provider profile. An existing session still requires explicit configuration adoption.
The [catalog contract](compatibility.md#model-catalog) distinguishes advertised capabilities from verified behavior. The integrated CLI and terminal gate remains pending.
