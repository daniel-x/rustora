// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Parachutable.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::damage::DamageTypes;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_map::WorldExt;
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;

pub fn register(r: &mut TraitRegistry) {
    r.register("Parachutable", |n| load_info!(ParachutableInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ParachutableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub killed_on_impassable_terrain: bool,
    pub damage_types: DamageTypes,
    pub water_terrain_types: IndexSet<String>,
    pub fall_rate: i32,
    pub parachuting_condition: Option<String>,
}

impl Default for ParachutableInfo {
    fn default() -> Self {
        ParachutableInfo {
            instance_name: None,
            killed_on_impassable_terrain: true,
            damage_types: DamageTypes::default(),
            water_terrain_types: IndexSet::from(["Water".to_string()]),
            fall_rate: 13,
            parachuting_condition: None,
        }
    }
}

pub struct Parachutable {
    info: Rc<ParachutableInfo>,
    positionable: Rc<dyn IPositionable>,
    pub ignore_actor: RefCell<Option<ActorRef>>,
    token: Cell<i32>,
    is_in_air: Cell<bool>,
}

register_traits!(Parachutable: INotifyParachute);
info_impl!(ParachutableInfo => |info, init| Some(Rc::new(Parachutable {
    info,
    positionable: init.self_.trait_::<dyn IPositionable>(),
    ignore_actor: RefCell::new(None),
    token: Cell::new(INVALID_CONDITION_TOKEN),
    is_in_air: Cell::new(false),
})));

pub fn fall_rate(a: &ActorRef) -> i32 {
    a.info.trait_info::<ParachutableInfo>().expect("Parachutable").fall_rate
}

impl Parachutable {
    pub fn is_in_air(&self) -> bool {
        self.is_in_air.get()
    }
}

impl INotifyParachute for Parachutable {
    fn on_parachute(&self, self_: &ActorRef) {
        self.is_in_air.set(true);
        if self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(self.info.parachuting_condition.as_deref().unwrap_or("")));
        }
        crate::activities::notify_blocker(self_, self_.location());
    }

    fn on_landed(&self, self_: &ActorRef) {
        self.is_in_air.set(false);
        if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
        if !self.info.killed_on_impassable_terrain {
            return;
        }
        let cell = self_.location();
        if self.positionable.can_enter_cell(cell, Some(self_), BlockedByActor::All) {
            return;
        }
        let w = self_.world();
        if let Some(ig) = self.ignore_actor.borrow().as_ref() {
            let blocked = w
                .actor_map()
                .get_actors_at(cell)
                .iter()
                .any(|a| a.id != ig.id && a.id != self_.id && w.map().distance_above_terrain(a.center_position()).length == 0);
            if !blocked {
                return;
            }
        }
        crate::health::kill_actor(self_, Some(self_), &self.info.damage_types);
    }
}
