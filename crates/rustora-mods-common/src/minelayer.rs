// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Minelayer.cs`, `Activities/LayMines.cs` and `Traits/Mine.cs`. The minefield order generator
//! and `INotifyMineLaying` (no RA implementer) are left out.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::world_types::{CPos, WDist};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef, for_each_activity};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInits, LocationInit, OwnerInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{activity_common, info_impl, load_info, register_traits};

use crate::activities::{MoveAdjacentTo, MoveCooldownHelper, Wait};
use crate::actor_map::WorldExt;
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::Mobile;
use crate::resupply::{AmmoPool, RearmableInfo, Resupply};
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Minelayer", |n| load_info!(MinelayerInfo, n));
    r.register("Mine", |n| load_info!(MineInfo, n));
    r.register("MineImmune", |n| load_info!(MineImmuneInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MinelayerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub mine: String,
    pub ammo_pool_name: String,
    pub minefield_depth: WDist,
    pub terrain_types: IndexSet<String>,
    pub ammo_usage: i32,
    pub pre_lay_delay: i32,
    pub after_laying_delay: i32,
}

impl Default for MinelayerInfo {
    fn default() -> Self {
        MinelayerInfo {
            instance_name: None,
            mine: "minv".into(),
            ammo_pool_name: "primary".into(),
            minefield_depth: WDist::new(1536),
            terrain_types: IndexSet::new(),
            ammo_usage: 1,
            pre_lay_delay: 0,
            after_laying_delay: 20,
        }
    }
}

pub struct Minelayer {
    pub info: Rc<MinelayerInfo>,
    minefield_start: Cell<CPos>,
}

register_traits!(Minelayer: IResolveOrder, ISync, ITick);
info_impl!(MinelayerInfo => |info, _| Some(Rc::new(Minelayer { info, minefield_start: Cell::new(CPos::ZERO) })));

impl ISync for Minelayer {
    fn sync_hash(&self) -> i32 {
        self.minefield_start.get().hash()
    }
}

impl Minelayer {
    pub fn is_cell_acceptable(&self, self_: &ActorRef, cell: CPos) -> bool {
        let w = self_.world();
        if !w.map().contains(cell) {
            return false;
        }
        self.info.terrain_types.is_empty() || self.info.terrain_types.contains(&w.map().terrain_type_info(cell).ty)
    }
}

/// `Minelayer.GetMinefieldCells`, with the single precision math of `float2`.
fn get_minefield_cells(start: CPos, end: CPos, depth: WDist) -> Vec<CPos> {
    let (min_x, min_y) = (start.x().min(end.x()), start.y().min(end.y()));
    let (max_x, max_y) = (start.x().max(end.x()), start.y().max(end.y()));
    let p = end - start;
    let mut q = (p.y as f32, -(p.x as f32));
    if start != end {
        let len = (q.0 * q.0 + q.1 * q.1).sqrt();
        let inv = 1.0 / len;
        q = (inv * q.0, inv * q.1);
    } else {
        q = (1.0, 0.0);
    }
    let c = -(q.0 * start.x() as f32 + q.1 * start.y() as f32);
    let mut out = Vec::new();
    for i in min_x..=max_x {
        for j in min_y..=max_y {
            if (q.0 * i as f32 + q.1 * j as f32 + c).abs() * 1024.0 < depth.length as f32 {
                out.push(CPos::new(i, j));
            }
        }
    }
    out
}

impl IResolveOrder for Minelayer {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        let s = order.order_string.as_str();
        if s != "BeginMinefield" && s != "PlaceMinefield" && s != "PlaceMine" {
            return;
        }
        if !order.target.is_valid_for(self_) {
            return;
        }
        let cell = self_.world().map().cell_containing(order.target.center_position());
        match s {
            "BeginMinefield" => self.minefield_start.set(cell),
            "PlaceMine" => {
                if self.is_cell_acceptable(self_, cell) {
                    self_.queue_activity_queued(order.queued, LayMines::new(self_, None));
                }
            }
            _ => {
                let start = order.extra_location;
                self.minefield_start.set(start);
                let mobile = self_.trait_or_default::<Mobile>();
                let shroud = self_.owner().shroud();
                let mut minefield: Vec<CPos> = get_minefield_cells(start, cell, self.info.minefield_depth)
                    .into_iter()
                    .filter(|&c| {
                        self.is_cell_acceptable(self_, c)
                            && shroud.is_explored(c)
                            && mobile.as_ref().is_some_and(|m| m.can_enter_cell(c, None, BlockedByActor::Immovable) && m.can_stay_in_cell(c))
                    })
                    .collect();
                minefield.sort_by_key(|&c| (c - start).length_squared());
                self_.queue_activity_queued(order.queued, LayMines::new(self_, Some(minefield)));
            }
        }
    }
}

