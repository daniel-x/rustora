// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Buildings/Gate.cs` and `WorldUtils.ContainsTemporaryBlocker`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::world_types::{CPos, WDist};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::LocationInit;
use rustora_engine::conditional::*;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_map::WorldExt;
use crate::interfaces::*;
use crate::locomotor::building_of;

pub fn register(r: &mut TraitRegistry) {
    r.register("Gate", |n| load_info!(GateInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GateInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub opening_sound: Option<String>,
    pub closing_sound: Option<String>,
    pub close_delay: i32,
    pub transition_delay: i32,
    pub blocks_projectiles_height: WDist,
    pub blocks_projectiles_valid_relationships: PlayerRelationship,
}

impl Default for GateInfo {
    fn default() -> Self {
        GateInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            opening_sound: None,
            closing_sound: None,
            close_delay: 150,
            transition_delay: 33,
            blocks_projectiles_height: WDist::new(640),
            blocks_projectiles_valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
        }
    }
}

/// A building that opens for friendly actors and closes when nothing stands in it.
#[derive(rustora_engine::persist::Persist)]
pub struct Gate {
    info: Rc<GateInfo>,
    state: ConditionalState,
    footprint: Vec<CPos>,
    blocked_positions: RefCell<Vec<CPos>>,
    open_position: i32,
    position: Cell<i32>,
    desired_position: Cell<i32>,
    remaining_open_time: Cell<i32>,
}

register_traits!(Gate: ITick, ITemporaryBlocker, IBlocksProjectiles, INotifyAddedToWorld, INotifyRemovedFromWorld,
    INotifyBlockingMove, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GateInfo => |info, init| {
    let top_left = init.inits.get::<LocationInit>().expect("Gate needs a LocationInit").value;
    let building = init.self_.info.trait_info::<crate::building::BuildingInfo>().expect("Gate requires Building");
    let footprint = building.tiles(top_left);
    Some(Rc::new(Gate {
        state: ConditionalState::new_pausable(&info.base),
        blocked_positions: RefCell::new(footprint.clone()),
        footprint,
        open_position: info.transition_delay,
        position: Cell::new(info.transition_delay),
        desired_position: Cell::new(0),
        remaining_open_time: Cell::new(0),
        info,
    }))
});

impl ConditionalHooks for Gate {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl INotifyCreated for Gate {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}
rustora_engine::conditional_observers_impl!(Gate);

impl Gate {
    pub fn position(&self) -> i32 {
        self.position.get()
    }

    pub fn open_position(&self) -> i32 {
        self.open_position
    }

    pub fn footprint(&self) -> &[CPos] {
        &self.footprint
    }

    fn can_remove(&self, self_: &ActorRef, blocking: &ActorRef) -> bool {
        !self.state.is_trait_disabled() && !self.state.is_trait_paused() && crate::util::appears_friendly_to(blocking, self_)
    }

    fn is_blocked(&self, self_: &ActorRef) -> bool {
        let w = self_.world();
        let am = w.actor_map();
        self.blocked_positions
            .borrow()
            .iter()
            .any(|c| am.get_actors_at(*c).iter().any(|a| a.id != self_.id))
    }

    fn play(&self, self_: &ActorRef, sound: &Option<String>) {
        if let Some(s) = sound {
            rustora_engine::sound::play_at(
                &self_.world(),
                rustora_engine::sound::SoundType::World,
                s,
                self_.center_position(),
                1.0,
            );
        }
    }
}

impl ISync for Gate {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.position.get()
    }
}

impl ITick for Gate {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() || self.state.is_trait_paused() {
            return;
        }
        let w = self_.world();
        let position = self.position.get();
        if self.desired_position.get() < position {
            if position == self.open_position {
                self.play(self_, &self.info.closing_sound);
                if let Some(b) = building_of(self_) {
                    w.actor_map().add_influence(self_, b);
                }
            }
            self.position.set(position - 1);
        } else if self.desired_position.get() > position {
            if position == 0 {
                self.play(self_, &self.info.opening_sound);
            }
            self.position.set(position + 1);
            if self.position.get() == self.open_position {
                if let Some(b) = building_of(self_) {
                    w.actor_map().remove_influence(self_, b);
                }
                self.remaining_open_time.set(self.info.close_delay);
            }
        }
        if self.position.get() == self.open_position {
            if self.is_blocked(self_) {
                self.remaining_open_time.set(self.info.close_delay);
            } else {
                self.remaining_open_time.set(self.remaining_open_time.get() - 1);
                if self.remaining_open_time.get() <= 0 {
                    self.desired_position.set(0);
                }
            }
        }
    }
}

impl ITemporaryBlocker for Gate {
    fn can_remove_blockage(&self, self_: &ActorRef, blocking: &ActorRef) -> bool {
        self.can_remove(self_, blocking)
    }

    fn is_blocking(&self, _self_: &ActorRef, cell: CPos) -> bool {
        self.position.get() != self.open_position && self.blocked_positions.borrow().contains(&cell)
    }
}

impl INotifyBlockingMove for Gate {
    fn on_notify_blocking_move(&self, self_: &ActorRef, blocking: &ActorRef) {
        if self.position.get() != self.open_position && self.can_remove(self_, blocking) {
            self.desired_position.set(self.open_position);
        }
    }
}

impl INotifyAddedToWorld for Gate {
    fn added_to_world(&self, _self_: &ActorRef) {
        *self.blocked_positions.borrow_mut() = self.footprint.clone();
    }
}

impl INotifyRemovedFromWorld for Gate {
    fn removed_from_world(&self, _self_: &ActorRef) {
        self.blocked_positions.borrow_mut().clear();
    }
}

impl IBlocksProjectiles for Gate {
    fn blocking_height(&self) -> WDist {
        WDist::new(self.info.blocks_projectiles_height.length * (self.open_position - self.position.get()) / self.open_position)
    }

    fn valid_relationships(&self) -> PlayerRelationship {
        self.info.blocks_projectiles_valid_relationships
    }

    fn is_trait_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }
}

/// `WorldUtils.ContainsTemporaryBlocker`.
pub fn contains_temporary_blocker(w: &World, cell: CPos, ignore_actor: Option<&ActorRef>) -> bool {
    for a in w.actor_map().get_actors_at(cell) {
        if ignore_actor.is_some_and(|i| i.id == a.id) {
            continue;
        }
        if a.traits_implementing::<dyn ITemporaryBlocker>()
            .iter()
            .any(|t| t.is_blocking(&a, cell))
        {
            return true;
        }
    }
    false
}
