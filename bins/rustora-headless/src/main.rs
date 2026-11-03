// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Headless rustora. Subcommands:
//! `world <engine-dir> <support-dir> <replay> [tick]` prints the sync detail of a replay after world load (or after `tick`).
//! `verify-replay <engine-dir> <support-dir> <replay> [sync-dump]` replays a recorded game and compares the sync hashes.

use std::path::PathBuf;
use std::rc::Rc;

use rustora_engine::order_manager::OrderManager;
use rustora_engine::world::World;
use rustora_launch::{LoadedGame, ReplayDriver, Step, load_game};
use rustora_net::replay::Replay;

/// Type names of an activity and its nested children, like `Fly > Turn`.
fn activity_chain(a: Option<rustora_engine::activity::ActivityRef>) -> String {
    let mut out = Vec::new();
    let mut cur = a;
    while let Some(x) = cur {
        out.push(x.type_name());
        cur = x.base().child_activity();
    }
    out.join(" > ")
}

fn print_detail(world: &World) {
    println!("{} {}", world.world_tick(), world.sync_hash());
    for l in world.sync_detail() {
        println!("{l}");
    }
}

/// Runs the replay up to `max_ticks` and calls `on_tick` after each world tick.
fn run_replay(world: &Rc<World>, replay: Replay, max_ticks: i32, mut on_tick: impl FnMut(&World, i32) -> bool) -> OrderManager {
    let mut driver = ReplayDriver::new(world, replay);
    while world.world_tick() < max_ticks {
        match driver.step() {
            Step::Ticked => {
                if !on_tick(world, world.sync_hash()) {
                    break;
                }
            }
            Step::Waiting => {}
            Step::Ended => {
                println!("replay ended at tick {}", world.world_tick());
                break;
            }
        }
    }
    driver.orders
}

/// Runs the bots of a replay next to the recorded orders and compares their orders and bot random counts
/// with the C# harness dumps (`.orders`, and the third column of `.sync`).
fn verify_bots(world: &Rc<World>, replay: Replay, orders_path: &str, sync_path: Option<&String>) {
    let expected: Vec<String> = std::fs::read_to_string(orders_path).expect("read order dump").lines().map(str::to_string).collect();
    let counts: std::collections::HashMap<i32, i32> = sync_path.map_or_else(Default::default, |p| {
        std::fs::read_to_string(p)
            .expect("read sync dump")
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(' ').collect();
                Some((f.first()?.parse().ok()?, f.get(2)?.parse().ok()?))
            })
            .collect()
    });
    let last_tick = counts.keys().copied().max().unwrap_or(i32::MAX);
    rustora_mods_common::bots::activate_bots(world);
    let issued: Rc<std::cell::RefCell<Vec<String>>> = Rc::default();
    let sink = issued.clone();
    world.set_order_observer(Rc::new(move |w, o| sink.borrow_mut().push(rustora_launch::format_order(w.world_tick(), o))));
    let mut next = 0;
    let mut failure: Option<String> = None;
    run_replay(world, replay, i32::MAX, |w, _| {
        let t = w.world_tick();
        let count = w.bot_random.borrow().total_count;
        if let Some(&c) = counts.get(&t) {
            if c != count {
                failure = Some(format!("bot random count at tick {t}: expected {c}, got {count}"));
                return false;
            }
        }
        if w.world_tick() > last_tick {
            return false;
        }
        w.take_issued_orders();
        for got in issued.borrow_mut().drain(..) {
            match expected.get(next) {
                Some(e) if *e == got => next += 1,
                e => {
                    failure = Some(format!("order {next} differs\n  expected {}\n  got      {got}", e.map_or("<end>", |s| s.as_str())));
                    return false;
                }
            }
        }
        true
    });
    println!("ticks {}, {next} of {} orders match", world.world_tick(), expected.len());
    if let Some(f) = failure {
        println!("{f}");
        std::process::exit(1);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: rustora-headless world|verify-replay <engine-dir> <support-dir> <replay> [...]");
        std::process::exit(2);
    }
    let engine = PathBuf::from(&args[2]);
    let LoadedGame { world, replay, .. } = load_game(&engine, &args[3], &args[4]);
    match args[1].as_str() {
        "world" => {
            let tick: i32 = args.get(5).map_or(0, |t| t.parse().expect("tick"));
            if tick > 0 {
                let trace: Option<u32> = std::env::var("TRACE_ACTOR").ok().and_then(|v| v.parse().ok());
                let trace_frozen: Option<(usize, u32)> = std::env::var("TRACE_FROZEN").ok().and_then(|v| {
                    let (a, b) = v.split_once(':')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                });
                run_replay(&world, replay, tick, |w, _| {
                    if let Some(a) = trace.and_then(|id| w.get_actor_by_id(id)) {
                        let facing = a.trait_or_default::<dyn rustora_engine::traits::IFacing>().map(|f| f.facing().angle);
                        let prone = (a.trait_or_default::<rustora_mods_common::turreted::Turreted>().map(|t| t.is_prone()), a.trait_or_default::<rustora_mods_common::mobile::Mobile>().map(|m| m.movement_speed_for_cell(a.location())));
                        eprintln!("trace {} pos={:?} facing={:?} prone={:?} activity={:?}", w.world_tick(), a.center_position(), facing, prone, activity_chain(a.current_activity()));
                    }
                    if let Some((pi, fid)) = trace_frozen {
                        let p = w.players().into_iter().find(|p| p.player_actor().id as usize == pi).expect("player");
                        if let Some(fa) = p.frozen_actor_layer().and_then(|l| l.from_id(fid)) {
                            let sh = p.shroud();
                            let vis: Vec<u8> = fa.footprint.iter().map(|&c| sh.get_visibility(c)).collect();
                            eprintln!("frozen {} {} visible={} actor_alive={} footprint={:?} cell_visibility={:?}", w.world_tick(), fa.info().name, fa.visible(), fa.actor().is_some(), fa.footprint, vis);
                        }
                    }
                    true
                });
            }
            print_detail(&world);
        }
        "verify-replay" => {
            let dump: Vec<(i32, i32)> = args.get(5).map_or_else(Vec::new, |p| {
                std::fs::read_to_string(p)
                    .expect("read sync dump")
                    .lines()
                    .filter(|l| !l.starts_with('#'))
                    .filter_map(|l| {
                        let mut it = l.split(' ');
                        Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                    })
                    .collect()
            });
            let mut mismatch = None;
            let start = std::time::Instant::now();
            let om = run_replay(&world, replay, i32::MAX, |w, hash| {
                let t = w.world_tick();
                if let Some((_, h)) = dump.iter().find(|(k, _)| *k == t).or(dump.get((t - 1) as usize)) {
                    if *h != hash {
                        mismatch = Some(t);
                        return false;
                    }
                }
                true
            });
            println!("ticks {} in {:?}", world.world_tick(), start.elapsed());
            if let Some(f) = om.out_of_sync {
                println!("recorded sync hash differs at frame {f}");
            }
            match mismatch {
                Some(t) => {
                    println!("first mismatch at tick {t}");
                    std::process::exit(1);
                }
                None => println!("all compared ticks match"),
            }
        }
        "verify-bots" => verify_bots(&world, replay, &args[5], args.get(6)),
        c => panic!("unknown command {c}"),
    }
}
