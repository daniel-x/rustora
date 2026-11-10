# rustora

A portvibe (a vibecoded port) of [OpenRA](https://www.openra.net) to Rust, aiming at full
compatibility with OpenRA release-20250330 (data, replays, network play and save games). The first
target is the Red Alert mod.

The original game content is not part of this repository. It is installed by the content
installer, like in OpenRA.

## Building

```
cargo build --release
```

## Running

`rustora` opens the main menu like the OpenRA launcher; `rustora <host>:<port>` joins a server. The mod
data is looked up next to the executable, in `../share/rustora`, in `../Resources` (macOS app bundle), or,
for a build in a source checkout, in an OpenRA release-20250330 source tree in a directory beside the
executable's parent directories; `--engine-dir <dir>` overrides it. Settings, content and maps live in
the OpenRA support directory, so they are shared with an OpenRA installation, or in a `Support`
directory next to the mod data if one exists; `--support-dir <dir>` overrides it. The files rustora
writes for itself, logs, exception reports, replays and saves, go to `rustora` in that directory, for
example `~/.config/openra/rustora`.
`Section.Field=value` arguments override settings. `rustora --help` lists all options.

`rustora --replay <file> --viewer` plays back a replay in a debug viewer. Arrow keys or WASD scroll, the
mouse wheel zooms, space pauses, `+` and `-` change the speed, Tab cycles the shroud of each player,
Escape quits. `--screenshot <out.webp> --tick <n>` renders one frame offscreen instead.

`rustora --headless <command> ...` runs without window, graphics and sound, also on servers without
ALSA or a GPU. `rustora --headless verify-replay <engine-dir> <support-dir> <replay> [sync-dump]`
replays a game and compares the sync hashes; `bins/rustora/src/headless.rs` lists all commands.

`rustora-server` is the multiplayer game server. It runs independently, or dependent on a client that started it
for its game and ends with that client. It finds the mod data and the support directory like the client. Single
player games run their server inside the client.

## Packaging

`packaging/linux/package.sh <openra-dir>`, `packaging/windows/package.ps1 <openra-dir>` and
`packaging/macos/package.sh <openra-dir>` build archives with the binaries and the RA mod data of an
OpenRA release-20250330 source tree. The release workflow runs them for tags `v*`.

## License

GPL-3.0-or-later, the same terms as OpenRA, of which this is a derived work. See `COPYING` for the
license text and `AUTHORS` for the rustora authors and the OpenRA developers and contributors.

## Authorship

This project, including its purpose and overall architecture, was conceived and designed by its
human authors. The implementation was generated in full with AI assistance (Claude Code), under
the human authors' direction.
