// Copyright (c) The OpenRA Developers and Contributors. Rustora is a Rust port of OpenRA.
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ports of `Bridge`, `LegacyBridgeHut`, `LegacyBridgeLayer`, `RepairsBridges` and `RepairBridge`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};

use rustora_core::world_types::{CPos, CVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInits, LocationInit, OwnerInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::damage::{Damage, DamageState, DamageTypeTag, DamageTypes};
use rustora_engine::effect::Effect;
use rustora_engine::map::TerrainTile;
use rustora_engine::order::Order;
use rustora_engine::render::{IRender, Renderable};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, IOrderVoice};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_map::WorldExt;
use crate::actor_reference::{AllowZeroHealthInit, HealthInit};
use crate::base::ParentActorInit;
use crate::building::BuildingInfo;
use crate::cargo::{Enter, EnterHooks};
use crate::cash_repair::EnterBehaviour;
use crate::demolition::IDemolishable;
use crate::health::{Health, kill_actor};
use crate::interfaces::*;
use crate::orders::{IIssueOrder, IOrderTargeter, TargetModifiers, UnitOrderTargeter};
use crate::util::apply_percentage_modifiers;

pub fn register(r: &mut TraitRegistry) {
    r.register("Bridge", |n| load_info!(BridgeInfo, n));
    r.register("LegacyBridgeHut", |n| load_info!(LegacyBridgeHutInfo, n));
    r.register("LegacyBridgeLayer", |n| load_info!(LegacyBridgeLayerInfo, n));
    r.register("RepairsBridges", |n| load_info!(RepairsBridgesInfo, n));
}

/// `DelayedAction`.
#[derive(rustora_engine::persist::Persist)]
#[persist(effect)]
pub struct DelayedAction {
    #[persist(me)]
    me: Weak<DelayedAction>,
    delay: Cell<i32>,
    action: RefCell<Option<BridgeAction>>,
}

/// What a [`DelayedAction`] does when its delay is over.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
pub enum BridgeAction {
    /// Repairs the next bridge segment; `hut` counts the finished directions.
    Repair {
        bridge: Rc<Bridge>,
        by: ActorRef,
        direction: i32,
        hut: Rc<LegacyBridgeHut>,
    },
    /// Demolishes the next bridge segment.
    Demolish {
        bridge: Rc<Bridge>,
        by: ActorRef,
        direction: i32,
        damage_types: DamageTypes,
    },
    /// A function of a Lua script, which save games cannot keep.
    Closure(rustora_engine::persist::Unsaved<RefCell<Option<Box<dyn FnOnce()>>>>),
    /// The demolition charge of a hut goes off.
    HutDemolish {
        hut: ActorRef,
        by: ActorRef,
        bridge: Rc<Bridge>,
        damage_types: DamageTypes,
    },
}

impl BridgeAction {
    fn run(self) {
        match self {
            BridgeAction::Closure(f) => {
                let f = f.0.borrow_mut().take();
                if let Some(f) = f {
                    f();
                }
            }
            BridgeAction::Repair {
                bridge,
                by,
                direction,
                hut,
            } => bridge.repair(&by, direction, hut),
            BridgeAction::Demolish {
                bridge,
                by,
                direction,
                damage_types,
            } => bridge.demolish(&by, direction, damage_types),
            BridgeAction::HutDemolish {
                hut,
                by,
                bridge,
                damage_types,
            } => {
                if hut.is_dead() {
                    return;
                }
                let damage = Damage::new(0, DamageTypes::default());
                let modifiers = hut
                    .traits_implementing::<dyn IDamageModifier>()
                    .into_iter()
                    .chain(hut.owner().player_actor().traits_implementing::<dyn IDamageModifier>())
                    .map(|t| t.get_damage_modifier(None, &damage))
                    .collect::<Vec<_>>();
                if apply_percentage_modifiers(100, modifiers) > 0 {
                    bridge.for_each(|b, d| b.demolish(&by, d, damage_types));
                }
            }
        }
    }
}

impl DelayedAction {
    pub fn new(delay: i32, action: BridgeAction) -> Rc<DelayedAction> {
        Rc::new_cyclic(|me| DelayedAction {
            me: me.clone(),
            delay: Cell::new(delay),
            action: RefCell::new(Some(action)),
        })
    }
}

