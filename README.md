# rustora

A portvibe (a vibecoded port) of [OpenRA](https://www.openra.net) to Rust, aiming at full
compatibility with OpenRA release-20250330 (data, replays, network play and save games). The first
target is the Red Alert mod.

The original game content is not part of this repository. It is installed by the content
installer, like in OpenRA. The RA mod data of OpenRA release-20250330 (rules, maps, chrome and scripts,
GPL-3.0-or-later like OpenRA) is in `data/`; `scripts/update-mod-data.sh [openra-ref]` replaces it with
the mod data of another OpenRA tag or commit from GitHub.

## System requirements and first start

- Windows 10 or 11, 64-bit, with a graphics driver for DirectX 12, Vulkan or OpenGL.
- macOS 11 or newer, on Intel or Apple Silicon.
- Linux on x86_64 with glibc 2.28 or newer (Debian 10, Ubuntu 20.04, Fedora 29, RHEL 8 or newer), X11 or
  Wayland, and a Vulkan or OpenGL driver. Sound needs ALSA (`libasound2`), which practically every desktop
  has; without it the game runs silently.

Unpack the archive and start `rustora` (`rustora.exe` on Windows, `Rustora.app` on macOS). On the first
start the game asks for the content of the original Red Alert: **Quick Install** downloads 13 MB from a mirror
of the 2008 freeware release, without music and videos, and opens the main menu when it is done. **Advanced
Install** also copies the music and videos from an original game disc or installation.

The packages are not signed by a known developer, so Windows and macOS warn on the first start:

- Windows shows "Windows protected your PC". Click **More info**, then **Run anyway**.
- macOS refuses to open the app because it cannot verify the developer. The app has only an ad-hoc signature,
  which Apple Silicon needs to run it at all, but which names no developer. Open **System Settings**,
  **Privacy & Security**, and click **Open Anyway** next to the message about Rustora, or remove the
  quarantine flag of the download in a terminal: `xattr -dr com.apple.quarantine Rustora.app`. After that the
  app opens normally. Without a developer identity, macOS may ask again after an update whether the app may
  accept incoming network connections, when you host a multiplayer game.

## Building

```
cargo build --release
```

## Running

`rustora` opens the main menu like the OpenRA launcher; `rustora <host>:<port>` joins a server. The mod
data is looked up next to the executable, in `../share/rustora`, in `../Resources` (macOS app bundle), or,
for a build in a source checkout, in a directory beside the executable's parent directories, which finds
`data/`; `--engine-dir <dir>` overrides it. Settings, content and maps live in
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

`packaging/linux/package.sh`, `packaging/windows/package.ps1` and `packaging/macos/package.sh` build
archives with the binaries, the RA mod data of `data/` and the current GeoIP database of OpenRA, which
they download. The release workflow runs them for tags `v*`. The Linux script builds against glibc 2.28 with
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild), which needs zig. The Windows binaries link the
C runtime statically (`.cargo/config.toml`), so they need no Visual C++ Redistributable.

## License

GPL-3.0-or-later, the same terms as OpenRA, of which this is a derived work. See `COPYING` for the
license text and `AUTHORS` for the rustora authors and the OpenRA developers and contributors.

## Authorship

This project, including its purpose and overall architecture, was conceived and designed by its
human authors. The implementation was generated in full with AI assistance (Claude Code), under
the human authors' direction.
