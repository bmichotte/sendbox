# MCP server embedded in the app, over streamable HTTP

The MCP server runs inside the Sendbox app process and is exposed over streamable HTTP on `127.0.0.1` (its own configurable port), rather than as a separate stdio process reading the on-disk store. Being in-process lets "wait for an email" react directly to new captures instead of watching files, and keeps one owner of the Inbox. The cost is that agents can only reach the MCP while the app is running, which is acceptable: without the app, nothing is being captured anyway.

## Considered Options

- **Stdio process reading the store**: the default shape for MCP servers, works without the app running, but only ever sees stale Emails and needs file watching to wait for new ones.
