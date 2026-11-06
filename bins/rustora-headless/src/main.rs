// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Headless rustora. Subcommands:
//! `world <engine-dir> <support-dir> <replay> [tick]` prints the sync detail of a replay after world load (or after `tick`).
//! `EXTRA_STEPS=n` runs n more replay steps after the last tick, e.g. to process the orders of a paused frame.
//! `verify-replay <engine-dir> <support-dir> <replay> [sync-dump]` replays a recorded game and compares the sync hashes.
//! `verify-bots <engine-dir> <support-dir> <replay> <order-dump> [sync-dump]` compares the Rust bots with a C# bot game.
//! `skirmish <engine-dir> <support-dir> <map> <bots> <ticks> <seed> [sync-out] [cs-sync-dump]` hosts a bot game,
//! writes `tick hash botrandomcount` per tick and compares all three columns with a C# sync dump.
//! `map-uid <engine-dir> <map>` prints the uid of a map, given by uid or file name.
//! `join <engine-dir> <support-dir> <host:port> <ticks> [--bots <spec>] [--wait-clients <n>] [--sync-out <path>] [--name <text>]`
//! joins a server. With bots it hosts them like the C# `Harness.Bots`, without them it spectates.
//! `--save-at <tick>:<name>` requests a game save after that world tick, `--load-save <name>` loads a save.
//! `lan-probe <seconds> [host:port]` lists the LAN games that answer, broadcasting unless a beacon address is given.
//! `server-list <mod-version>` lists the RA games of the master server.

use std::path::PathBuf;
use std::rc::Rc;

use rustora_engine::order_manager::OrderManager;
use rustora_engine::world::World;
use rustora_launch::{LoadedGame, ReplayDriver, Step, load_game};
use rustora_net::replay::Replay;