impl Effect for DelayedAction {
    fn tick(&self, world: &Rc<World>) {
        self.delay.set(self.delay.get() - 1);
        if self.delay.get() > 0 {
            return;
        }
        let Some(me) = self.me.upgrade() else {
            return;
        };
        world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
            w.remove_effect(&(me.clone() as Rc<dyn Effect>));
            let action = me.action.borrow_mut().take();
            if let Some(action) = action {
                action.run();
            }
        }));
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BridgeInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub long: bool,
    pub repair_propagation_delay: i32,
    pub template: u16,
    pub damaged_template: u16,
    pub destroyed_template: u16,
    pub destroyed_plus_north_template: u16,
    pub destroyed_plus_south_template: u16,
    pub destroyed_plus_both_template: u16,
    pub shore_pieces: Vec<String>,
    pub north_offset: Vec<i32>,
    pub south_offset: Vec<i32>,
    pub demolish_weapon: String,
    pub damage_types: DamageTypes,
}

impl Default for BridgeInfo {
    fn default() -> Self {
        BridgeInfo {
            instance_name: None,
            long: false,
            repair_propagation_delay: 20,
            template: 0,
            damaged_template: 0,
            destroyed_template: 0,
            destroyed_plus_north_template: 0,
            destroyed_plus_south_template: 0,
            destroyed_plus_both_template: 0,
            shore_pieces: vec!["br1".into(), "br2".into()],
            north_offset: Vec::new(),
            south_offset: Vec::new(),
            demolish_weapon: "Demolish".into(),
            damage_types: DamageTypes::default(),
        }
    }
}

