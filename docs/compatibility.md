# Compatibility and formats

## Support boundary


| Feature | Implemented support | Remaining work |
| --- | --- | --- |
| Cards and personas | The product imports CCv2 and CCv3 JSON, PNG, and bounded CHARX cards. The product stores personas. | The product needs remaining container compatibility checks. |
| Worldbooks | The product stores portable books, lorebook envelopes, and native World Info archives. The core profile executes portable books. | The core profile does not execute native World Info rules. |
| Conversations | The product stores branches, alternatives, drafts, configuration revisions, and native exports. | The product needs broader reference workflow coverage. |
| Providers | Five adapters stream text. OpenRouter has local and authorized live evidence. | Other direct vendor endpoints need live validation. |
| Model discovery | The library fetches and filters advertised model capabilities. CLI and terminal selectors exist. | Integrated checks remain pending. Advertisements do not prove model behavior. |
| Tools, MCP, and skills | The essential [agent driver](../crates/tavern/README.md#agent-and-tools) runs a bounded tool loop on OpenRouter and custom adapters. Separate tool plugins connect stdio and HTTP MCP servers through `rmcp` and add Agent Skills. | Durable tool records, saved continuations for each tool step, and per-character skill selection remain pending. |
| Web search and fetch | The provider runs its native web search tool. The essential `web_fetch` tool reads public HTTPS pages through the scoped host fetcher. | A custom endpoint has no web search. |
| Voice and image input | The provider contract accepts text only. | Typed media inputs, storage, permissions, and adapters remain pending. |
| Speech and realtime audio | The runtime has no audio operation. | Recognition, synthesis, and realtime sessions remain pending. |
| Image generation | The runtime has no image generation operation. | Typed generation operations and artifact storage remain pending. |
| Embeddings and reranking | The runtime has no dedicated operations. | Typed operations and adapters remain pending. |
| Installed plugins | The product installs local plugin directories and restores granted plugins. External processes supply card sources. | External processes do not supply arbitrary runtime service families. |
| Extensions | An extension sets the terminal bubble colors. | Other appearance settings remain pending. |

The product reports an adapter contract separately from a remote model capability. The product must not infer model support from a provider name.
A trait definition does not establish complete feature support. The following sections record format, provider, and validation limits.

## Compatibility and validation

The current implementation follows the selected `core-v1` design. The implementation does not claim complete SillyTavern or RisuAI behavior.
The investigation uses pinned source revisions and isolated upstream module probes. Complete reference coverage remains open.
One SillyTavern browser workflow has two fresh runs and two server restarts. The candidate CLI repeats the same authored conversation twice.
The stored transcript matches the selected shared assertions. That result checks transport and saved state only.
The complete native prompts remain different. Prompt parity therefore fails for these native-default captures.
The RisuAI local server also completes the authored conversation after terms approval. Two authored-preset runs from each reference supply eight complete matching request bodies.
The current CLI repeats those cases against frozen source captures. Twenty altered requests fail the complete request comparison.

### Current evidence

The private vector adapter executes 56 selected assertions without expected outputs. The comparator checks declared JSON types and complete values.
Three source-policy cases substitute DNS and HTTP operations in the exact source module. Those cases do not verify live DNS, TLS, or website availability.
The external timeout case uses an actual process deadline.

The Rust tests pass on macOS with Rust 1.99.1. The suite covers archives, prompt compilation, provider protocols, durable sessions, fencing, cancellation, plugins, extensions, tools, configuration adoption, and exports.
Request tests compare inspection, persistence, and dispatch. The suite uses local model endpoints and makes no paid model request.
Two terminal workflows and separate authorized live OpenRouter checks pass. The live checks cover character and persona names, restart, inspection, and credential-free storage.
Other direct vendor endpoints have no live credential test yet. Private records retain the candidate hash, commands, model, and usage.

### Declared profile rules

The importer accepts CCv2 and CCv3 JSON, PNG, and bounded CHARX. The importer selects exact `ccv3` before exact `chara` and rejects malformed selected envelopes.
The archive importer rejects legacy card envelopes. The typed JSON reader supplies a legacy V1 model.
Unknown JSON survives import. The runtime never executes imported scripts.
Reduced-profile acceptance preserves unsupported fields without execution or capability grants.

The one-pass macro evaluator supports `{{char}}`, `{{user}}`, and scoped `{{original}}`. The transcript remains literal.
Examples require character or user speaker prefixes; other text returns `invalid_examples`. Lore supports constants, literal primary keys, any-match secondary selection, deterministic order, and before/after-character placement.
Recursive, timed, probabilistic, script, and regex rules remain unsupported. A provider can reject a locally estimated context.

### Open acceptance work

Selected assertions do not close the full requirement audit. Isolated Linux checks import cards from Chub and RisuRealm twice through real HTTPS.
Those checks use a public DNS snapshot and the unchanged source policy. The local host DNS returns restricted addresses that the source policy rejects.
Unix process tests cover cancellation, shutdown, child cleanup, queues, frames, callbacks, and blocked pipes. Windows process cleanup still needs validation.
Database tests cover upgrades from versions 1 through 4 to version 5, rollback, and future-marker rejection. Worldbook tests cover bindings, immutable snapshots, replay, restart, limits, and version 4 archives.
The version 4 JSON schema validates version 4 worldbook archives. The historical version 3 schema does not.

The private corpus contains 31 frozen public card artifacts from six repository publishers. The corpus includes 30 syntactically valid JSON documents from GitHub hosting.
The sample does not estimate community usage. The project must capture complete reference workflows and audit each required feature.
The current live test proves its recorded candidate workflow only.

### Source identity and license

The investigation pins SillyTavern at `06bde939fb1e9c4c8d8641d810f0a916b5bce127`. The investigation pins RisuAI at `cb7f122d44089421b903197ef3f3e04e6706d5b6`.
DeepSeek Harness supplies an architecture reference at `8bd073392eef6f54e921a093b95076248eb08cbb`. Research copies remain outside the tracked implementation.
RustyTavern uses the Apache License, Version 2.0. The tracked repository contains no copied upstream source files. The project makes no clean-room claim.
A change that copies or adapts upstream code must first settle the attribution and license requirements of that code.

## Character cards and worldbooks

The card crate defines typed character and worldbook records. The archive objects retain their original bytes and JSON.
The typed records expose fields for inspection and editing.

### Character records

`CharacterCard` contains the version, specification version, and character data. `CharacterData` contains the standard character fields.
The data includes instructions, greetings, creator metadata, assets, and an embedded `CharacterBook`. The CCv3 fields include nickname, multilingual notes, sources, group greetings, and timestamps.
`CharacterAsset` retains its type, URI, name, and extension. The record retains extra asset fields.

`CardDocument::parse_json` accepts versioned CCv2 and CCv3 JSON. The reader also accepts legacy V1 JSON.
The reader distinguishes a worldbook from a legacy character card. `Card::definition` obtains typed data from an imported card archive.
`CharacterCard::new` constructs a versioned document. The constructor supplies the required group greeting array for CCv3.

The format model follows the [CCv2 specification](https://github.com/malfoyslastname/character-card-spec-v2/blob/main/spec_v2.md) and [CCv3 specification](https://github.com/kwaroran/character-card-spec-v3/blob/main/SPEC_V3.md).

### Worldbook records

`CharacterBook` represents an embedded or portable book. `BookEntry` represents its keys, content, insertion settings, identity, and extension data.
`WorldInfoFile` represents a standalone SillyTavern World Info file. The file maps entry identifiers to `WorldInfoEntry` records.
`WorldBookDocument` also accepts the `lorebook_v3` envelope.

The native entry model covers every field in the pinned SillyTavern entry definition. The fields include secondary-key logic, recursion, probabilities, insertion position, groups, timed effects, filters, outlets, and automation identifiers.
The model retains additional native fields. The [SillyTavern guide](https://docs.sillytavern.app/usage/core-concepts/worldinfo/) describes these settings.

`WorldBook::import` checks the JSON shape and resource bounds. Each book permits at most 4096 entries.
`WorldBook::definition` obtains the typed document. `WorldBook::export_original` returns the exact imported bytes.
`WorldBook::export_edited` serializes a checked typed document.

### Field preservation

Each extensible record retains unrecognized fields in an `extra` map. Each extension object retains arbitrary JSON values.
`OptionalField` distinguishes `Missing`, `Null`, and `Value`. This distinction preserves nullable World Info settings during typed edits.
The typed serializer can supply standard defaults for omitted required fields. The archive retains the original input for exact export.

The format models retain asset references as data. The runtime profile decides which stored settings it executes.
The current `core-v1` profile retains its existing execution rules.

### CHARX archives

`Card::import` also accepts CHARX archives. The archive contains a root `card.json` entry and optional assets.
The importer accepts stored and Deflate compression, stream data descriptors, and bounded ZIP64 records. The importer retains all archive bytes for exact original export.
An edited export replaces `card.json` and retains the other entry payloads. The export retains unknown card fields and application files.

`Card::embedded_asset` reads one asset through its exact `embeded://` URI. The lookup uses case-sensitive paths.
The lookup makes no network request. The CLI provides the same operation.

```sh
rustytavern --home ./home card import character.charx
rustytavern --home ./home card asset CHARACTER_ID \
  --uri embeded://assets/icon/images/portrait.png --output portrait.png
```

The importer checks every entry before it stores a card revision. The importer rejects duplicate paths, unsafe paths, special files, encryption, unsupported compression, and invalid CRC values.
The importer never extracts an archive to the filesystem. The importer never executes `module.risum` or other archive code.

| Resource | Limit |
| --- | --- |
| Compressed archive | 32 MiB |
| Entry count | 4096 |
| Expanded archive | 64 MiB |
| Expanded asset | 32 MiB |
| Card JSON | 8 MiB |
| Path | 1024 UTF-8 bytes |

The [archive tests](../crates/core/tests/charx.rs) cover preservation, edits, paths, corruption, and resource bounds.

### Remaining format work

The archive importer accepts CCv2 and CCv3 JSON, PNG, and CHARX. The typed JSON reader supplies the legacy V1 model.
JPEG-prefixed archives and legacy PNG asset chunks still need implementation. The standalone worldbook API provides archive storage and session bindings.
The core-v1 profile executes portable books. Native World Info execution remains open.
The project keeps these gaps open before it claims comprehensive format support.

## Provider contracts

RustyTavern supplies five adapters. Each prepared request records its `contract_id`.
The contract identifies the implemented adapter subset. The contract does not identify every capability of the remote service.

```sh
cargo run --locked -- --home ./home config capabilities
```

The command reports roles, text input and output, streaming, parameters, system placement, and token-count mode. The contracts expose no multimodal input or reasoning controls.
Each contract carries text, thinking text, and [tool calls](#provider-tool-transport). The counter uses conservative UTF-8 byte estimates.
The counter does not report an exact vendor tokenizer result. The provider validates restrictions for the selected model.
An explicit model ID does not require catalog discovery.

| CLI kind | Contract ID | Wire API |
| --- | --- | --- |
| `openrouter` | `openrouter.chat_completions.agent-v1` | The adapter uses Chat Completions SSE. |
| `openai` | `openai.responses.agent-v1` | The adapter uses Responses SSE. |
| `anthropic` | `anthropic.messages.agent-v1` | The adapter uses Messages SSE. |
| `gemini` | `gemini.generate_content.agent-v1` | The adapter uses native `streamGenerateContent` SSE. |
| `custom` | `custom.chat_completions.agent-v1` | The adapter uses a declared Chat Completions subset. |

The serialized protocol names are `openrouter` and `openai`.
The request stores a credential reference, `config:NAME`. The adapter resolves its value from `config.toml` only during dispatch. A custom provider can omit the key.
The endpoint must use HTTP or HTTPS without URL credentials, authentication query parameters, or fragments. The adapter disables HTTP redirects.

### Parameters

The `--parameters` argument accepts a JSON object. The adapters accept numeric `temperature` and `top_p` values.
The adapter validates `top_p` within `0` to `1`. The adapter validates `temperature` within `0` to `2`.
The Anthropic adapter uses the narrower `0` to `1` temperature range. The selected model can impose further restrictions.
The host does not remove a rejected parameter automatically.

The Responses adapter rejects `stop` before dispatch. The other adapters accept a nonempty stop string or a nonempty array of nonempty strings.
The Chat Completions subset accepts up to four strings. The Gemini adapter accepts up to five strings.
The Anthropic adapter accepts up to 8191 strings. The Anthropic and Gemini adapters convert a scalar stop string into an array.
The adapter rejects unsupported parameter names and invalid value types before dispatch.

```sh
cargo run --locked -- --home ./home config provider \
  --name openrouter --kind openrouter --model openai/gpt-4.1-nano \
  --parameters '{"temperature":0.7,"top_p":0.9}'
```

The OpenAI parameter contract follows the [Responses reference](https://developers.openai.com/api/reference/resources/responses/methods/create). The Anthropic contract follows the [Messages reference](https://platform.claude.com/docs/en/api/messages/create).
The Gemini mapping follows the [generation reference](https://ai.google.dev/api/generate-content). Some current models restrict sampling parameters further.
The provider response remains authoritative for those model restrictions.

### System message format

The `--merge-system-messages` option selects an optional Chat Completions transformation. The provider configuration stores that choice as `merge_system_messages`.
The option defaults to `false`. The adapter joins adjacent system sections with two line breaks when the option is enabled.
The adapter preserves text, whitespace, roles, and non-system boundaries. The logical prompt plan retains its original sections.
The prepared request records the transformation. The final request budget uses the transformed body.
The OpenRouter and custom adapters support this option. The other protocols reject the option before dispatch.
Older configuration records retain their identity when the option is absent.

### OpenRouter routing

The default request includes `allow_fallbacks: true` and `require_parameters: true`. The profile can supply an explicit provider order.
OpenRouter then tries another endpoint of the same model when one endpoint fails, is rate limited, or needs another key. Each endpoint must support every requested parameter.
A profile can pin one endpoint with `allow_fallbacks: false`. The runtime does not retry its own request or change its model automatically.
The [OpenRouter routing reference](https://openrouter.ai/docs/guides/routing/provider-selection) defines the remote routing controls.

```sh
cargo run --locked -- --home ./home config provider \
  --name routed --kind openrouter --model openai/gpt-4.1-nano \
  --routing '{"order":["openai"],"allow_fallbacks":false,"require_parameters":true}'
```

The other adapters reject an OpenRouter routing object. The prepared request records the complete routing object.
An empty provider order lets OpenRouter select the initial provider. The disabled fallback setting still applies to that request.

### System text and completion

The Chat Completions and Responses adapters retain ordered system messages. The Anthropic adapter consolidates system sections in its top-level `system` field.
The Gemini adapter consolidates system sections in `systemInstruction`. The Gemini adapter maps assistant roles to `model`.
The prepared request records these logical-to-wire transformations.

The stream must contain a terminal success signal before the runtime commits an assistant node. The runtime keeps partial output when a provider reports a limit or a failure.
The runtime saves the usage of each completed model call before it emits a usage event. A failed or cancelled model call reports no usage.
The adapter never treats premature connection closure as success.
The [Rig](https://crates.io/crates/rig-core) library parses each wire protocol. Rig rejects malformed content values and malformed tool items as `invalid_provider_stream`.
The runtime streams recognized reasoning content as `AgentEvent::Reasoning`. The reasoning content never enters the character reply. Rig reports usage in one shape for every protocol.
Rig maps an Anthropic `model_context_window_exceeded` stop to an output limit, so the runtime reports `output_limit`.

### Error classification

The adapter reads at most 64 KiB from an HTTP 400 error body. The adapter uses recognized error fields to classify that response.
The adapter recognizes `context_length_exceeded` as `context_exceeded`. The adapter also recognizes the Anthropic `invalid_request_error` prefix `prompt is too long`.
The Gemini signature requires `INVALID_ARGUMENT` and an input-token overflow message. An unknown or oversized error body retains `provider_http`.
HTTP 401 and 403 retain `provider_auth` without an error-body read. HTTP 402 retains `provider_credit`.
HTTP 429 retains `provider_rate_limit`. HTTP 413 retains `context_exceeded`.
HTTP 502, 503, and 504 retain `provider_unavailable`.
Some OpenRouter endpoints fail with an invalid upstream response when a request carries `openrouter:web_search` with function tools and system messages.
When the first model call fails with `provider_unavailable` before any output, the runtime repeats it once without the web search tool.

The stream classifier recognizes context, authentication, and rate-limit error markers. The library does not keep the error code of a Responses `response.failed` event.
The adapter reports that event as `invalid_provider_stream`. The runtime retains earlier text and usage after these failures.
The adapter excludes raw error-body text from the public error. The public error retains the HTTP status or the normalized stream category.

Cancellation closes an unfinished error-body read. The configured `timeout_seconds` value also bounds the host driver phase.
The CLI can therefore report `driver_timeout` before the HTTP timeout expires. A direct adapter caller receives `provider_transport` for an HTTP read timeout.
The [Anthropic context reference](https://platform.claude.com/docs/en/build-with-claude/context-windows) defines the native context signals.

### Local model example

```sh
cargo run --locked -- --home ./home config provider \
  --name local --kind custom --model local-model \
  --endpoint http://127.0.0.1:8080/v1/chat/completions
```

The custom adapter requires the API root or the full `/chat/completions` URL. Rig adds `/chat/completions` to an API root. The local server must implement the documented Chat Completions SSE subset.
The server must send a supported finish reason and the final `[DONE]` frame. The caller can use `--api-key-env NAME` when the server requires authentication.

## Complete request comparison

Request equality determines prompt parity. A fixed model reply does not establish prompt parity.
The fixed reply only checks transport, response handling, and saved conversation state.

### Reference comparison

Each case must identify one reference application and one source revision. Each case must freeze the card, persona, conversation, preset, provider, model, and generation options.
The case must record the effective plugin configuration. The capture server must record the complete request body at the model API boundary.
The comparator must compare the reference capture with the candidate dispatch. The comparator must also compare candidate inspection with candidate dispatch.

The comparator must retain every message role and every message position. The comparator must retain whitespace and Unicode text exactly.
The comparator must check system instructions, character fields, persona fields, lore, examples, and selected history. The comparator must check model identity, output limits, sampling options, stop values, and protocol options.
The comparator must distinguish a missing field from an explicit `null` value. The comparator can ignore JSON object key order.
The comparator must not trim, merge, reorder, or rewrite messages to obtain a match. The report must show each difference with its JSON path and both values.

The report must describe transport differences separately. An isolated capture endpoint can differ from the real model endpoint.
The report must identify that substitution. The report must exclude authentication headers and credential values.

### Current scope

The current `core-v1` profile uses RustyTavern rules. The product has no SillyTavern or RisuAI prompt compatibility profile.
Native-default captures differ and fail prompt parity. A target profile must implement selected reference rules through normal compilation.
A fixture request must not replace compilation.

A shared authored preset matches eight complete request pairs across both references. Each reference supplies two original runs.
The candidate uses fresh data directories and checks inspection, persistence, and dispatch. Twenty altered requests fail comparison.
The preset selects a custom persona block, removes native markers, enables usage streaming, and omits zero-valued presence and frequency fields through native settings. The RisuAI case selects `merge_system_messages` and verifies two-line-break separators exactly.
The comparator excludes no body fields. The case covers one card, one persona, and two turns.
Lore, examples, overflow, overrides, and non-ASCII text remain outside that case.


## Model catalog

The [catalog module](../crates/runtime/src/provider/catalog.rs) supplies `catalog::fetch` and `CatalogFilter`. The function accepts provider configuration, an optional catalog endpoint, a credential resolver, and cancellation.
Rig lists the models of each protocol. The Rig model catalog adds input and output modalities, parameters, prices in USD for one million tokens, and limits that the listing omits.
The catalog matches the vendor of the protocol first. Then it matches the model ID in any vendor, so a custom endpoint that serves a known model also gets its facts.
The `raw` field holds the Rig listing entry. Rig lists the OpenRouter models that write text, because only those models can produce a chat reply.
Filters match input modality, output modality, and supported parameter. A missing fact does not satisfy a requested filter.
The evidence value is `advertised_unverified`. The facts do not prove successful model operations.
The transport disables redirects. The function enforces cancellation and the provider timeout. Rig checks an OpenRouter key before it lists the public catalog.
Custom providers require an explicit catalog endpoint. Four local HTTP tests cover listing, authentication, catalog facts, errors, and cancellation.

## Provider tool transport

The [provider module](../crates/runtime/src/provider/mod.rs) has two parts:
- [`request.rs`](../crates/runtime/src/provider/request.rs) builds one protocol-neutral `ChatBody` from the prompt plan. `PreparedRequest::body` stores that body.
- [`client.rs`](../crates/runtime/src/provider/client.rs) builds the Rig model and the Rig agent run, and maps the Rig errors.

`ProviderAdapter::prepare_agent` adds the tool definitions to the saved request. `ProviderAdapter::model` returns the Rig model.
The Rig agent loop validates the tool calls, runs the tools, and sends the results back in the native format of each protocol.
The [typed records](../crates/session/src/provider.rs) define `ToolDefinition`.

The ignored `live_providers` test sends text and tool conversations to OpenRouter models:

```sh
OPENROUTER_API_KEY=... cargo test -p rustytavern --test live_providers -- --ignored --nocapture
```
