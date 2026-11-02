// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Turreted.cs`.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;

use rustora_core::world_types::{WAngle, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_reference::TurretFacingInit;
use crate::attack::AttackBase;
use crate::interfaces::*;
use crate::render::BodyOrientation;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Turreted", |n| load_info!(TurretedInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TurretedInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub turret: String,
    pub turn_speed: WAngle,
    pub initial_facing: WAngle,
    pub realign_delay: i32,
    pub offset: WVec,
}

impl Default for TurretedInfo {
    fn default() -> Self {
        TurretedInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            turret: "primary".into(),
            turn_speed: WAngle::new(512),
            initial_facing: WAngle::ZERO,
            realign_delay: 40,
            offset: WVec::ZERO,
        }
    }
}

pub struct Turreted {
    pub info: Rc<TurretedInfo>,
    state: ConditionalState,
    attack: OnceCell<Option<Rc<AttackBase>>>,
    facing: OnceCell<Option<Rc<dyn IFacing>>>,
    body: OnceCell<Rc<BodyOrientation>>,
    pub quantized_facings: Cell<i32>,
    desired_direction: Cell<WVec>,
    realign_tick: Cell<i32>,
    realign_desired: Cell<bool>,
    local_orientation: Cell<WRot>,
}

register_traits!(Turreted: ITick, IDeathActorInitModifier, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TurretedInfo => |info, init| {
    let facing = init.inits.get_for::<TurretFacingInit>(info.instance_name.as_deref()).map_or(info.initial_facing, |f| f.value);
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(Turreted {
        info,
        state,
        attack: OnceCell::new(),
        facing: OnceCell::new(),
        body: OnceCell::new(),
        quantized_facings: Cell::new(0),
        desired_direction: Cell::new(WVec::ZERO),
        realign_tick: Cell::new(0),
        realign_desired: Cell::new(false),
        local_orientation: Cell::new(WRot::from_yaw(facing)),
    }))
});

impl ConditionalHooks for Turreted {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_disabled(&self, self_: &ActorRef) {
        if let Some(a) = self.attack.get().and_then(|a| a.as_ref()) {
            if a.is_aiming() {
                a.on_stop_order(self_);
            }
        }
    }
}

impl ISync for Turreted {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.quantized_facings.get()
    }
}

impl IObservesVariables for Turreted {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for Turreted {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl INotifyCreated for Turreted {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
        let attack = self_
            .traits_implementing::<AttackBase>()
            .into_iter()
            .find(|a| a.is_turreted_with(&self.info.turret));
        let _ = self.attack.set(attack);
        let _ = self.facing.set(self_.trait_or_default::<dyn IFacing>());
        let _ = self.body.set(self_.trait_::<BodyOrientation>());
    }
}

impl ITick for Turreted {
    fn tick(&self, _self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        if let Some(a) = self.attack.get().and_then(|a| a.as_ref()) {
            if a.is_aiming() {
                self.realign_tick.set(0);
                return;
            }
            if self.realign_tick.get() < self.info.realign_delay {
                self.realign_tick.set(self.realign_tick.get() + 1);
            } else if self.info.realign_delay > -1 {
                self.realign_desired.set(true);
                self.desired_direction.set(WVec::ZERO);
            }
            self.move_turret();
        } else {
            self.realign_tick.set(0);
            self.move_turret();
        }
    }
}

impl IDeathActorInitModifier for Turreted {
    fn modify_death_actor_init(&self, _self_: &ActorRef, init: &mut rustora_engine::actor_init::ActorInits) {
        let mut i = TurretFacingInit::new(self.local_orientation.get().yaw);
        i.instance_name = self.info.instance_name.clone();
        init.push(Rc::new(i));
    }
}

impl Turreted {
    pub fn name(&self) -> &str {
        &self.info.turret
    }

    pub fn local_orientation(&self) -> WRot {
        self.local_orientation.get()
    }

    pub fn offset(&self) -> WVec {
        self.info.offset
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    fn body_facing(&self) -> Option<&Rc<dyn IFacing>> {
        self.facing.get().and_then(|f| f.as_ref())
    }

    pub fn world_orientation(&self) -> WRot {
        let world = match self.body_facing() {
            Some(f) => self.local_orientation.get().rotate(&f.orientation()),
            None => self.local_orientation.get(),
        };
        if self.quantized_facings.get() == 0 {
            return world;
        }
        WRot::from_yaw(self.body.get().expect("created").quantize_facing_to(world.yaw, self.quantized_facings.get()))
    }

    fn desired_local_facing(&self) -> WAngle {
        let d = self.desired_direction.get();
        if d == WVec::ZERO {
            return self.local_orientation.get().yaw;
        }
        let Some(f) = self.body_facing() else { return d.yaw() };
        let o = f.orientation();
        if o.pitch == WAngle::ZERO && o.roll == WAngle::ZERO {
            return d.yaw() - o.yaw;
        }
        d.rotate(&(-o)).yaw()
    }

    fn desired(&self) -> WAngle {
        if self.realign_desired.get() { self.info.initial_facing } else { self.desired_local_facing() }
    }

    fn move_turret(&self) {
        let desired = self.desired();
        let lo = self.local_orientation.get();
        if desired == lo.yaw {
            return;
        }
        self.local_orientation.set(lo.with_yaw(util::tick_facing(lo.yaw, desired, self.info.turn_speed)));
        if desired == self.local_orientation.get().yaw {
            self.realign_desired.set(false);
            self.desired_direction.set(WVec::ZERO);
        }
    }

    pub fn face_target(&self, self_: &ActorRef, target: &Target) -> bool {
        let Some(attack) = self.attack.get().and_then(|a| a.as_ref()) else { return false };
        if self.is_trait_disabled() || self.is_trait_paused() || attack.is_trait_disabled() || attack.is_trait_paused() {
            return false;
        }
        if target.target_type() == TargetType::Invalid {
            self.desired_direction.set(WVec::ZERO);
            return false;
        }
        let turret_pos = self_.center_position() + self.position(self_);
        let target_pos = attack.get_target_position(turret_pos, target);
        self.desired_direction.set(target_pos - turret_pos);
        self.realign_desired.set(false);
        self.move_turret();
        self.has_achieved_desired_facing()
    }

    pub fn has_achieved_desired_facing(&self) -> bool {
        self.desired() == self.local_orientation.get().yaw
    }

    /// Turret offset in world space.
    pub fn position(&self, self_: &ActorRef) -> WVec {
        let body = self.body.get().expect("created");
        body.local_to_world(self.offset().rotate(&body.quantize_orientation(self_.orientation())))
    }
}
