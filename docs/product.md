# Product vision and roadmap

RustyTavern gives a local character the ability to help through action, media, memory, and realtime conversation.
The character keeps its identity across sessions through cards, personas, worldbooks, and durable history.
The Rust library supplies the harness. The interactive CLI and script interface expose the same product behavior.
The project excludes a web frontend and a web backend.

## Required target

The full target includes all four horizons below. The MVP defines the first useful product, not the full goal.
Each horizon extends the same persistent character. A later horizon must preserve the earlier user workflows.
The roadmap sets product outcomes without dates. The current implementation does not satisfy the MVP exit criteria.

## Horizon 1: A character that can act

**Principle:** A useful character must complete bounded tasks and show its actions.
**Product form:** The MVP is a local character agent through a Rust library, interactive CLI, and script interface.

| Story ID | User story |
| --- | --- |
| H1-01 | I import a character card, select my persona, and bind worldbooks so the agent uses the intended identity and context. |
| H1-02 | I discover models and inspect their advertised capabilities so I can select a suitable provider and model. |
| H1-03 | I ask the character to complete a task with tools so I receive a completed action with an inspectable result. |
| H1-04 | I connect an MCP server and grant specific operations so the character can use my selected external tools. |
| H1-05 | I ask the character to search the web and fetch a page so its answer uses inspectable source material. |
| H1-06 | I approve or deny a requested action and stop active work so I keep control of external effects and cost. |
| H1-07 | I resume a session, choose a branch, and export its records so I can continue work without losing identity or history. |
| H1-08 | I install an extension or embed the library so I can add a tool without changing the core host. |

**Observable exit criteria:**

- A user imports identity records, discovers a model, and completes a tool-assisted task through the terminal.
- A library caller completes the same task through typed operations.
- A task uses an MCP tool and web search or fetch with explicit grants and inspectable results.
- The host bounds steps, requests, elapsed time, and result sizes. The host stops a denied, cancelled, failed, or exhausted task with durable records.
- A restart retains identity, worldbook selections, branches, action records, and credential-free exports.

## Horizon 2: A character that can see and speak

**Principle:** The user can choose the most useful form of input and output for a task.
**Product form:** The character handles media files and recorded speech through the same local library and CLI.

| Story ID | User story |
| --- | --- |
| H2-01 | I submit an image with my message so the character can answer questions about its contents. |
| H2-02 | I submit a voice recording so speech recognition can turn my speech into a visible message. |
| H2-03 | I ask the character to speak its reply so I can listen to the result. |
| H2-04 | I ask the character to generate an image so I receive a saved artifact that I can inspect and export. |
| H2-05 | I select models by input and output capability so an unsupported media operation fails before a paid request. |
| H2-06 | I review saved media and its source so I can understand what the character received and produced. |

**Observable exit criteria:**

- A terminal user and a library caller each complete image-input, speech-recognition, speech-synthesis, and image-generation workflows.
- Each workflow retains typed artifacts, provenance, and conversation links across restart and export.
- The host enforces format and size limits, grants, cancellation, and credential exclusion for each media operation.
- The product distinguishes provider advertisements from verified operation support and reports unsupported combinations explicitly.

## Horizon 3: A character with useful memory

**Principle:** The character recalls relevant information under user control and applies reusable skills.
**Product form:** The local agent adds inspectable retrieval and skill workflows to persistent identity and history.

| Story ID | User story |
| --- | --- |
| H3-01 | I select local material for memory so the character can retrieve facts beyond the active context window. |
| H3-02 | I use embeddings and reranking so the character selects relevant passages for my current task. |
| H3-03 | I inspect a retrieved passage and its source so I can check the basis of an answer. |
| H3-04 | I add, correct, or remove a memory source so I control the information that future tasks can retrieve. |
| H3-05 | I install and select a skill so the character can apply a reusable procedure through granted tools. |
| H3-06 | I keep separate character memories and skill selections so one character does not silently use another character's material. |

**Observable exit criteria:**

- A user imports memory material and completes a task that retrieves relevant passages through embeddings and reranking.
- The task records source references and distinguishes retrieved context from durable conversation history.
- A user changes or removes material and observes that change in subsequent retrieval.
- A skill-assisted task runs through the same bounded host operations and permission checks as the MVP.
- Restart and export preserve selected memory and skill references without granting new access automatically.

## Horizon 4: A character present in realtime

**Principle:** The character responds to a live conversation while the user keeps control of turn-taking.
**Product form:** The local agent supports duplex realtime audio with interruption and durable conversation records.

| Story ID | User story |
| --- | --- |
| H4-01 | I speak and hear the character through a realtime audio session so I can converse without recorded-file steps. |
| H4-02 | I interrupt a spoken reply so the character stops playback and listens to my next turn. |
| H4-03 | I inspect the transcript and action status so I can understand the durable result of a live exchange. |
| H4-04 | I use a permitted tool during a live conversation so the character can act without losing turn control. |
| H4-05 | I stop or disconnect a session so the host releases audio and provider resources and retains completed work. |

**Observable exit criteria:**

- A user completes a duplex audio exchange through the local interface and a library integration.
- An interruption stops the active response and prevents late output from changing a closed turn.
- A live tool task retains permission checks, execution bounds, and visible status.
- Cancellation, timeout, and disconnect tests leave coherent transcripts and release device and provider resources.

## Implementation evidence

The [compatibility guide](compatibility.md#support-boundary) records current support, validation evidence, and remaining work.
The roadmap defines required outcomes. The implementation evidence does not establish MVP completion.

## Ownership and acceptance

The core host owns persistence, permissions, cancellation, resource limits, and bounded execution loops.
Plugins implement behavior and adapters. Extensions package plugins, commands, defaults, and assets.
The [architecture](architecture.md#target-runtime) defines those boundaries. The [developer guide](developer.md) defines current technical contracts.
The [compatibility guide](compatibility.md) records tested behavior and open limits.
No horizon claims complete SillyTavern or RisuAI parity. Each exit criterion requires observable library and terminal evidence.