impl BridgeInfo {
    /// `Templates`.
    fn templates(&self) -> Vec<(u16, i32)> {
        [
            (self.template, 100),
            (self.damaged_template, 49),
            (self.destroyed_template, 0),
            (self.destroyed_plus_north_template, 0),
            (self.destroyed_plus_south_template, 0),
            (self.destroyed_plus_both_template, 0),
        ]
        .into_iter()
        .filter(|t| t.0 != 0)
        .collect()
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Bridge {
    info: Rc<BridgeInfo>,
    self_: ActorRef,
    neighbours: RefCell<[Option<Rc<Bridge>>; 2]>,
    huts: RefCell<[Option<Rc<LegacyBridgeHut>>; 2]>,
    hut: RefCell<Option<Rc<LegacyBridgeHut>>>,
    is_dangling: Cell<Option<bool>>,
    template: Cell<u16>,
    footprint: RefCell<Vec<(CPos, u8)>>,
    killed_units: Cell<bool>,
}

register_traits!(Bridge: IRender, INotifyCreated, INotifyDamageStateChanged);
info_impl!(BridgeInfo => |info, init| Some(Rc::new(Bridge {
    info,
    self_: init.self_.clone(),
    neighbours: RefCell::new([None, None]),
    huts: RefCell::new([None, None]),
    hut: RefCell::new(None),
    is_dangling: Cell::new(None),
    template: Cell::new(0),
    footprint: RefCell::new(Vec::new()),
    killed_units: Cell::new(false),
})));

fn same_hut(a: &Option<Rc<LegacyBridgeHut>>, b: &Option<Rc<LegacyBridgeHut>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

impl Bridge {
    fn me(&self) -> Rc<Bridge> {
        self.self_.trait_::<Bridge>()
    }

    fn health(&self) -> Rc<Health> {
        self.self_.trait_::<Health>()
    }

    fn neighbour(&self, direction: usize) -> Option<Rc<Bridge>> {
        self.neighbours.borrow()[direction].clone()
    }

    fn own_hut(&self) -> Option<Rc<LegacyBridgeHut>> {
        self.hut.borrow().clone()
    }

    fn is_dangling(&self) -> bool {
        if let Some(v) = self.is_dangling.get() {
            return v;
        }
        let same = {
            let huts = self.huts.borrow();
            same_hut(&huts[0], &huts[1])
        };
        let v = same && (self.neighbour(0).is_none() || self.neighbour(1).is_none());
        self.is_dangling.set(Some(v));
        v
    }

    /// `Do`.
    fn for_each(&self, mut action: impl FnMut(&Rc<Bridge>, i32)) {
        action(&self.me(), -1);
        for d in 0..=1 {
            if let Some(n) = self.neighbour(d) {
                action(&n, d as i32);
            }
        }
    }

    fn create(&self, template: u16, footprint: Vec<(CPos, u8)>) {
        self.template.set(template);
        *self.footprint.borrow_mut() = footprint;
        self.apply_terrain();
    }

    fn apply_terrain(&self) {
        let world = self.self_.world();
        let map = world.map();
        let template = self.template.get();
        let size_x = map.terrain.templates[&template].size.0;
        let location = self.self_.location();
        for &(c, _) in self.footprint.borrow().iter() {
            let index = (c.x() - location.x()) + size_x * (c.y() - location.y());
            let tile = TerrainTile {
                ty: template,
                index: index as u8,
            };
            map.set_custom_terrain(c, map.terrain.terrain_index_of_tile(tile));
        }
    }

    fn link_neighbouring_bridges(&self, layer: &LegacyBridgeLayer) {
        for d in 0..=1usize {
            if self.neighbour(d).is_some() {
                continue;
            }
            let offset = if d == 0 { &self.info.north_offset } else { &self.info.south_offset };
            if offset.len() < 2 {
                continue;
            }
            let n = layer.get_bridge(self.self_.location() + CVec::new(offset[0], offset[1]));
            if let Some(n) = &n {
                n.neighbours.borrow_mut()[1 - d] = Some(self.me());
            }
            self.neighbours.borrow_mut()[d] = n;
        }
    }

    fn add_hut(&self, hut: &Rc<LegacyBridgeHut>) {
        {
            let mut huts = self.huts.borrow_mut();
            if same_hut(&huts[0], &huts[1]) {
                huts[1] = Some(hut.clone());
            }
        }
        if self.own_hut().is_some() {
            *self.hut.borrow_mut() = None;
            return;
        }
        *self.hut.borrow_mut() = Some(hut.clone());
        {
            let mut huts = self.huts.borrow_mut();
            if huts[0].is_none() {
                huts[0] = Some(hut.clone());
            }
        }
        for d in 0..=1 {
            let mut next = self.neighbour(d);
            while let Some(b) = next {
                b.huts.borrow_mut()[d] = Some(hut.clone());
                next = if b.own_hut().is_none() { b.neighbour(d) } else { None };
            }
        }
    }

    fn get_hut(&self, index: usize) -> Option<Rc<LegacyBridgeHut>> {
        self.huts.borrow()[index].clone()
    }

    fn kill_units_on_bridge(&self) {
        let world = self.self_.world();
        let footprint = self.footprint.borrow().clone();
        for (c, _) in footprint {
            for a in world.actor_map().get_actors_at(c) {
                let cannot_exist = a
                    .traits_implementing::<dyn IPositionable>()
                    .first()
                    .is_some_and(|p| !p.can_exist_in_cell(c));
                if cannot_exist {
                    kill_actor(&a, Some(&self.self_), &self.info.damage_types);
                }
            }
        }
    }

    fn neighbour_is_dead_shore(&self, neighbour: &Option<Rc<Bridge>>) -> bool {
        neighbour
            .as_ref()
            .is_some_and(|n| self.info.shore_pieces.iter().any(|p| *p == n.self_.info.name) && n.health().is_dead())
    }

    fn long_bridge_segment_is_dead(&self) -> bool {
        if !self.info.long {
            return self.health().is_dead();
        }
        self.neighbour_is_dead_shore(&self.neighbour(0)) || self.neighbour_is_dead_shore(&self.neighbour(1)) || self.health().is_dead()
    }

    fn choose_template(&self) -> u16 {
        let info = &self.info;
        if info.long && self.long_bridge_segment_is_dead() {
            let previous_is_dead = self.neighbour(0).is_some_and(|n| n.long_bridge_segment_is_dead());
            let next_is_dead = self.neighbour(1).is_some_and(|n| n.long_bridge_segment_is_dead());
            return match (previous_is_dead, next_is_dead) {
                (true, true) => info.destroyed_plus_both_template,
                (true, false) => info.destroyed_plus_north_template,
                (false, true) => info.destroyed_plus_south_template,
                (false, false) => info.destroyed_template,
            };
        }
        let ds = self.health().damage_state();
        if ds == DamageState::DEAD && info.destroyed_template > 0 {
            info.destroyed_template
        } else if ds >= DamageState::HEAVY && info.damaged_template > 0 {
            info.damaged_template
        } else {
            info.template
        }
    }

    fn update_state(&self) {
        let new_template = self.choose_template();
        if new_template == self.template.get() {
            return;
        }
        self.template.set(new_template);
        self.apply_terrain();
        if self.long_bridge_segment_is_dead() && !self.killed_units.get() {
            self.killed_units.set(true);
            self.kill_units_on_bridge();
        }
    }

    fn repair(self: &Rc<Self>, repairer: &ActorRef, direction: i32, on_complete: Rc<LegacyBridgeHut>) {
        let initial_damage = self.health().damage_state();
        let world = self.self_.world();
        let (me, by, done) = (self.clone(), repairer.clone(), on_complete.clone());
        world.add_frame_end_task(Box::new(move |_| {
            let health = me.health();
            if health.is_dead() {
                health.resurrect(&me.self_, Some(&by));
                me.killed_units.set(false);
                me.kill_units_on_bridge();
            } else {
                let damage = Damage::new(-health.max_hp(), DamageTypes::default());
                health.inflict_damage(&me.self_, Some(&by), &damage, true);
            }
            let finished = if direction < 0 {
                me.neighbour(0).is_none() && me.neighbour(1).is_none()
            } else {
                me.own_hut().is_some() || me.neighbour(direction as usize).is_none()
            };
            if finished {
                done.repair_directions.set(done.repair_directions.get() - 1);
            }
        }));
        if direction < 0 || self.own_hut().is_some() {
            return;
        }
        let Some(next) = self.neighbour(direction as usize) else {
            return;
        };
        let delay = if initial_damage == DamageState::UNDAMAGED || self.neighbour_is_dead_shore(&Some(next.clone())) {
            0
        } else {
            self.info.repair_propagation_delay
        };
        let by = repairer.clone();
        world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
            w.add_effect(DelayedAction::new(
                delay,
                BridgeAction::Repair {
                    bridge: next,
                    by,
                    direction,
                    hut: on_complete,
                },
            ));
        }));
    }

