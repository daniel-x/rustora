// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! rustora client. Plays back a replay in a window:
//! `rustora <engine-dir> <support-dir> <replay>`
//! or renders one frame offscreen to a lossless WebP file:
//! `rustora <engine-dir> <support-dir> <replay> --screenshot <out.webp> [--tick N] [--player I] [--at X,Y] [--size WxH] [--zoom Z]`.
//!
//! Opens the user interface: `rustora --ui <engine-dir> <support-dir> [WxH] [Section.Field=value...]`. Without WxH the
//! window follows `Graphics.Mode` (default borderless fullscreen); WxH opens a window of that size.
//!
//! Window controls: arrow keys or WASD scroll, mouse wheel zooms, space pauses, `+`/`-` change the speed,
//! Tab cycles the shroud of each player and the full view, Escape quits.

mod app;
mod client;
mod headless;
mod net;
mod palettes;
mod scene;
mod script_host;
mod sprites;
mod ui_sprites;
mod window;

use std::path::PathBuf;

use rustora_launch::{LoadedGame, ReplayDriver, Step, load_game};
use rustora_render::{Camera, Renderer};

struct Args {
    engine: PathBuf,
    support: String,
    replay: String,
    screenshot: Option<String>,
    tick: i32,
    player: Option<usize>,
    at: Option<(f32, f32)>,
    size: (u32, u32),
    zoom: f32,
}

/// The synopsis of `rustora --help`.
const HELP: &str = "\
rustora - an OpenRA compatible Red Alert game

Usage:
  rustora [Key=Value]...
  rustora <engine-dir> <support-dir> <replay> [replay options]
  rustora --ui <engine-dir> <support-dir> [WxH] [Key=Value]...
  rustora --ui-screenshot <engine-dir> <support-dir> <out.webp> <WxH> [X,Y]...
  rustora --headless <command> [arguments]...
  rustora --help
  rustora --headless --help

Modes:
  [Key=Value]...  Opens the main menu like the OpenRA launcher. Without arguments it uses the default
                  directories and offers the content download when the game files are missing.
                  Engine.EngineDir=<dir> and Engine.SupportDir=<dir> override the directories, other
                  Section.Field=value arguments override settings, Launch.Connect=<host:port> joins a
                  server, Launch.Replay=<file> plays a replay.
  <replay>        Plays back a replay in a window: arrow keys or WASD scroll, the mouse wheel zooms,
                  space pauses, + and - change the speed, Tab cycles the shroud, Escape quits.
  --ui            Opens the menu with an explicit engine and support directory and window size.
  --ui-screenshot Renders the menu offscreen after the given clicks and writes a WebP file.
  --headless      Runs a command without window, graphics or sound; see rustora --headless --help.

Replay options:
  --screenshot <out.webp>  render one frame offscreen to a lossless WebP file instead of a window
  --tick <n>               the tick of the screenshot (default 0)
  --player <index>         render with the shroud of this player
  --at <x>,<y>             center of the screenshot in world pixels (default map center)
  --size <w>x<h>           screenshot size (default 1280x800)
  --zoom <z>               screenshot zoom (default 1)

