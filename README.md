# rustora

A port of [OpenRA](https://www.openra.net) to Rust, aiming at full compatibility with OpenRA
release-20250330 (data, replays, network play and save games). The first target is the Red Alert
mod.

The original game content is not part of this repository. It is installed by the content
installer, like in OpenRA.

## Building

```
cargo build --release
```

## Running

`rustora <engine-dir> <support-dir> <replay>` plays back a replay in a window. Arrow keys or WASD
scroll, the mouse wheel zooms, space pauses, `+` and `-` change the speed, Tab cycles the shroud of
each player, Escape quits. `--screenshot <out.webp> --tick <n>` renders one frame offscreen instead.

`rustora-headless verify-replay <engine-dir> <support-dir> <replay> [sync-dump]` replays a game
without graphics and compares the sync hashes.

## License

GPL-3.0-or-later, the same terms as OpenRA, of which this is a derived work.

## Authorship

This project, including its purpose and overall architecture, was conceived and designed by its
human authors. The implementation was generated in full with AI assistance (Claude Code), under
the human authors' direction.
