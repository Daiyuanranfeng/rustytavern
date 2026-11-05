# Web tool plugin

The plugin ID is `org.rustytavern.tools.web`.
The factory `tools.web` registers `web_fetch` as `tools.web/1`.
The tool reads one public HTTPS page through the scoped host fetcher.
The fetcher checks destination addresses and redirects.
The tool removes HTML scripts, styles, and tags.

The `network:https:*` grant permits every public HTTPS host.
A `network:https:HOST` grant permits one host.
The bundled manifest requests only the wildcard grant.
A copied package can request specific hosts.
