// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! rustora client. `rustora` opens the main menu with the default directories; see [`HELP`] for the arguments.
//! On Windows it is a GUI program without a console window of its own.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod client;
mod headless;
mod net;
mod palettes;
mod scene;
mod script_host;
mod single_player;
mod snapshot;
mod sprites;
mod ui_sprites;
mod window;
#[cfg(windows)]
mod windows_console;

use std::path::{Path, PathBuf};

use rustora_launch::{LoadedGame, ReplayDriver, Step, load_game};
use rustora_render::{Camera, Renderer};

/// mimalloc keeps the many small trait, actor and activity allocations close together. On an 8 bot benchmark game it
/// had 19% fewer cache misses than the glibc allocator and ran 26% faster.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// The synopsis of `rustora --help`.
const HELP: &str = "\
rustora - an OpenRA compatible Red Alert game

Usage:
  rustora [options] [<host>:<port>] [Section.Field=value]...
  rustora --headless <command> [arguments]...

Without arguments rustora opens the main menu. It finds the game data next to the executable, in
an installed layout, or in an OpenRA source tree beside a source checkout. It uses the support
directory of OpenRA, so content and settings of an OpenRA installation are found, and it offers
the content download when the game files are missing.

Arguments:
  <host>:<port>          joins the server at this address
  Section.Field=value    overrides a setting, for example Graphics.Mode=Windowed; Game.Mod=cnc starts
                         Tiberian Dawn instead of Red Alert

Options:
  --engine-dir <dir>     the directory with mods/ra (OpenRA release-20250330 layout)
  --support-dir <dir>    the directory for settings, content, maps, replays, saves and logs
  --window <w>x<h>       opens a window of this size instead of following Graphics.Mode
  --menu-screenshot <out.webp>
                         renders the menu offscreen in the --window size (default 1280x800)
                         after the --click positions and writes a lossless WebP file
  --click <x>,<y>        a click for --menu-screenshot, repeatable
  --replay <file>        plays back an .orarep replay file
  --viewer               plays the --replay in the debug viewer: arrow keys or WASD scroll, the
                         mouse wheel zooms, space pauses, + and - change the speed, Tab cycles
                         the shroud of each player, Escape quits
  --headless             runs a command without window, graphics or sound; see
                         rustora --headless --help
  -h, --help             shows this help

Replay screenshot options:
  --screenshot <out.webp>  renders one frame of the --replay offscreen to a lossless WebP file
  --tick <n>               the tick of the screenshot (default 0)
  --player <index>         renders with the shroud of this player
  --at <x>,<y>             the center of the screenshot in world pixels (default map center)
  --size <w>x<h>           the screenshot size (default 1280x800)
  --zoom <z>               the screenshot zoom (default 1)

Environment:
  RUSTORA_ALSA_DEVICE      the ALSA output device on Linux (default \"default\")
";

/// The parsed command line of the client.
struct Args {
    engine: Option<PathBuf>,
    support: Option<PathBuf>,
    window: Option<(u32, u32)>,
    replay: Option<String>,
    connect: Option<String>,
    settings: Vec<String>,
    menu_screenshot: Option<String>,
    clicks: Vec<(i32, i32)>,
    viewer: bool,
    screenshot: Option<String>,
    tick: i32,
    player: Option<usize>,
    at: Option<(f32, f32)>,
    size: (u32, u32),
    zoom: f32,
}

fn usage(problem: &str) -> ! {
    eprintln!("{problem}. See rustora --help.");
    std::process::exit(2);
}

fn parse_pair<T: std::str::FromStr>(text: &str, sep: char, what: &str) -> (T, T) {
    text.split_once(sep)
        .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)))
        .unwrap_or_else(|| usage(&format!("invalid {what} {text}")))
}

fn parse_value<T: std::str::FromStr>(text: &str, what: &str) -> T {
    text.parse().unwrap_or_else(|_| usage(&format!("invalid {what} {text}")))
}

fn parse_args(raw: &[String]) -> Args {
    let mut args = Args {
        engine: None,
        support: None,
        window: None,
        replay: None,
        connect: None,
        settings: Vec::new(),
        menu_screenshot: None,
        clicks: Vec::new(),
        viewer: false,
        screenshot: None,
        tick: 0,
        player: None,
        at: None,
        size: (1280, 800),
        zoom: 1.0,
    };
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_else(|| usage(&format!("{a} needs a value")));
        match a.as_str() {
            "--engine-dir" => args.engine = Some(PathBuf::from(value())),
            "--support-dir" => args.support = Some(PathBuf::from(value())),
            "--window" => args.window = Some(parse_pair(&value(), 'x', "window size")),
            "--menu-screenshot" => args.menu_screenshot = Some(value()),
            "--click" => args.clicks.push(parse_pair(&value(), ',', "click position")),
            "--replay" => args.replay = Some(value()),
            "--viewer" => args.viewer = true,
            "--screenshot" => args.screenshot = Some(value()),
            "--tick" => args.tick = parse_value(&value(), "tick"),
            "--player" => args.player = Some(parse_value(&value(), "player index")),
            "--at" => args.at = Some(parse_pair(&value(), ',', "position")),
            "--zoom" => args.zoom = parse_value(&value(), "zoom"),
            "--size" => args.size = parse_pair(&value(), 'x', "size"),
            o if o.starts_with('-') => usage(&format!("unknown option {o}")),
            s if s.contains('=') => match s.split_once('=') {
                Some(("Engine.EngineDir", dir)) => args.engine = Some(PathBuf::from(dir)),
                Some(("Engine.SupportDir", dir)) => args.support = Some(PathBuf::from(dir)),
                _ => args.settings.push(s.to_string()),
            },
            s if s.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok()) => args.connect = Some(s.to_string()),
            s => usage(&format!("{s} is no <host>:<port> address")),
        }
    }
    let needs_replay = args.viewer || args.screenshot.is_some();
    if needs_replay && args.replay.is_none() {
        usage("--viewer and --screenshot need --replay <file>");
    }
    args
}

