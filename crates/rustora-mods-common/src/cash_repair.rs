// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `DeliversCash`, `AcceptsDeliveredCash`, `DonateCash`, `InstantlyRepairable`, `InstantlyRepairs`, `InstantRepair`.
//!
//! Not ported: the floating cash text of `DonateCash`.

use std::rc::{Rc, Weak};

use indexmap::IndexSet;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageState, DamageTypes};
use rustora_engine::frozen::FrozenActorRef;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, IOrderVoice, SoundType};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

use crate::base::PlayerExperience;
use crate::cargo::{Enter, EnterHooks};
use crate::orders::{IIssueOrder, IOrderTargeter, UnitOrderTargeter};
use crate::player::PlayerResources;

pub fn register(r: &mut TraitRegistry) {
    r.register("DeliversCash", |n| load_info!(DeliversCashInfo, n));
    r.register("AcceptsDeliveredCash", |n| load_info!(AcceptsDeliveredCashInfo, n));
    r.register("InstantlyRepairable", |n| load_info!(InstantlyRepairableInfo, n));
    r.register("InstantlyRepairs", |n| load_info!(InstantlyRepairsInfo, n));
    r.register("Cloneable", |n| load_info!(CloneableInfo, n));
}

/// `Cloneable` only tags actors for `ClonesProducedUnits`, which RA does not use.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct CloneableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(CloneableInfo => |_info, _init| None);

/// Tag of `BitSet<InstantlyRepairType>`.
pub struct InstantlyRepairType;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnterBehaviour {
    Exit,
    Suicide,
    #[default]
    Dispose,
}

rustora_core::field_enum!(EnterBehaviour, [Exit, Suicide, Dispose]);

pub trait INotifyCashTransfer {
    fn on_accepting_cash(&self, self_: &ActorRef, donor: &ActorRef);
    fn on_delivering_cash(&self, self_: &ActorRef, acceptor: &ActorRef);
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DeliversCashInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub payload: i32,
    pub player_experience: i32,
    #[field(name = "Type")]
    pub ty: Option<String>,
    pub sounds: Vec<String>,
    pub cursor: String,
    pub voice: String,
}

impl Default for DeliversCashInfo {
    fn default() -> Self {
        DeliversCashInfo {
            instance_name: None,
            payload: 500,
            player_experience: 0,
            ty: None,
            sounds: Vec::new(),
            cursor: "enter".into(),
            voice: "Action".into(),
        }
    }
}

pub struct DeliversCash {
    info: Rc<DeliversCashInfo>,
}

register_traits!(DeliversCash: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCashTransfer);
info_impl!(DeliversCashInfo => |info, _init| Some(Rc::new(DeliversCash { info })));

fn accepts_cash_from(
    info: &AcceptsDeliveredCashInfo,
    target_owner_relationship: PlayerRelationship,
    delivers_type: Option<&String>,
) -> bool {
    if !info.valid_relationships.has_relationship(target_owner_relationship) {
        return false;
    }
    info.valid_types.is_empty() || delivers_type.is_some_and(|t| !t.is_empty() && info.valid_types.contains(t))
}

