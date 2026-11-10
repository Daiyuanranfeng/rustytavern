# Changelog

This file records each user-visible change.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
The project uses [Semantic Versioning](https://semver.org/). Before version 1.0.0, a minor version can break compatibility.

## Unreleased

### Added

- The Apache-2.0 license.
- Contributor, agent, and security documents.
- Continuous integration, a dependency policy, and release builds.
- A version 4 session export schema that describes worldbook archives.

### Security

- Upgrade `rusqlite` to 0.41 for the SQLite fix in RUSTSEC-2026-0459.
- Upgrade `zip` to 9 for RUSTSEC-2026-0367, RUSTSEC-2026-0368, RUSTSEC-2026-0369, and RUSTSEC-2026-0429.
- Upgrade `rmcp` to 3.9 for RUSTSEC-2026-0462.

## 0.1.0

### Added

- Character cards in PNG, JSON, and CHARX formats, with Chub and RisuRealm sources.
- Worldbooks, personas, custom prompt blocks, prefill, and preset import.
- Providers for OpenRouter, Anthropic, Google Gemini, OpenAI, and custom servers, on Rig.
- An agent loop with native tool calls, web search, web fetch, MCP tools, and skills.
- Sessions with regenerate, edit, branches, and native exports.
- Chat memory with summaries and facts.
- An interactive terminal app and a script interface.
- Native runtime plugins, installed third-party plugins, and external card-source processes.
