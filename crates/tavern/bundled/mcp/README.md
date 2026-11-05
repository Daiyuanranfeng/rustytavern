# MCP extension

The MCP extension adds Model Context Protocol tools for the [agent extension](../agent/README.md).
The extension owns one [tool plugin](plugins/README.md).
The package ID is `org.rustytavern.extension.mcp`.

```sh
cargo run --locked -- --data-dir ./data extension install --builtin mcp
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.mcp --grant mcp.connect
```
