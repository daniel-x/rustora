# rustora

A portvibe (a vibecoded port) of [OpenRA](https://www.openra.net) to Rust, aiming at full
compatibility with OpenRA, currently to OpenRA release-20250330 (data, replays, network play and
save games).

Almost all code and docs in this project are ai generated, i.e. vibe coded. In fact, currently,
this section in this file that you are reading right now are the only human-written part. This
project is a re-re-make of the 1995 video game Command & Conquer: Red Alert. The project in front
of you is based on the remake called OpenRA ( https://www.openra.net ), which has seen ongoing
development from 2007 to 2026 (today) and will probably go on to be developed.  

The game resources like sprites and sounds are freeware, so you can download and use them legally.
However, EA missed to explicitly attach a license to this freeware, so they still have a copyright.
That adds one subtlety: The game resources can be downloaded legally, but it is a gray area if you
were to package them into other software. That's why you have to click the download button when you
start rustora (or OpenRA) for the first time. Content that can't be packaged into rustora due to
legal obstacles gets downloaded at that step.

I haven't tested rustora on macOS or windows. If you test it there, please let me know. You can
tell me on the Signal messenger in this group:  
https://signal.group/#CjQKIOVB51Kt6ZNnJ1MUCJZ1Yy_uXkYk5jhF1jTu3u2OZ-K_EhCSG5pbi4zC2d35tVQDhQQU

Why re-re-make? Because I can... I am trying out what can be done with ai for software development
and translating a 6MB C# codebase into rust was an interesting experiment. It took only a few days.
Take-aways:
- Translating the 6MB codebase took about 350 USD API costs for the one shot and about the same on
  top for fixing many bugs and doing some enhancements. (I used a 20 Eur/month plan, so I didn't
  have to pay 700 USD.); it was ~18h API time in total. The stats say 7d session time, but that's
  just an upper bound. Most of those 7d is idle time. I estimate that the real running time of the
  implementation (ai working + ai waiting for tool results) is just about 48h.
- It helped greatly to use tmux to make claude code able to self-compact when it decides to do so.
  This allowed the ai to compact after every milestone, which kept usually keeps the avergage
  context size around 150k to 250k. It did a total of 37 such self-compactions. I only gave it this
  possibility during the run when I realized it would be helpful. I felt that afterwards it took
  about twice as long to use up a 5h usage window's token quota.
- I had it stop automatically before using up all token quota and to set a timer for continuing
  after the usage window resets. This can happen one or 2 hours before the 5h window resets. This
  was crucial for making it run autonomously. Otherwise, when paused because the token quota was
  used up, it did not resume working by itself after the usage window reset.
- The first one-shot result was already playable, but it had many bugs. Most bugs could be solved
  by extensive testing. A good start was to replay 800 recorded real world matches by the ai and
  have it fix any in-game diversions from the reference game client automatically.
- Leftover were many UI bugs, which were not cought by the game replays, because they didn't use
  UI. If i do something comparable again, I will take care to add better test coverage.
- Even more can be automated. After the ai writes a plan for the port, check the port and think
  about aspects that the ai doesn't know, like feasibility/availability of additional tests. Tell
  it to add them to the plan.
- Read the plan before steering the ai to implement it (I didn't read it before.). Solving small
  issues in the plan helps you later to get a good one-shot result.
- I used the model Opus 5.5 throughout. I heard that having Opus 5.5 delegate to Sonnet utilizes
  the token quota much more efficiently. I yet have to try that out.

(end of human-written text)

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
