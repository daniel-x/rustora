// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Headless rustora. Subcommands:
//! `world <engine-dir> <support-dir> <replay> [tick]` prints the sync detail of a replay after world load (or after `tick`).
//! `verify-replay <engine-dir> <support-dir> <replay> [sync-dump]` replays a recorded game and compares the sync hashes.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use rustora_engine::map::{Map, MapGrid};
use rustora_engine::order_manager::{OrderManager, ReplayConnection};
use rustora_engine::rules::{Ruleset, TraitRegistry};
use rustora_engine::terrain::TerrainInfo;
use rustora_engine::world::{World, WorldType};
use rustora_fs::{Folder, MapFs, Package, Platform, ZipPackage};
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;
use rustora_net::replay::Replay;

fn find_map(engine: &Path, uid: &str) -> Option<Arc<dyn Package>> {
    for e in std::fs::read_dir(engine.join("mods/ra/maps")).ok()?.flatten() {
        let p = e.path();
        let pkg: Arc<dyn Package> = if p.is_dir() {
            Arc::new(Folder::new(&p))
        } else {
            Arc::new(ZipPackage::open_file(&p)?)
        };
        if rustora_game::map::compute_uid(pkg.as_ref(), 12).ok()? == uid || MapPackage::load(pkg.clone()).ok()?.uid == uid {
            return Some(pkg);
        }
    }
    None
}

fn load_terrain(md: &ModData, tileset: &str) -> TerrainInfo {
    for f in &md.manifest.tile_sets {
        let y = ModData::load_yaml(&md.file_system, std::slice::from_ref(f), None).expect("tileset");
        let info = TerrainInfo::load(&y).expect("terrain");
        if info.id == tileset {
            return info;
        }
    }
    panic!("unknown tileset {tileset}");
}

fn load_game(engine: &Path, support: &str, replay_path: &str) -> (Rc<World>, Replay) {
    let md = ModData::load(Platform::new(engine, support), "ra").expect("load mod");
    let mut replay = Replay::parse(&std::fs::read(replay_path).expect("read replay")).expect("parse replay");
    let lobby = replay.lobby_info.take().expect("lobby info");
    let uid = lobby.global_settings.map.clone().expect("map uid");
    let pkg = find_map(engine, &uid).expect("map not found");
    let map_package = MapPackage::load(pkg.clone()).expect("load map");

    let fs = MapFs {
        package: pkg.as_ref(),
        fs: &md.file_system,
    };
    let rules_yaml = ModData::load_yaml(&fs, &md.manifest.rules, map_package.section("Rules")).expect("rules");
    let mut registry = TraitRegistry::default();
    rustora_mods_common::register(&mut registry);
    let weapons_yaml = ModData::load_yaml(&fs, &md.manifest.weapons, map_package.section("Weapons")).expect("weapons");
    let mut weapon_registry = rustora_engine::weapon::WeaponRegistry::default();
    rustora_mods_common::weapons::register_weapon_parts(&mut weapon_registry);
    let rules = Rc::new(Ruleset::load(&registry, &weapon_registry, &rules_yaml, &weapons_yaml).expect("ruleset"));
    let map_package_sequences = map_package.section("Sequences").cloned();
    let tileset = map_package
        .yaml
        .nodes
        .iter()
        .find(|n| n.key == "Tileset")
        .map(|n| n.value.value_str().to_string())
        .expect("tileset");
    let terrain = Rc::new(load_terrain(&md, &tileset));
    let map = Rc::new(Map::load(map_package, MapGrid::default(), terrain).expect("map"));

    let sequences = ModData::load_yaml(&fs, &md.manifest.sequences, map_package_sequences.as_ref()).expect("sequences");
    let md = Rc::new(md);
    let md2 = md.clone();
    let pkg2 = pkg.clone();
    let read_file: rustora_engine::sequence::ReadFile = Rc::new(move |name: &str| pkg2.get(name).or_else(|| md2.file_system.try_open(name)));
    map.set_sequences(Rc::new(rustora_engine::sequence::SequenceSet::new(&sequences, &tileset, read_file)));
    let world = World::new(map, rules, lobby, WorldType::Regular, 40, 3);
    world.set_is_replay(true);
    world.load_complete();
    (world, replay)
}

fn print_detail(world: &World) {
    println!("{} {}", world.world_tick(), world.sync_hash());
    for l in world.sync_detail() {
        println!("{l}");
    }
}

/// Runs the replay up to `max_ticks` and calls `on_tick` after each world tick.
fn run_replay(world: &Rc<World>, replay: Replay, max_ticks: i32, mut on_tick: impl FnMut(&World, i32) -> bool) -> OrderManager {
    let mut om = OrderManager::new(world.clone(), Box::new(ReplayConnection::new(replay.chunks, 3)));
    om.start_game();
    let mut idle = 0;
    while world.world_tick() < max_ticks {
        om.tick_immediate();
        if om.try_tick() {
            idle = 0;
            world.tick();
            if !on_tick(world, world.sync_hash()) {
                break;
            }
        } else {
            idle += 1;
            if idle > 10 {
                println!("replay ended at tick {}", world.world_tick());
                break;
            }
        }
    }
    om
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: rustora-headless world|verify-replay <engine-dir> <support-dir> <replay> [...]");
        std::process::exit(2);
    }
    let engine = PathBuf::from(&args[2]);
    let (world, replay) = load_game(&engine, &args[3], &args[4]);
    match args[1].as_str() {
        "world" => {
            let tick: i32 = args.get(5).map_or(0, |t| t.parse().expect("tick"));
            if tick > 0 {
                let trace: Option<u32> = std::env::var("TRACE_ACTOR").ok().and_then(|v| v.parse().ok());
                run_replay(&world, replay, tick, |w, _| {
                    if let Some(a) = trace.and_then(|id| w.get_actor_by_id(id)) {
                        let facing = a.trait_or_default::<dyn rustora_engine::traits::IFacing>().map(|f| f.facing().angle);
                        let prone = (a.trait_or_default::<rustora_mods_common::turreted::Turreted>().map(|t| t.is_prone()), a.trait_or_default::<rustora_mods_common::mobile::Mobile>().map(|m| m.movement_speed_for_cell(a.location())));
                        eprintln!("trace {} pos={:?} facing={:?} prone={:?} activity={:?}", w.world_tick(), a.center_position(), facing, prone, a.current_activity().map(|x| x.type_name()));
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
        c => panic!("unknown command {c}"),
    }
}
