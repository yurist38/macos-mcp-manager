# Compatibility notes

Initial implementation targets user files on macOS 14+. This is a fixture-tested compatibility matrix, not a claim that all client versions have been exercised live.

| Integration | Toggle strategy | Fixture coverage | Live-client verification |
| --- | --- | --- | --- |
| Codex direct servers | Native `enabled` policy | TOML tables, inline tables, comments, secrets, round trips | Pending |
| Codex configured plugins | Whole-plugin `enabled` and existing nested server policy | Parent/child enablement, quoted plugin IDs | Pending |
| Claude Desktop user servers | Private set-aside/restore | Strict JSON, nested credentials, restart persistence | Pending |
| Claude Code user servers | Private set-aside/restore | User definitions; unrelated project fields preserved | Pending |
| VS Code user definitions | Private set-aside/restore | JSONC comments, trailing commas, inputs preserved | Pending; native enablement deliberately unsupported |
| Cursor user definitions | Private set-aside/restore | Shared JSON adapter round trips | Pending |

Before marking a client version verified: create a harmless test MCP entry in that client, disable it through the manager, reload as instructed, confirm tools are unavailable, restore it, reload, and confirm the entry returns. Also change an unrelated setting in the client while the manager is open and confirm it survives.

Claude Code's native `/mcp` disable lists are project-specific, so they are not used to implement a global user-definition switch. VS Code stores native enablement separately from its server configuration; the app therefore labels its checkmarks as configuration presence. Neither integration claims to override project, extension, discovery, or organization settings.

