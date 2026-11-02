// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/GainsExperience.cs`, `Traits/GivesExperience.cs`, `Traits/ChangesHealth.cs`, `Traits/Multipliers`,
//! `Traits/Conditions/GrantCondition.cs`, `GrantConditionOnDamageState.cs` and `ExternalCondition.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, Damage, DamageState, DamageTypes};
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits, value_init};

use crate::base::PlayerExperience;
use crate::interfaces::*;
use crate::production::ValuedInfo;
use crate::util;

pub trait IGainsExperienceModifier {
    fn get_gains_experience_modifier(&self) -> i32;
}

pub trait IGivesExperienceModifier {
    fn get_gives_experience_modifier(&self) -> i32;
}

macro_rules! multiplier {
    ($name:ident, $info:ident, $iface:ident, $($method:tt)*) => {
        #[derive(Clone, Debug, Default, FieldLoad)]
        pub struct $info {
            #[field(ignore)]
            pub instance_name: Option<String>,
            #[field(flatten)]
            pub base: ConditionalTraitInfo,
            #[field(require)]
            pub modifier: i32,
        }

        pub struct $name {
            info: Rc<$info>,
            state: ConditionalState,
        }

        register_traits!($name: $iface, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
        info_impl!($info => |info, _init| {
            let state = ConditionalState::new(&info.base);
            Some(Rc::new($name { info, state }))
        });
        conditional_simple!($name);

        impl $iface for $name {
            $($method)*
        }
    };
}

macro_rules! simple_modifier {
    ($method:ident) => {
        fn $method(&self) -> i32 {
            if self.state.is_trait_disabled() { 100 } else { self.info.modifier }
        }
    };
}

multiplier!(DamageMultiplier, DamageMultiplierInfo, IDamageModifier,
    fn get_damage_modifier(&self, _attacker: Option<&ActorRef>, _damage: &Damage) -> i32 {
        if self.state.is_trait_disabled() { 100 } else { self.info.modifier }
    }
);
multiplier!(FirepowerMultiplier, FirepowerMultiplierInfo, IFirepowerModifier, simple_modifier!(get_firepower_modifier););
multiplier!(SpeedMultiplier, SpeedMultiplierInfo, ISpeedModifier, simple_modifier!(get_speed_modifier););
multiplier!(ReloadDelayMultiplier, ReloadDelayMultiplierInfo, IReloadModifier, simple_modifier!(get_reload_modifier););
multiplier!(RangeMultiplier, RangeMultiplierInfo, IRangeModifier, simple_modifier!(get_range_modifier););
multiplier!(InaccuracyMultiplier, InaccuracyMultiplierInfo, IInaccuracyModifier, simple_modifier!(get_inaccuracy_modifier););
multiplier!(GainsExperienceMultiplier, GainsExperienceMultiplierInfo, IGainsExperienceModifier, simple_modifier!(get_gains_experience_modifier););
multiplier!(GivesExperienceMultiplier, GivesExperienceMultiplierInfo, IGivesExperienceModifier, simple_modifier!(get_gives_experience_modifier););

pub fn register(r: &mut TraitRegistry) {
    r.register("DamageMultiplier", |n| load_info!(DamageMultiplierInfo, n));
    r.register("FirepowerMultiplier", |n| load_info!(FirepowerMultiplierInfo, n));
    r.register("SpeedMultiplier", |n| load_info!(SpeedMultiplierInfo, n));
    r.register("ReloadDelayMultiplier", |n| load_info!(ReloadDelayMultiplierInfo, n));
    r.register("RangeMultiplier", |n| load_info!(RangeMultiplierInfo, n));
    r.register("InaccuracyMultiplier", |n| load_info!(InaccuracyMultiplierInfo, n));
    r.register("GainsExperienceMultiplier", |n| load_info!(GainsExperienceMultiplierInfo, n));
    r.register("GivesExperienceMultiplier", |n| load_info!(GivesExperienceMultiplierInfo, n));
    r.register("GainsExperience", |n| load_info!(GainsExperienceInfo, n));
    r.register("GivesExperience", |n| load_info!(GivesExperienceInfo, n));
    r.register("ChangesHealth", |n| load_info!(ChangesHealthInfo, n));
    r.register("GrantCondition", |n| load_info!(GrantConditionInfo, n));
    r.register("GrantConditionOnDamageState", |n| load_info!(GrantConditionOnDamageStateInfo, n));
    r.register("ExternalCondition", |n| load_info!(ExternalConditionInfo, n));
}

value_init!(ExperienceInit, i32);

#[derive(Clone, Debug, FieldLoad)]
pub struct GainsExperienceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub conditions: IndexMap<i32, String>,
    pub experience_modifier: i32,
}

