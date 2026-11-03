// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Loads the RA mod, the map and the rules of a replay or a skirmish into a [`World`], and steps the game tick by tick.

pub mod skirmish;

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
    let mut replay = Replay::parse(&std::fs::read(replay_path).expect("read replay")).expect("parse replay");
    let lobby = replay.lobby_info.take().expect("lobby info");
    let uid = lobby.global_settings.map.clone().expect("map uid");
    let pkg = find_map(engine, &uid).expect("map not found");
    let (world, mod_data, read_file) = load_world(engine, support, pkg, lobby, true);
    LoadedGame {
        world,
        replay,
        mod_data,
        read_file,
    }
}

/// Opens a map of the RA mod by its file or folder name, like `Launch.Map=ore-lord.oramap`.
pub fn open_map(engine: &Path, name: &str) -> Option<Arc<dyn Package>> {
    let p = engine.join("mods/ra/maps").join(name);
    if p.is_dir() {
        Some(Arc::new(Folder::new(&p)))
    } else {
        Some(Arc::new(ZipPackage::open_file(&p)?))
    }
}

/// Loads the RA mod and the map `pkg` with its custom rules and creates the world for `lobby`.
pub fn load_world(engine: &Path, support: &str, pkg: Arc<dyn Package>, lobby: rustora_net::session::Session, is_replay: bool) -> (Rc<World>, Rc<ModData>, ReadFile) {
    let md = ModData::load(Platform::new(engine, support), "ra").expect("load mod");
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
    world.set_is_replay(is_replay);
    world.load_complete();
    (world, md, read_file)
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

fn format_actors(actors: &Option<Vec<Option<rustora_engine::actor::ActorRef>>>) -> String {
    match actors {
        None => "-".into(),
        Some(v) => v.iter().map(|a| a.as_ref().map_or_else(|| "null".to_string(), |a| a.id.to_string())).collect::<Vec<_>>().join(","),
    }
}

/// `RustoraHarness.FormatOrder`: one line of a bot order dump.
pub fn format_order(tick: i32, o: &rustora_engine::order::Order) -> String {
    use rustora_engine::target::TargetType;
    let t = &o.target;
    let target = match t.target_type() {
        TargetType::Actor => format!("actor:{}", t.actor().expect("actor target").id),
        TargetType::FrozenActor => format!("frozen:{}", t.frozen_actor().expect("frozen target").id()),
        TargetType::Terrain => {
            let p = t.center_position();
            format!("terrain:{},{},{}", p.x, p.y, p.z)
        }
        TargetType::Invalid => "none".into(),
    };
    let cap = |b: bool| if b { "True" } else { "False" };
    format!(
        "{tick} {} {} {target} {} {},{} {} {} {} {} {}",
        o.order_string,
        o.subject.as_ref().map_or_else(|| "-".to_string(), |a| a.id.to_string()),
        o.target_string.as_deref().unwrap_or("-"),
        o.extra_location.x(),
        o.extra_location.y(),
        format_actors(&o.extra_actors),
        o.extra_data,
        cap(o.queued),
        cap(o.is_immediate),
        format_actors(&o.grouped_actors),
    )
}