    fn aggregate_into(&self, direction: i32, damage: &mut DamageState) {
        let ds = self.health().damage_state();
        if ds > *damage {
            *damage = ds;
        }
        if direction >= 0
            && self.own_hut().is_none()
            && let Some(n) = self.neighbour(direction as usize)
        {
            n.aggregate_into(direction, damage);
        }
    }

    /// `AggregateDamageState`: the worst span damage before the other hut.
    fn aggregate_damage_state(&self) -> DamageState {
        let mut damage = self.health().damage_state();
        self.for_each(|b, d| b.aggregate_into(d, &mut damage));
        damage
    }

    fn demolish(self: &Rc<Self>, saboteur: &ActorRef, direction: i32, damage_types: BitSet<DamageTypeTag>) {
        let initial_damage = self.health().damage_state();
        let world = self.self_.world();
        let (me, by, types) = (self.clone(), saboteur.clone(), damage_types);
        world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
            if let Some(weapon) = w.rules().weapon(&me.info.demolish_weapon) {
                let target = Target::from_pos(me.self_.center_position());
                rustora_engine::weapon::weapon_impact(&weapon, &target, &by);
            }
            kill_actor(&me.self_, Some(&by), &types);
        }));
        if direction < 0 || self.own_hut().is_some() {
            return;
        }
        let Some(next) = self.neighbour(direction as usize) else {
            return;
        };
        let delay = if initial_damage == DamageState::DEAD || self.neighbour_is_dead_shore(&Some(next.clone())) {
            0
        } else {
            self.info.repair_propagation_delay
        };
        let by = saboteur.clone();
        world.add_frame_end_task(Box::new(move |w: &Rc<World>| {
            w.add_effect(DelayedAction::new(
                delay,
                BridgeAction::Demolish {
                    bridge: next,
                    by,
                    direction,
                    damage_types,
                },
            ));
        }));
    }
}

