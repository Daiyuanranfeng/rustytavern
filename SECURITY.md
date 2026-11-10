# Security policy

## Supported versions

RustyTavern is alpha software. Only the latest release and the `main` branch get security fixes.

## Report a vulnerability

Do not open a public issue for a security problem.

Use [GitHub private vulnerability reporting](https://github.com/SebastianElvis/rustytavern/security/advisories/new).
If you cannot use it, send an email to me@runchao.rocks.

Include these items in the report:

- The affected version or commit.
- The steps to reproduce the problem.
- The effect of the problem.

The maintainer replies within 7 days.
The maintainer tells you when a fix is available. The advisory credits you unless you ask for no credit.

## Security model

- RustyTavern runs on your computer. It has no network server.
- The `config.toml` file holds API keys in plain text. The application writes the file with owner-only permissions. Protect your home directory.
- A third-party plugin or an MCP server runs code on your computer. Install only plugins and servers that you trust.
- A character card, a worldbook, or a web page can contain prompt-injection text. The terminal grants a source capability only after an explicit confirmation.

Problems in these areas are in scope:

- A leak of an API key to a log, an export, or a wrong provider.
- A plugin or a card that gets a capability without a grant.
- A crafted card, archive, or session file that causes code execution, a path traversal, or a crash.