impl IIssueOrder for DeliversCash {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        let (a, f) = (self.info.ty.clone(), self.info.ty.clone());
        vec![Box::new(UnitOrderTargeter::new(
            "DeliverCash",
            5,
            &self.info.cursor,
            false,
            true,
            Box::new(move |self_, target, _, _| {
                target
                    .info
                    .trait_info::<AcceptsDeliveredCashInfo>()
                    .is_some_and(|i| accepts_cash_from(&i, target.owner().relationship_with(Some(&self_.owner())), a.as_ref()))
            }),
            Box::new(move |self_, target, _, _| {
                target.info().trait_info::<AcceptsDeliveredCashInfo>().is_some_and(|i| {
                    accepts_cash_from(
                        &i,
                        target
                            .owner()
                            .map_or(PlayerRelationship::NONE, |o| o.relationship_with(Some(&self_.owner()))),
                        f.as_ref(),
                    )
                })
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "DeliverCash").then(|| Order::new("DeliverCash", Some(self_.clone()), target.clone(), queued))
    }
}

impl IOrderVoice for DeliversCash {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "DeliverCash").then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for DeliversCash {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "DeliverCash" {
            return;
        }
        let hooks = DonateCashHooks {
            payload: self.info.payload,
            player_experience: self.info.player_experience,
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

impl INotifyCashTransfer for DeliversCash {
    fn on_accepting_cash(&self, _self_: &ActorRef, _donor: &ActorRef) {}
    fn on_delivering_cash(&self, self_: &ActorRef, _acceptor: &ActorRef) {
        sound::play_random_at(&self_.world(), SoundType::World, &self.info.sounds, self_.center_position(), 1.0);
    }
}

/// The `DonateCash` activity without a target line, as scripts queue it.
pub fn donate_cash_activity(self_: &ActorRef, target: &Target, payload: i32, player_experience: i32) -> rustora_engine::activity::ActivityRef {
    Enter::new(self_, target, Box::new(DonateCashHooks { payload, player_experience }))
}

struct DonateCashHooks {
    payload: i32,
    player_experience: i32,
}

impl EnterHooks for DonateCashHooks {
    fn type_name(&self) -> &'static str {
        "DonateCash"
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        let target_owner = target_actor.owner();
        target_owner.player_actor().trait_::<PlayerResources>().change_cash(self.payload);
        if let Some(exp) = self_.owner().player_actor().trait_or_default::<PlayerExperience>()
            && !Rc::ptr_eq(&target_owner, &self_.owner())
        {
            exp.give_experience(self.player_experience);
        }
        for n in target_actor.traits_implementing::<dyn INotifyCashTransfer>() {
            n.on_accepting_cash(target_actor, self_);
        }
        for n in self_.traits_implementing::<dyn INotifyCashTransfer>() {
            n.on_delivering_cash(self_, target_actor);
        }
        self_.dispose();
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AcceptsDeliveredCashInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub valid_types: IndexSet<String>,
    pub valid_relationships: PlayerRelationship,
    pub sounds: Vec<String>,
}

impl Default for AcceptsDeliveredCashInfo {
    fn default() -> Self {
        AcceptsDeliveredCashInfo {
            instance_name: None,
            valid_types: IndexSet::new(),
            valid_relationships: PlayerRelationship::ALLY,
            sounds: Vec::new(),
        }
    }
}

pub struct AcceptsDeliveredCash {
    info: Rc<AcceptsDeliveredCashInfo>,
}

register_traits!(AcceptsDeliveredCash: INotifyCashTransfer);
info_impl!(AcceptsDeliveredCashInfo => |info, _init| Some(Rc::new(AcceptsDeliveredCash { info })));

impl INotifyCashTransfer for AcceptsDeliveredCash {
    fn on_accepting_cash(&self, self_: &ActorRef, _donor: &ActorRef) {
        sound::play_random_at(&self_.world(), SoundType::World, &self.info.sounds, self_.center_position(), 1.0);
    }
    fn on_delivering_cash(&self, _self_: &ActorRef, _acceptor: &ActorRef) {}
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InstantlyRepairableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub types: BitSet<InstantlyRepairType>,
}

pub struct InstantlyRepairable {
    pub info: Rc<InstantlyRepairableInfo>,
    state: ConditionalState,
}

register_traits!(InstantlyRepairable: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(InstantlyRepairableInfo => |info, _init| Some(Rc::new(InstantlyRepairable { state: ConditionalState::new(&info.base), info })));
conditional_simple!(InstantlyRepairable);

#[derive(Clone, Debug, FieldLoad)]
pub struct InstantlyRepairsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub types: BitSet<InstantlyRepairType>,
    pub voice: String,
    pub enter_behaviour: EnterBehaviour,
    pub valid_relationships: PlayerRelationship,
    pub repair_sound: Option<String>,
    pub cursor: String,
    pub repair_blocked_cursor: String,
}

impl Default for InstantlyRepairsInfo {
    fn default() -> Self {
        InstantlyRepairsInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            types: BitSet::default(),
            voice: "Action".into(),
            enter_behaviour: EnterBehaviour::Dispose,
            valid_relationships: PlayerRelationship::ALLY,
            repair_sound: None,
            cursor: "goldwrench".into(),
            repair_blocked_cursor: "goldwrench-blocked".into(),
        }
    }
}

pub struct InstantlyRepairs {
    info: Rc<InstantlyRepairsInfo>,
    state: ConditionalState,
    me: Weak<InstantlyRepairs>,
}

register_traits!(InstantlyRepairs: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(InstantlyRepairsInfo => |info, _init| Some(Rc::new_cyclic(|me| InstantlyRepairs { state: ConditionalState::new(&info.base), info, me: me.clone() })));
conditional_simple!(InstantlyRepairs);

impl InstantlyRepairs {
    /// The `InstantRepair` activity without a target line, as scripts queue it.
    pub fn instant_repair_activity(&self, self_: &ActorRef, target: &Target) -> rustora_engine::activity::ActivityRef {
        Enter::new(self_, target, Box::new(InstantRepairHooks { info: self.info.clone() }))
    }

    fn is_valid_actor(&self, target: &ActorRef) -> bool {
        target
            .trait_or_default::<InstantlyRepairable>()
            .is_some_and(|r| !r.state.is_trait_disabled() && (r.info.types.is_empty() || r.info.types.overlaps(&self.info.types)))
    }

    fn is_valid_frozen(&self, target: &FrozenActorRef) -> bool {
        target
            .info()
            .trait_info::<InstantlyRepairableInfo>()
            .is_some_and(|r| r.types.is_empty() || r.types.overlaps(&self.info.types))
    }

    fn is_valid_order(&self, order: &Order) -> bool {
        match order.target.target_type() {
            TargetType::FrozenActor => order
                .target
                .frozen_actor()
                .is_some_and(|f| self.is_valid_frozen(f) && f.damage_state() > DamageState::UNDAMAGED),
            TargetType::Actor => order.target.actor().is_some_and(|a| {
                self.is_valid_actor(a)
                    && a.trait_or_default::<dyn IHealth>()
                        .is_some_and(|h| h.damage_state() > DamageState::UNDAMAGED)
            }),
            _ => false,
        }
    }
}

impl IIssueOrder for InstantlyRepairs {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let (a, f) = (self.me.upgrade().expect("alive"), self.me.upgrade().expect("alive"));
        vec![Box::new(UnitOrderTargeter::new(
            "InstantRepair",
            6,
            &self.info.cursor,
            true,
            true,
            Box::new(move |self_, target, _, cursor| {
                if !a.is_valid_actor(target)
                    || !a
                        .info
                        .valid_relationships
                        .has_relationship(target.owner().relationship_with(Some(&self_.owner())))
                {
                    return false;
                }
                if target
                    .trait_or_default::<dyn IHealth>()
                    .is_none_or(|h| h.damage_state() == DamageState::UNDAMAGED)
                {
                    *cursor = Some(a.info.repair_blocked_cursor.clone());
                }
                true
            }),
            Box::new(move |self_, target, _, cursor| {
                if !f.is_valid_frozen(target)
                    || !f.info.valid_relationships.has_relationship(
                        target
                            .owner()
                            .map_or(PlayerRelationship::NONE, |o| o.relationship_with(Some(&self_.owner()))),
                    )
                {
                    return false;
                }
                if target.damage_state() == DamageState::UNDAMAGED {
                    *cursor = Some(f.info.repair_blocked_cursor.clone());
                }
                true
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "InstantRepair").then(|| Order::new("InstantRepair", Some(self_.clone()), target.clone(), queued))
    }
}

impl IOrderVoice for InstantlyRepairs {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "InstantRepair" && self.is_valid_order(order)).then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for InstantlyRepairs {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "InstantRepair" || !self.is_valid_order(order) {
            return;
        }
        let hooks = InstantRepairHooks { info: self.info.clone() };
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

struct InstantRepairHooks {
    info: Rc<InstantlyRepairsInfo>,
}

impl EnterHooks for InstantRepairHooks {
    fn type_name(&self) -> &'static str {
        "InstantRepair"
    }

    fn try_start_enter(&self, enter: &Enter, self_: &ActorRef, target_actor: &ActorRef) -> bool {
        let health = target_actor.trait_or_default::<dyn IHealth>();
        let repairable = target_actor.trait_or_default::<InstantlyRepairable>();
        let stance = self_.owner().relationship_with(Some(&target_actor.owner()));
        let can_enter = health.is_some_and(|h| h.damage_state() != DamageState::UNDAMAGED)
            && repairable.is_some_and(|r| !r.state.is_trait_disabled())
            && self.info.valid_relationships.has_relationship(stance);
        if !can_enter {
            enter.cancel_keep_queue(self_);
        }
        can_enter
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        let (Some(health), Some(repairable)) = (
            target_actor.trait_or_default::<dyn IHealth>(),
            target_actor.trait_or_default::<InstantlyRepairable>(),
        ) else {
            return;
        };
        if repairable.state.is_trait_disabled() || health.damage_state() == DamageState::UNDAMAGED {
            return;
        }
        let stance = self_.owner().relationship_with(Some(&target_actor.owner()));
        if !self.info.valid_relationships.has_relationship(stance) {
            return;
        }
        health.inflict_damage(
            target_actor,
            Some(self_),
            &Damage::new(-health.max_hp(), DamageTypes::default()),
            false,
        );
        if let Some(s) = self.info.repair_sound.as_deref().filter(|s| !s.is_empty()) {
            sound::play_at(&self_.world(), SoundType::World, s, target_actor.center_position(), 1.0);
        }
        match self.info.enter_behaviour {
            EnterBehaviour::Dispose => self_.dispose(),
            EnterBehaviour::Suicide => {
                if let Some(h) = self_.trait_or_default::<dyn IHealth>() {
                    h.kill(self_, Some(self_), &DamageTypes::default());
                }
            }
            EnterBehaviour::Exit => {}
        }
    }
}
