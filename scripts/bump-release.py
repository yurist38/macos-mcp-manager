#!/usr/bin/env python3
"""Prepare the next patch release from Cargo and all existing stable tags."""
import json
import pathlib
import re
import subprocess

metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"
]))
package = next(p for p in metadata["packages"] if p["name"] == "macos-mcp-manager")
current = package["version"]
versions = [tuple(map(int, current.split("-")[0].split("+")[0].split(".")))]
for tag in subprocess.check_output(["git", "tag", "--list"], text=True).splitlines():
    match = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", tag)
    if match:
        versions.append(tuple(map(int, match.groups())))
major, minor, patch = max(versions)
version = f"{major}.{minor}.{patch + 1}"

manifest = pathlib.Path(package["manifest_path"])
text = manifest.read_text()
text, count = re.subn(r'(?m)^version\s*=\s*"' + re.escape(current) + r'"',
                      f'version = "{version}"', text, count=1)
if count != 1:
    raise SystemExit("Could not update package version")
lock = pathlib.Path(metadata["workspace_root"]) / "Cargo.lock"
lock_text, count = re.subn(
    r'(\[\[package\]\]\nname = "macos-mcp-manager"\nversion = ")' + re.escape(current) + r'(")',
    lambda match: match[1] + version + match[2], lock.read_text(), count=1)
if count != 1:
    raise SystemExit("Could not update root package in Cargo.lock")
manifest.write_text(text)
lock.write_text(lock_text)
print(f"version={version}")
print("prerelease=false")
