# Skills extension

The skills extension adds Agent Skills for the [agent extension](../agent/README.md).
The extension owns one [tool plugin](plugins/README.md).
The package ID is `org.rustytavern.extension.skills`.

```sh
cargo run --locked -- --data-dir ./data extension install --builtin skills
cargo run --locked -- --data-dir ./data extension enable org.rustytavern.extension.skills \
  --grant skills.read --grant prompt.contribute:before_history
```
