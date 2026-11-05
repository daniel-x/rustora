// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Infiltrates`, `Infiltrate`, the `InfiltrateFor*` traits, `Disguise` and `IgnoresDisguise`.
//!
//! Not ported: floating cash text. `DisguiseTooltip` loads as a plain `Tooltip`, so the world tooltip of a disguised
//! actor still shows the real unit. `InfiltrateForDecoration` draws nothing yet.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::field_flags;
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_ext::{all_target_types, enabled_target_types};
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::actor_init::{ActorInits, FactionInit, OwnerInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::AttackInfo;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, IOrderVoice};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

use crate::attack::{Armament, Barrel};
use crate::base::{PlayerExperience, PowerManager};
use crate::cargo::{Enter, EnterHooks};
use crate::cash_repair::EnterBehaviour;
use crate::gps::IPreventsShroudReset;
use crate::interfaces::*;
use crate::orders::{IIssueOrder, IOrderTargeter, UnitOrderTargeter};
use crate::player::PlayerResources;
use crate::production::ValuedInfo;
use crate::support_powers::SupportPowerManager;
use crate::tooltip::Tooltip;
use crate::transforms::{Transform, TransformArgs};

pub fn register(r: &mut TraitRegistry) {
    r.register("Infiltrates", |n| load_info!(InfiltratesInfo, n));
    r.register("InfiltrateForSupportPower", |n| load_info!(InfiltrateForSupportPowerInfo, n));
    r.register("InfiltrateForSupportPowerReset", |n| {
        load_info!(InfiltrateForSupportPowerResetInfo, n)
    });
    r.register("InfiltrateForTransform", |n| load_info!(InfiltrateForTransformInfo, n));
    r.register("InfiltrateForCash", |n| load_info!(InfiltrateForCashInfo, n));
    r.register("InfiltrateForPowerOutage", |n| load_info!(InfiltrateForPowerOutageInfo, n));
    r.register("InfiltrateForExploration", |n| load_info!(InfiltrateForExplorationInfo, n));
    r.register("InfiltrateForDecoration", |n| load_info!(InfiltrateForDecorationInfo, n));
    r.register("Disguise", |n| load_info!(DisguiseInfo, n));
    r.register("DisguiseTooltip", |n| load_info!(crate::tooltip::TooltipInfo, n));
    r.register("IgnoresDisguise", |n| load_info!(IgnoresDisguiseInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct InfiltratesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub types: BitSet<TargetableType>,
    pub voice: String,
    pub valid_relationships: PlayerRelationship,
    pub enter_behaviour: EnterBehaviour,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
    pub enter_cursor: String,
}

impl Default for InfiltratesInfo {
    fn default() -> Self {
        InfiltratesInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            types: BitSet::default(),
            voice: "Action".into(),
            valid_relationships: PlayerRelationship(PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0),
            enter_behaviour: EnterBehaviour::Dispose,
            notification: None,
            text_notification: None,
            enter_cursor: "enter".into(),
        }
    }
}

pub struct Infiltrates {
    info: Rc<InfiltratesInfo>,
    state: ConditionalState,
    me: std::rc::Weak<Infiltrates>,
}

register_traits!(Infiltrates: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(InfiltratesInfo => |info, _init| Some(Rc::new_cyclic(|me| Infiltrates { state: ConditionalState::new(&info.base), info, me: me.clone() })));
conditional_simple!(Infiltrates);

impl Infiltrates {
    fn is_valid_order(&self, order: &Order) -> bool {
        if self.state.is_trait_disabled() {
            return false;
        }
        let types = match order.target.target_type() {
            TargetType::FrozenActor => order.target.frozen_actor().map(|f| f.target_types()),
            TargetType::Actor => order.target.actor().map(enabled_target_types),
            _ => None,
        };
        types.is_some_and(|t| self.info.types.overlaps(&t))
    }

    pub fn can_infiltrate_target(&self, self_: &ActorRef, target: &Target) -> bool {
        let info = &self.info;
        match target.target_type() {
            TargetType::Actor => {
                let a = target.actor().expect("type was checked");
                info.types.overlaps(&enabled_target_types(a))
                    && info
                        .valid_relationships
                        .has_relationship(self_.owner().relationship_with(Some(&a.owner())))
            }
            TargetType::FrozenActor => {
                let f = target.frozen_actor().expect("type was checked");
                f.is_valid()
                    && info.types.overlaps(&f.target_types())
                    && f.owner()
                        .is_some_and(|o| info.valid_relationships.has_relationship(self_.owner().relationship_with(Some(&o))))
            }
            _ => false,
        }
    }
}

impl IIssueOrder for Infiltrates {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let (a, f) = (self.info.clone(), self.info.clone());
        vec![Box::new(UnitOrderTargeter::new(
            "Infiltrate",
            7,
            &self.info.enter_cursor,
            true,
            true,
            Box::new(move |self_, target, _, _| {
                a.valid_relationships
                    .has_relationship(self_.owner().relationship_with(Some(&target.owner())))
                    && a.types.overlaps(&all_target_types(target))
            }),
            Box::new(move |self_, target, _, _| {
                target
                    .owner()
                    .is_some_and(|o| f.valid_relationships.has_relationship(self_.owner().relationship_with(Some(&o))))
                    && f.types.overlaps(&crate::targetable::info_all_target_types(target.info()))
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "Infiltrate").then(|| Order::new("Infiltrate", Some(self_.clone()), target.clone(), queued))
    }
}

impl IOrderVoice for Infiltrates {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "Infiltrate" && self.is_valid_order(order)).then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for Infiltrates {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "Infiltrate" || !self.is_valid_order(order) || !self.can_infiltrate_target(self_, &order.target) {
            return;
        }
        let hooks = InfiltrateHooks {
            infiltrates: self.me.upgrade().expect("alive"),
            notifiers: self_.traits_implementing::<dyn INotifyInfiltration>(),
            enter_actor: RefCell::new(None),
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

/// `Infiltrate`.
struct InfiltrateHooks {
    infiltrates: Rc<Infiltrates>,
    notifiers: Vec<Rc<dyn INotifyInfiltration>>,
    enter_actor: RefCell<Option<ActorRef>>,
}

impl EnterHooks for InfiltrateHooks {
    fn type_name(&self) -> &'static str {
        "Infiltrate"
    }

    fn tick_inner(&self, enter: &Enter, self_: &ActorRef, _target: &Target, _target_is_dead_or_hidden_actor: bool) {
        if self.infiltrates.state.is_trait_disabled() {
            enter.cancel_keep_queue(self_);
        }
    }

    fn try_start_enter(&self, enter: &Enter, self_: &ActorRef, target_actor: &ActorRef) -> bool {
        if !self
            .infiltrates
            .can_infiltrate_target(self_, &Target::from_actor(Some(target_actor)))
        {
            enter.cancel_keep_queue(self_);
            return false;
        }
        *self.enter_actor.borrow_mut() = Some(target_actor.clone());
        true
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        if self.enter_actor.borrow().as_ref().is_none_or(|a| a.id != target_actor.id)
            || !self
                .infiltrates
                .can_infiltrate_target(self_, &Target::from_actor(Some(target_actor)))
        {
            return;
        }
        for n in &self.notifiers {
            n.infiltrating(self_);
        }
        let info = &self.infiltrates.info;
        for t in target_actor.traits_implementing::<dyn INotifyInfiltrated>() {
            t.infiltrated(target_actor, self_, info.types);
        }
        let owner = self_.owner();
        if let Some(n) = info.notification.as_deref().filter(|n| !n.is_empty()) {
            sound::play_notification(&self_.world(), Some(&owner), "Speech", n, Some(&owner.faction));
        }
        self_
            .world()
            .sound
            .add_transient_line(Some(&owner), info.text_notification.as_deref());
        match info.enter_behaviour {
            EnterBehaviour::Dispose => self_.dispose(),
            EnterBehaviour::Suicide => {
                if let Some(h) = self_.trait_or_default::<dyn IHealth>() {
                    h.kill(self_, Some(self_), &Default::default());
                }
            }
            EnterBehaviour::Exit => {}
        }
    }
}

fn give_experience(infiltrator: &ActorRef, amount: i32) {
    if let Some(exp) = infiltrator.owner().player_actor().trait_or_default::<PlayerExperience>() {
        exp.give_experience(amount);
    }
}

/// Notification fields shared by the `InfiltrateFor*` traits.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InfiltrateNotifications {
    pub infiltrated_notification: Option<String>,
    pub infiltrated_text_notification: Option<String>,
    pub infiltration_notification: Option<String>,
    pub infiltration_text_notification: Option<String>,
}

impl InfiltrateNotifications {
    fn play(&self, self_: &ActorRef, infiltrator: &ActorRef) {
        let world = self_.world();
        let (owner, spy_owner) = (self_.owner(), infiltrator.owner());
        if let Some(n) = &self.infiltrated_notification {
            sound::play_notification(&world, Some(&owner), "Speech", n, Some(&owner.faction));
        }
        if let Some(n) = &self.infiltration_notification {
            sound::play_notification(&world, Some(&spy_owner), "Speech", n, Some(&spy_owner.faction));
        }
        world
            .sound
            .add_transient_line(Some(&owner), self.infiltrated_text_notification.as_deref());
        world
            .sound
            .add_transient_line(Some(&spy_owner), self.infiltration_text_notification.as_deref());
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InfiltrateForSupportPowerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub proxy: String,
    pub types: BitSet<TargetableType>,
    pub player_experience: i32,
    #[field(flatten)]
    pub notifications: InfiltrateNotifications,
}

pub struct InfiltrateForSupportPower {
    info: Rc<InfiltrateForSupportPowerInfo>,
}

register_traits!(InfiltrateForSupportPower: INotifyInfiltrated);
info_impl!(InfiltrateForSupportPowerInfo => |info, _init| Some(Rc::new(InfiltrateForSupportPower { info })));

impl INotifyInfiltrated for InfiltrateForSupportPower {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        if !self.info.types.overlaps(&types) {
            return;
        }
        self.info.notifications.play(self_, infiltrator);
        give_experience(infiltrator, self.info.player_experience);
        let (proxy, owner) = (self.info.proxy.clone(), infiltrator.owner());
        infiltrator.world().add_frame_end_task(Box::new(move |w| {
            w.create_actor(&proxy, ActorInits::new().with(OwnerInit::from_player(&owner)));
        }));
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InfiltrateForSupportPowerResetInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub types: BitSet<TargetableType>,
    pub player_experience: i32,
    #[field(flatten)]
    pub notifications: InfiltrateNotifications,
}

pub struct InfiltrateForSupportPowerReset {
    info: Rc<InfiltrateForSupportPowerResetInfo>,
}

register_traits!(InfiltrateForSupportPowerReset: INotifyInfiltrated);
info_impl!(InfiltrateForSupportPowerResetInfo => |info, _init| Some(Rc::new(InfiltrateForSupportPowerReset { info })));

impl INotifyInfiltrated for InfiltrateForSupportPowerReset {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        if !self.info.types.overlaps(&types) {
            return;
        }
        self.info.notifications.play(self_, infiltrator);
        give_experience(infiltrator, self.info.player_experience);
        let manager = self_.owner().player_actor().trait_::<SupportPowerManager>();
        for power in manager.get_powers_for_actor(self_).into_iter().filter(|p| !p.disabled()) {
            power.reset_timer();
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct InfiltrateForTransformInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub into_actor: String,
    pub force_health_percentage: i32,
    pub skip_make_anims: bool,
    pub player_experience: i32,
    pub types: BitSet<TargetableType>,
}

impl Default for InfiltrateForTransformInfo {
    fn default() -> Self {
        InfiltrateForTransformInfo {
            instance_name: None,
            into_actor: String::new(),
            force_health_percentage: 0,
            skip_make_anims: true,
            player_experience: 0,
            types: BitSet::default(),
        }
    }
}

pub struct InfiltrateForTransform {
    info: Rc<InfiltrateForTransformInfo>,
    faction: String,
}

register_traits!(InfiltrateForTransform: INotifyInfiltrated);
info_impl!(InfiltrateForTransformInfo => |info, init| Some(Rc::new(InfiltrateForTransform {
    info,
    faction: init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone()),
})));

impl INotifyInfiltrated for InfiltrateForTransform {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        if !self.info.types.overlaps(&types) {
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
        give_experience(infiltrator, self.info.player_experience);
        self_.queue_activity_queued(false, Transform::new(&self.info.into_actor, args));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct InfiltrateForCashInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub types: BitSet<TargetableType>,
    pub percentage: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub player_experience: i32,
    pub player_experience_percentage: i32,
    #[field(flatten)]
    pub notifications: InfiltrateNotifications,
    pub show_ticks: bool,
}

impl Default for InfiltrateForCashInfo {
    fn default() -> Self {
        InfiltrateForCashInfo {
            instance_name: None,
            types: BitSet::default(),
            percentage: 100,
            minimum: -1,
            maximum: i32::MAX,
            player_experience: 0,
            player_experience_percentage: 0,
            notifications: InfiltrateNotifications::default(),
            show_ticks: true,
        }
    }
}

pub struct InfiltrateForCash {
    info: Rc<InfiltrateForCashInfo>,
}

register_traits!(InfiltrateForCash: INotifyInfiltrated);
info_impl!(InfiltrateForCashInfo => |info, _init| Some(Rc::new(InfiltrateForCash { info })));

impl INotifyInfiltrated for InfiltrateForCash {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        let info = &self.info;
        if !info.types.overlaps(&types) {
            return;
        }
        let target_resources = self_.owner().player_actor().trait_::<PlayerResources>();
        let spy_resources = infiltrator.owner().player_actor().trait_::<PlayerResources>();
        let spy_value = infiltrator.info.trait_info::<ValuedInfo>();
        let to_take = info.maximum.min(target_resources.get_cash_and_resources() * info.percentage / 100);
        let fallback = if info.minimum >= 0 {
            info.minimum
        } else {
            spy_value.map_or(0, |v| v.cost)
        };
        let to_give = to_take.max(fallback);
        target_resources.take_cash(to_take, false);
        spy_resources.give_cash(to_give);
        give_experience(
            infiltrator,
            info.player_experience + to_take * info.player_experience_percentage / 100,
        );
        info.notifications.play(self_, infiltrator);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct InfiltrateForPowerOutageInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub types: BitSet<TargetableType>,
    pub duration: i32,
    pub player_experience: i32,
    #[field(flatten)]
    pub notifications: InfiltrateNotifications,
}

impl Default for InfiltrateForPowerOutageInfo {
    fn default() -> Self {
        InfiltrateForPowerOutageInfo {
            instance_name: None,
            types: BitSet::default(),
            duration: 500,
            player_experience: 0,
            notifications: InfiltrateNotifications::default(),
        }
    }
}

pub struct InfiltrateForPowerOutage {
    info: Rc<InfiltrateForPowerOutageInfo>,
}

register_traits!(InfiltrateForPowerOutage: INotifyInfiltrated);
info_impl!(InfiltrateForPowerOutageInfo => |info, _init| Some(Rc::new(InfiltrateForPowerOutage { info })));

impl INotifyInfiltrated for InfiltrateForPowerOutage {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        if !self.info.types.overlaps(&types) {
            return;
        }
        self.info.notifications.play(self_, infiltrator);
        give_experience(infiltrator, self.info.player_experience);
        self_
            .owner()
            .player_actor()
            .trait_::<PowerManager>()
            .trigger_power_outage(self.info.duration);
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InfiltrateForExplorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub types: BitSet<TargetableType>,
    pub player_experience: i32,
    #[field(flatten)]
    pub notifications: InfiltrateNotifications,
}

pub struct InfiltrateForExploration {
    info: Rc<InfiltrateForExplorationInfo>,
}

register_traits!(InfiltrateForExploration: INotifyInfiltrated);
info_impl!(InfiltrateForExplorationInfo => |info, _init| Some(Rc::new(InfiltrateForExploration { info })));

impl INotifyInfiltrated for InfiltrateForExploration {
    fn infiltrated(&self, self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        if !self.info.types.overlaps(&types) {
            return;
        }
        self.info.notifications.play(self_, infiltrator);
        give_experience(infiltrator, self.info.player_experience);
        let owner = self_.owner();
        infiltrator.owner().shroud().explore(&owner.shroud());
        let prevent_reset = owner
            .player_actor()
            .traits_implementing::<dyn IPreventsShroudReset>()
            .iter()
            .any(|p| p.prevent_shroud_reset(self_));
        if !prevent_reset {
            owner.shroud().reset_exploration();
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InfiltrateForDecorationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub types: BitSet<TargetableType>,
    pub player_experience: i32,
}

pub struct InfiltrateForDecoration {
    info: Rc<InfiltrateForDecorationInfo>,
    state: ConditionalState,
}

register_traits!(InfiltrateForDecoration: INotifyInfiltrated, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(InfiltrateForDecorationInfo => |info, _init| Some(Rc::new(InfiltrateForDecoration { state: ConditionalState::new(&info.base), info })));
conditional_simple!(InfiltrateForDecoration);

impl INotifyInfiltrated for InfiltrateForDecoration {
    fn infiltrated(&self, _self_: &ActorRef, infiltrator: &ActorRef, types: BitSet<TargetableType>) {
        if self.info.types.overlaps(&types) {
            give_experience(infiltrator, self.info.player_experience);
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct IgnoresDisguiseInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(IgnoresDisguiseInfo => |_info, _init| None);

/// `RevealDisguiseType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RevealDisguiseType(pub i32);

#[allow(non_upper_case_globals)]
impl RevealDisguiseType {
    pub const None: RevealDisguiseType = RevealDisguiseType(0);
    pub const Attack: RevealDisguiseType = RevealDisguiseType(1);
    pub const Damaged: RevealDisguiseType = RevealDisguiseType(2);
    pub const Load: RevealDisguiseType = RevealDisguiseType(4);
    pub const Unload: RevealDisguiseType = RevealDisguiseType(8);
    pub const Infiltrate: RevealDisguiseType = RevealDisguiseType(16);
    pub const Demolish: RevealDisguiseType = RevealDisguiseType(32);
    pub const Move: RevealDisguiseType = RevealDisguiseType(64);
}

field_flags!(
    RevealDisguiseType,
    [None, Attack, Damaged, Load, Unload, Infiltrate, Demolish, Move]
);

#[derive(Clone, Debug, FieldLoad)]
pub struct DisguiseInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub voice: String,
    pub disguised_condition: Option<String>,
    pub valid_relationships: PlayerRelationship,
    pub target_types: BitSet<TargetableType>,
    pub reveal_disguise_on: RevealDisguiseType,
    pub disguised_as_conditions: IndexMap<String, String>,
    pub cursor: String,
}

impl Default for DisguiseInfo {
    fn default() -> Self {
        DisguiseInfo {
            instance_name: None,
            voice: "Action".into(),
            disguised_condition: None,
            valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
            target_types: BitSet::new(&["Disguise"]),
            reveal_disguise_on: RevealDisguiseType::Attack,
            disguised_as_conditions: IndexMap::new(),
            cursor: "ability".into(),
        }
    }
}

pub struct Disguise {
    info: Rc<DisguiseInfo>,
    self_: ActorRef,
    as_actor: RefCell<Rc<ActorInfo>>,
    as_player: RefCell<Option<PlayerRef>>,
    disguised_token: Cell<i32>,
    disguised_as_token: Cell<i32>,
    last_pos: Cell<Option<rustora_core::world_types::CPos>>,
}

register_traits!(Disguise: IEffectiveOwner, IIssueOrder, IResolveOrder, IOrderVoice, INotifyAttack, INotifyDamage, INotifyLoadCargo, INotifyUnloadCargo, INotifyDemolition, INotifyInfiltration, ITick);
info_impl!(DisguiseInfo => |info, init| Some(Rc::new(Disguise {
    info,
    self_: init.self_.clone(),
    as_actor: RefCell::new(init.self_.info.clone()),
    as_player: RefCell::new(None),
    disguised_token: Cell::new(INVALID_CONDITION_TOKEN),
    disguised_as_token: Cell::new(INVALID_CONDITION_TOKEN),
    last_pos: Cell::new(None),
})));

impl Disguise {
    pub fn as_actor(&self) -> Rc<ActorInfo> {
        self.as_actor.borrow().clone()
    }

    pub fn as_player(&self) -> Option<PlayerRef> {
        self.as_player.borrow().clone()
    }

    fn reveals_on(&self, flag: RevealDisguiseType) -> bool {
        self.info.reveal_disguise_on.0 & flag.0 != 0
    }

    /// `DisguiseAs(Actor)`: `None` removes the disguise.
    pub fn disguise_as_actor(&self, target: Option<&ActorRef>) {
        let old_actor = self.as_actor();
        let old_owner = self.as_player();
        match target {
            Some(t) => {
                let disguise = t.trait_or_default::<Disguise>().filter(|d| d.as_player.borrow().is_some());
                if let Some(d) = disguise {
                    *self.as_player.borrow_mut() = d.as_player();
                    *self.as_actor.borrow_mut() = d.as_actor();
                } else {
                    assert!(
                        t.traits_implementing::<Tooltip>().iter().any(|x| !x.is_trait_disabled()),
                        "Missing tooltip or invalid target."
                    );
                    *self.as_player.borrow_mut() = Some(t.owner());
                    *self.as_actor.borrow_mut() = t.info.clone();
                }
            }
            None => {
                *self.as_player.borrow_mut() = None;
                *self.as_actor.borrow_mut() = self.self_.info.clone();
            }
        }
        self.handle_disguise(old_actor, old_owner);
    }

    fn disguise_as_info(&self, actor_info: Rc<ActorInfo>, new_owner: Option<PlayerRef>) {
        let old_actor = self.as_actor();
        let old_owner = self.as_player();
        *self.as_player.borrow_mut() = new_owner;
        *self.as_actor.borrow_mut() = actor_info;
        self.handle_disguise(old_actor, old_owner);
    }

    fn handle_disguise(&self, old_actor: Rc<ActorInfo>, old_owner: Option<PlayerRef>) {
        let self_ = &self.self_;
        let disguised = self.as_player.borrow().is_some();
        if disguised != old_owner.is_some() {
            if disguised && self.disguised_token.get() == INVALID_CONDITION_TOKEN {
                if let Some(c) = &self.info.disguised_condition {
                    self.disguised_token.set(self_.grant_condition(c));
                }
            } else if !disguised && self.disguised_token.get() != INVALID_CONDITION_TOKEN {
                self.disguised_token.set(self_.revoke_condition(self.disguised_token.get()));
            }
        }
        let as_actor = self.as_actor();
        if !Rc::ptr_eq(&as_actor, &old_actor) {
            if self.disguised_as_token.get() != INVALID_CONDITION_TOKEN {
                self.disguised_as_token.set(self_.revoke_condition(self.disguised_as_token.get()));
            }
            if let Some(c) = self.info.disguised_as_conditions.get(&as_actor.name) {
                self.disguised_as_token.set(self_.grant_condition(c));
            }
        }
    }
}

impl IEffectiveOwner for Disguise {
    fn disguised(&self) -> bool {
        self.as_player.borrow().is_some()
    }
    fn owner(&self) -> Option<PlayerRef> {
        self.as_player()
    }
}

impl IIssueOrder for Disguise {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        let (a, f) = (self.info.clone(), self.info.clone());
        let mut targeter = UnitOrderTargeter::new(
            "Disguise",
            7,
            &self.info.cursor,
            true,
            true,
            Box::new(move |self_, target, _, _| {
                a.valid_relationships
                    .has_relationship(self_.owner().relationship_with(Some(&target.owner())))
                    && target.id != self_.id
                    && a.target_types.overlaps(&all_target_types(target))
            }),
            Box::new(move |self_, target, _, _| {
                target
                    .owner()
                    .is_some_and(|o| f.valid_relationships.has_relationship(self_.owner().relationship_with(Some(&o))))
                    && f.target_types.overlaps(&crate::targetable::info_all_target_types(target.info()))
            }),
        );
        targeter.force_attack = Some(false);
        vec![Box::new(targeter)]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "Disguise").then(|| Order::new("Disguise", Some(self_.clone()), target.clone(), queued))
    }
}

impl IResolveOrder for Disguise {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "Disguise" {
            return;
        }
        match order.target.target_type() {
            TargetType::Actor => {
                let t = order.target.actor().expect("type was checked");
                self.disguise_as_actor((t.id != self_.id && t.is_in_world()).then_some(t));
            }
            TargetType::FrozenActor => {
                let f = order.target.frozen_actor().expect("type was checked");
                self.disguise_as_info(f.info().clone(), f.owner());
            }
            _ => {}
        }
    }
}

impl IOrderVoice for Disguise {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "Disguise").then(|| self.info.voice.clone())
    }
}

impl INotifyAttack for Disguise {
    fn preparing_attack(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {}
    fn attacking(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {
        if self.reveals_on(RevealDisguiseType::Attack) {
            self.disguise_as_actor(None);
        }
    }
}

impl INotifyDamage for Disguise {
    fn damaged(&self, _self_: &ActorRef, e: &AttackInfo) {
        if self.reveals_on(RevealDisguiseType::Damaged) && e.damage.value > 0 {
            self.disguise_as_actor(None);
        }
    }
}

impl INotifyLoadCargo for Disguise {
    fn loading(&self, _self_: &ActorRef) {
        if self.reveals_on(RevealDisguiseType::Load) {
            self.disguise_as_actor(None);
        }
    }
}

impl INotifyUnloadCargo for Disguise {
    fn unloading(&self, _self_: &ActorRef) {
        if self.reveals_on(RevealDisguiseType::Unload) {
            self.disguise_as_actor(None);
        }
    }
}

impl INotifyDemolition for Disguise {
    fn demolishing(&self, _self_: &ActorRef) {
        if self.reveals_on(RevealDisguiseType::Demolish) {
            self.disguise_as_actor(None);
        }
    }
}

impl INotifyInfiltration for Disguise {
    fn infiltrating(&self, _self_: &ActorRef) {
        if self.reveals_on(RevealDisguiseType::Infiltrate) {
            self.disguise_as_actor(None);
        }
    }
}

impl ITick for Disguise {
    fn tick(&self, self_: &ActorRef) {
        let location = self_.location();
        if self.reveals_on(RevealDisguiseType::Move) && self.last_pos.get().is_some_and(|p| p != location) {
            self.disguise_as_actor(None);
        }
        self.last_pos.set(Some(location));
    }
}
