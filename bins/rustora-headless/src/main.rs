// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Headless rustora. `rustora-headless world <engine-dir> <support-dir> <replay>` builds the
//! world of a replay and prints the sync detail after world load, for comparison with the
//! C# harness dump.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use rustora_engine::map::{Map, MapGrid};
use rustora_engine::rules::{Ruleset, TraitRegistry};
use rustora_engine::world::{World, WorldType};
use rustora_fs::{Folder, MapFs, Package, Platform, ZipPackage};
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;
use rustora_net::replay::Replay;

fn find_map(engine: &std::path::Path, uid: &str) -> Option<Arc<dyn Package>> {
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 || args[1] != "world" {
        eprintln!("usage: rustora-headless world <engine-dir> <support-dir> <replay>");
        std::process::exit(2);
    }
    let engine = PathBuf::from(&args[2]);
    let md = ModData::load(Platform::new(&engine, &args[3]), "ra").expect("load mod");
    let replay = Replay::parse(&std::fs::read(&args[4]).expect("read replay")).expect("parse replay");
    let lobby = replay.lobby_info.expect("lobby info");
    let uid = lobby.global_settings.map.clone().expect("map uid");
    let pkg = find_map(&engine, &uid).expect("map not found");
    let map_package = MapPackage::load(pkg.clone()).expect("load map");

    let fs = MapFs {
        package: pkg.as_ref(),
        fs: &md.file_system,
    };
    let rules_yaml = ModData::load_yaml(&fs, &md.manifest.rules, map_package.section("Rules")).expect("rules");
    let mut registry = TraitRegistry::default();
    rustora_mods_common::register(&mut registry);
    let rules = Rc::new(Ruleset::load(&registry, &rules_yaml).expect("ruleset"));
    let map = Rc::new(Map::load(map_package, MapGrid::default()).expect("map"));

    let world = World::new(map, rules, lobby, WorldType::Regular, 40, 3);
    world.load_complete();
    println!("0 {}", world.sync_hash());
    for l in world.sync_detail() {
        println!("{l}");
    }
}
