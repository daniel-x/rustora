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

`rustora [Key=Value]...` opens the main menu like the OpenRA launcher. The mod data is looked up next to
the executable, in `../share/rustora` or in `../Resources` (macOS app bundle); `Engine.EngineDir=<dir>`
overrides it, for example with an OpenRA release-20250330 source tree. Settings, content and replays
live in the OpenRA support directory, or in a `Support` directory next to the mod data if one exists;
`Engine.SupportDir=<dir>` overrides it.

`rustora <engine-dir> <support-dir> <replay>` plays back a replay in a window. Arrow keys or WASD
scroll, the mouse wheel zooms, space pauses, `+` and `-` change the speed, Tab cycles the shroud of
each player, Escape quits. `--screenshot <out.webp> --tick <n>` renders one frame offscreen instead.

`rustora-headless verify-replay <engine-dir> <support-dir> <replay> [sync-dump]` replays a game
without graphics and compares the sync hashes.

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
