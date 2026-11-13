#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Usage: scripts/update-mod-data.sh [openra-ref]
# Replaces data/ with the mod data of OpenRA at the tag openra-ref (default release-20250330): mods/{common,
# common-content,ra,ra-content,cnc,cnc-content}, without the files listed in scripts/openra-assets.txt. Those files
# derive from the original Westwood games or have an unclear license; the content installers of ra and cnc get a
# required package that downloads them from the OpenRA source archive of the same tag into
# ^SupportDir|Content/openra-assets/<ref> (see ASSETS.md). Like in an OpenRA release package, {DEV_VERSION} in the
# mod.yaml files becomes the ref. data/ is an engine directory for the client and the server, and the packaging
# scripts ship it. Needs curl and python3.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
ref="${1:-release-20250330}"
url="https://github.com/OpenRA/OpenRA/archive/refs/tags/$ref.zip"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL -o "$work/openra.zip" "$url"
python3 - "$root" "$ref" "$url" "$work" <<'PY'
import fnmatch, hashlib, os, re, shutil, sys, zipfile

root, ref, url, work = sys.argv[1:5]
archive = os.path.join(work, "openra.zip")
sha1 = hashlib.sha1(open(archive, "rb").read()).hexdigest()
with zipfile.ZipFile(archive) as z:
    z.extractall(work)
    top = z.namelist()[0].split("/")[0]
src = os.path.join(work, top)
new = os.path.join(work, "data")
mods = ["common", "common-content", "ra", "ra-content", "cnc", "cnc-content"]
os.makedirs(os.path.join(new, "mods"))
for m in mods:
    shutil.copytree(os.path.join(src, "mods", m), os.path.join(new, "mods", m))
shutil.copy(os.path.join(src, "global mix database.dat"), new)

patterns = []
for line in open(os.path.join(root, "scripts", "openra-assets.txt")):
    line = line.split("#", 1)[0].strip()
    if line:
        patterns.append(line)

def rx(glob):
    out = ""
    i = 0
    while i < len(glob):
        if glob.startswith("**", i):
            out += ".*"
            i += 2
        elif glob[i] == "*":
            out += "[^/]*"
            i += 1
        else:
            out += re.escape(glob[i])
            i += 1
    return re.compile(out + "$")

regexes = [rx(p) for p in patterns]
shipped = []
for dirpath, _, files in os.walk(new):
    for f in files:
        shipped.append(os.path.relpath(os.path.join(dirpath, f), new).replace(os.sep, "/"))
assets = sorted(f for f in shipped if any(r.match(f) for r in regexes))
for f in assets:
    os.remove(os.path.join(new, f))

replacements = os.path.join(root, "scripts", "assets")
for dirpath, _, files in os.walk(replacements):
    for f in files:
        rel = os.path.relpath(os.path.join(dirpath, f), replacements)
        shutil.copy(os.path.join(dirpath, f), os.path.join(new, "mods", rel))

note = ("The files of this folder are not shipped with rustora. The content installer downloads them from the OpenRA "
        "source archive into the support directory; see ASSETS.md of rustora.\n")
for dirpath, dirs, files in os.walk(new):
    if not files and not dirs:
        open(os.path.join(dirpath, "README.md"), "w").write(note)

target = "^SupportDir|Content/openra-assets/" + ref
extract = "".join(f"\t\t{target}/{f}: {top}/{f}\n" for f in assets)
download = (f"openra-assets: OpenRA Assets\n\tType: ZipFile\n\tSHA1: {sha1}\n\tURL: {url}\n\tExtract:\n{extract}")
for game in ["ra", "cnc"]:
    content = os.path.join(new, "mods", game + "-content")
    with open(os.path.join(content, "installer", "downloads.yaml"), "a") as f:
        f.write("\n" + download)
    tests = [a for a in assets if a.startswith(f"mods/{game}/bits/")][:3]
    tests += [a for a in assets if a.startswith(f"mods/{game}/uibits/")][:1]
    package = ("\t\tContentPackage@openra-assets:\n"
               "\t\t\tTitle: modcontent-package-openra-assets\n"
               "\t\t\tIdentifier: openra-assets\n"
               f"\t\t\tTestFiles: {', '.join(target + '/' + t for t in tests)}\n"
               "\t\t\tRequired: true\n"
               "\t\t\tDownload: openra-assets\n")
    manifest = os.path.join(content, "mod.yaml")
    lines = open(manifest).read().split("\n")
    start = lines.index("ModContent:")
    at = next(i for i in range(start, len(lines)) if lines[i] == "\tPackages:") + 1
    lines.insert(at, package.rstrip("\n"))
    open(manifest, "w").write("\n".join(lines))
    with open(os.path.join(content, "fluent", "chrome.ftl"), "a") as f:
        f.write("modcontent-package-openra-assets = OpenRA Assets\n")
    # The content check of the game mod mounts its content packages; this required one fails until the download.
    game_manifest = os.path.join(new, "mods", game, "mod.yaml")
    lines = open(game_manifest).read().split("\n")
    at = lines.index("\tContentPackages:") + 1
    lines.insert(at, f"\t\t{target}/mods/{game}/bits")
    open(game_manifest, "w").write("\n".join(lines))

for m in mods:
    p = os.path.join(new, "mods", m, "mod.yaml")
    if not os.path.exists(p):
        continue
    text = open(p).read().replace("{DEV_VERSION}", ref)
    open(p, "w").write(text)

open(os.path.join(new, "README.md"), "w").write(f"""# Mod data

The mod data of the RA and C&C mods of [OpenRA](https://github.com/OpenRA/OpenRA) {ref}, licensed under the GNU GPL
version 3 or later like OpenRA, without the files that derive from the original Westwood games or have an unclear
license. The content installer downloads those from the OpenRA source archive; see ASSETS.md.
`scripts/update-mod-data.sh` writes this directory; do not edit it by hand.
""")
print(f"{len(assets)} files moved to the openra-assets download (archive SHA1 {sha1})")
PY
rm -rf "$root/data"
mv "$work/data" "$root/data"
echo "data/ now holds the mod data of OpenRA $ref"