/// Type names of an activity and its nested children with their targets, like the C# `Harness.TraceActor` output:
/// `Move[1024,2048,0] > Turn`.
fn activity_chain(self_: &rustora_engine::actor::ActorRef, a: Option<rustora_engine::activity::ActivityRef>) -> String {
    use rustora_engine::target::TargetType;
    let mut out = Vec::new();
    let mut cur = a;
    while let Some(x) = cur {
        let targets: Vec<String> = x
            .get_targets(self_)
            .iter()
            .map(|t| match t.target_type() {
                TargetType::Actor => format!("a{}", t.actor().expect("actor").id),
                TargetType::Invalid => "invalid".to_string(),
                _ => {
                    let p = t.center_position();
                    format!("{},{},{}", p.x, p.y, p.z)
                }
            })
            .collect();
        if targets.is_empty() {
            out.push(x.type_name().to_string());
        } else {
            out.push(format!("{}[{}]", x.type_name(), targets.join(",")));
        }
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

/// Runs the replay up to `max_ticks` and calls `on_tick` after each world tick. `PROGRESS=n` prints the time and
/// actor count of every n ticks.
fn run_replay(world: &Rc<World>, replay: Replay, max_ticks: i32, mut on_tick: impl FnMut(&World, i32) -> bool) -> OrderManager {
    let mut driver = ReplayDriver::new(world, replay);
    let progress: i32 = std::env::var("PROGRESS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut last = std::time::Instant::now();
    while world.world_tick() < max_ticks && driver.orders.out_of_sync.is_none() {
        match driver.step() {
            Step::Ticked => {
                if progress > 0 && world.world_tick() % progress == 0 {
                    println!(
                        "progress tick {} {:.3}s actors {}",
                        world.world_tick(),
                        last.elapsed().as_secs_f64(),
                        world.actors().len()
                    );
                    last = std::time::Instant::now();
                }
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
    let extra_steps: u32 = std::env::var("EXTRA_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for _ in 0..extra_steps {
        driver.step();
    }
    driver.orders
}

/// Runs the bots of a replay next to the recorded orders and compares their orders and bot random counts
/// with the C# harness dumps (`.orders`, and the third column of `.sync`).
/// The dump's `PauseGame` orders come from the C# client at game over, not from bots, so they are skipped.
fn verify_bots(world: &Rc<World>, replay: Replay, orders_path: &str, sync_path: Option<&String>) {
    let expected: Vec<String> = std::fs::read_to_string(orders_path)
        .expect("read order dump")
        .lines()
        .filter(|l| l.split(' ').nth(1) != Some("PauseGame"))
        .map(str::to_string)
        .collect();
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
    world.set_order_observer(Rc::new(move |w, o| {
        sink.borrow_mut().push(rustora_launch::format_order(w.world_tick(), o))
    }));
    let mut next = 0;
    let mut failure: Option<String> = None;
    run_replay(world, replay, i32::MAX, |w, _| {
        let t = w.world_tick();
        let count = w.bot_random.borrow().total_count;
        if let Some(&c) = counts.get(&t)
            && c != count
        {
            failure = Some(format!("bot random count at tick {t}: expected {c}, got {count}"));
            return false;
        }
        if w.world_tick() > last_tick {
            return false;
        }
        w.take_issued_orders();
        for got in issued.borrow_mut().drain(..) {
            match expected.get(next) {
                Some(e) if *e == got => next += 1,
                e => {
                    failure = Some(format!(
                        "order {next} differs\n  expected {}\n  got      {got}",
                        e.map_or("<end>", |s| s.as_str())
                    ));
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

/// Reads `tick hash botrandomcount` lines of a C# sync dump, skipping detail lines.
fn read_sync_dump(path: &str) -> std::collections::HashMap<i32, String> {
    std::fs::read_to_string(path)
        .expect("read sync dump")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| Some((l.split(' ').next()?.parse().ok()?, l.to_string())))
        .collect()
}

fn skirmish(args: &[String]) {
    use rustora_launch::skirmish::{SkirmishDriver, parse_bots, skirmish_session};
    let [engine, support, map, bots, ticks, seed] = &args[2..8] else {
        panic!("usage: rustora-headless skirmish <engine-dir> <support-dir> <map> <bots> <ticks> <seed> [sync-out] [cs-sync-dump]")
    };
    let engine = PathBuf::from(engine);
    let (ticks, seed): (i32, i32) = (ticks.parse().expect("ticks"), seed.parse().expect("seed"));
    let pkg = rustora_launch::open_map(&engine, map).expect("map not found");
    let map_package = rustora_game::map::MapPackage::load(pkg.clone()).expect("load map");
    let lobby = skirmish_session(&map_package, &parse_bots(bots).expect("bots"), seed).expect("lobby");
    let (world, _, _) = rustora_launch::load_world(&engine, support, pkg, lobby, false);
    let mut out = args
        .get(8)
        .map(|p| std::io::BufWriter::new(std::fs::File::create(p).expect("create sync dump")));
    let expected = args.get(9).map(|p| read_sync_dump(p));
    let start = std::time::Instant::now();
    let mut driver = SkirmishDriver::new(&world);
    let mut mismatch = None;
    while world.world_tick() < ticks {
        match driver.step() {
            Step::Ticked => {
                let line = format!(
                    "{} {} {}",
                    world.world_tick(),
                    world.sync_hash(),
                    world.bot_random.borrow().total_count
                );
                if let Some(o) = out.as_mut() {
                    use std::io::Write;
                    writeln!(o, "{line}").expect("write sync dump");
                }
                if let Some(e) = expected.as_ref().and_then(|e| e.get(&world.world_tick()))
                    && *e != line
                {
                    mismatch = Some(format!(
                        "first mismatch at tick {}\n  expected {e}\n  got      {line}",
                        world.world_tick()
                    ));
                    break;
                }
            }
            Step::Waiting => {}
            Step::Ended => break,
        }
    }
    println!(
        "ticks {} in {:?}, game over: {}",
        world.world_tick(),
        start.elapsed(),
        world.is_game_over()
    );
    if let Some(e) = &expected {
        let last = e.keys().copied().max().unwrap_or(0);
        match mismatch {
            Some(m) => {
                println!("{m}");
                std::process::exit(1);
            }
            None if world.world_tick() < last => {
                println!("stopped before the C# dump ends at tick {last}");
                std::process::exit(1);
            }
            None => println!("all {last} ticks of the C# dump match"),
        }
    }
}

/// Joins a server, runs up to `ticks` world ticks and writes `tick hash botrandomcount` per tick.
fn join(args: &[String]) {
    use rustora_launch::net_client::{JoinOptions, join_game};
    use rustora_launch::skirmish::{SkirmishDriver, parse_bots};
    let [engine, support, address, ticks] = &args[2..6] else {
        panic!(
            "usage: rustora-headless join <engine-dir> <support-dir> <host:port> <ticks> [--bots <spec>] [--wait-clients <n>] [--sync-out <path>] [--name <text>] [--save-at <tick>:<name>] [--load-save <name>]"
        )
    };
    let engine = PathBuf::from(engine);
    let ticks: i32 = ticks.parse().expect("ticks");
    let mut options = JoinOptions::default();
    let mut sync_out = None;
    let mut save_at: Option<(i32, String)> = None;
    for pair in args[6..].chunks(2) {
        let value = pair.get(1).expect("option value");
        match pair[0].as_str() {
            "--bots" => options.bots = parse_bots(value).expect("bots"),
            "--wait-clients" => options.wait_clients = value.parse().expect("wait clients"),
            "--sync-out" => sync_out = Some(std::io::BufWriter::new(std::fs::File::create(value).expect("create sync dump"))),
            "--name" => options.name = value.clone(),
            "--load-save" => options.load_save = Some(value.clone()),
            "--save-at" => {
                let (tick, name) = value.split_once(':').expect("--save-at <tick>:<name>");
                save_at = Some((tick.parse().expect("save tick"), name.to_string()));
            }
            o => panic!("unknown option {o}"),
        }
    }
    let game = join_game(&engine, support, address, &options).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let world = game.world.clone();
    let mut driver = SkirmishDriver::with_connection(&world, Box::new(game.connection), game.is_host);
    while world.world_tick() < ticks {
        match driver.step() {
            Step::Ticked => {
                if let Some(o) = sync_out.as_mut() {
                    use std::io::Write;
                    writeln!(
                        o,
                        "{} {} {}",
                        world.world_tick(),
                        world.sync_hash(),
                        world.bot_random.borrow().total_count
                    )
                    .expect("write sync dump");
                }
                if let Some((_, name)) = save_at.as_ref().filter(|(tick, _)| *tick == world.world_tick()) {
                    for order in world.request_game_save(name) {
                        driver.orders.issue_order(order);
                    }
                }
            }
            Step::Waiting => std::thread::sleep(std::time::Duration::from_micros(200)),
            Step::Ended => break,
        }
    }
    println!("ticks {}, game over: {}", world.world_tick(), world.is_game_over());
    if let Some(frame) = driver.orders.out_of_sync {
        println!("out of sync at frame {frame}");
        std::process::exit(1);
    }
}

fn print_game(g: &rustora_net::game_server::GameServer) {
    let text = |s: &Option<String>| s.clone().unwrap_or_default();
    println!(
        "{} {} state={} map={} {}@{} players={} bots={} spectators={} clients={}",
        text(&g.address),
        text(&g.name),
        g.state,
        text(&g.map),
        text(&g.mod_id),
        text(&g.version),
        g.players,
        g.bots,
        g.spectators,
        g.clients.len()
    );
}

/// Waits `seconds` for LAN beacons, then prints their games.
fn lan_probe(seconds: u64, target: Option<&String>) {
    use rustora_net::beacon::{LAN_GAME_BEACON, Probe};
    let probe = match target {
        Some(t) => Probe::start_to(LAN_GAME_BEACON, t.parse().expect("beacon address")),
        None => Probe::start(LAN_GAME_BEACON),
    }
    .expect("start probe");
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    for b in probe.locations() {
        match rustora_net::game_server::parse_lan_game(&b.data, b.address.ip()) {
            Some(g) => print_game(&g),
            None => println!("{} answered with unreadable data: {:?}", b.address, b.data),
        }
    }
}

fn server_list(version: &str) {
    use rustora_net::game_server::{parse_game_list, server_list_url};
    let url = server_list_url(rustora_net::web::SERVER_LIST_URL, version, "ra", version);
    let games = parse_game_list(&rustora_net::web::get(&url).expect("query")).expect("parse");
    println!("{} games", games.len());
    games.iter().take(5).for_each(print_game);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("skirmish") => return skirmish(&args),
        Some("join") => return join(&args),
        Some("lan-probe") if args.len() >= 3 => return lan_probe(args[2].parse().expect("seconds"), args.get(3)),
        Some("server-list") if args.len() == 3 => return server_list(&args[2]),
        Some("map-uid") if args.len() == 4 => {
            let pkg = rustora_launch::open_map(&PathBuf::from(&args[2]), &args[3]).expect("map not found");
            println!("{}", rustora_game::map::MapPackage::load(pkg).expect("load map").uid);
            return;
        }
        _ => {}
    }
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
                if let Some((a, b)) = std::env::var("TRACE_RANDOM").ok().and_then(|v| {
                    let (a, b) = v.split_once(',')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                }) {
                    world.shared_random.borrow_mut().trace = Some((a, b));
                }
                let trace: Option<u32> = std::env::var("TRACE_ACTOR").ok().and_then(|v| v.parse().ok());
                let trace_frozen: Option<(usize, u32)> = std::env::var("TRACE_FROZEN").ok().and_then(|v| {
                    let (a, b) = v.split_once(':')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                });
                let trace_cell: Option<rustora_core::world_types::CPos> = std::env::var("TRACE_CELL").ok().and_then(|v| {
                    let (x, y) = v.split_once(',')?;
                    Some(rustora_core::world_types::CPos::new(x.parse().ok()?, y.parse().ok()?))
                });
                run_replay(&world, replay, tick, |w, _| {
                    if let Some(c) = trace_cell {
                        use rustora_mods_common::actor_map::WorldExt;
                        let actors: Vec<String> = w
                            .actor_map()
                            .get_actors_at(c)
                            .iter()
                            .map(|a| format!("{}:{}@{:?}", a.id, a.info.name, a.center_position()))
                            .collect();
                        eprintln!("cell {} {:?}", w.world_tick(), actors);
                    }
                    if let Some(a) = trace.and_then(|id| w.get_actor_by_id(id)) {
                        let facing = a
                            .trait_or_default::<dyn rustora_engine::traits::IFacing>()
                            .map(|f| f.facing().angle);
                        let prone = (
                            a.trait_or_default::<rustora_mods_common::turreted::Turreted>()
                                .map(|t| t.is_prone()),
                            a.trait_or_default::<rustora_mods_common::mobile::Mobile>()
                                .map(|m| m.movement_speed_for_cell(a.location())),
                        );
                        eprintln!(
                            "trace {} pos={:?} facing={:?} prone={:?} activity={:?} scan={:?}",
                            w.world_tick(),
                            a.center_position(),
                            facing,
                            prone,
                            activity_chain(&a, a.current_activity()),
                            a.trait_or_default::<rustora_mods_common::auto_target::AutoTarget>()
                                .map(|t| t.next_scan_time())
                        );
                    }
                    if let Some((pi, fid)) = trace_frozen {
                        let p = w
                            .players()
                            .into_iter()
                            .find(|p| p.player_actor().id as usize == pi)
                            .expect("player");
                        if let Some(fa) = p.frozen_actor_layer().and_then(|l| l.from_id(fid)) {
                            let sh = p.shroud();
                            let vis: Vec<u8> = fa.footprint.iter().map(|&c| sh.get_visibility(c)).collect();
                            eprintln!(
                                "frozen {} {} owner={:?} types={:?} visible={} actor_alive={} footprint={:?} cell_visibility={:?}",
                                w.world_tick(),
                                fa.info().name,
                                fa.owner().map(|o| o.internal_name.clone()),
                                fa.target_types(),
                                fa.visible(),
                                fa.actor().is_some(),
                                fa.footprint,
                                vis
                            );
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
                if let Some((_, h)) = dump.iter().find(|(k, _)| *k == t).or(dump.get((t - 1) as usize))
                    && *h != hash
                {
                    mismatch = Some(t);
                    return false;
                }
                true
            });
            println!("ticks {} in {:?}", world.world_tick(), start.elapsed());
            if let Some(f) = om.out_of_sync {
                println!("recorded sync hash differs at frame {f}: {:?}", om.out_of_sync_hashes);
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