impl Default for GainsExperienceInfo {
    fn default() -> Self {
        GainsExperienceInfo { instance_name: None, conditions: IndexMap::new(), experience_modifier: -1 }
    }
}

pub struct GainsExperience {
    info: Rc<GainsExperienceInfo>,
    self_: ActorRef,
    initial_experience: i32,
    next_level: RefCell<Vec<(i32, String)>>,
    experience: Cell<i32>,
    level: Cell<i32>,
    max_level: i32,
}

register_traits!(GainsExperience: INotifyCreated, ISync, IResolveOrder, ITransformActorInitModifier);
info_impl!(GainsExperienceInfo => |info, init| Some(Rc::new(GainsExperience {
    self_: init.self_.clone(),
    initial_experience: init.inits.get_for::<ExperienceInit>(info.instance_name.as_deref()).map_or(0, |e| e.value),
    next_level: RefCell::new(Vec::new()),
    experience: Cell::new(0),
    level: Cell::new(0),
    max_level: info.conditions.len() as i32,
    info,
})));

impl ISync for GainsExperience {
    fn sync_hash(&self) -> i32 {
        self.experience.get() ^ self.level.get()
    }
}

impl INotifyCreated for GainsExperience {
    fn created(&self, self_: &ActorRef) {
        let required = if self.info.experience_modifier < 0 { self_.info.trait_info::<ValuedInfo>().map_or(1, |v| v.cost) } else { self.info.experience_modifier };
        *self.next_level.borrow_mut() = self.info.conditions.iter().map(|(k, v)| (k.wrapping_mul(required), v.clone())).collect();
        if self.initial_experience > 0 {
            self.give_experience(self.initial_experience);
        }
    }
}

impl GainsExperience {
    pub fn can_gain_level(&self) -> bool {
        self.level.get() < self.max_level
    }

    pub fn level(&self) -> i32 {
        self.level.get()
    }

    pub fn give_levels(&self, num_levels: i32) {
        if self.max_level == 0 {
            return;
        }
        let new_level = (self.level.get() + num_levels).min(self.max_level);
        let required = self.next_level.borrow()[(new_level - 1) as usize].0;
        self.give_experience(required - self.experience.get());
    }

    pub fn give_experience(&self, amount: i32) {
        assert!(amount >= 0, "Revoking experience is not implemented.");
        if self.max_level == 0 {
            return;
        }
        let top = self.next_level.borrow()[(self.max_level - 1) as usize].0;
        self.experience.set(self.experience.get().wrapping_add(amount).clamp(0, top));
        while self.level.get() < self.max_level && self.experience.get() >= self.next_level.borrow()[self.level.get() as usize].0 {
            let condition = self.next_level.borrow()[self.level.get() as usize].1.clone();
            self.self_.grant_condition(&condition);
            self.level.set(self.level.get() + 1);
        }
    }
}

impl IResolveOrder for GainsExperience {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "DevLevelUp" && self_.owner().player_actor().trait_::<crate::base::DeveloperMode>().enabled() {
            self.give_levels(if order.extra_data as i32 > 0 { order.extra_data as i32 } else { 1 });
        }
    }
}

impl ITransformActorInitModifier for GainsExperience {
    fn modify_transform_actor_init(&self, _self_: &ActorRef, init: &mut rustora_engine::actor_init::ActorInits) {
        let mut i = ExperienceInit::new(self.experience.get());
        i.instance_name = self.info.instance_name.clone();
        init.push(Rc::new(i));
    }
}

pub fn can_gain_level(a: &ActorRef) -> bool {
    a.trait_or_default::<GainsExperience>().is_some_and(|g| g.can_gain_level())
}

