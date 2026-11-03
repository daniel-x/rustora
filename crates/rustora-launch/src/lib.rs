// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Loads the RA mod, the map and the rules of a replay into a [`World`], and steps the replay tick by tick.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use rustora_engine::map::{Map, MapGrid};
use rustora_engine::order_manager::{OrderManager, ReplayConnection};
use rustora_engine::rules::{Ruleset, TraitRegistry};
use rustora_engine::sequence::ReadFile;
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

/// A loaded replay: the world after `load_complete`, the remaining replay data, and file access for assets.
pub struct LoadedGame {
    pub world: Rc<World>,
    pub replay: Replay,
    pub mod_data: Rc<ModData>,
    /// Opens a file from the map package or the mod file system.
    pub read_file: ReadFile,
}

pub fn load_game(engine: &Path, support: &str, replay_path: &str) -> LoadedGame {
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
    let read_file: ReadFile = Rc::new(move |name: &str| pkg.get(name).or_else(|| md2.file_system.try_open(name)));
    map.set_sequences(Rc::new(rustora_engine::sequence::SequenceSet::new(
        &sequences,
        &tileset,
        read_file.clone(),
    )));
    let world = World::new(map, rules, lobby, WorldType::Regular, 40, 3);
    world.set_is_replay(true);
    world.load_complete();
    LoadedGame {
        world,
        replay,
        mod_data: md,
        read_file,
    }
}

/// Outcome of one [`ReplayDriver::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Ticked,
    Waiting,
    Ended,
}

/// Feeds the orders of a replay into the world one net frame at a time.
pub struct ReplayDriver {
    pub orders: OrderManager,
    world: Rc<World>,
    idle: u32,
}

impl ReplayDriver {
    pub fn new(world: &Rc<World>, replay: Replay) -> ReplayDriver {
        let mut orders = OrderManager::new(world.clone(), Box::new(ReplayConnection::new(replay.chunks, 3)));
        orders.start_game();
        ReplayDriver {
            orders,
            world: world.clone(),
            idle: 0,
        }
    }

    /// Advances the world by one tick if the orders for it are available.
    pub fn step(&mut self) -> Step {
        if self.idle > 10 {
            return Step::Ended;
        }
        self.orders.tick_immediate();
        if self.orders.try_tick() {
            self.idle = 0;
            self.world.tick();
            Step::Ticked
        } else {
            self.idle += 1;
            if self.idle > 10 { Step::Ended } else { Step::Waiting }
        }
    }
}
