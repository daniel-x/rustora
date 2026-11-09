// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/AutoTarget.cs`, `Traits/AutoTargetPriority.cs` and `Traits/Guard.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rustora_core::world_types::WDist;
use rustora_derive::FieldLoad;
use rustora_engine::activity::LINE_ORANGE_RED;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN, hash_actor};
use rustora_engine::actor_ext::enabled_target_types;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::*;
use rustora_engine::{conditional_observers_impl, conditional_simple, info_impl, load_info, register_traits, value_init};

use crate::actor_map::WorldExt;
use crate::attack::{AttackBase, AttackSource, activity_stance_changed};
use crate::attack_move::AttackMoveActivity;
use crate::cargo::Passenger;
use crate::interfaces::*;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("AutoTarget", |n| load_info!(AutoTargetInfo, n));
    r.register("AutoTargetPriority", |n| load_info!(AutoTargetPriorityInfo, n));
    r.register("Guard", |n| load_info!(GuardInfo, n));
    r.register("Guardable", |n| load_info!(GuardableInfo, n));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, rustora_engine::persist::Persist)]
#[persist(value)]
pub enum UnitStance {
    #[default]
    HoldFire,
    ReturnFire,
    Defend,
    AttackAnything,
}

rustora_core::field_enum!(UnitStance, [HoldFire, ReturnFire, Defend, AttackAnything]);
value_init!(StanceInit, UnitStance);

pub trait INotifyStanceChanged {
    fn stance_changed(&self, self_: &ActorRef, auto_target: &AutoTarget, old_stance: UnitStance, new_stance: UnitStance);
}

pub trait IOverrideAutoTarget {
    fn try_get_auto_target_override(&self, self_: &ActorRef) -> Option<Target>;
}

pub trait IDisableEnemyAutoTarget {
    fn disable_enemy_auto_target(&self, self_: &ActorRef, attacker: &ActorRef) -> bool;
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AutoTargetInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub allow_movement: bool,
    pub allow_turning: bool,
    pub scan_on_idle: bool,
    pub scan_radius: i32,
    #[field(name = "InitialStanceAI")]
    pub initial_stance_ai: UnitStance,
    pub initial_stance: UnitStance,
    pub hold_fire_condition: Option<String>,
    pub return_fire_condition: Option<String>,
    pub defend_condition: Option<String>,
    pub attack_anything_condition: Option<String>,
    pub enable_stances: bool,
    pub minimum_scan_time_interval: i32,
    pub maximum_scan_time_interval: i32,
}

impl Default for AutoTargetInfo {
    fn default() -> Self {
        AutoTargetInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            allow_movement: true,
            allow_turning: true,
            scan_on_idle: true,
            scan_radius: -1,
            initial_stance_ai: UnitStance::AttackAnything,
            initial_stance: UnitStance::Defend,
            hold_fire_condition: None,
            return_fire_condition: None,
            defend_condition: None,
            attack_anything_condition: None,
            enable_stances: true,
            minimum_scan_time_interval: 3,
            maximum_scan_time_interval: 8,
        }
    }
}

