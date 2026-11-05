// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! World actor traits that set up a game: factions, players, spawn points, map actors,
//! starting units.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::{Color, load};
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::{CPos, CVec, MersenneTwister, WAngle, WDist};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInits, FacingInit, LocationInit, OwnerInit};
use rustora_engine::player::{Player, PlayerRef, PlayerSetup};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};
use rustora_net::session::Client;

use crate::actor_reference::{ActorReference, SkipMakeAnimsInit, SpawnedByMapInit};

pub fn register(r: &mut TraitRegistry) {
    r.register("Faction", |n| load_info!(FactionInfo, n));
    r.register("CreateMapPlayers", |n| load_info!(CreateMapPlayersInfo, n));
    r.register("MapStartingLocations", |n| load_info!(MapStartingLocationsInfo, n));
    r.register("SpawnMapActors", |n| load_info!(SpawnMapActorsInfo, n));
    r.register("StartingUnits", |n| load_info!(StartingUnitsInfo, n));
    r.register("SpawnStartingUnits", |n| load_info!(SpawnStartingUnitsInfo, n));
}

/// `FactionInfo`: only the info matters, the trait is empty.
#[derive(Clone, Debug, FieldLoad)]
pub struct FactionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: Option<String>,
    pub internal_name: Option<String>,
    pub random_faction_members: IndexSet<String>,
    pub side: Option<String>,
    pub description: Option<String>,
    pub selectable: bool,
}

impl Default for FactionInfo {
    fn default() -> Self {
        FactionInfo {
            instance_name: None,
            name: None,
            internal_name: None,
            random_faction_members: IndexSet::new(),
            side: None,
            description: None,
            selectable: true,
        }
    }
}

impl FactionInfo {
    fn is_selectable(&self) -> bool {
        self.selectable
    }

    fn internal(&self) -> &str {
        self.internal_name.as_deref().unwrap_or("")
    }
}

pub struct Faction;
register_traits!(Faction);
info_impl!(FactionInfo => |_, _| Some(Rc::new(Faction)));

/// `Player.ResolveFaction`.
pub fn resolve_faction(name: &str, factions: &[Rc<FactionInfo>], r: &mut MersenneTwister, require_selectable: bool) -> Rc<FactionInfo> {
    let selectable: Vec<Rc<FactionInfo>> = factions
        .iter()
        .filter(|f| !require_selectable || f.is_selectable())
        .cloned()
        .collect();
    let pick = |r: &mut MersenneTwister, v: &[Rc<FactionInfo>]| v[r.next_max(v.len() as i32) as usize].clone();
    let mut selected = match selectable.iter().find(|f| f.internal() == name) {
        Some(f) => f.clone(),
        None => pick(r, &selectable),
    };
    let mut i = 0;
    while i <= 10 && !selected.random_faction_members.is_empty() {
        let members: Vec<&String> = selected.random_faction_members.iter().collect();
        let faction = members[r.next_max(members.len() as i32) as usize].clone();
        selected = selectable
            .iter()
            .find(|f| f.internal() == faction)
            .cloned()
            .unwrap_or_else(|| panic!("Unknown faction: {faction}"));
        i += 1;
    }
    selected
}

fn display_faction(name: &str, factions: &[Rc<FactionInfo>]) -> Rc<FactionInfo> {
    factions
        .iter()
        .find(|f| f.internal() == name)
        .cloned()
        .unwrap_or_else(|| factions[0].clone())
}

/// `PlayerReference` from map.yaml.
#[derive(Clone, Debug, FieldLoad)]
pub struct PlayerReference {
    pub name: String,
    pub palette: Option<String>,
    pub bot: Option<String>,
    pub starting_units_class: Option<String>,
    pub allow_bots: bool,
    pub playable: bool,
    pub required: bool,
    pub owns_world: bool,
    pub spectating: bool,
    pub non_combatant: bool,
    pub lock_faction: bool,
    pub faction: String,
    pub lock_color: bool,
    pub color: Color,
    pub home_location: CPos,
    pub lock_spawn: bool,
    pub spawn: i32,
    pub lock_team: bool,
    pub team: i32,
    pub lock_handicap: bool,
    pub handicap: i32,
    pub allies: Vec<String>,
    pub enemies: Vec<String>,
}

