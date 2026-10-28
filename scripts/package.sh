#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != Darwin ]]; then
  echo "Packaging requires macOS." >&2
  exit 1
fi

MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release --locked
bundle="dist/MCP Manager.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp target/release/macos-mcp-manager "$bundle/Contents/MacOS/macos-mcp-manager"
cp resources/Info.plist "$bundle/Contents/Info.plist"
cp LICENSE "$bundle/Contents/Resources/LICENSE"
cp README.md "$bundle/Contents/Resources/README.md"
codesign --force --deep --sign "${SIGNING_IDENTITY:--}" "$bundle"
codesign --verify --deep --strict "$bundle"
echo "Built $bundle"
echo "Move it to /Applications to install."
echo "The default signature is ad hoc; public distribution requires Developer ID signing and notarization."

