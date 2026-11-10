#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: scripts/update-mod-data.sh [openra-ref]
# Replaces data/ with the RA mod data of OpenRA at the tag or commit openra-ref (default release-20250330),
# downloaded from GitHub: mods/{common,common-content,ra,ra-content} and global mix database.dat. Like in an OpenRA
# release package, {DEV_VERSION} in the mod.yaml files becomes the ref. data/ is an engine directory for the client
# and the server, and the packaging scripts ship it.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
ref="${1:-release-20250330}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL "https://github.com/OpenRA/OpenRA/archive/$ref.tar.gz" | tar -xz -C "$work"
src="$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -1)"
new="$work/data"
mkdir -p "$new/mods"
for m in common common-content ra ra-content; do
  cp -R "$src/mods/$m" "$new/mods/"
done
cp "$src/global mix database.dat" "$new/"
for f in "$new"/mods/*/mod.yaml; do
  sed "s/{DEV_VERSION}/$ref/g" "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done
cat > "$new/README.md" <<TEXT
# RA mod data

The mod data of the RA mod of [OpenRA](https://github.com/OpenRA/OpenRA) $ref, licensed under the GNU GPL version 3
or later like OpenRA. \`scripts/update-mod-data.sh\` writes this directory; do not edit it by hand.
TEXT
rm -rf "$root/data"
mv "$new" "$root/data"
echo "data/ now holds the mod data of OpenRA $ref"