impl Default for PlayerReference {
    fn default() -> Self {
        PlayerReference {
            name: String::new(),
            palette: None,
            bot: None,
            starting_units_class: None,
            allow_bots: true,
            playable: false,
            required: false,
            owns_world: false,
            spectating: false,
            non_combatant: false,
            lock_faction: false,
            faction: String::new(),
            lock_color: false,
            color: Color::from_argb(255, 0xEE, 0xEE, 0xEE),
            home_location: CPos::ZERO,
            lock_spawn: false,
            spawn: 0,
            lock_team: false,
            team: 0,
            lock_handicap: false,
            handicap: 0,
            allies: Vec::new(),
            enemies: Vec::new(),
        }
    }
}

/// `LobbyCommands.MakeSlotFromPlayerReference`.
pub fn make_slot(pr: &PlayerReference) -> Option<rustora_net::session::Slot> {
    pr.playable.then(|| rustora_net::session::Slot {
        player_reference: pr.name.clone(),
        closed: false,
        allow_bots: pr.allow_bots,
        lock_faction: pr.lock_faction,
        lock_color: pr.lock_color,
        lock_team: pr.lock_team,
        lock_handicap: pr.lock_handicap,
        lock_spawn: pr.lock_spawn,
        required: pr.required,
    })
}

/// `Server.SyncClientToPlayerReference`.
pub fn sync_client_to_player_reference(c: &mut rustora_net::session::Client, pr: &PlayerReference) {
    if pr.lock_faction {
        c.faction = Some(pr.faction.clone());
    }
    if pr.lock_spawn {
        c.spawn_point = pr.spawn;
    }
    if pr.lock_team {
        c.team = pr.team;
    }
    if pr.lock_handicap {
        c.handicap = pr.handicap;
    }
    c.color = if pr.lock_color { pr.color } else { c.preferred_color };
}

