#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != Darwin ]]; then
  echo "Packaging requires macOS." >&2
  exit 1
fi

bundle="dist/MCP Manager.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
if [[ "${UNIVERSAL:-0}" == 1 ]]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release --locked --target "$target"
  done
  lipo -create \
    target/aarch64-apple-darwin/release/macos-mcp-manager \
    target/x86_64-apple-darwin/release/macos-mcp-manager \
    -output "$bundle/Contents/MacOS/macos-mcp-manager"
  lipo "$bundle/Contents/MacOS/macos-mcp-manager" -verify_arch arm64
  lipo "$bundle/Contents/MacOS/macos-mcp-manager" -verify_arch x86_64
else
  MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release --locked
  cp target/release/macos-mcp-manager "$bundle/Contents/MacOS/macos-mcp-manager"
fi
cp resources/Info.plist "$bundle/Contents/Info.plist"
cp resources/AppIcon.icns "$bundle/Contents/Resources/AppIcon.icns"
version="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "macos-mcp-manager"))')"
# AppKit expects a numeric bundle version, even for a prerelease Cargo version.
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version%%[-+]*}" "$bundle/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${BUILD_NUMBER:-1}" "$bundle/Contents/Info.plist"
cp LICENSE "$bundle/Contents/Resources/LICENSE"
cp README.md "$bundle/Contents/Resources/README.md"
codesign --force --deep --sign "${SIGNING_IDENTITY:--}" "$bundle"
codesign --verify --deep --strict "$bundle"
touch "$bundle"
echo "Built $bundle"
echo "Move it to /Applications to install."
echo "The default signature is ad hoc; public distribution requires Developer ID signing and notarization."

