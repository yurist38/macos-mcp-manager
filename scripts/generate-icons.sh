#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

icon_directory="$(mktemp -d "${TMPDIR:-/tmp}/mcp-manager-icons.XXXXXX")"
trap 'rm -rf "$icon_directory"' EXIT
iconset="$icon_directory/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" resources/AppIcon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  retina_size=$((size * 2))
  sips -z "$retina_size" "$retina_size" resources/AppIcon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o resources/AppIcon.icns
