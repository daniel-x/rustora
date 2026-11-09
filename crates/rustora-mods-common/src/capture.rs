// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `CaptureManager`, `Capturable`, `Captures`, `CaptureActor`, `ProximityCaptor`, `ProximityCapturable`,
//! `TransformOnCapture`,
//! `GivesCashOnCapture`, `RequiresSpecificOwners`, `CapturableProgressBar` and `CapturableProgressBlink`.
//!
//! Not ported: the floating cash text of `GivesCashOnCapture` and the range circle and flash of `ProximityCapturable`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageTypeTag};
use rustora_engine::frozen::FrozenActorRef;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::IOrderVoice;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{conditional_plumbing, info_impl, load_info, register_traits};

use crate::actor_map::{ActorCallback, WorldExt};
use crate::auto_target::IDisableEnemyAutoTarget;
use crate::base::PlayerExperience;
use crate::cargo::{Enter, EnterHooks};
use crate::interfaces::*;
use crate::mobile::Mobile;
use crate::orders::{IIssueOrder, IOrderTargeter, UnitOrderTargeter};
use crate::player::PlayerResources;
use crate::transforms::{Transform, TransformArgs};
use crate::util::{appears_friendly_to, closest_position};

pub fn register(r: &mut TraitRegistry) {
    r.register("CaptureManager", |n| load_info!(CaptureManagerInfo, n));
    r.register("Capturable", |n| load_info!(CapturableInfo, n));
    r.register("Captures", |n| load_info!(CapturesInfo, n));
    r.register("ProximityCaptor", |n| load_info!(ProximityCaptorInfo, n));
    r.register("ProximityCapturable", |n| load_info!(ProximityCapturableInfo, n));
    r.register("RequiresSpecificOwners", |n| load_info!(RequiresSpecificOwnersInfo, n));
    r.register("TransformOnCapture", |n| load_info!(TransformOnCaptureInfo, n));
    r.register("GivesCashOnCapture", |n| load_info!(GivesCashOnCaptureInfo, n));
    r.register("CapturableProgressBar", |n| load_info!(CapturableProgressBarInfo, n));
    r.register("CapturableProgressBlink", |n| load_info!(CapturableProgressBlinkInfo, n));
}

/// Tag of `BitSet<CaptureType>`.
pub struct CaptureType;

pub trait ICaptureProgressWatcher {
    fn update(&self, self_: &ActorRef, captor: &ActorRef, target: &ActorRef, progress: i32, total: i32);
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CaptureManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub capturing_condition: Option<String>,
    pub being_captured_condition: Option<String>,
    pub prevents_auto_target: bool,
}

