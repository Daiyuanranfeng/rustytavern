# Skills tool plugin

The plugin ID is `org.rustytavern.tools.skills`.
The factory `tools.skills` reads `DATA_DIR/skills/NAME/SKILL.md` at activation.
Each `SKILL.md` file has `name` and `description` front matter.

The plugin contributes the skill catalog to the `before_history` prompt slot.
The catalog lists each skill name and description.
The saved request therefore records the catalog.
The plugin registers the `skill` tool as `tools.skills/1`.
The tool reads the full skill only when the model needs it.
The optional `path` argument reads another file inside the skill directory.
Without skills, the plugin contributes nothing and registers an empty tool set.
