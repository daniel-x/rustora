# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: packaging/windows/package.ps1 [out-dir]
# Builds the release binaries and writes rustora-<version>-windows-x86_64.zip with the binaries and the RA mod data
# of data/. The game content is installed at first start.
param([string]$Out = "")
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot/../.."
if ($Out -eq "") { $Out = "$root/target/distrib" }
New-Item -ItemType Directory -Force $Out | Out-Null
$version = (Select-String -Path "$root/Cargo.toml" -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "rustora-$version-windows-x86_64"
cargo build --profile dist --manifest-path "$root/Cargo.toml" -p rustora -p rustora-server
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$stage = Join-Path $Out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($b in "rustora", "rustora-server") { Copy-Item "$root/target/dist/$b.exe" $stage }
Copy-Item -Recurse "$root/data/mods" $stage
Copy-Item "$root/data/global mix database.dat", "$root/COPYING", "$root/AUTHORS", "$root/README.md" $stage
# The GeoIP database of the server, the current monthly one like OpenRA's make.ps1 downloads it.
$geoip = "IP2LOCATION-LITE-DB1.IPV6.BIN.ZIP"
try {
    Invoke-WebRequest -Uri "https://github.com/OpenRA/GeoIP-Database/releases/download/monthly/$geoip" -OutFile (Join-Path $stage $geoip)
} catch {
    Remove-Item (Join-Path $stage $geoip) -ErrorAction SilentlyContinue
    Write-Warning "No GeoIP database; the server of this package looks up no countries"
}
$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Output $zip
