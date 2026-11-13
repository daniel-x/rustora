#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: packaging/linux/package.sh [out-dir]
# Builds the release binaries and writes rustora-<version>-linux-<arch>.tar.gz with the binaries and the mod data
# of data/. The game content is installed at first start. The binaries are built with cargo-zigbuild against glibc
# $GLIBC_VERSION (default 2.28), so they also run on older distributions; this needs zig and cargo-zigbuild.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$(mkdir -p "${1:-$root/target/distrib}" && cd "${1:-$root/target/distrib}" && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
arch="$(uname -m)"
name="rustora-${version:-dev}-linux-$arch"
target="$arch-unknown-linux-gnu"
command -v cargo-zigbuild > /dev/null || { echo "cargo-zigbuild and zig are needed, see the usage" >&2; exit 1; }
cargo zigbuild --profile dist --manifest-path "$root/Cargo.toml" --target "$target.${GLIBC_VERSION:-2.28}" \
  -p rustora -p rustora-server
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"
for b in rustora rustora-server; do
  install -m 755 "$root/target/$target/dist/$b" "$stage/"
  strip "$stage/$b"
done
cp -R "$root/data/mods" "$stage/"
# The GeoIP database of the server, the current monthly one like OpenRA's fetch-geoip.sh downloads it.
geoip=IP2LOCATION-LITE-DB1.IPV6.BIN.ZIP
if ! curl -fsSL -o "$stage/$geoip" "https://github.com/OpenRA/GeoIP-Database/releases/download/monthly/$geoip"; then
  rm -f "$stage/$geoip"
  echo "Warning: no GeoIP database; the server of this package looks up no countries" >&2
fi
cp "$root/COPYING" "$root/AUTHORS" "$root/README.md" "$root/ASSETS.md" "$stage/"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
