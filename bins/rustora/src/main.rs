// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! rustora client. Plays back a replay in a window:
//! `rustora <engine-dir> <support-dir> <replay>`
//! or renders one frame offscreen to a lossless WebP file:
//! `rustora <engine-dir> <support-dir> <replay> --screenshot <out.webp> [--tick N] [--player I] [--at X,Y] [--size WxH] [--zoom Z]`.
//!
//! Window controls: arrow keys or WASD scroll, mouse wheel zooms, space pauses, `+`/`-` change the speed,
//! Tab cycles the shroud of each player and the full view, Escape quits.

mod app;
mod scene;
mod sprites;

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

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!(
            "usage: rustora <engine-dir> <support-dir> <replay> [--screenshot out.webp] [--tick N] [--player I] [--at X,Y] [--size WxH] [--zoom Z]"
        );
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
            o => panic!("unknown option {o}"),
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
    let quads = scene.build(&mut renderer, &world, viewer.as_ref(), &cam);
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
    let args = parse_args();
    let game = load_game(&args.engine, &args.support, &args.replay);
    match args.screenshot.clone() {
        Some(out) => screenshot(game, &args, &out),
        None => app::run(game),
    }
}