impl AutoTargetInfo {
    fn condition_by_stance(&self, s: UnitStance) -> Option<&str> {
        match s {
            UnitStance::HoldFire => self.hold_fire_condition.as_deref(),
            UnitStance::ReturnFire => self.return_fire_condition.as_deref(),
            UnitStance::Defend => self.defend_condition.as_deref(),
            UnitStance::AttackAnything => self.attack_anything_condition.as_deref(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct AutoTarget {
    pub info: Rc<AutoTargetInfo>,
    state: ConditionalState,
    attack_bases: Vec<Rc<AttackBase>>,
    allow_movement: bool,
    next_scan_time: Cell<i32>,
    stance: Cell<UnitStance>,
    pub aggressor: RefCell<Option<ActorRef>>,
    pub predicted_stance: Cell<UnitStance>,
    override_auto_target: RefCell<Vec<Rc<dyn IOverrideAutoTarget>>>,
    notify_stance_changed: RefCell<Vec<Rc<dyn INotifyStanceChanged>>>,
    target_priorities: OnceCell<Vec<Rc<AutoTargetPriority>>>,
    condition_token: Cell<i32>,
}

register_traits!(AutoTarget: INotifyIdle, INotifyDamage, ITick, IResolveOrder, ISync, INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait);
info_impl!(AutoTargetInfo => |info, init| {
    let self_ = &init.self_;
    let owner = self_.owner();
    let initial = if owner.is_bot || !owner.playable { info.initial_stance_ai } else { info.initial_stance };
    let stance = init.inits.get::<StanceInit>().map_or(initial, |s| s.value);
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(AutoTarget {
        state,
        attack_bases: self_.traits_implementing::<AttackBase>(),
        allow_movement: info.allow_movement && self_.trait_or_default::<dyn IMove>().is_some(),
        next_scan_time: Cell::new(0),
        stance: Cell::new(stance),
        aggressor: RefCell::new(None),
        predicted_stance: Cell::new(stance),
        override_auto_target: RefCell::new(Vec::new()),
        notify_stance_changed: RefCell::new(Vec::new()),
        target_priorities: OnceCell::new(),
        condition_token: Cell::new(INVALID_CONDITION_TOKEN),
        info,
    }))
});

impl ConditionalHooks for AutoTarget {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_observers_impl!(AutoTarget);

impl ISync for AutoTarget {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.next_scan_time.get() ^ hash_actor(self.aggressor.borrow().as_ref())
    }
}

impl INotifyCreated for AutoTarget {
    fn created(&self, self_: &ActorRef) {
        let mut priorities = self_.traits_implementing::<AutoTargetPriority>();
        priorities.sort_by_key(|p| std::cmp::Reverse(p.info.priority));
        let _ = self.target_priorities.set(priorities);
        *self.override_auto_target.borrow_mut() = self_.traits_implementing::<dyn IOverrideAutoTarget>();
        *self.notify_stance_changed.borrow_mut() = self_.traits_implementing::<dyn INotifyStanceChanged>();
        self.apply_stance_condition(self_);
        conditional_created(self, self_);
    }
}

impl AutoTarget {
    pub fn stance(&self) -> UnitStance {
        self.stance.get()
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn allow_move(&self) -> bool {
        self.allow_movement && self.stance.get() > UnitStance::Defend
    }

    pub fn active_attack_bases(&self) -> Vec<Rc<AttackBase>> {
        self.attack_bases.iter().filter(|t| !t.is_trait_disabled()).cloned().collect()
    }

    fn active_target_priorities(&self) -> Vec<Rc<AutoTargetPriorityInfo>> {
        self.target_priorities.get().map_or_else(Vec::new, |v| {
            v.iter().filter(|t| !t.state.is_trait_disabled()).map(|t| t.info.clone()).collect()
        })
    }

    pub fn set_stance(&self, self_: &ActorRef, value: UnitStance) {
        if self.stance.get() == value {
            return;
        }
        let old = self.stance.get();
        self.stance.set(value);
        self.predicted_stance.set(value);
        self.apply_stance_condition(self_);
        let list = self.notify_stance_changed.borrow().clone();
        for n in list {
            n.stance_changed(self_, self, old, value);
        }
        if let Some(current) = self_.current_activity() {
            rustora_engine::activity::for_each_activity(Some(current), true, &mut |a| activity_stance_changed(a, self_, self, old, value));
        }
    }

    fn apply_stance_condition(&self, self_: &ActorRef) {
        if self.condition_token.get() != INVALID_CONDITION_TOKEN {
            self.condition_token.set(self_.revoke_condition(self.condition_token.get()));
        }
        if let Some(c) = self.info.condition_by_stance(self.stance.get()) {
            self.condition_token.set(self_.grant_condition(c));
        }
    }

    pub fn next_scan_time(&self) -> i32 {
        self.next_scan_time.get()
    }

    pub fn scan_for_target(&self, self_: &ActorRef, allow_move: bool, allow_turn: bool, ignore_scan_interval: bool) -> Target {
        let active = self.active_attack_bases();
        if (ignore_scan_interval || self.next_scan_time.get() <= 0) && !active.is_empty() {
            let overrides = self.override_auto_target.borrow().clone();
            for oat in overrides {
                if let Some(t) = oat.try_get_auto_target_override(self_) {
                    return t;
                }
            }
            if !ignore_scan_interval {
                let next = self_
                    .world()
                    .shared_random
                    .borrow_mut()
                    .next_range(self.info.minimum_scan_time_interval, self.info.maximum_scan_time_interval);
                self.next_scan_time.set(next);
            }
            for ab in &active {
                let stances = ab.unforced_attack_target_stances();
                if stances != PlayerRelationship::NONE {
                    let range = if self.info.scan_radius > 0 {
                        WDist::from_cells(self.info.scan_radius)
                    } else {
                        ab.get_maximum_range()
                    };
                    let target = self.choose_target(self_, ab, stances, range, allow_move, allow_turn);
                    if target.target_type() != TargetType::Invalid {
                        return target;
                    }
                }
            }
        }
        Target::Invalid
    }

    pub fn scan_and_attack(&self, self_: &ActorRef, allow_move: bool, allow_turn: bool) {
        let target = self.scan_for_target(self_, allow_move, allow_turn, false);
        if target.target_type() != TargetType::Invalid {
            self.attack(&target, allow_move);
        }
    }

    fn attack(&self, target: &Target, allow_move: bool) {
        for ab in self.active_attack_bases() {
            ab.attack_target(target, AttackSource::AutoTarget, false, allow_move, false, None);
        }
    }

    pub fn has_valid_target_priority(&self, self_: &ActorRef, owner: Option<&PlayerRef>, target_types: &BitSet<TargetableType>) -> bool {
        let Some(owner) = owner else { return false };
        if self.stance.get() <= UnitStance::ReturnFire {
            return false;
        }
        self.active_target_priorities().iter().any(|ati| {
            ati.valid_relationships
                .has_relationship(self_.owner().relationship_with(Some(owner)))
                && ati.valid_targets.overlaps(target_types)
                && !ati.invalid_targets.overlaps(target_types)
        })
    }

    fn choose_target(
        &self,
        self_: &ActorRef,
        ab: &Rc<AttackBase>,
        attack_stances: PlayerRelationship,
        scan_range: WDist,
        allow_move: bool,
        allow_turn: bool,
    ) -> Target {
        let mut chosen = Target::Invalid;
        let mut chosen_priority = i32::MIN;
        let mut chosen_range = 0;
        let active = self.active_target_priorities();
        if active.is_empty() {
            return chosen;
        }
        let w = self_.world();
        let pos = self_.center_position();
        let mut targets: Vec<Target> = w
            .find_actors_in_circle(pos, scan_range)
            .iter()
            .map(|a| Target::from_actor(Some(a)))
            .collect();
        if (allow_move || ab.info.target_frozen_actors)
            && let Some(layer) = self_.owner().frozen_actor_layer()
        {
            targets.extend(
                layer
                    .frozen_actors_in_circle(&w, pos, scan_range, true)
                    .into_iter()
                    .map(Target::from_frozen_actor),
            );
        }
        let me = self_.owner();
        for target in targets {
            let (target_types, owner) = match target.target_type() {
                TargetType::Actor => {
                    let a = target.actor().unwrap();
                    if attack_stances == PlayerRelationship::ENEMY && !util::appears_hostile_to(a, self_) {
                        continue;
                    }
                    let types = enabled_target_types(a);
                    if prevents_auto_target(self_, a) || !a.can_be_viewed_by_player(Some(&me)) {
                        continue;
                    }
                    (types, a.owner())
                }
                TargetType::FrozenActor => {
                    let f = target.frozen_actor().unwrap();
                    let Some(owner) = f.owner() else { continue };
                    if attack_stances == PlayerRelationship::ENEMY && me.relationship_with(Some(&owner)) == PlayerRelationship::ALLY {
                        continue;
                    }
                    if me.is_bot && f.actor().is_none() {
                        continue;
                    }
                    (f.target_types(), owner)
                }
                _ => continue,
            };
            let relationship = me.relationship_with(Some(&owner));
            let valid: Vec<&Rc<AutoTargetPriorityInfo>> = active
                .iter()
                .filter(|ati| {
                    ati.priority >= chosen_priority
                        && ati.valid_relationships.has_relationship(relationship)
                        && ati.valid_targets.overlaps(&target_types)
                        && !ati.invalid_targets.overlaps(&target_types)
                })
                .collect();
            if valid.is_empty() {
                continue;
            }
            let any_armament = ab
                .choose_armaments_for_target(&target, false)
                .iter()
                .any(|arm| allow_move || (target.is_in_range(pos, arm.max_range()) && !target.is_in_range(pos, arm.weapon.min_range)));
            if !any_armament {
                continue;
            }
            if !allow_turn && !ab.target_in_firing_arc(self_, &target, ab.info.facing_tolerance) {
                continue;
            }
            let target_range = (target.center_position() - pos).length();
            for ati in valid {
                if chosen.target_type() == TargetType::Invalid
                    || chosen_priority < ati.priority
                    || (chosen_priority == ati.priority && target_range < chosen_range)
                {
                    chosen = target.clone();
                    chosen_priority = ati.priority;
                    chosen_range = target_range;
                }
            }
        }
        chosen
    }
}

fn prevents_auto_target(attacker: &ActorRef, target: &ActorRef) -> bool {
    target
        .traits_implementing::<dyn IDisableEnemyAutoTarget>()
        .iter()
        .any(|d| d.disable_enemy_auto_target(target, attacker))
}

impl INotifyOwnerChanged for AutoTarget {
    fn on_owner_changed(&self, self_: &ActorRef, _old_owner: &PlayerRef, _new_owner: &PlayerRef) {
        let owner = self_.owner();
        self.set_stance(
            self_,
            if owner.is_bot || !owner.playable {
                self.info.initial_stance_ai
            } else {
                self.info.initial_stance
            },
        );
    }
}

impl IResolveOrder for AutoTarget {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "SetUnitStance" && self.info.enable_stances {
            let s = match order.extra_data {
                0 => UnitStance::HoldFire,
                1 => UnitStance::ReturnFire,
                2 => UnitStance::Defend,
                _ => UnitStance::AttackAnything,
            };
            self.set_stance(self_, s);
        }
    }
}

impl INotifyDamage for AutoTarget {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.is_trait_disabled() || !self_.is_idle() || self.stance.get() < UnitStance::ReturnFire || e.damage.value < 0 {
            return;
        }
        let Some(mut attacker) = e.attacker.clone() else { return };
        if attacker.disposed() {
            return;
        }
        let overrides = self.override_auto_target.borrow().clone();
        if overrides.iter().any(|o| o.try_get_auto_target_override(self_).is_some()) {
            return;
        }
        if !attacker.is_in_world()
            && let Some(transport) = attacker.trait_or_default::<Passenger>().and_then(|p| p.transport.borrow().clone())
        {
            attacker = transport;
        }
        if !self.allow_move() && !attacker.can_be_viewed_by_player(Some(&self_.owner())) {
            return;
        }
        let attacker_as_target = Target::from_actor(Some(&attacker));
        if !self
            .active_attack_bases()
            .iter()
            .any(|a| a.has_any_valid_weapons(&attacker_as_target, false, false))
        {
            return;
        }
        if util::appears_friendly_to(&attacker, self_) {
            return;
        }
        let mut aggressor = Some(attacker);
        if self.stance.get() > UnitStance::ReturnFire {
            let auto_target = self.scan_for_target(self_, self.allow_move(), true, false);
            if auto_target.target_type() != TargetType::Invalid {
                aggressor = auto_target.actor().cloned();
            }
        }
        *self.aggressor.borrow_mut() = aggressor.clone();
        self.attack(&Target::from_actor(aggressor.as_ref()), self.allow_move());
    }
}

impl INotifyIdle for AutoTarget {
    fn tick_idle(&self, self_: &ActorRef) {
        if self.is_trait_disabled() || !self.info.scan_on_idle || self.stance.get() < UnitStance::Defend {
            return;
        }
        let allow_turn = self.info.allow_turning && self.stance.get() > UnitStance::HoldFire;
        self.scan_and_attack(self_, self.allow_move(), allow_turn);
    }
}

impl ITick for AutoTarget {
    fn tick(&self, _self_: &ActorRef) {
        if !self.is_trait_disabled() && self.next_scan_time.get() > 0 {
            self.next_scan_time.set(self.next_scan_time.get() - 1);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AutoTargetPriorityInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub valid_targets: BitSet<TargetableType>,
    pub invalid_targets: BitSet<TargetableType>,
    pub valid_relationships: PlayerRelationship,
    pub priority: i32,
}

impl Default for AutoTargetPriorityInfo {
    fn default() -> Self {
        AutoTargetPriorityInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            valid_targets: BitSet::new(&["Ground", "Water", "Air"]),
            invalid_targets: BitSet::default(),
            valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
            priority: 1,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct AutoTargetPriority {
    pub info: Rc<AutoTargetPriorityInfo>,
    state: ConditionalState,
}

register_traits!(AutoTargetPriority: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(AutoTargetPriorityInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(AutoTargetPriority { info, state }))
});
conditional_simple!(AutoTargetPriority);

#[derive(Clone, Debug, FieldLoad)]
pub struct GuardInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub voice: String,
}

impl Default for GuardInfo {
    fn default() -> Self {
        GuardInfo {
            instance_name: None,
            voice: "Action".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Guard {
    info: Rc<GuardInfo>,
    mv: OnceCell<Rc<dyn IMove>>,
}

register_traits!(Guard: IResolveOrder, INotifyCreated, rustora_engine::sound::IOrderVoice);
info_impl!(GuardInfo => |info, _init| Some(Rc::new(Guard { info, mv: OnceCell::new() })));

impl rustora_engine::sound::IOrderVoice for Guard {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "Guard").then(|| self.info.voice.clone())
    }
}

impl INotifyCreated for Guard {
    fn created(&self, self_: &ActorRef) {
        let _ = self.mv.set(self_.trait_::<dyn IMove>());
    }
}

impl Guard {
    pub fn guard_target(&self, self_: &ActorRef, target: &Target, queued: bool) {
        let Some(a) = target.actor().filter(|_| target.target_type() == TargetType::Actor) else {
            return;
        };
        let range = a.info.trait_info::<GuardableInfo>().expect("guardable").range;
        let (mv, me, t) = (self.mv.get().unwrap().clone(), self_.clone(), target.clone());
        self_.queue_activity_queued(
            queued,
            AttackMoveActivity::new(
                self_,
                crate::attack_move::MoveOrder::Follow {
                    mv,
                    self_: me,
                    target: t,
                    min_range: WDist::ZERO,
                    max_range: range,
                    line_color: Some(LINE_ORANGE_RED),
                },
                false,
            ),
        );
        crate::target_lines::show_target_lines(self_);
    }
}

impl IResolveOrder for Guard {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Guard" {
            self.guard_target(self_, &order.target, order.queued);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GuardableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub range: WDist,
}

impl Default for GuardableInfo {
    fn default() -> Self {
        GuardableInfo {
            instance_name: None,
            range: WDist::from_cells(2),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Guardable;

register_traits!(Guardable);
info_impl!(GuardableInfo => |_info, _init| Some(Rc::new(Guardable)));

rustora_engine::persist_trait_refs!(dyn INotifyStanceChanged, dyn IOverrideAutoTarget);

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_values!(dyn rustora_engine::actor_init::ActorInit: StanceInit);
}
