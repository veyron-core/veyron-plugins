#!/usr/bin/env bash
# Build a release archive for a plugin, register/update it in registry.json.
#
# Usage:
#   scripts/package.sh <plugin-dir-name> "<Display Name>" "<description>"
#
# Example:
#   scripts/package.sh network "Network" "Outbound HTTP for plugins/kernel via one http_request action."
#
# Reads plugins/<plugin-dir-name>/plugin.json for plugin_id (slug), version,
# binary name, permissions, and kernel_compatibility_range. Builds the
# release binary, writes dist/<slug>-<version>.zip (binary + plugin.json,
# flat) and dist/<slug>-<version>-src.zip (plugin.json + src/ + Cargo.toml),
# then inserts or updates the matching entry in registry.json (matched by
# slug; a new slug gets the next monotonically increasing zero-padded id).
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <plugin-dir-name> <display-name> <description>" >&2
    exit 1
fi

plugin_dir_name="$1"
display_name="$2"
description="$3"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
plugin_dir="$repo_root/plugins/$plugin_dir_name"
manifest="$plugin_dir/plugin.json"
dist_dir="$repo_root/dist"
registry="$repo_root/registry.json"

if [[ ! -f "$manifest" ]]; then
    echo "error: $manifest not found" >&2
    exit 1
fi

slug=$(jq -r '.plugin_id' "$manifest")
version=$(jq -r '.version' "$manifest")
binary=$(jq -r '.binary' "$manifest")
min_kernel=$(jq -r '.kernel_compatibility_range.min' "$manifest")
max_kernel=$(jq -r '.kernel_compatibility_range.max' "$manifest")
# registry permissions are lowercase, no PERMISSION_ prefix
permissions_json=$(jq -c '[.permissions[] | ltrimstr("PERMISSION_") | ascii_downcase]' "$manifest")

echo "==> building release binary for $plugin_dir_name ($slug $version)"
cargo build --release --manifest-path "$plugin_dir/Cargo.toml"

bin_path="$plugin_dir/target/release/$binary"
if [[ ! -f "$bin_path" ]]; then
    echo "error: built binary not found at $bin_path" >&2
    exit 1
fi

mkdir -p "$dist_dir"
archive_name="$slug-$version.zip"
src_archive_name="$slug-$version-src.zip"
archive_path="$dist_dir/$archive_name"
src_archive_path="$dist_dir/$src_archive_name"

echo "==> writing $archive_name"
rm -f "$archive_path"
zip -j -q "$archive_path" "$bin_path" "$manifest"

echo "==> writing $src_archive_name"
rm -f "$src_archive_path"
src_stage=$(mktemp -d)
trap 'rm -rf "$src_stage"' EXIT
src_root="$src_stage/$plugin_dir_name-src"
mkdir -p "$src_root"
cp "$manifest" "$src_root/"
cp -r "$plugin_dir/src" "$src_root/"
cp "$plugin_dir/Cargo.toml" "$src_root/"
(cd "$src_stage" && zip -rq "$src_archive_path" "$plugin_dir_name-src")

sha256=$(sha256sum "$archive_path" | awk '{print $1}')
archive_url="https://raw.githubusercontent.com/veyron-core/veyron-plugins/main/dist/$archive_name"
source_url="https://github.com/veyron-core/veyron-plugins/tree/main/plugins/$plugin_dir_name"

echo "==> updating registry.json (slug=$slug)"
python3 - "$registry" "$slug" "$display_name" "$description" "$version" \
    "$permissions_json" "$archive_url" "$source_url" "$sha256" "$min_kernel" "$max_kernel" <<'PYEOF'
import json
import sys

(registry_path, slug, name, description, version, permissions_json,
 archive_url, source_url, sha256, min_kernel, max_kernel) = sys.argv[1:12]

with open(registry_path) as f:
    entries = json.load(f)

permissions = json.loads(permissions_json)

entry = {
    "id": None,  # filled below
    "slug": slug,
    "name": name,
    "description": description,
    "version": version,
    "permissions": permissions,
    "archive_url": archive_url,
    "source_url": source_url,
    "sha256": sha256,
    "min_kernel_version": min_kernel,
    "max_kernel_version": max_kernel,
}

existing = next((e for e in entries if e["slug"] == slug), None)
if existing:
    entry["id"] = existing["id"]
    entries[entries.index(existing)] = entry
else:
    next_id = max((int(e["id"]) for e in entries), default=0) + 1
    entry["id"] = f"{next_id:03d}"
    entries.append(entry)

with open(registry_path, "w") as f:
    json.dump(entries, f, indent=2)
    f.write("\n")
PYEOF

echo "==> done: $archive_path ($sha256)"