pub fn give_levels(a: &ActorRef, levels: i32) {
    if let Some(g) = a.trait_or_default::<GainsExperience>() {
        g.give_levels(levels);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GivesExperienceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub experience: i32,
    pub valid_relationships: PlayerRelationship,
    pub actor_experience_modifier: i32,
    pub player_experience_modifier: i32,
}

impl Default for GivesExperienceInfo {
    fn default() -> Self {
        GivesExperienceInfo {
            instance_name: None,
            experience: -1,
            valid_relationships: PlayerRelationship(PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0),
            actor_experience_modifier: 10000,
            player_experience_modifier: 0,
        }
    }
}

pub struct GivesExperience {
    info: Rc<GivesExperienceInfo>,
    exp: Cell<i32>,
    modifiers: OnceCell<Vec<Rc<dyn IGivesExperienceModifier>>>,
}

register_traits!(GivesExperience: INotifyKilled, INotifyCreated);
info_impl!(GivesExperienceInfo => |info, _init| Some(Rc::new(GivesExperience { info, exp: Cell::new(0), modifiers: OnceCell::new() })));

impl INotifyCreated for GivesExperience {
    fn created(&self, self_: &ActorRef) {
        let exp = if self.info.experience >= 0 { self.info.experience } else { self_.info.trait_info::<ValuedInfo>().map_or(0, |v| v.cost) };
        self.exp.set(exp);
        let _ = self.modifiers.set(self_.traits_implementing::<dyn IGivesExperienceModifier>());
    }
}

impl INotifyKilled for GivesExperience {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        let Some(attacker) = e.attacker.as_ref().filter(|a| self.exp.get() != 0 && !a.disposed()) else { return };
        if !self.info.valid_relationships.has_relationship(attacker.owner().relationship_with(Some(&self_.owner()))) {
            return;
        }
        let mods = self.modifiers.get().expect("modifiers");
        self.exp.set(util::apply_percentage_modifiers(self.exp.get(), mods.iter().map(|m| m.get_gives_experience_modifier()).collect::<Vec<_>>()));
        if let Some(killer) = attacker.trait_or_default::<GainsExperience>() {
            let mut list: Vec<i32> = attacker.traits_implementing::<dyn IGainsExperienceModifier>().iter().map(|x| x.get_gains_experience_modifier()).collect();
            list.push(self.info.actor_experience_modifier);
            killer.give_experience(util::apply_percentage_modifiers(self.exp.get(), list));
        }
        if let Some(p) = attacker.owner().player_actor().trait_or_default::<PlayerExperience>() {
            p.give_experience(util::apply_percentage_modifiers(self.exp.get(), [self.info.player_experience_modifier]));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ChangesHealthInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub step: i32,
    pub percentage_step: i32,
    pub delay: i32,
    pub start_if_below: i32,
    pub damage_cooldown: i32,
    pub damage_types: DamageTypes,
}

impl Default for ChangesHealthInfo {
    fn default() -> Self {
        ChangesHealthInfo { instance_name: None, base: ConditionalTraitInfo::default(), step: 5, percentage_step: 0, delay: 5, start_if_below: 50, damage_cooldown: 0, damage_types: DamageTypes::default() }
    }
}

pub struct ChangesHealth {
    info: Rc<ChangesHealthInfo>,
    state: ConditionalState,
    health: Rc<dyn IHealth>,
    ticks: Cell<i32>,
    damage_ticks: Cell<i32>,
}

register_traits!(ChangesHealth: ITick, INotifyDamage, ISync, INotifyCreated, IObservesVariables, IDisabledTrait);
info_impl!(ChangesHealthInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(ChangesHealth { info, state, health: init.self_.trait_::<dyn IHealth>(), ticks: Cell::new(0), damage_ticks: Cell::new(0) }))
});