/// The engine directory of `explicit`, or the one found next to the executable. Exits when it has no RA mod.
fn engine_dir(explicit: Option<PathBuf>) -> PathBuf {
    let engine = explicit.clone().or_else(rustora_fs::Platform::find_engine_dir);
    match engine {
        Some(dir) if dir.join("mods/ra/mod.yaml").is_file() => dir,
        Some(dir) => {
            eprintln!("No RA mod data (mods/ra/mod.yaml) in {}.", dir.display());
            std::process::exit(1);
        }
        None => {
            eprintln!(
                "No RA mod data (mods/ra/mod.yaml) found next to the executable, in ../share/rustora, in \
                 ../Resources, or in a directory beside the executable's parent directories. Run rustora from an \
                 unpacked release archive, or pass --engine-dir <dir> with the mods of an OpenRA release-20250330 \
                 source tree."
            );
            std::process::exit(1);
        }
    }
}

/// The support directory of `explicit`, or the default one of OpenRA, created if missing.
fn support_dir(explicit: Option<PathBuf>, engine: &Path) -> String {
    let support = explicit.unwrap_or_else(|| rustora_fs::Platform::user_support_dir(engine));
    if let Err(e) = std::fs::create_dir_all(&support) {
        eprintln!("cannot create the support directory {}: {e}", support.display());
        std::process::exit(1);
    }
    support.to_string_lossy().into_owned()
}

fn screenshot(game: LoadedGame, args: &Args, out: &str) {
    let LoadedGame {
        world, replay, read_file, ..
    } = game;
    let mut driver = ReplayDriver::new(&world, replay);
    while world.world_tick() < args.tick {
        if driver.step() == Step::Ended {
            break;
        }
    }
    let (device, queue) = rustora_render::headless_device().expect("no graphics adapter");
    let mut renderer = Renderer::new(device, queue, rustora_render::wgpu::TextureFormat::Rgba8Unorm);
    let mut scene = scene::Scene::new(&mut renderer, &world, read_file);
    let (w, h) = args.size;
    let (cx, cy) = args.at.unwrap_or_else(|| app::map_center(&world));
    let cam = Camera {
        x: cx - w as f32 / 2.0 / args.zoom,
        y: cy - h as f32 / 2.0 / args.zoom,
        zoom: args.zoom,
        width: w,
        height: h,
    };
    let viewer = args.player.map(|i| world.players()[i].clone());
    let quads = scene.build(&mut renderer, &world, viewer.as_ref(), &cam, &[]);
    let rgba = rustora_render::render_to_rgba(&mut renderer, cam, &quads);
    let file = std::fs::File::create(out).expect("create screenshot");
    image_webp::WebPEncoder::new(std::io::BufWriter::new(file))
        .encode(&rgba, w, h, image_webp::ColorType::Rgba8)
        .expect("encode webp");
    println!(
        "tick {} with {} quads, camera at {:.0},{:.0}, written to {out}",
        world.world_tick(),
        quads.len(),
        cam.x,
        cam.y
    );
}

fn main() {
    #[cfg(windows)]
    windows_console::attach_parent_console();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if matches!(raw.first().map(String::as_str), Some("--help" | "-h")) {
        print!("{HELP}");
        return;
    }
    if raw.first().map(String::as_str) == Some("--headless") {
        headless::main(std::env::args().take(1).chain(raw[1..].iter().cloned()).collect());
        return;
    }
    let args = parse_args(&raw);
    let engine = engine_dir(args.engine.clone());
    let support = support_dir(args.support.clone(), &engine);
    if let Some(out) = &args.menu_screenshot {
        client::screenshot(&engine, &support, args.window.unwrap_or((1280, 800)), &args.clicks, out);
        return;
    }
    if let Some(replay) = &args.replay
        && (args.viewer || args.screenshot.is_some())
    {
        let game = load_game(&engine, &support, replay);
        match &args.screenshot {
            Some(out) => screenshot(game, &args, out),
            None => app::run(game),
        }
        return;
    }
    let mut settings = args.settings.clone();
    settings.extend(args.replay.iter().map(|r| format!("Launch.Replay={r}")));
    settings.extend(args.connect.iter().map(|c| format!("Launch.Connect={c}")));
    window::run(&engine, &support, args.window, &settings);
}