impl Default for CaptureManagerInfo {
    fn default() -> Self {
        CaptureManagerInfo {
            instance_name: None,
            capturing_condition: None,
            being_captured_condition: None,
            prevents_auto_target: true,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct CaptureManager {
    info: Rc<CaptureManagerInfo>,
    self_: ActorRef,
    mv: RefCell<Option<Rc<dyn IMove>>>,
    progress_watchers: RefCell<Vec<Rc<dyn ICaptureProgressWatcher>>>,
    ally_captures_types: Cell<BitSet<CaptureType>>,
    neutral_captures_types: Cell<BitSet<CaptureType>>,
    enemy_captures_types: Cell<BitSet<CaptureType>>,
    capturable_types: Cell<BitSet<CaptureType>>,
    capturable: RefCell<Vec<Rc<Capturable>>>,
    captures: RefCell<Vec<Rc<Captures>>>,
    current_target: RefCell<Option<ActorRef>>,
    current_target_manager: RefCell<Option<Rc<CaptureManager>>>,
    current_target_delay: Cell<i32>,
    current_target_total: Cell<i32>,
    capturing_token: Cell<i32>,
    being_captured_token: Cell<i32>,
    entering_current_target: Cell<bool>,
    current_captors: RefCell<Vec<ActorRef>>,
    being_captured: Rc<Cell<bool>>,
}

register_traits!(CaptureManager: INotifyCreated, INotifyCapture, ITick, IDisableEnemyAutoTarget);
info_impl!(CaptureManagerInfo => |info, init| Some(Rc::new(CaptureManager {
    info,
    self_: init.self_.clone(),
    mv: RefCell::new(None),
    progress_watchers: RefCell::new(Vec::new()),
    ally_captures_types: Cell::new(BitSet::default()),
    neutral_captures_types: Cell::new(BitSet::default()),
    enemy_captures_types: Cell::new(BitSet::default()),
    capturable_types: Cell::new(BitSet::default()),
    capturable: RefCell::new(Vec::new()),
    captures: RefCell::new(Vec::new()),
    current_target: RefCell::new(None),
    current_target_manager: RefCell::new(None),
    current_target_delay: Cell::new(0),
    current_target_total: Cell::new(0),
    capturing_token: Cell::new(INVALID_CONDITION_TOKEN),
    being_captured_token: Cell::new(INVALID_CONDITION_TOKEN),
    entering_current_target: Cell::new(false),
    current_captors: RefCell::new(Vec::new()),
    being_captured: Rc::new(Cell::new(false)),
})));

impl CaptureManager {
    pub fn refresh_captures(&self) {
        let (mut ally, mut neutral, mut enemy) = (BitSet::default(), BitSet::default(), BitSet::default());
        for c in self.captures.borrow().iter().filter(|c| !c.state.is_trait_disabled()) {
            if c.info.valid_relationships.has_relationship(PlayerRelationship::ALLY) {
                ally = ally.union(&c.info.capture_types);
            }
            if c.info.valid_relationships.has_relationship(PlayerRelationship::NEUTRAL) {
                neutral = neutral.union(&c.info.capture_types);
            }
            if c.info.valid_relationships.has_relationship(PlayerRelationship::ENEMY) {
                enemy = enemy.union(&c.info.capture_types);
            }
        }
        self.ally_captures_types.set(ally);
        self.neutral_captures_types.set(neutral);
        self.enemy_captures_types.set(enemy);
    }

    pub fn refresh_capturable(&self) {
        let mut types = BitSet::default();
        for c in self.capturable.borrow().iter().filter(|c| !c.state.is_trait_disabled()) {
            types = types.union(&c.info.types);
        }
        self.capturable_types.set(types);
    }

    pub fn can_target(&self, target: &CaptureManager) -> bool {
        self.can_target_types(&target.self_.owner(), target.capturable_types.get())
    }

    pub fn can_target_frozen(&self, target: &FrozenActorRef) -> bool {
        let info = target.info();
        if !info.has_trait_info::<CaptureManagerInfo>() {
            return false;
        }
        let mut types = BitSet::default();
        for c in info.trait_infos::<CapturableInfo>() {
            types = types.union(&c.types);
        }
        target.owner().is_some_and(|o| self.can_target_types(&o, types))
    }

    fn can_target_types(&self, target: &PlayerRef, types: BitSet<CaptureType>) -> bool {
        let relationship = self.self_.owner().relationship_with(Some(target));
        if relationship.has_relationship(PlayerRelationship::ENEMY) {
            return types.overlaps(&self.enemy_captures_types.get());
        }
        if relationship.has_relationship(PlayerRelationship::NEUTRAL) {
            return types.overlaps(&self.neutral_captures_types.get());
        }
        if relationship.has_relationship(PlayerRelationship::ALLY) {
            return types.overlaps(&self.ally_captures_types.get());
        }
        false
    }

    pub fn valid_captures_with_lowest_sabotage_threshold(&self, target: &CaptureManager) -> Option<Rc<Captures>> {
        if target.self_.is_dead() {
            return None;
        }
        let relationship = self.self_.owner().relationship_with(Some(&target.self_.owner()));
        let mut captures: Vec<Rc<Captures>> = self
            .captures
            .borrow()
            .iter()
            .filter(|c| !c.state.is_trait_disabled())
            .cloned()
            .collect();
        captures.sort_by_key(|c| (c.info.sabotage_threshold, c.info.capture_delay));
        captures.into_iter().find(|c| {
            c.info.valid_relationships.has_relationship(relationship) && target.capturable_types.get().overlaps(&c.info.capture_types)
        })
    }

    /// `StartCapture`: returns the chosen `Captures` when the captor may enter.
    pub fn start_capture(&self, target_manager: &Rc<CaptureManager>) -> Option<Rc<Captures>> {
        let self_ = &self.self_;
        if self_.will_dispose() {
            return None;
        }
        let target = target_manager.self_.clone();
        let current = self.current_target.borrow().clone();
        if current.as_ref().is_none_or(|c| c.id != target.id) {
            let current_manager = self.current_target_manager.borrow().clone();
            if current_manager.is_some() {
                self.cancel_capture(current.as_ref(), current_manager.as_ref());
            }
            target_manager.add_captor(self_);
            *self.current_target.borrow_mut() = Some(target.clone());
            *self.current_target_manager.borrow_mut() = Some(target_manager.clone());
            self.current_target_delay.set(0);
        } else {
            self.current_target_delay.set(self.current_target_delay.get() + 1);
        }
        if self.capturing_token.get() == INVALID_CONDITION_TOKEN
            && let Some(c) = &self.info.capturing_condition
        {
            self.capturing_token.set(self_.grant_condition(c));
        }
        if target_manager.being_captured_token.get() == INVALID_CONDITION_TOKEN
            && let Some(c) = &target_manager.info.being_captured_condition
        {
            target_manager.being_captured_token.set(target.grant_condition(c));
        }
        let captures = self.valid_captures_with_lowest_sabotage_threshold(target_manager)?;
        if target.trait_or_default::<Mobile>().is_some_and(|m| m.is_moving_between_cells()) {
            return None;
        }
        let watchers = self.progress_watchers.borrow().clone();
        let target_watchers = target_manager.progress_watchers.borrow().clone();
        if !watchers.is_empty() || !target_watchers.is_empty() {
            let mut total = captures.info.capture_delay;
            let mv = self.mv.borrow().clone();
            if let Some(mv) = mv.filter(|_| captures.info.consumed_by_capture) {
                let positions = rustora_engine::actor_ext::targetable_positions(&target);
                if let Some(pos) = closest_position(&positions, self_.center_position()) {
                    total += mv.estimated_move_duration(self_, self_.center_position(), pos);
                }
            }
            self.current_target_total.set(total);
            let delay = self.current_target_delay.get();
            for w in &watchers {
                w.update(self_, self_, &target, delay, total);
            }
            for w in &target_watchers {
                w.update(&target, self_, &target, delay, total);
            }
        }
        let entering = self.current_target_delay.get() >= captures.info.capture_delay;
        self.entering_current_target.set(entering);
        entering.then_some(captures)
    }

    fn add_captor(&self, captor: &ActorRef) {
        let mut captors = self.current_captors.borrow_mut();
        if !captors.iter().any(|c| c.id == captor.id) {
            captors.push(captor.clone());
        }
    }

    pub fn cancel_capture(&self, target: Option<&ActorRef>, target_manager: Option<&Rc<CaptureManager>>) {
        if self.current_target.borrow().is_none() {
            return;
        }
        let self_ = &self.self_;
        if let Some(t) = target {
            for w in self.progress_watchers.borrow().clone() {
                w.update(self_, self_, t, 0, 0);
            }
        }
        if let (Some(m), Some(t)) = (target_manager, target) {
            for w in m.progress_watchers.borrow().clone() {
                w.update(t, self_, t, 0, 0);
            }
        }
        if self.capturing_token.get() != INVALID_CONDITION_TOKEN {
            self.capturing_token.set(self_.revoke_condition(self.capturing_token.get()));
        }
        if let Some(m) = target_manager {
            if m.being_captured_token.get() != INVALID_CONDITION_TOKEN
                && let Some(t) = target
            {
                m.being_captured_token.set(t.revoke_condition(m.being_captured_token.get()));
            }
            m.current_captors.borrow_mut().retain(|c| c.id != self_.id);
        }
        *self.current_target.borrow_mut() = None;
        *self.current_target_manager.borrow_mut() = None;
        self.current_target_delay.set(0);
        self.entering_current_target.set(false);
    }
}

impl INotifyCreated for CaptureManager {
    fn created(&self, self_: &ActorRef) {
        *self.mv.borrow_mut() = self_.trait_or_default::<dyn IMove>();
        *self.progress_watchers.borrow_mut() = self_.traits_implementing::<dyn ICaptureProgressWatcher>();
        *self.capturable.borrow_mut() = self_.traits_implementing::<Capturable>();
        *self.captures.borrow_mut() = self_.traits_implementing::<Captures>();
        self.refresh_captures();
        self.refresh_capturable();
    }
}

impl INotifyCapture for CaptureManager {
    fn on_capture(&self, self_: &ActorRef, _captor: &ActorRef, _old: &PlayerRef, _new: &PlayerRef, _types: BitSet<CaptureType>) {
        self.being_captured.set(true);
        let flag = self.being_captured.clone();
        self_.world().add_frame_end_task(Box::new(move |_| flag.set(false)));
    }
}

impl ITick for CaptureManager {
    fn tick(&self, self_: &ActorRef) {
        if !self.entering_current_target.get() {
            return;
        }
        if self.current_target_delay.get() < self.current_target_total.get() {
            self.current_target_delay.set(self.current_target_delay.get() + 1);
        }
        let (Some(target), Some(manager)) = (self.current_target.borrow().clone(), self.current_target_manager.borrow().clone()) else {
            return;
        };
        let (delay, total) = (self.current_target_delay.get(), self.current_target_total.get());
        for w in self.progress_watchers.borrow().clone() {
            w.update(self_, self_, &target, delay, total);
        }
        for w in manager.progress_watchers.borrow().clone() {
            w.update(&target, self_, &target, delay, total);
        }
    }
}

impl IDisableEnemyAutoTarget for CaptureManager {
    fn disable_enemy_auto_target(&self, _self_: &ActorRef, attacker: &ActorRef) -> bool {
        self.info.prevents_auto_target && self.current_captors.borrow().iter().any(|c| appears_friendly_to(attacker, c))
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct CapturableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub types: BitSet<CaptureType>,
    pub cancel_activity: bool,
}

#[derive(rustora_engine::persist::Persist)]
pub struct Capturable {
    info: Rc<CapturableInfo>,
    state: ConditionalState,
    manager: Rc<CaptureManager>,
}

register_traits!(Capturable: INotifyCapture, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CapturableInfo => |info, init| Some(Rc::new(Capturable {
    state: ConditionalState::new(&info.base),
    info,
    manager: init.self_.trait_::<CaptureManager>(),
})));

impl ConditionalHooks for Capturable {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, _self_: &ActorRef) {
        self.manager.refresh_capturable();
    }
    fn trait_disabled(&self, _self_: &ActorRef) {
        self.manager.refresh_capturable();
    }
}

conditional_plumbing!(Capturable);

impl INotifyCreated for Capturable {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifyCapture for Capturable {
    fn on_capture(&self, self_: &ActorRef, _captor: &ActorRef, _old: &PlayerRef, _new: &PlayerRef, _types: BitSet<CaptureType>) {
        if self.info.cancel_activity {
            self_.cancel_activity();
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CapturesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub capture_types: BitSet<CaptureType>,
    pub sabotage_threshold: i32,
    #[field(name = "SabotageHPRemoval")]
    pub sabotage_hp_removal: i32,
    pub sabotage_damage_types: BitSet<DamageTypeTag>,
    pub capture_delay: i32,
    pub consumed_by_capture: bool,
    pub player_experience: i32,
    pub valid_relationships: PlayerRelationship,
    pub player_experience_relationships: PlayerRelationship,
    pub sabotage_cursor: String,
    pub enter_cursor: String,
    pub enter_blocked_cursor: String,
    pub voice: String,
}

impl Default for CapturesInfo {
    fn default() -> Self {
        CapturesInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            capture_types: BitSet::default(),
            sabotage_threshold: 0,
            sabotage_hp_removal: 50,
            sabotage_damage_types: BitSet::default(),
            capture_delay: 0,
            consumed_by_capture: true,
            player_experience: 0,
            valid_relationships: PlayerRelationship(PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0),
            player_experience_relationships: PlayerRelationship::ENEMY,
            sabotage_cursor: "capture".into(),
            enter_cursor: "enter".into(),
            enter_blocked_cursor: "enter-blocked".into(),
            voice: "Action".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Captures {
    pub info: Rc<CapturesInfo>,
    state: ConditionalState,
    pub manager: Rc<CaptureManager>,
    me: Weak<Captures>,
}

register_traits!(Captures: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CapturesInfo => |info, init| Some(Rc::new_cyclic(|me| Captures {
    state: ConditionalState::new(&info.base),
    info,
    manager: init.self_.trait_::<CaptureManager>(),
    me: me.clone(),
})));

impl ConditionalHooks for Captures {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, _self_: &ActorRef) {
        self.manager.refresh_captures();
    }
    fn trait_disabled(&self, _self_: &ActorRef) {
        self.manager.refresh_captures();
    }
}

conditional_plumbing!(Captures);

impl INotifyCreated for Captures {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IIssueOrder for Captures {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let (a, f) = (self.me.upgrade().expect("alive"), self.me.upgrade().expect("alive"));
        vec![Box::new(UnitOrderTargeter::new(
            "CaptureActor",
            6,
            &self.info.enter_cursor,
            true,
            true,
            Box::new(move |_self_, target, _, cursor| {
                let manager = target.trait_or_default::<CaptureManager>();
                if !manager.is_some_and(|m| a.manager.can_target(&m)) {
                    *cursor = Some(a.info.enter_blocked_cursor.clone());
                    return false;
                }
                *cursor = Some(a.info.enter_cursor.clone());
                if a.info.sabotage_threshold > 0 && !target.owner().non_combatant {
                    let health = target.trait_::<dyn IHealth>();
                    if health.hp() as i64 * 100 > a.info.sabotage_threshold as i64 * health.max_hp() as i64 {
                        *cursor = Some(a.info.sabotage_cursor.clone());
                    }
                }
                true
            }),
            Box::new(move |_self_, target, _, cursor| {
                if !f.manager.can_target_frozen(target) {
                    *cursor = Some(f.info.enter_blocked_cursor.clone());
                    return false;
                }
                *cursor = Some(f.info.enter_cursor.clone());
                if f.info.sabotage_threshold > 0 && target.owner().is_some_and(|o| !o.non_combatant) {
                    let max_hp = target.info().trait_info::<crate::health::HealthInfo>().map_or(1, |h| h.hp);
                    if target.hp() as i64 * 100 > f.info.sabotage_threshold as i64 * max_hp as i64 {
                        *cursor = Some(f.info.sabotage_cursor.clone());
                    }
                }
                true
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "CaptureActor").then(|| Order::new("CaptureActor", Some(self_.clone()), target.clone(), queued))
    }
}

impl IOrderVoice for Captures {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "CaptureActor").then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for Captures {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "CaptureActor" || self.state.is_trait_disabled() {
            return;
        }
        let hooks = CaptureActorHooks {
            manager: self.manager.clone(),
            enter_actor: RefCell::new(None),
            enter_manager: RefCell::new(None),
        };
        self_.queue_activity_queued(
            order.queued,
            rustora_engine::activity::with_line_color(
                Enter::new(self_, &order.target, Box::new(hooks)),
                rustora_engine::activity::LINE_CRIMSON,
            ),
        );
        crate::target_lines::show_target_lines(self_);
    }
}

/// `new CaptureActor(self, target, null)` as queued by scripts.
pub fn capture_activity(self_: &ActorRef, target: &Target) -> rustora_engine::activity::ActivityRef {
    let hooks = CaptureActorHooks {
        manager: self_.trait_::<CaptureManager>(),
        enter_actor: RefCell::new(None),
        enter_manager: RefCell::new(None),
    };
    Enter::new(self_, target, Box::new(hooks))
}

/// `CaptureActor`.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
struct CaptureActorHooks {
    manager: Rc<CaptureManager>,
    enter_actor: RefCell<Option<ActorRef>>,
    enter_manager: RefCell<Option<Rc<CaptureManager>>>,
}

impl CaptureActorHooks {
    fn set_enter_actor(&self, actor: &ActorRef) {
        if self.enter_actor.borrow().as_ref().is_none_or(|a| a.id != actor.id) {
            *self.enter_actor.borrow_mut() = Some(actor.clone());
            *self.enter_manager.borrow_mut() = actor.trait_or_default::<CaptureManager>();
        }
    }

    fn can_target_enter_actor(&self) -> bool {
        self.enter_manager.borrow().as_ref().is_some_and(|m| self.manager.can_target(m))
    }

    fn cancel_capture(&self) {
        let actor = self.enter_actor.borrow().clone();
        let manager = self.enter_manager.borrow().clone();
        self.manager.cancel_capture(actor.as_ref(), manager.as_ref());
    }

    fn do_capture(&self, self_: &ActorRef, captures: Rc<Captures>) {
        let enter_actor = self.enter_actor.borrow().clone().expect("set before capturing");
        let old_owner = enter_actor.owner();
        let captor = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |_| {
            if enter_actor.is_dead() || !Rc::ptr_eq(&old_owner, &enter_actor.owner()) {
                return;
            }
            let info = &captures.info;
            if info.sabotage_threshold > 0 && !enter_actor.owner().non_combatant {
                let health = enter_actor.trait_::<dyn IHealth>();
                if 100 * health.hp() as i64 > info.sabotage_threshold as i64 * health.max_hp() as i64 {
                    let damage = (health.max_hp() as i64 * info.sabotage_hp_removal as i64 / 100) as i32;
                    health.inflict_damage(&enter_actor, Some(&captor), &Damage::new(damage, info.sabotage_damage_types), false);
                    if info.consumed_by_capture {
                        captor.dispose();
                    }
                    return;
                }
            }
            enter_actor.change_owner_sync(captor.owner());
            for t in enter_actor.traits_implementing::<dyn INotifyCapture>() {
                t.on_capture(&enter_actor, &captor, &old_owner, &captor.owner(), info.capture_types);
            }
            if captor
                .owner()
                .relationship_with(Some(&old_owner))
                .has_relationship(info.player_experience_relationships)
                && let Some(exp) = captor.owner().player_actor().trait_or_default::<PlayerExperience>()
            {
                exp.give_experience(info.player_experience);
            }
            if info.consumed_by_capture {
                captor.dispose();
            }
        }));
    }
}

impl EnterHooks for CaptureActorHooks {
    fn type_name(&self) -> &'static str {
        "CaptureActor"
    }

    fn tick_inner(&self, enter: &Enter, self_: &ActorRef, target: &Target, target_is_dead_or_hidden_actor: bool) {
        if target.target_type() == TargetType::Actor
            && let Some(a) = target.actor()
        {
            self.set_enter_actor(a);
        }
        if !target_is_dead_or_hidden_actor && target.target_type() != TargetType::FrozenActor && !self.can_target_enter_actor() {
            enter.cancel_keep_queue(self_);
        }
    }

    fn try_start_enter(&self, enter: &Enter, self_: &ActorRef, target_actor: &ActorRef) -> bool {
        self.set_enter_actor(target_actor);
        if !self.can_target_enter_actor() {
            enter.cancel_keep_queue(self_);
            return false;
        }
        let manager = self.enter_manager.borrow().clone().expect("checked above");
        let Some(captures) = self.manager.start_capture(&manager) else {
            return false;
        };
        if !captures.info.consumed_by_capture {
            self.do_capture(self_, captures);
            enter.cancel_keep_queue(self_);
            return false;
        }
        true
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        if self.enter_actor.borrow().as_ref().is_none_or(|a| a.id != target_actor.id) {
            return;
        }
        let manager = self.enter_manager.borrow().clone().expect("set with the enter actor");
        if manager.being_captured.get() || !self.manager.can_target(&manager) {
            return;
        }
        if let Some(captures) = self.manager.valid_captures_with_lowest_sabotage_threshold(&manager) {
            self.do_capture(self_, captures);
        }
    }

    fn on_last_run(&self, _self_: &ActorRef) {
        self.cancel_capture();
    }

    fn on_actor_dispose(&self, _self_: &ActorRef) {
        self.cancel_capture();
    }

    fn on_cancel(&self, _self_: &ActorRef) {
        self.cancel_capture();
    }
}

/// `ProximityCaptor` only tags actors with the capture types for `ProximityCapturable`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProximityCaptorInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub types: BitSet<CaptureType>,
}

info_impl!(ProximityCaptorInfo => |_info, _init| None);

#[derive(Clone, Debug, FieldLoad)]
pub struct ProximityCapturableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub captor_types: BitSet<CaptureType>,
    pub must_be_clear: bool,
    pub sticky: bool,
    pub permanent: bool,
    pub draw_decoration: bool,
    pub range: rustora_core::world_types::WDist,
}

impl Default for ProximityCapturableInfo {
    fn default() -> Self {
        ProximityCapturableInfo {
            instance_name: None,
            captor_types: BitSet::new(&["Player", "Vehicle", "Tank", "Infantry"]),
            must_be_clear: false,
            sticky: false,
            permanent: false,
            draw_decoration: true,
            range: rustora_core::world_types::WDist::from_cells(5),
        }
    }
}

/// `ProximityCapturable` with `ProximityCapturableBase`: the actor goes to the owner of the captor that has been in
/// range the longest.
#[derive(rustora_engine::persist::Persist)]
pub struct ProximityCapturable {
    info: Rc<ProximityCapturableInfo>,
    me: Weak<ProximityCapturable>,
    self_: ActorRef,
    original_owner: PlayerRef,
    actors_in_range: RefCell<Vec<ActorRef>>,
    trigger: Cell<i32>,
    prev_position: Cell<rustora_core::world_types::WPos>,
    skip_trigger_update: Cell<bool>,
}

register_traits!(ProximityCapturable: crate::actor_map::ITriggerWatcher, ITick, INotifyAddedToWorld, INotifyRemovedFromWorld, INotifyOwnerChanged);
info_impl!(ProximityCapturableInfo => |info, init| Some(Rc::new_cyclic(|me| ProximityCapturable {
    info,
    me: me.clone(),
    self_: init.self_.clone(),
    original_owner: init.self_.owner(),
    actors_in_range: RefCell::new(Vec::new()),
    trigger: Cell::new(0),
    prev_position: Cell::new(rustora_core::world_types::WPos::ZERO),
    skip_trigger_update: Cell::new(false),
})));

impl ProximityCapturable {
    pub fn captured(&self) -> bool {
        !self.self_.is_owned_by(&self.original_owner)
    }

    fn create_trigger(&self, self_: &ActorRef) -> i32 {
        let entered = self.me.clone();
        let exited = self.me.clone();
        self_.world().actor_map().add_proximity_trigger(
            self_.center_position(),
            self.info.range,
            rustora_core::world_types::WDist::ZERO,
            Some(ActorCallback::Entered(entered.upgrade().expect("trait alive"))),
            Some(ActorCallback::Exited(exited.upgrade().expect("trait alive"))),
        )
    }

    fn captor_types(a: &ActorRef) -> Option<BitSet<CaptureType>> {
        a.info.trait_info::<ProximityCaptorInfo>().map(|pc| pc.types)
    }

    fn can_be_captured_by(&self, a: &ActorRef) -> bool {
        a.id != self.self_.id && Self::captor_types(a).is_some_and(|t| t.overlaps(&self.info.captor_types))
    }

    fn actor_entered(&self, other: &ActorRef) {
        if self.skip_trigger_update.get() || !self.can_be_captured_by(other) {
            return;
        }
        self.actors_in_range.borrow_mut().push(other.clone());
        self.update_ownership();
    }

    fn actor_left(&self, other: &ActorRef) {
        if self.skip_trigger_update.get() || !self.can_be_captured_by(other) {
            return;
        }
        {
            let mut in_range = self.actors_in_range.borrow_mut();
            if let Some(i) = in_range.iter().position(|a| a.id == other.id) {
                in_range.remove(i);
            }
        }
        self.update_ownership();
    }

    fn update_ownership(&self) {
        if self.captured() && self.info.permanent {
            self.skip_trigger_update.set(true);
            self.self_.world().actor_map().remove_proximity_trigger(self.trigger.get());
            return;
        }
        let in_range = self.actors_in_range.borrow().clone();
        let owner = self.self_.owner();
        match in_range.first() {
            None => {
                if self.captured() && !self.info.sticky {
                    self.change_ownership(self.original_owner.player_actor().clone());
                }
            }
            Some(captor) if self.info.must_be_clear => {
                let captor_owner = captor.owner();
                let is_clear = in_range
                    .iter()
                    .all(|a| captor_owner.relationship_with(Some(&a.owner())) == PlayerRelationship::ALLY);
                if self.captured() && !is_clear {
                    self.change_ownership(self.original_owner.player_actor().clone());
                } else if !Rc::ptr_eq(&owner, &captor_owner) && is_clear {
                    self.change_ownership(captor.clone());
                }
            }
            Some(captor) => {
                if !Rc::ptr_eq(&owner, &captor.owner()) {
                    self.change_ownership(captor.clone());
                }
            }
        }
    }

    fn change_ownership(&self, captor: ActorRef) {
        let me = self.me.clone();
        self.self_.world().add_frame_end_task(Box::new(move |_w| {
            let Some(t) = me.upgrade() else { return };
            let self_ = t.self_.clone();
            if self_.disposed() || captor.disposed() {
                return;
            }
            t.skip_trigger_update.set(true);
            let previous_owner = self_.owner();
            let new_owner = captor.owner();
            self_.change_owner(new_owner.clone());
            let types = Self::captor_types(&captor).unwrap_or_default();
            for n in self_.traits_implementing::<dyn INotifyCapture>() {
                n.on_capture(&self_, &captor, &previous_owner, &new_owner, types);
            }
        }));
    }
}

impl INotifyAddedToWorld for ProximityCapturable {
    fn added_to_world(&self, self_: &ActorRef) {
        if !self.skip_trigger_update.get() {
            self.trigger.set(self.create_trigger(self_));
        }
    }
}

impl INotifyRemovedFromWorld for ProximityCapturable {
    fn removed_from_world(&self, self_: &ActorRef) {
        if self.skip_trigger_update.get() {
            return;
        }
        self_.world().actor_map().remove_proximity_trigger(self.trigger.get());
        self.actors_in_range.borrow_mut().clear();
    }
}

impl ITick for ProximityCapturable {
    fn tick(&self, self_: &ActorRef) {
        let pos = self_.center_position();
        if !self_.is_in_world() || pos == self.prev_position.get() {
            return;
        }
        self_.world().actor_map().update_proximity_trigger(
            self.trigger.get(),
            pos,
            self.info.range,
            rustora_core::world_types::WDist::ZERO,
        );
        self.prev_position.set(pos);
    }
}

impl INotifyOwnerChanged for ProximityCapturable {
    /// `Game.RunAfterTick` resets the flag after the frame end tasks of this tick; no proximity trigger fires before.
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, _new: &PlayerRef) {
        let me = self.me.clone();
        self_.world().add_frame_end_task(Box::new(move |_w| {
            if let Some(t) = me.upgrade() {
                t.skip_trigger_update.set(false);
            }
        }));
    }
}

/// `RequiresSpecificOwners` only restricts the map editor.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct RequiresSpecificOwnersInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(RequiresSpecificOwnersInfo => |_info, _init| None);

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformOnCaptureInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub into_actor: String,
    pub force_health_percentage: i32,
    pub skip_make_anims: bool,
    pub capture_types: BitSet<CaptureType>,
}

impl Default for TransformOnCaptureInfo {
    fn default() -> Self {
        TransformOnCaptureInfo {
            instance_name: None,
            into_actor: String::new(),
            force_health_percentage: 0,
            skip_make_anims: true,
            capture_types: BitSet::default(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct TransformOnCapture {
    info: Rc<TransformOnCaptureInfo>,
    faction: String,
}

register_traits!(TransformOnCapture: INotifyCapture);
info_impl!(TransformOnCaptureInfo => |info, init| Some(Rc::new(TransformOnCapture {
    info,
    faction: init.inits.get::<rustora_engine::actor_init::FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone()),
})));

impl INotifyCapture for TransformOnCapture {
    fn on_capture(&self, self_: &ActorRef, _captor: &ActorRef, _old: &PlayerRef, _new: &PlayerRef, types: BitSet<CaptureType>) {
        if !self.info.capture_types.is_empty() && !self.info.capture_types.overlaps(&types) {
            return;
        }
        let mut args = TransformArgs {
            force_health_percentage: self.info.force_health_percentage,
            faction: Some(self.faction.clone()),
            skip_make_anims: self.info.skip_make_anims,
            ..TransformArgs::default()
        };
        if let Some(f) = self_.trait_or_default::<dyn IFacing>() {
            args.facing = f.facing();
        }
        self_.queue_activity_queued(false, Transform::new(&self.info.into_actor, args));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GivesCashOnCaptureInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub show_ticks: bool,
    pub display_duration: i32,
    pub amount: i32,
    pub capture_types: BitSet<CaptureType>,
}

impl Default for GivesCashOnCaptureInfo {
    fn default() -> Self {
        GivesCashOnCaptureInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            show_ticks: true,
            display_duration: 30,
            amount: 0,
            capture_types: BitSet::default(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct GivesCashOnCapture {
    info: Rc<GivesCashOnCaptureInfo>,
    state: ConditionalState,
}

register_traits!(GivesCashOnCapture: INotifyCapture, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GivesCashOnCaptureInfo => |info, _init| Some(Rc::new(GivesCashOnCapture { state: ConditionalState::new(&info.base), info })));
rustora_engine::conditional_simple!(GivesCashOnCapture);

impl INotifyCapture for GivesCashOnCapture {
    fn on_capture(&self, _self_: &ActorRef, _captor: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef, types: BitSet<CaptureType>) {
        if self.state.is_trait_disabled() || (!self.info.capture_types.is_empty() && !self.info.capture_types.overlaps(&types)) {
            return;
        }
        new_owner.player_actor().trait_::<PlayerResources>().change_cash(self.info.amount);
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct CapturableProgressBarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
}

#[derive(rustora_engine::persist::Persist)]
pub struct CapturableProgressBar {
    state: ConditionalState,
    progress: RefCell<indexmap::IndexMap<u32, (i32, i32)>>,
}

register_traits!(CapturableProgressBar: ICaptureProgressWatcher, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CapturableProgressBarInfo => |info, _init| Some(Rc::new(CapturableProgressBar {
    state: ConditionalState::new(&info.base),
    progress: RefCell::new(indexmap::IndexMap::new()),
})));
rustora_engine::conditional_simple!(CapturableProgressBar);

impl CapturableProgressBar {
    /// `ISelectionBar.GetValue`.
    pub fn value(&self) -> f32 {
        if self.state.is_trait_disabled() {
            return 0.0;
        }
        self.progress
            .borrow()
            .values()
            .map(|(c, t)| *c as f32 / *t as f32)
            .fold(0.0, f32::max)
    }
}

impl ICaptureProgressWatcher for CapturableProgressBar {
    fn update(&self, self_: &ActorRef, captor: &ActorRef, target: &ActorRef, current: i32, total: i32) {
        if self_.id != target.id {
            return;
        }
        if total == 0 {
            self.progress.borrow_mut().shift_remove(&captor.id);
        } else {
            self.progress.borrow_mut().insert(captor.id, (current, total));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CapturableProgressBlinkInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub interval: i32,
}

impl Default for CapturableProgressBlinkInfo {
    fn default() -> Self {
        CapturableProgressBlinkInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            interval: 50,
        }
    }
}

/// `CapturableProgressBlink` draws `FlashTarget` effects only, which the renderer doesn't have yet; the trait keeps
/// the sync shape of the conditional base.
#[derive(rustora_engine::persist::Persist)]
pub struct CapturableProgressBlink {
    state: ConditionalState,
}

register_traits!(CapturableProgressBlink: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(CapturableProgressBlinkInfo => |info, _init| Some(Rc::new(CapturableProgressBlink { state: ConditionalState::new(&info.base) })));
rustora_engine::conditional_simple!(CapturableProgressBlink);

rustora_engine::persist_trait_refs!(dyn ICaptureProgressWatcher);

impl crate::actor_map::ITriggerWatcher for ProximityCapturable {
    fn actor_entered(&self, a: &ActorRef) {
        ProximityCapturable::actor_entered(self, a)
    }
    fn actor_exited(&self, a: &ActorRef) {
        self.actor_left(a)
    }
}

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_values!(dyn crate::cargo::EnterHooks: CaptureActorHooks);
}