impl ConditionalHooks for ChangesHealth {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_observers_impl!(ChangesHealth);

impl INotifyCreated for ChangesHealth {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ISync for ChangesHealth {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.ticks.get() ^ self.damage_ticks.get()
    }
}

impl ITick for ChangesHealth {
    fn tick(&self, self_: &ActorRef) {
        if self_.is_dead() || self.state.is_trait_disabled() {
            return;
        }
        let info = &self.info;
        let max_hp = self.health.max_hp() as i64;
        if self.health.hp() as i64 >= info.start_if_below as i64 * max_hp / 100 {
            return;
        }
        if self.damage_ticks.get() > 0 {
            self.damage_ticks.set(self.damage_ticks.get() - 1);
            return;
        }
        self.ticks.set(self.ticks.get() - 1);
        if self.ticks.get() <= 0 {
            self.ticks.set(info.delay);
            let amount = -(info.step as i64 + info.percentage_step as i64 * max_hp / 100);
            self.health.inflict_damage(self_, Some(self_), &Damage::new(amount as i32, info.damage_types), false);
        }
    }
}

impl INotifyDamage for ChangesHealth {
    fn damaged(&self, _self_: &ActorRef, e: &AttackInfo) {
        if e.damage.value > 0 {
            self.damage_ticks.set(self.info.damage_cooldown);
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantConditionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub condition: String,
    pub grant_permanently: bool,
}

pub struct GrantCondition {
    info: Rc<GrantConditionInfo>,
    state: ConditionalState,
    token: Cell<i32>,
}

register_traits!(GrantCondition: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GrantConditionInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(GrantCondition { info, state, token: Cell::new(INVALID_CONDITION_TOKEN) }))
});

impl ConditionalHooks for GrantCondition {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        if self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }

    fn trait_disabled(&self, self_: &ActorRef) {
        if !self.info.grant_permanently && self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }
}
rustora_engine::conditional_plumbing!(GrantCondition);

impl INotifyCreated for GrantCondition {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GrantConditionOnDamageStateInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
    pub valid_damage_states: DamageState,
    pub grant_permanently: bool,
}

impl Default for GrantConditionOnDamageStateInfo {
    fn default() -> Self {
        GrantConditionOnDamageStateInfo { instance_name: None, condition: String::new(), valid_damage_states: DamageState(DamageState::HEAVY.0 | DamageState::CRITICAL.0), grant_permanently: false }
    }
}

impl GrantConditionOnDamageStateInfo {
    fn valid(&self, s: DamageState) -> bool {
        self.valid_damage_states.0 & s.0 == s.0
    }
}

pub struct GrantConditionOnDamageState {
    info: Rc<GrantConditionOnDamageStateInfo>,
    health: Rc<dyn IHealth>,
    token: Cell<i32>,
}

register_traits!(GrantConditionOnDamageState: INotifyDamageStateChanged, INotifyCreated);
info_impl!(GrantConditionOnDamageStateInfo => |info, init| Some(Rc::new(GrantConditionOnDamageState {
    info,
    health: init.self_.trait_::<dyn IHealth>(),
    token: Cell::new(INVALID_CONDITION_TOKEN),
})));

impl GrantConditionOnDamageState {
    fn grant_on_valid_state(&self, self_: &ActorRef, state: DamageState) {
        if self.info.valid(state) && self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }
}

impl INotifyCreated for GrantConditionOnDamageState {
    fn created(&self, self_: &ActorRef) {
        self.grant_on_valid_state(self_, self.health.damage_state());
    }
}