impl INotifyCreated for Bridge {
    fn created(&self, _self_: &ActorRef) {
        self.health().remove_on_death.set(false);
    }
}

impl IRender for Bridge {
    fn render(&self, self_: &ActorRef, out: &mut Vec<Renderable>) {
        let world = self_.world();
        let center_y = self_
            .info
            .trait_info::<BuildingInfo>()
            .expect("Bridge requires Building")
            .center_offset(&world)
            .y;
        let template = self.template.get();
        for &(c, index) in self.footprint.borrow().iter() {
            out.push(Renderable::Tile {
                tile: TerrainTile { ty: template, index },
                pos: world.map().center_of_cell(c),
                z_offset: -(center_y + 1024),
            });
        }
    }
}

impl INotifyDamageStateChanged for Bridge {
    fn damage_state_changed(&self, self_: &ActorRef, _e: &rustora_engine::damage::AttackInfo) {
        self.for_each(|b, _| b.update_state());
        if self.info.shore_pieces.iter().any(|p| *p == self_.info.name) {
            for d in 0..=1 {
                if let Some(far) = self.neighbour(d).and_then(|n| n.neighbour(d)) {
                    far.update_state();
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct LegacyBridgeHutInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct LegacyBridgeHut {
    bridge: RefCell<Option<Rc<Bridge>>>,
    repair_directions: Cell<i32>,
}

register_traits!(LegacyBridgeHut: IDemolishable);
info_impl!(LegacyBridgeHutInfo => |_info, init| {
    let parent = init.inits.get::<ParentActorInit>().expect("LegacyBridgeHut needs a parent bridge").0.clone();
    let hut = Rc::new_cyclic(|me: &Weak<LegacyBridgeHut>| {
        let me = me.clone();
        init.self_.world().add_frame_end_task(Box::new(move |_| {
            let (Some(hut), bridge) = (me.upgrade(), parent.trait_::<Bridge>()) else {
                return;
            };
            *hut.bridge.borrow_mut() = Some(bridge.clone());
            bridge.add_hut(&hut);
        }));
        LegacyBridgeHut {
            bridge: RefCell::new(None),
            repair_directions: Cell::new(0),
        }
    });
    Some(hut)
});

impl LegacyBridgeHut {
    fn bridge(&self) -> Rc<Bridge> {
        self.bridge
            .borrow()
            .clone()
            .expect("the hut is linked at the end of its first frame")
    }

    fn bridge_damage_state(&self) -> DamageState {
        self.bridge().aggregate_damage_state()
    }

    fn repairing(&self) -> bool {
        self.repair_directions.get() > 0
    }

    fn is_this(&self, other: Option<Rc<LegacyBridgeHut>>) -> bool {
        other.is_some_and(|o| std::ptr::eq(Rc::as_ptr(&o), self))
    }

    fn repair(self: &Rc<Self>, repairer: &ActorRef) {
        let bridge = self.bridge();
        let directions = if !self.is_this(bridge.get_hut(0)) && !self.is_this(bridge.get_hut(1)) {
            2
        } else {
            1
        };
        self.repair_directions.set(directions);
        bridge.for_each(|b, d| b.repair(repairer, d, self.clone()));
    }

    /// `RepairsBridges` refuses a hut with an undamaged, repairing or dangling bridge.
    fn cannot_be_ordered_to_repair(&self) -> bool {
        self.bridge_damage_state() == DamageState::UNDAMAGED || self.repairing() || self.bridge().is_dangling()
    }

    /// `RepairBridge.CanEnterHut`.
    fn can_enter(&self) -> bool {
        let bridge = self.bridge();
        self.bridge_damage_state() != DamageState::UNDAMAGED
            && !self.repairing()
            && bridge.get_hut(0).is_some()
            && bridge.get_hut(1).is_some()
    }
}

impl IDemolishable for LegacyBridgeHut {
    fn is_valid_target(&self, _self_: &ActorRef, _saboteur: &ActorRef) -> bool {
        self.bridge_damage_state() != DamageState::DEAD
    }

    fn demolish(&self, self_: &ActorRef, saboteur: &ActorRef, delay: i32, damage_types: BitSet<DamageTypeTag>) {
        self_.world().add_effect(DelayedAction::new(
            delay,
            BridgeAction::HutDemolish {
                hut: self_.clone(),
                by: saboteur.clone(),
                bridge: self.bridge(),
                damage_types,
            },
        ));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct LegacyBridgeLayerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub bridges: Vec<String>,
}

impl Default for LegacyBridgeLayerInfo {
    fn default() -> Self {
        LegacyBridgeLayerInfo {
            instance_name: None,
            bridges: vec!["bridge1".into(), "bridge2".into()],
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct LegacyBridgeLayer {
    info: Rc<LegacyBridgeLayerInfo>,
    bridges: RefCell<HashMap<CPos, Rc<Bridge>>>,
}

register_traits!(LegacyBridgeLayer: IWorldLoaded);
info_impl!(LegacyBridgeLayerInfo => |info, _init| Some(Rc::new(LegacyBridgeLayer {
    info,
    bridges: RefCell::new(HashMap::new()),
})));

impl LegacyBridgeLayer {
    fn get_bridge(&self, cell: CPos) -> Option<Rc<Bridge>> {
        self.bridges.borrow().get(&cell).cloned()
    }

    fn convert_bridge_to_actor(&self, world: &Rc<World>, cell: CPos, types: &BTreeMap<u16, (String, i32)>) {
        if self.bridges.borrow().contains_key(&cell) {
            return;
        }
        let map = world.map();
        let tile = *map.tiles.at(cell);
        let (size_x, size_y) = map.terrain.templates[&tile.ty].size;
        let (ni, nj) = (cell.x() - tile.index as i32 % size_x, cell.y() - tile.index as i32 / size_x);
        let (name, health) = &types[&tile.ty];
        let actor = world.create_actor(
            name,
            ActorInits::new()
                .with(LocationInit::new(CPos::new(ni, nj)))
                .with(OwnerInit::from_player(&world.world_actor().owner()))
                .with(HealthInit::new(*health))
                .with(AllowZeroHealthInit),
        );
        let bridge = actor.trait_::<Bridge>();
        let mut footprint = Vec::new();
        for ind in 0..size_x * size_y {
            let subtile = CPos::new(ni + ind % size_x, nj + ind / size_x);
            if !map.tiles.contains_cell(subtile) {
                continue;
            }
            let sub = *map.tiles.at(subtile);
            if sub.ty != tile.ty || sub.index as i32 != ind {
                continue;
            }
            footprint.push((subtile, ind as u8));
            self.bridges.borrow_mut().insert(subtile, bridge.clone());
        }
        bridge.create(tile.ty, footprint);
    }
}

impl IWorldLoaded for LegacyBridgeLayer {
    fn world_loaded(&self, world: &Rc<World>) {
        if world.is_restoring() {
            return;
        }
        let mut types = BTreeMap::new();
        for name in &self.info.bridges {
            let info = world
                .rules()
                .actor(name)
                .expect("bridge actor")
                .trait_info::<BridgeInfo>()
                .expect("bridge actor has Bridge");
            for (template, health) in info.templates() {
                types.insert(template, (name.clone(), health));
            }
        }
        for cell in world.map().all_cells() {
            if types.contains_key(&world.map().tiles.at(cell).ty) {
                self.convert_bridge_to_actor(world, cell, &types);
            }
        }
        for (_, bridge) in world.actors_with_trait::<Bridge>() {
            bridge.link_neighbouring_bridges(self);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RepairsBridgesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub voice: String,
    pub enter_behaviour: EnterBehaviour,
    pub target_cursor: String,
    pub target_blocked_cursor: String,
    pub repair_notification: Option<String>,
    pub repair_text_notification: Option<String>,
}

impl Default for RepairsBridgesInfo {
    fn default() -> Self {
        RepairsBridgesInfo {
            instance_name: None,
            voice: "Action".into(),
            enter_behaviour: EnterBehaviour::Dispose,
            target_cursor: "goldwrench".into(),
            target_blocked_cursor: "goldwrench-blocked".into(),
            repair_notification: None,
            repair_text_notification: None,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct RepairsBridges {
    info: Rc<RepairsBridgesInfo>,
}

register_traits!(RepairsBridges: IIssueOrder, IResolveOrder, IOrderVoice);
info_impl!(RepairsBridgesInfo => |info, _init| Some(Rc::new(RepairsBridges { info })));

fn order_hut(order: &Order) -> Option<Rc<LegacyBridgeHut>> {
    if order.order_string != "RepairBridge" || order.target.target_type() != TargetType::Actor {
        return None;
    }
    order.target.actor()?.trait_or_default::<LegacyBridgeHut>()
}

impl IIssueOrder for RepairsBridges {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        let info = self.info.clone();
        vec![Box::new(UnitOrderTargeter::new(
            "RepairBridge",
            6,
            &self.info.target_cursor,
            true,
            true,
            Box::new(move |_self, target, modifiers, cursor| {
                if modifiers.has_modifier(TargetModifiers::FORCE_MOVE) {
                    return false;
                }
                let Some(hut) = target.trait_or_default::<LegacyBridgeHut>() else {
                    return false;
                };
                let damage = hut.bridge_damage_state();
                if !modifiers.has_modifier(TargetModifiers::FORCE_ATTACK) && damage != DamageState::DEAD {
                    return false;
                }
                if hut.cannot_be_ordered_to_repair() {
                    *cursor = Some(info.target_blocked_cursor.clone());
                }
                true
            }),
            Box::new(|_, _, _, _| false),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "RepairBridge").then(|| Order::new("RepairBridge", Some(self_.clone()), target.clone(), queued))
    }
}

impl IOrderVoice for RepairsBridges {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        let hut = order_hut(order)?;
        (!hut.cannot_be_ordered_to_repair()).then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for RepairsBridges {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        let Some(hut) = order_hut(order) else {
            return;
        };
        if hut.cannot_be_ordered_to_repair() {
            return;
        }
        let hooks = RepairBridgeHooks {
            info: self.info.clone(),
            enter_actor: RefCell::new(None),
            enter_hut: RefCell::new(None),
        };
        self_.queue_activity_queued(
            order.queued,
            rustora_engine::activity::with_line_color(
                Enter::new(self_, &order.target, Box::new(hooks)),
                rustora_engine::activity::LINE_YELLOW,
            ),
        );
        crate::target_lines::show_target_lines(self_);
    }
}

/// `RepairBridge`.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
struct RepairBridgeHooks {
    info: Rc<RepairsBridgesInfo>,
    enter_actor: RefCell<Option<ActorRef>>,
    enter_hut: RefCell<Option<Rc<LegacyBridgeHut>>>,
}

impl RepairBridgeHooks {
    fn can_enter_hut(&self) -> bool {
        self.enter_hut.borrow().as_ref().is_some_and(|h| h.can_enter())
    }
}

impl EnterHooks for RepairBridgeHooks {
    fn type_name(&self) -> &'static str {
        "RepairBridge"
    }

    fn try_start_enter(&self, enter: &Enter, self_: &ActorRef, target_actor: &ActorRef) -> bool {
        *self.enter_actor.borrow_mut() = Some(target_actor.clone());
        *self.enter_hut.borrow_mut() = target_actor.trait_or_default::<LegacyBridgeHut>();
        if !self.can_enter_hut() {
            enter.cancel_keep_queue(self_);
            return false;
        }
        true
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        if self.enter_actor.borrow().as_ref().is_none_or(|a| a.id != target_actor.id) || !self.can_enter_hut() {
            return;
        }
        let hut = self.enter_hut.borrow().clone();
        if let Some(hut) = hut {
            hut.repair(self_);
        }
        let (world, owner) = (self_.world(), self_.owner());
        if let Some(n) = self.info.repair_notification.as_deref() {
            sound::play_notification(&world, Some(&owner), "Speech", n, Some(&owner.faction));
        }
        world
            .sound
            .add_transient_line(Some(&owner), self.info.repair_text_notification.as_deref());
        match self.info.enter_behaviour {
            EnterBehaviour::Dispose => self_.dispose(),
            EnterBehaviour::Suicide => kill_actor(self_, Some(self_), &DamageTypes::default()),
            EnterBehaviour::Exit => {}
        }
    }
}

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_effects!(DelayedAction);
    rustora_engine::persist_values!(dyn crate::cargo::EnterHooks: RepairBridgeHooks);
}