impl ITick for Minelayer {
    fn tick(&self, self_: &ActorRef) {
        let mut fields = Vec::new();
        for_each_activity(self_.current_activity(), true, &mut |a| {
            if a.as_any().downcast_ref::<LayMines>().is_some() {
                fields.push(a.clone());
            }
        });
        for f in fields {
            f.as_any().downcast_ref::<LayMines>().unwrap().clean_mine_field(self_);
        }
    }
}

/// `Activities/LayMines.cs`.
pub struct LayMines {
    base: ActivityBase,
    minelayer: Rc<Minelayer>,
    ammo_pools: Vec<Rc<AmmoPool>>,
    movement: Rc<dyn IMove>,
    mobile: Option<Rc<Mobile>>,
    rearmable_info: Option<Rc<RearmableInfo>>,
    helper: MoveCooldownHelper,
    minefield: RefCell<Option<Vec<CPos>>>,
    laying_mine: Cell<bool>,
}

impl LayMines {
    pub fn new(self_: &ActorRef, minefield: Option<Vec<CPos>>) -> ActivityRef {
        let mobile = self_.trait_or_default::<Mobile>();
        let helper = MoveCooldownHelper::new(mobile.clone());
        helper.retry_if_destination_blocked.set(true);
        Rc::new(LayMines {
            base: ActivityBase::default(),
            minelayer: self_.trait_::<Minelayer>(),
            ammo_pools: self_.traits_implementing::<AmmoPool>(),
            movement: self_.trait_::<dyn IMove>(),
            mobile,
            rearmable_info: self_.info.trait_info::<RearmableInfo>(),
            helper,
            minefield: RefCell::new(minefield),
            laying_mine: Cell::new(false),
        })
    }

    fn can_lay_mine(self_: &ActorRef, p: CPos) -> bool {
        self_.world().actor_map().get_actors_at(p).iter().all(|a| a.id == self_.id)
    }

    fn next_valid_cell(&self, self_: &ActorRef) -> Option<CPos> {
        self.minefield.borrow().as_ref()?.iter().copied().find(|&c| Self::can_lay_mine(self_, c))
    }

    fn pool(&self) -> Option<&Rc<AmmoPool>> {
        self.ammo_pools.iter().find(|p| p.info.name == self.minelayer.info.ammo_pool_name)
    }

    fn start_laying_mine(&self) -> bool {
        let Some(pool) = self.pool() else { return false };
        pool.current_ammo_count() >= self.minelayer.info.ammo_usage
    }

    fn lay_mine(&self, self_: &ActorRef) -> bool {
        let Some(pool) = self.pool() else { return false };
        if !pool.take_ammo(self_, self.minelayer.info.ammo_usage) {
            return false;
        }
        let loc = self_.location();
        if let Some(m) = self.minefield.borrow_mut().as_mut() {
            if let Some(i) = m.iter().position(|&c| c == loc) {
                m.remove(i);
            }
        }
        let (a, mine) = (self_.clone(), self.minelayer.info.mine.clone());
        self_.world().add_frame_end_task(Box::new(move |w| {
            if !Self::can_lay_mine(&a, a.location()) {
                return;
            }
            w.create_actor(&mine, ActorInits::new().with(LocationInit::new(a.location())).with(OwnerInit::from_player(&a.owner())));
        }));
        true
    }

    pub fn clean_mine_field(&self, self_: &ActorRef) {
        let mut mf = self.minefield.borrow_mut();
        let Some(minefield) = mf.as_mut() else { return };
        let w = self_.world();
        let owner = self_.owner();
        let shroud = owner.shroud();
        let mine = &self.minelayer.info.mine;
        let positionable = self_.trait_::<dyn IPositionable>();
        minefield.retain(|&c| {
            let has_mine = w.actor_map().get_actors_at(c).iter().any(|a| a.info.name.eq_ignore_ascii_case(mine) && a.can_be_viewed_by_player(Some(&owner)));
            let blocked = (!positionable.can_enter_cell(c, None, BlockedByActor::Immovable) || self.mobile.as_ref().is_some_and(|m| !m.can_stay_in_cell(c))) && shroud.is_visible(c);
            !(has_mine || blocked)
        });
    }
}

impl Activity for LayMines {
    activity_common!("LayMines");