Environment:
  RUSTORA_ALSA_DEVICE      the ALSA output device on Linux (default \"default\")
";

fn print_help() {
    print!("{HELP}");
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprint!("{HELP}");
        std::process::exit(2);
    }
    let mut args = Args {
        engine: PathBuf::from(&a[1]),
        support: a[2].clone(),
        replay: a[3].clone(),
        screenshot: None,
        tick: 0,
        player: None,
        at: None,
        size: (1280, 800),
        zoom: 1.0,
    };
    let mut i = 4;
    while i + 1 < a.len() {
        let v = &a[i + 1];
        match a[i].as_str() {
            "--screenshot" => args.screenshot = Some(v.clone()),
            "--tick" => args.tick = v.parse().expect("tick"),
            "--player" => args.player = Some(v.parse().expect("player index")),
            "--at" => {
                let (x, y) = v.split_once(',').expect("X,Y");
                args.at = Some((x.parse().expect("x"), y.parse().expect("y")));
            }
            "--zoom" => args.zoom = v.parse().expect("zoom"),
            "--size" => {
                let (w, h) = v.split_once('x').expect("WxH");
                args.size = (w.parse().expect("width"), h.parse().expect("height"));
            }
            o => {
                eprintln!("unknown option {o}\n");
                eprint!("{HELP}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    args
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

/// `rustora --ui-screenshot <engine-dir> <support-dir> <out.webp> <WxH> [X,Y clicks]`
fn ui_screenshot(a: &[String]) {
    let (w, h) = a[3].split_once('x').expect("WxH");
    let size = (w.parse().expect("width"), h.parse().expect("height"));
    let clicks: Vec<(i32, i32)> = a[4..]
        .iter()
        .map(|c| {
            let (x, y) = c.split_once(',').expect("X,Y");
            (x.parse().expect("x"), y.parse().expect("y"))
        })
        .collect();
    client::screenshot(std::path::Path::new(&a[0]), &a[1], size, &clicks, &a[2]);
}

/// `rustora --ui <engine-dir> <support-dir> [WxH] [Section.Field=value...]`
fn ui_window(a: &[String]) {
    let (settings, sizes): (Vec<String>, Vec<String>) = a[2..].iter().cloned().partition(|s| s.contains('='));
    let size = sizes.first().map(|s| {
        let (w, h) = s.split_once('x').expect("WxH");
        (w.parse().expect("width"), h.parse().expect("height"))
    });
    window::run(std::path::Path::new(&a[0]), &a[1], size, &settings);
}

/// Runs the menu like the OpenRA launcher: `rustora [Key=Value]...`, where `Engine.EngineDir` and
/// `Engine.SupportDir` override the directories.
fn launch(settings: &[String]) {
    let value = |k: &str| {
        settings
            .iter()
            .find_map(|s| s.strip_prefix(k)?.strip_prefix('='))
            .map(PathBuf::from)
    };
    let explicit = value("Engine.EngineDir");
    let engine = explicit.clone().unwrap_or_else(rustora_fs::Platform::default_engine_dir);
    if !engine.join("mods/ra/mod.yaml").is_file() {
        match explicit {
            Some(dir) => eprintln!("No RA mod data (mods/ra/mod.yaml) in Engine.EngineDir={}.", dir.display()),
            None => {
                eprintln!("No RA mod data (mods/ra/mod.yaml) found. Searched:");
                for d in rustora_fs::Platform::engine_dir_candidates() {
                    eprintln!("  {}", d.display());
                }
            }
        }
        eprintln!(
            "Run rustora from an unpacked release archive, or pass Engine.EngineDir=<dir> with the mods of an OpenRA \
             release-20250330 source tree. See rustora --help."
        );
        std::process::exit(1);
    }
    let support = value("Engine.SupportDir").unwrap_or_else(|| rustora_fs::Platform::user_support_dir(&engine));
    if let Err(e) = std::fs::create_dir_all(&support) {
        eprintln!("cannot create the support directory {}: {e}", support.display());
        std::process::exit(1);
    }
    window::run(&engine, &support.to_string_lossy(), None, settings);
}

fn main() {
    let raw: Vec<String> = std::env::args().collect();
    if matches!(raw.get(1).map(String::as_str), Some("--help" | "-h")) {
        print_help();
        return;
    }
    if raw.get(1).map(String::as_str) == Some("--headless") {
        headless::main(std::iter::once(raw[0].clone()).chain(raw[2..].iter().cloned()).collect());
        return;
    }
    if raw[1..].iter().all(|a| a.contains('=')) {
        launch(&raw[1..]);
        return;
    }
    if raw.get(1).map(String::as_str) == Some("--ui") {
        ui_window(&raw[2..]);
        return;
    }
    if raw.get(1).map(String::as_str) == Some("--ui-screenshot") {
        ui_screenshot(&raw[2..]);
        return;
    }
    let args = parse_args();
    let game = load_game(&args.engine, &args.support, &args.replay);
    match args.screenshot.clone() {
        Some(out) => screenshot(game, &args, &out),
        None => app::run(game),
    }
}