impl INotifyDamageStateChanged for GrantConditionOnDamageState {
    fn damage_state_changed(&self, self_: &ActorRef, e: &AttackInfo) {
        let granted = self.token.get() != INVALID_CONDITION_TOKEN;
        if granted && self.info.grant_permanently {
            return;
        }
        if !granted && !self.info.valid(e.previous_damage_state) {
            self.grant_on_valid_state(self_, self.health.damage_state());
        } else if granted && !self.info.valid(e.damage_state) && self.info.valid(e.previous_damage_state) {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ExternalConditionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
    pub source_cap: i32,
    pub total_cap: i32,
}

struct TimedToken {
    expires: i32,
    token: i32,
    source: usize,
}

/// `ExternalCondition`; sources are identified by a key because C# compares them by reference.
pub struct ExternalCondition {
    pub info: Rc<ExternalConditionInfo>,
    permanent_tokens: RefCell<IndexMap<usize, IndexSet<i32>>>,
    timed_tokens: RefCell<Vec<TimedToken>>,
}

register_traits!(ExternalCondition: ITick);
info_impl!(ExternalConditionInfo => |info, _init| Some(Rc::new(ExternalCondition { info, permanent_tokens: RefCell::new(IndexMap::new()), timed_tokens: RefCell::new(Vec::new()) })));

impl ExternalCondition {
    fn permanent_count(&self) -> i32 {
        self.permanent_tokens.borrow().values().map(|t| t.len() as i32).sum()
    }

    pub fn can_grant_condition(&self, source: usize) -> bool {
        let info = &self.info;
        if info.source_cap > 0 && self.permanent_tokens.borrow().get(&source).is_some_and(|t| t.len() as i32 >= info.source_cap) {
            return false;
        }
        !(info.total_cap > 0 && self.permanent_count() >= info.total_cap)
    }

    pub fn grant_condition(&self, self_: &ActorRef, source: usize, duration: i32, mut remaining: i32) -> i32 {
        if !self.can_grant_condition(source) {
            return INVALID_CONDITION_TOKEN;
        }
        let info = &self.info;
        let token = self_.grant_condition(&info.condition);
        if remaining <= 0 || remaining > duration {
            remaining = duration;
        }
        if duration > 0 {
            let mut timed = self.timed_tokens.borrow_mut();
            if info.source_cap > 0 {
                let timed_count = timed.iter().filter(|t| t.source == source).count() as i32;
                let permanent = self.permanent_tokens.borrow().get(&source).map_or(0, |t| t.len() as i32);
                if permanent + timed_count >= info.source_cap {
                    if let Some(i) = timed.iter().position(|t| t.source == source) {
                        let expire = timed.remove(i).token;
                        if self_.token_valid(expire) {
                            self_.revoke_condition(expire);
                        }
                    }
                }
            }
            if info.total_cap > 0 {
                let total = self.permanent_count() + timed.len() as i32;
                if total >= info.total_cap && !timed.is_empty() {
                    let expire = timed.remove(0).token;
                    if self_.token_valid(expire) {
                        self_.revoke_condition(expire);
                    }
                }
            }
            let new = TimedToken { expires: self_.world().world_tick() + remaining, token, source };
            match timed.iter().position(|t| t.expires >= new.expires) {
                Some(i) => timed.insert(i, new),
                None => timed.push(new),
            }
        } else {
            self.permanent_tokens.borrow_mut().entry(source).or_default().insert(token);
        }
        token
    }

    pub fn try_revoke_condition(&self, self_: &ActorRef, source: usize, token: i32) -> bool {
        let has_source = self.permanent_tokens.borrow().contains_key(&source);
        if has_source {
            if !self.permanent_tokens.borrow_mut().get_mut(&source).unwrap().shift_remove(&token) {
                return false;
            }
        } else {
            let mut timed = self.timed_tokens.borrow_mut();
            match timed.iter().position(|p| p.token == token) {
                Some(i) if timed[i].source == source => {
                    timed.remove(i);
                }
                _ => return false,
            }
        }
        if self_.token_valid(token) {
            self_.revoke_condition(token);
        }
        true
    }
}

impl ITick for ExternalCondition {
    fn tick(&self, self_: &ActorRef) {
        let mut timed = self.timed_tokens.borrow_mut();
        if timed.is_empty() {
            return;
        }
        let world_tick = self_.world().world_tick();
        let mut count = 0;
        while count < timed.len() && timed[count].expires < world_tick {
            if self_.token_valid(timed[count].token) {
                self_.revoke_condition(timed[count].token);
            }
            count += 1;
        }
        timed.drain(..count);
    }
}

fn source_key(source: &ActorRef) -> usize {
    source.id as usize
}

pub fn accepts_external_condition(a: &ActorRef, condition: &str, source: &ActorRef) -> bool {
    a.traits_implementing::<ExternalCondition>().iter().any(|t| t.info.condition == condition && t.can_grant_condition(source_key(source)))
}

/// `GrantExternalConditionCrateAction.GrantCondition`.
pub fn grant_external_condition(a: &ActorRef, condition: &str, levels: i32, duration: i32, source: &ActorRef) {
    let key = source_key(source);
    let externals: Vec<Rc<ExternalCondition>> = a.traits_implementing::<ExternalCondition>().into_iter().filter(|t| t.info.condition == condition).collect();
    let mut external: Option<Rc<ExternalCondition>> = None;
    for _ in 0..levels {
        if external.as_ref().is_none_or(|e| !e.can_grant_condition(key)) {
            external = externals.iter().find(|t| t.can_grant_condition(key)).cloned();
        }
        let Some(e) = &external else { break };
        e.grant_condition(a, key, duration, 0);
    }
}
