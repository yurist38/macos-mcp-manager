# Recovery and removing the app

Re-enable a disabled server from its client submenu to restore its definition. There is no manual undo or retained configuration history. The private save journal exists only to make interrupted toggles recoverable.

## Restore a disabled definition manually

1. Quit MCP Manager and the affected AI client.
2. Make a private copy of `~/Library/Application Support/MCP Manager` before editing anything. It contains credentials; never paste the whole file into an issue.
3. Open `state.json`. Each `parked` entry has a `source`, `name`, and raw JSON/JSONC `definition` string.
4. Restore the definition under its name inside the client's `mcpServers` object (or `servers` for VS Code). Decode the JSON string; do not paste the escaped string as the definition.
5. Remove only that corresponding entry from `parked`. Do not remove other disabled definitions.
6. Validate the client configuration and `state.json`, restart the client, and reopen MCP Manager.

If a disabled name also exists in the client, preserve both copies and decide which to keep. The app deliberately blocks the conflicting client instead of overwriting either definition.

## Interrupted-save conflicts

On launch and before writes, MCP Manager checks `state.json`'s `pending` change. If the client file matches `change.after`, it completes the state update. If it matches `change.before`, it cancels the pending write. If it matches neither, it stops recovery and preserves all files.

For manual resolution, first quit the app and copy the recovery folder privately. Compare the client file with the pending `change.before` and `change.after` strings. Choose one consistent pair:

- Keep the operation: reconcile the client file with the intended `after` content; set top-level `parked` to `change.parked_after`.
- Cancel the operation: reconcile the client file with the intended `before` content; set top-level `parked` to `change.parked_before`.

Preserve any external edits when reconciling. Then set `pending` to `null`, keep `version: 1`, and validate both JSON/TOML files before relaunching. Never delete the whole recovery state while it still contains disabled definitions you need.

## Uninstall

Re-enable all set-aside definitions first. Quit the app, then remove the app bundle. If an older build created a login item, remove `~/Library/LaunchAgents/dev.mcp-manager.plist` manually. Remove the private application data folder only once you have verified that all desired definitions are restored.