    fn on_first_run(&self, self_: &ActorRef) {
        let mut mf = self.minefield.borrow_mut();
        if mf.is_none() {
            *mf = Some(vec![self_.location()]);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let info = &self.minelayer.info;
        if self.laying_mine.get() {
            self.laying_mine.set(false);
            if self.lay_mine(self_) {
                if info.after_laying_delay > 0 {
                    self.base.queue_child(Wait::new(info.after_laying_delay));
                }
                return false;
            }
        }
        let w = self_.world();
        if let Some(r) = self.helper.tick(&w, false) {
            return r;
        }
        let loc = self_.location();
        let in_field = self.minefield.borrow().as_ref().is_none_or(|m| m.contains(&loc));
        if in_field && Self::can_lay_mine(self_, loc) {
            if let Some(ri) = &self.rearmable_info {
                if self.ammo_pools.iter().any(|p| p.info.name == info.ammo_pool_name && !p.has_ammo()) {
                    let owner = self_.owner();
                    let candidates = w.actors().into_iter().filter(|a| owner.relationship_with(Some(&a.owner())) == PlayerRelationship::ALLY && ri.rearm_actors.contains(&a.info.name));
                    let Some(rearm_target) = util::closest_to_with_path_from(candidates, self_) else { return true };
                    self.helper.notify_move_queued();
                    let t = Target::from_actor(Some(&rearm_target));
                    self.base.queue_child(MoveAdjacentTo::new_adjacent(self_, &t, None));
                    self.base.queue_child(self.movement.move_to(w.map().cell_containing(rearm_target.center_position()), 0, Some(rearm_target.clone()), false));
                    self.base.queue_child(Resupply::new(self_, &rearm_target, WDist::new(512), false));
                    return false;
                }
            }
            if !self.start_laying_mine() {
                return false;
            }
            if info.pre_lay_delay == 0 {
                if self.lay_mine(self_) && info.after_laying_delay > 0 {
                    self.base.queue_child(Wait::new(info.after_laying_delay));
                }
            } else {
                self.laying_mine.set(true);
                self.base.queue_child(Wait::new(info.pre_lay_delay));
            }
            return false;
        }
        if let Some(next) = self.next_valid_cell(self_) {
            self.helper.notify_move_queued();
            self.base.queue_child(self.movement.move_to(next, 0, None, false));
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MineInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub crush_classes: BitSet<CrushClass>,
    pub avoid_friendly: bool,
    pub block_friendly: bool,
    pub detonate_classes: BitSet<CrushClass>,
}

impl Default for MineInfo {
    fn default() -> Self {
        MineInfo { instance_name: None, crush_classes: BitSet::default(), avoid_friendly: true, block_friendly: true, detonate_classes: BitSet::default() }
    }
}

pub struct Mine {
    info: Rc<MineInfo>,
}

register_traits!(Mine: ICrushable, INotifyCrushed);
info_impl!(MineInfo => |info, _| Some(Rc::new(Mine { info })));

fn is_mine_immune(a: &ActorRef) -> bool {
    a.info.trait_info::<MineImmuneInfo>().is_some()
}

impl INotifyCrushed for Mine {
    fn warn_crush(&self, _self_: &ActorRef, _crusher: &ActorRef, _classes: BitSet<CrushClass>) {}

    fn on_crush(&self, self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>) {
        if !self.info.crush_classes.overlaps(&classes) {
            return;
        }
        if is_mine_immune(crusher) || (self_.owner().relationship_with(Some(&crusher.owner())) == PlayerRelationship::ALLY && self.info.avoid_friendly) {
            return;
        }
        let mobile = crusher.trait_or_default::<Mobile>();
        if let Some(m) = &mobile {
            if !self.info.detonate_classes.overlaps(&m.locomotor_info().crushes) {
                return;
            }
        }
        let types = mobile.map(|m| m.locomotor_info().crush_damage_types.clone()).unwrap_or_default();
        if let Some(h) = self_.cache().health.clone() {
            h.kill(self_, Some(crusher), &types);
        }
    }
}

impl ICrushable for Mine {
    fn crushable_by_actor(&self, self_: &ActorRef, crusher: &ActorRef, classes: BitSet<CrushClass>) -> bool {
        if self.info.block_friendly && !is_mine_immune(crusher) && self_.owner().relationship_with(Some(&crusher.owner())) == PlayerRelationship::ALLY {
            return false;
        }
        self.info.crush_classes.overlaps(&classes)
    }

    fn crushable_by(&self, self_: &ActorRef, classes: BitSet<CrushClass>) -> u64 {
        if !self.info.crush_classes.overlaps(&classes) {
            return 0;
        }
        let all = self_.world().all_players_mask.get();
        if self.info.block_friendly { all & !self_.owner().allied_players_mask.get() } else { all }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct MineImmuneInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(MineImmuneInfo => |_, _| None);
