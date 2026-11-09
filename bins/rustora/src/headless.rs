// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `rustora --headless <command> ...`: replay verification and test games without window, graphics or sound.
//! [`HELP`] lists the commands.

use std::path::PathBuf;
use std::rc::Rc;

use rustora_client::replay::Replay;
use rustora_engine::order_manager::OrderManager;
use rustora_engine::world::World;
use rustora_launch::{LoadedGame, ReplayDriver, Step, load_game};

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
fn run_replay(world: &Rc<World>, replay: Replay, max_ticks: i32, on_tick: impl FnMut(&World, i32) -> bool) -> OrderManager {
    run_driver(world, ReplayDriver::new(world, replay), max_ticks, on_tick)
}

/// [`run_replay`] with a given driver, such as one resumed from a save game.
fn run_driver(world: &Rc<World>, mut driver: ReplayDriver, max_ticks: i32, mut on_tick: impl FnMut(&World, i32) -> bool) -> OrderManager {
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
        panic!("usage: rustora --headless skirmish <engine-dir> <support-dir> <map> <bots> <ticks> <seed> [sync-out] [cs-sync-dump]")
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
            "usage: rustora --headless join <engine-dir> <support-dir> <host:port> <ticks> [--bots <spec>] [--wait-clients <n>] [--sync-out <path>] [--name <text>] [--save-at <tick>:<name>] [--load-save <name>]"
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

/// The synopsis of `rustora --headless --help`.
const HELP: &str = "\
rustora --headless - replay verification and test games without window, graphics or sound

Usage:
  rustora --headless verify-replay <engine-dir> <support-dir> <replay> [sync-dump] [save options]
  rustora --headless verify-save <engine-dir> <support-dir> <replay> <tick> [ticks]
  rustora --headless world <engine-dir> <support-dir> <replay> [tick] [--from-save <file>]
  rustora --headless verify-bots <engine-dir> <support-dir> <replay> <order-dump> [sync-dump]
  rustora --headless skirmish <engine-dir> <support-dir> <map> <bots> <ticks> <seed> [sync-out] [cs-sync-dump]
  rustora --headless join <engine-dir> <support-dir> <host:port> <ticks> [join options]
  rustora --headless map-uid <engine-dir> <map>
  rustora --headless lan-probe <seconds> [host:port]
  rustora --headless server-list <mod-version>
  rustora --headless --help

Commands:
  verify-replay  Replays a recorded game, compares its recorded sync hashes and, with a sync dump of
                 the C# reference (lines \"tick hash\"), every tick's hash.
  verify-save    Replays to <tick>, saves and restores the world, then replays both worlds [ticks]
                 more ticks (default 100) and compares their sync hashes and sync details.
  world          Prints the per-actor and per-trait sync detail after world load or after <tick>.
  verify-bots    Replays a C# bot game and compares the orders of the Rust bots with the C# order dump.
  skirmish       Hosts a bot game, writes \"tick hash botrandomcount\" per tick to <sync-out> and
                 compares all three columns with a C# sync dump. <bots> is Slot:BotType:Faction[:Spawn],...
  join           Joins a server. With --bots it hosts the bots like the C# Harness.Bots, without them it
                 spectates.
  map-uid        Prints the uid of a map, given by uid or file name.
  lan-probe      Lists the LAN games that answer within <seconds>, broadcasting unless a beacon address
                 is given.
  server-list    Lists the RA games of the master server for an engine version.

Save options of verify-replay:
  --save-every-ticks <n>  write a save game after every world tick that is a multiple of n
  --save-every-secs <s>   write a save game after a tick when s seconds passed since the last one
  --save-dir <dir>        where the save games go, as <replay-name>-<tick>.rsave (default: .)
  --from-save <file>      continue the replay from a save game of it instead of its start

Join options:
  --bots <spec>         bots to host, as for skirmish
  --wait-clients <n>    wait for n clients before starting
  --sync-out <path>     write \"tick hash\" per tick
  --name <text>         the player name
  --save-at <tick>:<name>  request a game save after that world tick
  --load-save <name>    load a game save

Environment:
  PROGRESS=<n>          replay commands: print the seconds per <n> ticks and the actor count
  EXTRA_STEPS=<n>       world: run n more replay steps after the last tick
  TRACE_ACTOR=<id>      world: print position, facing and activity chain of an actor each tick
  TRACE_CELL=<x>,<y>    world: print the actors in a cell each tick
  TRACE_FROZEN=<player-actor-id>:<frozen-id>
                        world: print the state of a frozen actor of a player each tick
  TRACE_RANDOM=<from>,<to>
                        world: print a backtrace for each shared random call in that range
";

/// Runs a headless command. `args[0]` is the program name and `args[1]` the command, as in a separate binary.
pub fn main(args: Vec<String>) {
    match args.get(1).map(String::as_str) {
        None | Some("--help" | "-h") => {
            print!("{HELP}");
            return;
        }
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
        eprintln!("unknown command or missing arguments: {}\n", args[1..].join(" "));
        eprint!("{HELP}");
        std::process::exit(2);
    }
    let engine = PathBuf::from(&args[2]);
    let LoadedGame { world, replay, .. } = load_game(&engine, &args[3], &args[4]);
    match args[1].as_str() {
        "world" => {
            let options = SaveOptions::parse(&args[5..]);
            let tick: i32 = options.positional.first().map_or(0, |t| t.parse().expect("tick"));
            let (world, driver) = match &options.from_save {
                None => (world.clone(), ReplayDriver::new(&world, replay)),
                Some(path) => {
                    let replay = Replay::parse(&std::fs::read(&args[4]).expect("read replay")).expect("parse replay");
                    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read save game {path}: {e}"));
                    let (w, _, _) = rustora_launch::restore_game(&engine, &args[3], &data, true, false)
                        .unwrap_or_else(|e| panic!("restore {path}: {e}"));
                    (w.clone(), ReplayDriver::resumed(&w, replay))
                }
            };
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
                run_driver(&world, driver, tick, |w, _| {
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
            let options = SaveOptions::parse(&args[5..]);
            let dump: Vec<(i32, i32)> = options.positional.first().map_or_else(Vec::new, |p| {
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
            let recorded = recorded_syncs(&replay);
            let (world, driver) = match &options.from_save {
                None => (world.clone(), ReplayDriver::new(&world, replay)),
                Some(path) => {
                    let replay = Replay::parse(&std::fs::read(&args[4]).expect("read replay")).expect("parse replay");
                    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read save game {path}: {e}"));
                    let (w, _, _) = rustora_launch::restore_game(&engine, &args[3], &data, true, false)
                        .unwrap_or_else(|e| panic!("restore {path}: {e}"));
                    println!("restored {path} at tick {}", w.world_tick());
                    (w.clone(), ReplayDriver::resumed(&w, replay))
                }
            };
            let stem = std::path::Path::new(&args[4])
                .file_stem()
                .map_or_else(|| "replay".to_string(), |s| s.to_string_lossy().into_owned());
            let mut saver = options.saver(&stem);
            let om = run_driver(&world, driver, i32::MAX, |w, hash| {
                let t = w.world_tick();
                if let Some((_, h)) = dump.iter().find(|(k, _)| *k == t).or(dump.get((t - 1) as usize))
                    && *h != hash
                {
                    mismatch = Some(t);
                    return false;
                }
                saver.after_tick(&world);
                true
            });
            println!("ticks {} in {:?}", world.world_tick(), start.elapsed());
            if let Some(f) = om.out_of_sync {
                let clients = recorded.get(&f).cloned().unwrap_or_default();
                let distinct: std::collections::BTreeSet<_> = clients.values().collect();
                if distinct.len() > 1 {
                    println!("recorded clients disagree at frame {f}: {clients:?}");
                }
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
        "verify-save" => {
            let tick: i32 = args.get(5).expect("tick").parse().expect("tick");
            let after: i32 = args.get(6).map_or(100, |t| t.parse().expect("ticks"));
            verify_save(&engine, &args[3], &args[4], world, replay, tick, after);
        }
        c => {
            eprintln!("unknown command {c}\n");
            eprint!("{HELP}");
            std::process::exit(2);
        }
    }
}

/// The save game options of `verify-replay`.
struct SaveOptions {
    positional: Vec<String>,
    every_ticks: Option<i32>,
    every_secs: Option<f64>,
    dir: PathBuf,
    from_save: Option<String>,
}

impl SaveOptions {
    fn parse(args: &[String]) -> SaveOptions {
        let mut o = SaveOptions {
            positional: Vec::new(),
            every_ticks: None,
            every_secs: None,
            dir: PathBuf::from("."),
            from_save: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = |name: &str| it.next().unwrap_or_else(|| panic!("{name} needs a value")).clone();
            match a.as_str() {
                "--save-every-ticks" => o.every_ticks = Some(value(a).parse().expect("ticks")),
                "--save-every-secs" => o.every_secs = Some(value(a).parse().expect("seconds")),
                "--save-dir" => o.dir = PathBuf::from(value(a)),
                "--from-save" => o.from_save = Some(value(a)),
                _ if a.starts_with("--") => panic!("unknown option {a}"),
                _ => o.positional.push(a.clone()),
            }
        }
        o
    }

    fn saver(&self, stem: &str) -> Saver {
        if self.every_ticks.is_some() || self.every_secs.is_some() {
            std::fs::create_dir_all(&self.dir).unwrap_or_else(|e| panic!("create {}: {e}", self.dir.display()));
        }
        Saver {
            every_ticks: self.every_ticks,
            every_secs: self.every_secs,
            dir: self.dir.clone(),
            stem: stem.to_string(),
            last: std::time::Instant::now(),
        }
    }
}

/// Writes save games at tick or wall clock time intervals while a replay runs.
struct Saver {
    every_ticks: Option<i32>,
    every_secs: Option<f64>,
    dir: PathBuf,
    stem: String,
    last: std::time::Instant,
}

impl Saver {
    fn after_tick(&mut self, world: &Rc<World>) {
        let t = world.world_tick();
        let by_ticks = self.every_ticks.is_some_and(|n| n > 0 && t % n == 0);
        let by_time = self.every_secs.is_some_and(|s| self.last.elapsed().as_secs_f64() >= s);
        if !by_ticks && !by_time {
            return;
        }
        self.last = std::time::Instant::now();
        let path = self.dir.join(format!("{}-{t}.rsave", self.stem));
        match rustora_launch::save_game(world) {
            Ok(data) => match std::fs::write(&path, &data) {
                Ok(()) => println!("saved {} ({} bytes)", path.display(), data.len()),
                Err(e) => eprintln!("save game {}: {e}", path.display()),
            },
            Err(e) => eprintln!("save game at tick {t}: {e}"),
        }
    }
}

/// Runs the replay to `tick`, saves and restores the world, then runs both worlds `after` more ticks and compares
/// their sync hashes and sync details each tick.
fn verify_save(engine: &std::path::Path, support: &str, replay_path: &str, world: Rc<World>, replay: Replay, tick: i32, after: i32) {
    let mut a = ReplayDriver::new(&world, replay);
    while world.world_tick() < tick {
        if a.step() == Step::Ended {
            println!("replay ended at tick {}", world.world_tick());
            std::process::exit(1);
        }
    }
    let start = std::time::Instant::now();
    let data = rustora_launch::save_game(&world).unwrap_or_else(|e| {
        println!("save failed: {e}");
        std::process::exit(1)
    });
    let saved = start.elapsed();
    let start = std::time::Instant::now();
    let (restored, _, _) = rustora_launch::restore_game(engine, support, &data, true, false).unwrap_or_else(|e| {
        println!("restore failed: {e}");
        std::process::exit(1)
    });
    println!(
        "tick {tick}: {} bytes, saved in {saved:?}, restored in {:?}",
        data.len(),
        start.elapsed()
    );
    match rustora_launch::save_game(&restored) {
        Ok(again) if again == data => {}
        Ok(again) => {
            let at = data
                .iter()
                .zip(&again)
                .position(|(x, y)| x != y)
                .unwrap_or(data.len().min(again.len()));
            println!(
                "the save of the restored world differs at byte {at} ({} / {} bytes)",
                data.len(),
                again.len()
            );
        }
        Err(e) => println!("saving the restored world failed: {e}"),
    }
    let second = Replay::parse(&std::fs::read(replay_path).expect("read replay")).expect("parse replay");
    let mut b = ReplayDriver::resumed(&restored, second);
    if !compare_worlds(&world, &restored) {
        std::process::exit(1);
    }
    while world.world_tick() < tick + after {
        let (sa, sb) = (a.step(), b.step());
        if sa != sb {
            println!(
                "tick {}: the original world steps {sa:?}, the restored one {sb:?}",
                world.world_tick()
            );
            std::process::exit(1);
        }
        if sa == Step::Ended {
            break;
        }
        if sa == Step::Ticked && !compare_worlds(&world, &restored) {
            std::process::exit(1);
        }
    }
    println!("ticks {tick} to {}: the restored world matches", world.world_tick());
}

/// Compares the tick, sync hash and saved state of two worlds and prints the first differences.
fn compare_worlds(a: &Rc<World>, b: &Rc<World>) -> bool {
    let (la, lb) = (a.local_random.borrow().total_count, b.local_random.borrow().total_count);
    if la != lb {
        println!("tick {}: local random draws {la} / {lb}", a.world_tick());
    }
    let (ba, bb) = (
        rustora_engine::savegame::debug_blocks(a).expect("save blocks"),
        rustora_engine::savegame::debug_blocks(b).expect("save blocks"),
    );
    if let Some(((la, da), (lb, db))) = ba.iter().zip(&bb).find(|((la, da), (lb, db))| la != lb || da != db) {
        println!("tick {}: the saved state differs first in {la} / {lb}", a.world_tick());
        let at = da.iter().zip(db).position(|(x, y)| x != y).unwrap_or(da.len().min(db.len()));
        println!("  {} / {} bytes, first difference at byte {at}", da.len(), db.len());
        let show = |d: &[u8]| {
            d[at.saturating_sub(8)..(at + 24).min(d.len())]
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("  original: {}", show(da));
        println!("  restored: {}", show(db));
        if let Some(id) = la
            .strip_prefix("actor ")
            .and_then(|s| s.split(' ').next())
            .and_then(|s| s.parse().ok())
            && let (Some(x), Some(y)) = (a.get_actor_by_id(id), b.get_actor_by_id(id))
        {
            println!("  original activities: {}", activity_chain(&x, x.current_activity()));
            println!("  restored activities: {}", activity_chain(&y, y.current_activity()));
        }
        return false;
    }
    if ba.len() != bb.len() {
        println!("tick {}: {} saved blocks differ from {}", a.world_tick(), ba.len(), bb.len());
        return false;
    }
    if a.world_tick() == b.world_tick() && a.sync_hash() == b.sync_hash() {
        return true;
    }
    println!(
        "tick {} / {}: sync hash {} differs from {}",
        a.world_tick(),
        b.world_tick(),
        a.sync_hash(),
        b.sync_hash()
    );
    let (da, db) = (a.sync_detail(), b.sync_detail());
    let mut shown = 0;
    for l in da.iter().filter(|l| !db.contains(l)) {
        println!("  original: {l}");
        shown += 1;
        if shown >= 20 {
            break;
        }
    }
    shown = 0;
    for l in db.iter().filter(|l| !da.contains(l)) {
        println!("  restored: {l}");
        shown += 1;
        if shown >= 20 {
            break;
        }
    }
    false
}

/// The recorded sync packets of each frame, as client id to (sync hash, defeat state).
fn recorded_syncs(replay: &rustora_client::replay::Replay) -> std::collections::HashMap<i32, std::collections::BTreeMap<i32, (i32, u64)>> {
    let mut out: std::collections::HashMap<i32, std::collections::BTreeMap<i32, (i32, u64)>> = Default::default();
    for p in replay.chunks.iter().flat_map(|c| &c.packets) {
        if let Ok(rustora_net::order::Packet::Sync {
            frame,
            sync_hash,
            defeat_state,
        }) = rustora_net::order::Packet::parse(p.client_id, &p.data)
        {
            out.entry(frame).or_default().insert(p.client_id, (sync_hash, defeat_state));
        }
    }
    out
}