/// `MapPlayers`: player references keyed by name, in definition order.
pub fn map_players(defs: &[MiniYamlNode]) -> IndexMap<String, PlayerReference> {
    let mut out = IndexMap::new();
    for d in defs {
        let pr: PlayerReference = load(&MiniYaml::new(Some(d.key.clone()), d.value.nodes.clone())).expect("valid player reference");
        out.insert(pr.name.clone(), pr);
    }
    out
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct CreateMapPlayersInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

/// Player references of the created players, used for alliance masks and starting units.
#[derive(Default)]
pub struct CreateMapPlayers {
    pub references: RefCell<Vec<(PlayerRef, PlayerReference)>>,
}

register_traits!(CreateMapPlayers: ICreatePlayers);
info_impl!(CreateMapPlayersInfo => |_, _| Some(Rc::new(CreateMapPlayers::default())));

fn world_factions(world: &World) -> Vec<Rc<FactionInfo>> {
    world.rules().actor("world").expect("world actor").trait_infos::<FactionInfo>()
}

fn create_player(world: &Rc<World>, client: Option<&Client>, pr: &PlayerReference, r: &mut MersenneTwister) -> PlayerRef {
    let factions = world_factions(world);
    let in_mission_map = world
        .map()
        .package
        .section("Visibility")
        .is_some_and(|v| v.value_str().contains("MissionSelector"));
    let setup = match client {
        Some(c) => {
            let faction_name = c.faction.clone().unwrap_or_default();
            let faction = resolve_faction(&faction_name, &factions, r, !pr.lock_faction);
            let display = display_faction(&faction_name, &factions);
            let asp = world.world_actor().trait_or_default::<dyn IAssignSpawnPoints>();
            let home_location = match &asp {
                Some(a) => a.assign_home_location(world, c, r),
                None => pr.home_location,
            };
            let spawn_point = asp.as_ref().map_or(c.spawn_point, |a| a.spawn_point_for_client(c.index));
            PlayerSetup {
                internal_name: pr.name.clone(),
                player_name: c.name.clone().unwrap_or_default(),
                faction: faction.internal().to_string(),
                display_faction: display.internal().to_string(),
                non_combatant: false,
                playable: true,
                client_index: c.index,
                home_location,
                handicap: c.handicap,
                bot_type: c.bot.clone(),
                spectating: false,
                color: c.color,
                spawn_point,
                display_spawn_point: c.spawn_point,
                in_mission_map,
            }
        }
        None => {
            let faction = resolve_faction(&pr.faction, &factions, r, false);
            let display = display_faction(&pr.faction, &factions);
            PlayerSetup {
                internal_name: pr.name.clone(),
                player_name: pr.name.clone(),
                faction: faction.internal().to_string(),
                display_faction: display.internal().to_string(),
                non_combatant: pr.non_combatant,
                playable: pr.playable,
                client_index: world.lobby_info.clients.iter().find(|c| c.is_admin).map_or(0, |c| c.index),
                home_location: pr.home_location,
                handicap: pr.handicap,
                bot_type: pr.bot.clone(),
                spectating: pr.spectating,
                color: pr.color,
                spawn_point: 0,
                display_spawn_point: 0,
                in_mission_map,
            }
        }
    };
    Player::create(world, setup)
}

impl ICreatePlayers for CreateMapPlayers {
    fn create_players(&self, w: &Rc<World>, r: &mut MersenneTwister) {
        let players = map_players(w.map().package.section_nodes("Players"));
        let mut refs: Vec<(PlayerRef, PlayerReference)> = Vec::new();
        let mut world_owner_found = false;
        for pr in players.values().filter(|p| !p.playable) {
            let p = create_player(w, None, pr, r);
            if pr.owns_world {
                world_owner_found = true;
                w.set_world_owner(&p);
            }
            refs.push((p, pr.clone()));
        }
        assert!(
            world_owner_found,
            "Map {} does not define a player actor owning the world.",
            w.map().title
        );

        for (slot_name, slot) in &w.lobby_info.slots {
            let Some(client) = w.lobby_info.client_in_slot(slot_name) else {
                continue;
            };
            let pr = players.get(&slot.player_reference).expect("slot player reference");
            let p = create_player(w, Some(client), pr, r);
            refs.push((p, pr.clone()));
        }

        let everyone = PlayerReference {
            name: "Everyone".into(),
            non_combatant: true,
            spectating: true,
            faction: "Random".into(),
            allies: refs
                .iter()
                .filter(|(p, _)| !p.non_combatant && p.playable)
                .map(|(p, _)| p.internal_name.clone())
                .collect(),
            ..Default::default()
        };
        let p = create_player(w, None, &everyone, r);
        refs.push((p, everyone));

        w.set_players(refs.iter().map(|(p, _)| p.clone()).collect());
        for (p, pr) in &refs {
            for (q, qr) in &refs {
                setup_player_masks(w, p, pr, q, qr);
            }
        }
        for (p, _) in &refs {
            if !p.spectating() {
                w.all_players_mask.set(w.all_players_mask.get() | p.player_mask);
            }
        }
        *self.references.borrow_mut() = refs;
    }
}

fn setup_player_masks(w: &World, p: &PlayerRef, pr: &PlayerReference, q: &PlayerRef, qr: &PlayerReference) {
    if Rc::ptr_eq(p, q) || pr.allies.contains(&q.internal_name) {
        p.allied_players_mask.set(p.allied_players_mask.get() | q.player_mask);
        return;
    }
    if pr.enemies.contains(&q.internal_name) {
        p.enemy_players_mask.set(p.enemy_players_mask.get() | q.player_mask);
        return;
    }
    if pr.playable && qr.playable {
        let pc = w.lobby_info.client_with_index(p.client_index);
        let qc = w.lobby_info.client_with_index(q.client_index);
        if let (Some(pc), Some(qc)) = (pc, qc) {
            if pc.team != 0 && pc.team == qc.team {
                p.allied_players_mask.set(p.allied_players_mask.get() | q.player_mask);
            } else {
                p.enemy_players_mask.set(p.enemy_players_mask.get() | q.player_mask);
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MapStartingLocationsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub initial_explore_range: WDist,
    pub separate_team_spawns_checkbox_enabled: bool,
}

impl Default for MapStartingLocationsInfo {
    fn default() -> Self {
        MapStartingLocationsInfo {
            instance_name: None,
            initial_explore_range: WDist::from_cells(5),
            separate_team_spawns_checkbox_enabled: true,
        }
    }
}

pub struct MapStartingLocations {
    info: Rc<MapStartingLocationsInfo>,
    state: RefCell<SpawnState>,
}

#[derive(Default)]
struct SpawnState {
    separate_team_spawns: bool,
    spawn_locations: Vec<CPos>,
    available: Vec<i32>,
    occupied: IndexMap<i32, i32>,
}

register_traits!(MapStartingLocations: INotifyCreated, IAssignSpawnPoints, IWorldLoaded);
info_impl!(MapStartingLocationsInfo => |info, _| Some(Rc::new(MapStartingLocations { info, state: RefCell::new(SpawnState::default()) })));

impl INotifyCreated for MapStartingLocations {
    fn created(&self, self_: &ActorRef) {
        let w = self_.world();
        let mut s = self.state.borrow_mut();
        s.separate_team_spawns = w
            .lobby_info
            .global_settings
            .option_bool("separateteamspawns", self.info.separate_team_spawns_checkbox_enabled);
        for n in w.map().package.section_nodes("Actors") {
            if n.value.value_str() == "mpspawn" {
                let r = ActorReference::from_yaml("mpspawn", &n.value).expect("mpspawn reference");
                s.spawn_locations
                    .push(r.inits.get::<LocationInit>().expect("mpspawn location").value);
            }
        }
        let count = s.spawn_locations.len() as i32;
        s.available = (1..=count).filter(|i| !w.lobby_info.disabled_spawn_points.contains(i)).collect();
        for (slot, _) in &w.lobby_info.slots {
            let Some(c) = w.lobby_info.client_in_slot(slot) else { continue };
            if c.spawn_point == 0 {
                continue;
            }
            let sp = c.spawn_point;
            s.available.retain(|&x| x != sp);
            s.occupied.insert(sp, c.index);
        }
    }
}

impl IAssignSpawnPoints for MapStartingLocations {
    fn assign_home_location(&self, _world: &World, client: &Client, r: &mut MersenneTwister) -> CPos {
        let mut s = self.state.borrow_mut();
        if client.spawn_point > 0 && client.spawn_point as usize <= s.spawn_locations.len() {
            return s.spawn_locations[client.spawn_point as usize - 1];
        }
        let spawn_point = if s.occupied.is_empty() || !s.separate_team_spawns {
            s.available[r.next_max(s.available.len() as i32) as usize]
        } else {
            let mut best: Option<(i32, i64)> = None;
            for &cand in &s.available {
                let cell = s.spawn_locations[cand as usize - 1];
                let sum: i64 = s
                    .occupied
                    .keys()
                    .map(|&k| (s.spawn_locations[k as usize - 1] - cell).length_squared() as i64)
                    .sum();
                if best.is_none_or(|(_, b)| sum > b) {
                    best = Some((cand, sum));
                }
            }
            best.expect("available spawn").0
        };
        s.available.retain(|&x| x != spawn_point);
        s.occupied.insert(spawn_point, client.index);
        s.spawn_locations[spawn_point as usize - 1]
    }

    fn spawn_point_for_client(&self, client_index: i32) -> i32 {
        self.state
            .borrow()
            .occupied
            .iter()
            .find(|(_, c)| **c == client_index)
            .map_or(0, |(k, _)| *k)
    }
}

impl IWorldLoaded for MapStartingLocations {
    fn world_loaded(&self, world: &Rc<World>) {
        for p in world.players() {
            if !p.playable {
                continue;
            }
            let cells = rustora_engine::shroud::Shroud::projected_cells_in_cell_range(
                world.map(),
                p.home_location,
                self.info.initial_explore_range,
            );
            for q in world.players() {
                if p.is_allied_with(Some(&q)) {
                    q.shroud().explore_projected_cells(&cells);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct SpawnMapActorsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(Default)]
pub struct SpawnMapActors {
    pub actors: RefCell<IndexMap<String, ActorRef>>,
}

register_traits!(SpawnMapActors: IWorldLoaded);
info_impl!(SpawnMapActorsInfo => |_, _| Some(Rc::new(SpawnMapActors::default())));

impl IWorldLoaded for SpawnMapActors {
    fn world_loaded(&self, world: &Rc<World>) {
        let world_owner = world.world_actor().owner();
        for n in world.map().package.section_nodes("Actors") {
            let mut r = ActorReference::from_yaml(n.value.value_str(), &n.value).expect("map actor reference");
            let owner = r.owner_name().unwrap_or_default();
            if !world.players().iter().any(|p| p.internal_name == owner) {
                r.inits.0.retain(|i| i.as_any().downcast_ref::<OwnerInit>().is_none());
                r.inits.push(Rc::new(OwnerInit::from_player(&world_owner)));
            }
            r.inits.push(Rc::new(SkipMakeAnimsInit));
            r.inits.push(Rc::new(SpawnedByMapInit));
            let a = world.create_actor(&r.actor_type, r.inits);
            self.actors.borrow_mut().insert(n.key.clone(), a);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct StartingUnitsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub class: String,
    pub class_name: Option<String>,
    pub factions: IndexSet<String>,
    pub base_actor: Option<String>,
    pub base_actor_offset: CVec,
    pub base_actor_facing: Option<WAngle>,
    pub support_actors: Vec<String>,
    pub inner_support_radius: i32,
    pub outer_support_radius: i32,
    pub support_actors_facing: Option<WAngle>,
}

impl Default for StartingUnitsInfo {
    fn default() -> Self {
        StartingUnitsInfo {
            instance_name: None,
            class: "none".into(),
            class_name: None,
            factions: IndexSet::new(),
            base_actor: None,
            base_actor_offset: CVec::ZERO,
            base_actor_facing: Some(WAngle::new(512)),
            support_actors: Vec::new(),
            inner_support_radius: 2,
            outer_support_radius: 4,
            support_actors_facing: None,
        }
    }
}

pub struct StartingUnits;
register_traits!(StartingUnits);
info_impl!(StartingUnitsInfo => |_, _| Some(Rc::new(StartingUnits)));

#[derive(Clone, Debug, FieldLoad)]
pub struct SpawnStartingUnitsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub starting_units_class: String,
}

impl Default for SpawnStartingUnitsInfo {
    fn default() -> Self {
        SpawnStartingUnitsInfo {
            instance_name: None,
            starting_units_class: "none".into(),
        }
    }
}

pub struct SpawnStartingUnits {
    info: Rc<SpawnStartingUnitsInfo>,
}

register_traits!(SpawnStartingUnits: IWorldLoaded);
info_impl!(SpawnStartingUnitsInfo => |info, _| Some(Rc::new(SpawnStartingUnits { info })));

impl SpawnStartingUnits {
    /// The support actor part of `SpawnUnitsForPlayer`.
    fn spawn_support_actors(&self, w: &Rc<World>, p: &PlayerRef, group: &StartingUnitsInfo, home_locations: &[CPos]) {
        if group.support_actors.is_empty() {
            return;
        }
        let spawn_cells: Vec<CPos> = w
            .map()
            .find_tiles_in_annulus(p.home_location, group.inner_support_radius + 1, group.outer_support_radius, false)
            .collect();
        let path_finder = w.world_actor().trait_or_default::<crate::path_finder_trait::PathFinder>();
        for s in &group.support_actors {
            let rules = w.rules().actor(&s.to_lowercase()).unwrap_or_else(|| panic!("unknown actor {s}"));
            let mobile = rules.trait_info::<crate::mobile::MobileInfo>();
            let aircraft = rules.trait_info::<crate::aircraft::AircraftInfo>();
            let can_enter = |c: CPos| match (&mobile, &aircraft) {
                (Some(m), _) => m.can_enter_cell(
                    w,
                    None,
                    c,
                    rustora_engine::traits::subcell::FULL_CELL,
                    None,
                    crate::locomotor::BlockedByActor::All,
                ),
                (None, Some(a)) => a.can_enter_cell(w, c, None, crate::locomotor::BlockedByActor::All),
                (None, None) => panic!("{s} has no supported IPositionableInfo"),
            };
            let locomotor = mobile.as_ref().map(|m| crate::locomotor::locomotor_by_name(w, &m.locomotor));
            let valid: Vec<CPos> = spawn_cells
                .iter()
                .copied()
                .filter(|&c| can_enter(c))
                .filter(|&c| match (&path_finder, &locomotor) {
                    (Some(pf), Some(l)) => home_locations
                        .iter()
                        .any(|&h| pf.path_might_exist_for_locomotor_blocked_by_immovable(l, c, h)),
                    _ => true,
                })
                .collect();
            let cell = crate::bots::random_or_default(&valid, &w.shared_random).unwrap_or(CPos::ZERO);
            if cell == CPos::ZERO {
                log::debug!("No cells available to spawn starting unit {s} for player {}", p.player_name);
                continue;
            }
            let shares_cell = mobile.as_ref().is_some_and(|m| m.locomotor_info(w).shares_cell);
            let sub = if shares_cell {
                crate::actor_map::WorldExt::actor_map(&**w).free_subcell(cell, rustora_engine::traits::subcell::ANY, true)
            } else {
                0
            };
            let facing = group
                .support_actors_facing
                .unwrap_or_else(|| WAngle::new(w.shared_random.borrow_mut().next_max(1024)));
            let inits = ActorInits::new()
                .with(OwnerInit::from_player(p))
                .with(LocationInit::new(cell))
                .with(rustora_engine::actor_init::SubCellInit::new(sub))
                .with(FacingInit::new(facing))
                .with(SpawnedByMapInit);
            w.create_actor(&s.to_lowercase(), inits);
        }
    }
}

impl IWorldLoaded for SpawnStartingUnits {
    fn world_loaded(&self, w: &Rc<World>) {
        let refs = w.world_actor().trait_::<CreateMapPlayers>().references.borrow().clone();
        for p in w.players() {
            if !p.playable {
                continue;
            }
            let pr = refs.iter().find(|(q, _)| Rc::ptr_eq(q, &p)).map(|(_, r)| r.clone());
            let class = pr.and_then(|r| r.starting_units_class).unwrap_or_else(|| {
                w.lobby_info
                    .global_settings
                    .option_str("startingunits", &self.info.starting_units_class)
                    .to_string()
            });
            let groups: Vec<Rc<StartingUnitsInfo>> = w
                .rules()
                .actor("world")
                .expect("world")
                .trait_infos::<StartingUnitsInfo>()
                .into_iter()
                .filter(|g| g.class == class && g.factions.contains(&p.faction))
                .collect();
            if groups.is_empty() {
                panic!("No starting units defined for faction {} with class {class}", p.faction);
            }
            let group = groups[w.shared_random.borrow_mut().next_max(groups.len() as i32) as usize].clone();
            let home_locations: Vec<CPos> = match &group.base_actor {
                Some(base) => {
                    let facing = group
                        .base_actor_facing
                        .unwrap_or_else(|| WAngle::new(w.shared_random.borrow_mut().next_max(1024)));
                    let inits = ActorInits::new()
                        .with(LocationInit::new(p.home_location + group.base_actor_offset))
                        .with(OwnerInit::from_player(&p))
                        .with(SkipMakeAnimsInit)
                        .with(FacingInit::new(facing))
                        .with(SpawnedByMapInit);
                    let base_actor = w.create_actor(&base.to_lowercase(), inits);
                    let movable = base_actor
                        .trait_or_default::<crate::mobile::Mobile>()
                        .is_some_and(|m| !m.is_trait_disabled() && !m.is_trait_paused() && !m.is_immovable());
                    if movable {
                        vec![base_actor.location()]
                    } else {
                        let occupied: Vec<CPos> = base_actor
                            .trait_::<dyn IOccupySpace>()
                            .occupied_cells()
                            .into_iter()
                            .map(|(c, _)| c)
                            .collect();
                        crate::util::expand_footprint(&occupied, true)
                            .into_iter()
                            .filter(|c| !occupied.contains(c))
                            .collect()
                    }
                }
                None => vec![p.home_location],
            };
            self.spawn_support_actors(w, &p, &group, &home_locations);
        }
    }
}

/// The `GameInformation` that `World` creates and fills in `LoadComplete` (`GameInformation.AddPlayer` for each
/// player), stored in the world, which keeps it up to date for the replay metadata.
pub fn create_game_information(
    world: &World,
    mod_id: &str,
    version: &str,
) -> std::sync::Arc<std::sync::Mutex<rustora_net::game_information::GameInformation>> {
    use rustora_net::game_information::{GameInformation, GamePlayer, GameRoot, now_utc};
    let factions = world.world_actor().info.trait_infos::<FactionInfo>();
    let faction_name = |internal: &str| factions.iter().find(|f| f.internal() == internal).and_then(|f| f.name.clone());
    let lobby = &world.lobby_info;
    let mut info = GameInformation {
        root: GameRoot {
            mod_id: Some(mod_id.to_string()),
            version: Some(version.to_string()),
            map_uid: Some(world.map().uid.clone()),
            map_title: Some(world.map().title.clone()),
            disabled_spawn_points: lobby.disabled_spawn_points.iter().copied().collect(),
            start_time_utc: Some(now_utc()),
            ..Default::default()
        },
        players: Vec::new(),
    };
    let mut names = Vec::new();
    for p in world.players() {
        if p.non_combatant || !p.playable {
            continue;
        }
        let Some(client) = lobby.client_with_index(p.client_index) else {
            continue;
        };
        info.players.push(GamePlayer {
            client_index: p.client_index,
            name: p.player_name.clone(),
            is_human: !p.is_bot,
            is_bot: p.is_bot,
            bot_type: p.bot_type.clone(),
            faction_name: faction_name(&p.faction),
            faction_id: Some(p.faction.clone()),
            color: p.color,
            display_faction_name: faction_name(&p.display_faction),
            display_faction_id: Some(p.display_faction.clone()),
            team: client.team,
            spawn_point: p.spawn_point,
            handicap: client.handicap,
            is_random_faction: client.faction.as_deref() != Some(p.faction.as_str()),
            is_random_spawn_point: p.display_spawn_point == 0,
            fingerprint: client.fingerprint.clone(),
            ..Default::default()
        });
        names.push(p.internal_name.clone());
    }
    let info = std::sync::Arc::new(std::sync::Mutex::new(info));
    world.set_game_info(info.clone(), names);
    info
}
