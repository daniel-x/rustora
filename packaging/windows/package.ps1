# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: packaging/windows/package.ps1 <openra-source-dir> [out-dir]
# Builds the release binaries and writes rustora-<version>-windows-x86_64.zip with the binaries and the mod data
# of the RA mod taken from an OpenRA release-20250330 source tree. The game content is installed at first start.
param([Parameter(Mandatory = $true)][string]$OpenRA, [string]$Out = "")
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot/../.."
$OpenRA = Resolve-Path $OpenRA
if ($Out -eq "") { $Out = "$root/target/distrib" }
New-Item -ItemType Directory -Force $Out | Out-Null
$version = (Select-String -Path "$root/Cargo.toml" -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "rustora-$version-windows-x86_64"
cargo build --profile dist --manifest-path "$root/Cargo.toml" -p rustora -p rustora-server-bin
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$stage = Join-Path $Out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force "$stage/mods" | Out-Null
foreach ($b in "rustora", "rustora-server") { Copy-Item "$root/target/dist/$b.exe" $stage }
foreach ($m in "common", "common-content", "ra", "ra-content") { Copy-Item -Recurse "$OpenRA/mods/$m" "$stage/mods/" }
Get-ChildItem "$stage/mods/*/mod.yaml" | ForEach-Object {
  (Get-Content -Raw $_.FullName).Replace("{DEV_VERSION}", "release-20250330") | Set-Content -NoNewline $_.FullName
}
Copy-Item "$OpenRA/global mix database.dat", "$root/COPYING", "$root/AUTHORS", "$root/README.md" $stage
$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Output $zip
