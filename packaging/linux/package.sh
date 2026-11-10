#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: packaging/linux/package.sh <openra-source-dir> [out-dir]
# Builds the release binaries and writes rustora-<version>-linux-<arch>.tar.gz with the binaries and the mod data
# of the RA mod taken from an OpenRA release-20250330 source tree. The game content is installed at first start.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
openra="$(cd "$1" && pwd)"
out="$(mkdir -p "${2:-$root/target/distrib}" && cd "${2:-$root/target/distrib}" && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
name="rustora-${version:-dev}-linux-$(uname -m)"
cargo build --profile dist --manifest-path "$root/Cargo.toml" -p rustora -p rustora-server
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage/mods"
for b in rustora rustora-server; do
  install -m 755 "$root/target/dist/$b" "$stage/"
  strip "$stage/$b"
done
for m in common common-content ra ra-content; do
  cp -r "$openra/mods/$m" "$stage/mods/"
done
sed -i 's/{DEV_VERSION}/release-20250330/g' "$stage"/mods/*/mod.yaml
cp "$openra/global mix database.dat" "$stage/"
# The GeoIP database of the server: the one of the OpenRA tree if it is younger than 30 days, else a download of
# the current monthly one like OpenRA's fetch-geoip.sh, else an older one of the OpenRA tree.
geoip=IP2LOCATION-LITE-DB1.IPV6.BIN.ZIP
copy_geoip() {
  if [ -n "$(find "$openra" -maxdepth 1 -name "$geoip" -mtime -30 -print)" ]; then
    cp "$openra/$geoip" "$1/"
  elif ! curl -fsSL -o "$1/$geoip" "https://github.com/OpenRA/GeoIP-Database/releases/download/monthly/$geoip"; then
    rm -f "$1/$geoip"
    if [ -f "$openra/$geoip" ]; then
      cp "$openra/$geoip" "$1/"
    else
      echo "Warning: no GeoIP database; the server of this package looks up no countries" >&2
    fi
  fi
}
copy_geoip "$stage"
cp "$root/COPYING" "$root/AUTHORS" "$root/README.md" "$stage/"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
