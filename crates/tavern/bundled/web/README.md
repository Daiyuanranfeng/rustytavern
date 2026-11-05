# Web extension

The web extension adds the `web_fetch` tool for the [agent extension](../agent/README.md).
The extension owns one [tool plugin](plugins/README.md).
The package ID is `org.rustytavern.extension.web`.

```sh
cargo run --locked -- --data-dir ./data extension install --builtin web
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.web --grant 'network:https:*'
```
