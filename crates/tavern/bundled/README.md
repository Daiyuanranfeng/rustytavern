# Bundled extensions

The product supplies optional extensions as reference packages.
The user must install and enable each package explicitly.

| Package | Feature | Interface guide |
| --- | --- | --- |
| [Agent](agent/README.md) | The package replaces the single-reply driver with a tool loop. | [Driver plugin](agent/plugins/README.md) |
| [MCP](mcp/README.md) | The package adds MCP server tools to the agent. | [Tool plugin](mcp/plugins/README.md) |
| [Skills](skills/README.md) | The package adds Agent Skills to the agent. | [Tool plugin](skills/plugins/README.md) |
| [Web](web/README.md) | The package adds the web fetch tool to the agent. | [Tool plugin](web/plugins/README.md) |
| [Chub](chub/README.md) | The package imports Chub character cards. | [Source plugin](chub/plugins/README.md) |
| [RisuRealm](risurealm/README.md) | The package imports RisuRealm character PNG files. | [Source plugin](risurealm/plugins/README.md) |

The [extension guide](../../../docs/developer.md#product-extensions) defines package design and implementation.
The [plugin guide](../../../docs/developer.md#runtime-plugins) defines runtime design and implementation.
The [standard plugin reference](../../../docs/developer.md#standard-plugins) describes integral product plugins.
The [example index](../examples/README.md) covers native plugins and independent processes.
