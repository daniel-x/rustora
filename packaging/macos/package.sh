#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: packaging/macos/package.sh [out-dir]
# Builds universal (x86_64 and arm64) release binaries and writes rustora-<version>-macos.zip with Rustora.app,
# whose Contents/Resources hold the RA mod data of data/.
# Needs the rustup targets x86_64-apple-darwin and aarch64-apple-darwin. The app is not signed.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$(mkdir -p "${1:-$root/target/distrib}" && cd "${1:-$root/target/distrib}" && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
name="rustora-${version:-dev}-macos"
for t in x86_64-apple-darwin aarch64-apple-darwin; do
  cargo build --profile dist --manifest-path "$root/Cargo.toml" --target "$t" -p rustora -p rustora-server
done
app="$out/$name/Rustora.app"
rm -rf "$out/$name"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for b in rustora rustora-server; do
  lipo -create -output "$app/Contents/MacOS/$b" \
    "$root/target/x86_64-apple-darwin/dist/$b" "$root/target/aarch64-apple-darwin/dist/$b"
done
res="$app/Contents/Resources"
cp -R "$root/data/mods" "$root/data/global mix database.dat" "$res/"
# The GeoIP database of the server, the current monthly one like OpenRA's fetch-geoip.sh downloads it.
geoip=IP2LOCATION-LITE-DB1.IPV6.BIN.ZIP
if ! curl -fsSL -o "$res/$geoip" "https://github.com/OpenRA/GeoIP-Database/releases/download/monthly/$geoip"; then
  rm -f "$res/$geoip"
  echo "Warning: no GeoIP database; the server of this package looks up no countries" >&2
fi
cp "$root/COPYING" "$root/AUTHORS" "$root/README.md" "$out/$name/"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key><string>rustora</string>
	<key>CFBundleIdentifier</key><string>io.github.rustora</string>
	<key>CFBundleName</key><string>Rustora</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>${version:-0.0.0}</string>
	<key>CFBundleVersion</key><string>${version:-0.0.0}</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
(cd "$out" && rm -f "$name.zip" && ditto -c -k --keepParent "$name" "$name.zip")
echo "$out/$name.zip"
