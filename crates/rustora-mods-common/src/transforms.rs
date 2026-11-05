// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Transforms.cs`, `Activities/Transform.cs`, `Traits/Sellable.cs` and `Activities/Sell.cs`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::world_types::{CPos, CVec, WAngle};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef, for_each_activity};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInits, FacingInit, FactionInit, LocationInit, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::damage::DamageState;
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::IOrderVoice;
use rustora_engine::target::Target;
use rustora_engine::target::TargetType;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{activity_common, conditional_plumbing, conditional_simple, info_impl, load_info, register_traits};

use crate::actor_reference::{HealthInit, SkipMakeAnimsInit};
use crate::base::can_place_building;
use crate::building::BuildingInfo;
use crate::cargo::{Cargo, CargoInfo};
use crate::experience::{ExternalCondition, IConditionTimerWatcher};
use crate::interfaces::*;
use crate::locomotor::{BlockedByActor, Locomotor, MOVEMENT_COST_FOR_UNREACHABLE_CELL};
use crate::orders::{IIssueDeployOrder, IIssueOrder, IOrderTargeter, TargetModifiers, UnitOrderTargeter};
use crate::player::PlayerResources;
use crate::production::get_sell_value;
use crate::render::WithMakeAnimation;
use crate::tooltip::IProvideTooltipInfo;

pub fn register(r: &mut TraitRegistry) {
    r.register("TransformsIntoMobile", |n| load_info!(TransformsIntoMobileInfo, n));
    r.register("TransformsIntoPassenger", |n| load_info!(TransformsIntoPassengerInfo, n));
    r.register("TransformsIntoRepairable", |n| load_info!(TransformsIntoRepairableInfo, n));
    r.register("TransformsIntoTransforms", |n| load_info!(TransformsIntoTransformsInfo, n));
    r.register("TransferTimedExternalConditionOnTransform", |n| {
        load_info!(TransferTimedExternalConditionOnTransformInfo, n)
    });
    r.register("Transforms", |n| load_info!(TransformsInfo, n));
    r.register("Sellable", |n| load_info!(SellableInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub transform_sounds: Vec<String>,
    pub no_transform_sounds: Vec<String>,
    pub transform_notification: Option<String>,
    pub no_transform_notification: Option<String>,
    pub transform_text_notification: Option<String>,
    pub no_transform_text_notification: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    #[field(require)]
    pub into_actor: String,
    pub offset: CVec,
    pub facing: WAngle,
    pub deploy_cursor: String,
    pub deploy_blocked_cursor: String,
}

impl Default for TransformsInfo {
    fn default() -> Self {
        TransformsInfo {
            transform_sounds: Vec::new(),
            no_transform_sounds: Vec::new(),
            transform_notification: None,
            no_transform_notification: None,
            transform_text_notification: None,
            no_transform_text_notification: None,
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            into_actor: String::new(),
            offset: CVec::ZERO,
            facing: WAngle::new(384),
            deploy_cursor: "deploy".into(),
            deploy_blocked_cursor: "deploy-blocked".into(),
        }
    }
}

pub struct Transforms {
    pub info: Rc<TransformsInfo>,
    state: ConditionalState,
    self_: ActorRef,
    building_info: Option<Rc<BuildingInfo>>,
    faction: String,
}

register_traits!(Transforms: IResolveOrder, crate::orders::IIssueOrder, crate::orders::IIssueDeployOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TransformsInfo => |info, init| {
    let w = init.world();
    let actor_info = w.rules().actor(&info.into_actor.to_lowercase()).unwrap_or_else(|| panic!("unknown actor {}", info.into_actor));
    let building_info = actor_info.trait_info::<BuildingInfo>();
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(Transforms { info, state, self_: init.self_.clone(), building_info, faction }))
});

conditional_simple!(Transforms);

impl Transforms {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    pub fn can_deploy(&self) -> bool {
        if self.is_trait_paused() || self.is_trait_disabled() {
            return false;
        }
        match &self.building_info {
            None => true,
            Some(b) => can_place_building(&self.self_.world(), self.self_.location() + self.info.offset, b, Some(&self.self_)),
        }
    }

    pub fn get_transform_activity(&self) -> ActivityRef {
        Transform::new(
            &self.info.into_actor,
            TransformArgs {
                offset: self.info.offset,
                facing: self.info.facing,
                force_health_percentage: 0,
                skip_make_anims: false,
                faction: Some(self.faction.clone()),
                sounds: self.info.transform_sounds.clone(),
                notification: self.info.transform_notification.clone(),
                text_notification: self.info.transform_text_notification.clone(),
            },
        )
    }

    pub fn deploy_transform(&self, queued: bool) {
        // Clearing blockers is done by the issuing client through its own orders.
        if !queued && !self.can_deploy() {
            let world = self.self_.world();
            let owner = self.self_.owner();
            for s in &self.info.no_transform_sounds {
                rustora_engine::sound::play_to_player(&world, rustora_engine::sound::SoundType::World, &owner, s);
            }
            rustora_engine::sound::notify_owner(
                &self.self_,
                "Speech",
                self.info.no_transform_notification.as_deref(),
                self.info.no_transform_text_notification.as_deref(),
            );
            return;
        }
        self.self_.queue_activity_queued(queued, self.get_transform_activity());
    }
}

impl crate::orders::IIssueOrder for Transforms {
    /// The cursor is chosen when the targeter is made instead of when it targets; nothing changes in between.
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn crate::orders::IOrderTargeter>> {
        if self.is_trait_disabled() {
            return Vec::new();
        }
        let cursor = if self.can_deploy() {
            &self.info.deploy_cursor
        } else {
            &self.info.deploy_blocked_cursor
        }
        .clone();
        vec![Box::new(crate::orders::DeployOrderTargeter::new(
            "DeployTransform",
            5,
            Box::new(move || cursor.clone()),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn crate::orders::IOrderTargeter, _target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "DeployTransform").then(|| Order::new("DeployTransform", Some(self_.clone()), Target::Invalid, queued))
    }
}

impl crate::orders::IIssueDeployOrder for Transforms {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order> {
        Some(Order::new("DeployTransform", Some(self_.clone()), Target::Invalid, queued))
    }

    fn can_issue_deploy_order(&self, _self_: &ActorRef, _queued: bool) -> bool {
        !self.is_trait_paused() && !self.is_trait_disabled()
    }
}

impl IResolveOrder for Transforms {
    fn resolve_order(&self, _self_: &ActorRef, order: &Order) {
        if order.order_string == "DeployTransform" && !self.is_trait_paused() && !self.is_trait_disabled() {
            self.deploy_transform(order.queued);
        }
    }
}

/// Shared part of `TransformsIntoMobile`, `TransformsIntoPassenger` and `TransformsIntoRepairable`: runs
/// `order_string` on the actor this one transforms into.
fn queue_after_transform(self_: &ActorRef, order: &Order, transforms: &[Rc<Transforms>], order_string: &str) {
    let current = self_.current_activity().filter(|a| a.as_any().is::<Transform>());
    let transform = transforms.iter().find(|t| !t.is_trait_disabled() && !t.is_trait_paused());
    if transform.is_none() && current.is_none() {
        return;
    }
    let activity = current
        .clone()
        .unwrap_or_else(|| transform.expect("checked").get_transform_activity());
    if !order.queued
        && let Some(next) = activity.base().next_activity()
    {
        rustora_engine::activity::cancel(&next, self_, false);
    }
    rustora_engine::activity::queue(&activity, Some(IssueOrderAfterTransform::new(order_string, &order.target)));
    if current.is_none() {
        self_.queue_activity_queued(order.queued, activity);
    }
}

fn can_transform(self_: &ActorRef, transforms: &[Rc<Transforms>]) -> bool {
    self_.current_activity().is_some_and(|a| a.as_any().is::<Transform>())
        || transforms.iter().any(|t| !t.is_trait_disabled() && !t.is_trait_paused())
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformsIntoMobileInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub locomotor: String,
    pub cursor: String,
    pub terrain_cursors: IndexMap<String, String>,
    pub blocked_cursor: String,
    pub voice: String,
    pub requires_force_move: bool,
}

impl Default for TransformsIntoMobileInfo {
    fn default() -> Self {
        TransformsIntoMobileInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            locomotor: String::new(),
            cursor: "move".into(),
            terrain_cursors: IndexMap::new(),
            blocked_cursor: "move-blocked".into(),
            voice: "Action".into(),
            requires_force_move: false,
        }
    }
}

pub struct TransformsIntoMobile {
    info: Rc<TransformsIntoMobileInfo>,
    state: ConditionalState,
    me: Weak<TransformsIntoMobile>,
    transforms: RefCell<Vec<Rc<Transforms>>>,
    locomotor: RefCell<Option<Rc<Locomotor>>>,
}

register_traits!(TransformsIntoMobile: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TransformsIntoMobileInfo => |info, _init| Some(Rc::new_cyclic(|me| TransformsIntoMobile {
    state: ConditionalState::new(&info.base),
    info,
    me: me.clone(),
    transforms: RefCell::new(Vec::new()),
    locomotor: RefCell::new(None),
})));

impl ConditionalHooks for TransformsIntoMobile {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(TransformsIntoMobile);

impl INotifyCreated for TransformsIntoMobile {
    fn created(&self, self_: &ActorRef) {
        *self.transforms.borrow_mut() = self_.traits_implementing::<Transforms>();
        *self.locomotor.borrow_mut() = self_
            .world()
            .world_actor()
            .traits_implementing::<Locomotor>()
            .into_iter()
            .find(|l| l.info.name == self.info.locomotor);
        conditional_created(self, self_);
    }
}

impl TransformsIntoMobile {
    fn move_into_shroud(&self) -> bool {
        self.locomotor.borrow().as_ref().expect("set when created").info.move_into_shroud
    }
}

impl IIssueOrder for TransformsIntoMobile {
    fn orders(&self, self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        vec![Box::new(TransformMoveTargeter {
            mobile: self.me.upgrade().expect("alive"),
            reject_move: !crate::conditions::accepts_order(self_, "Move"),
            is_queued: Cell::new(false),
        })]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        order
            .as_any()
            .is::<TransformMoveTargeter>()
            .then(|| Order::new("Move", Some(self_.clone()), target.clone(), queued))
    }
}

impl IResolveOrder for TransformsIntoMobile {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.state.is_trait_disabled() {
            return;
        }
        if order.order_string == "Move" {
            if !order.target.is_valid_for(self_) {
                return;
            }
            let world = self_.world();
            let cell = world.map().clamp(world.map().cell_containing(order.target.center_position()));
            if !self.move_into_shroud() && !self_.owner().shroud().is_explored(cell) {
                return;
            }
            let transforms = self.transforms.borrow().clone();
            queue_after_transform(self_, order, &transforms, "Move");
        } else if order.order_string == "Stop" {
            let current = self_.current_activity();
            let Some(current) = current else {
                return;
            };
            if current.as_any().is::<crate::resupply::Resupply>() || current.as_any().is::<crate::aircraft::ReturnToBase>() {
                return;
            }
            self_.cancel_activity();
        }
    }
}

impl IOrderVoice for TransformsIntoMobile {
    fn voice_phrase_for_order(&self, self_: &ActorRef, order: &Order) -> Option<String> {
        if self.state.is_trait_disabled() {
            return None;
        }
        match order.order_string.as_str() {
            "Move" => {
                if !self.move_into_shroud() && order.target.target_type() != TargetType::Invalid {
                    let cell = self_.world().map().cell_containing(order.target.center_position());
                    if !self_.owner().shroud().is_explored(cell) {
                        return None;
                    }
                }
                Some(self.info.voice.clone())
            }
            "Stop" => Some(self.info.voice.clone()),
            _ => None,
        }
    }
}

struct TransformMoveTargeter {
    mobile: Rc<TransformsIntoMobile>,
    reject_move: bool,
    is_queued: Cell<bool>,
}

impl IOrderTargeter for TransformMoveTargeter {
    fn order_id(&self) -> &str {
        "Move"
    }

    fn order_priority(&self) -> i32 {
        4
    }

    fn can_target(&self, self_: &ActorRef, target: &Target, modifiers: &mut TargetModifiers, cursor: &mut Option<String>) -> bool {
        let mobile = &self.mobile;
        if self.reject_move
            || target.target_type() != TargetType::Terrain
            || (mobile.info.requires_force_move && !modifiers.has_modifier(TargetModifiers::FORCE_MOVE))
        {
            return false;
        }
        let world = self_.world();
        let map = world.map();
        let location = map.cell_containing(target.center_position());
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        let explored = self_.owner().shroud().is_explored(location);
        let locomotor = mobile.locomotor.borrow().clone().expect("set when created");
        let transforms = mobile.transforms.borrow();
        let blocked = !map.contains(location)
            || !can_transform(self_, &transforms)
            || (!explored && !locomotor.info.move_into_shroud)
            || (explored
                && locomotor.movement_cost_to_enter_cell(Some(self_), location, BlockedByActor::All, None, false, 0)
                    == MOVEMENT_COST_FOR_UNREACHABLE_CELL);
        let terrain_cursor = explored
            .then(|| mobile.info.terrain_cursors.get(&map.terrain_type_info(location).ty))
            .flatten();
        *cursor = Some(if blocked {
            mobile.info.blocked_cursor.clone()
        } else {
            terrain_cursor.unwrap_or(&mobile.info.cursor).clone()
        });
        true
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(
        &self,
        self_: &ActorRef,
        target: &Target,
        _actors_at: &[ActorRef],
        _xy: CPos,
        modifiers: TargetModifiers,
    ) -> bool {
        if let Some(a) = target.actor().filter(|_| target.target_type() == TargetType::Actor) {
            let selected = crate::selection::Selection::of(&self_.world()).is_some_and(|s| s.contains(a));
            if !Rc::ptr_eq(&a.owner(), &self_.owner()) || selected {
                return true;
            }
        }
        modifiers.has_modifier(TargetModifiers::FORCE_MOVE)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformsIntoPassengerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub cargo_type: Option<String>,
    pub weight: i32,
    pub voice: String,
    pub requires_force_move: bool,
    pub enter_cursor: String,
    pub enter_blocked_cursor: String,
}

impl Default for TransformsIntoPassengerInfo {
    fn default() -> Self {
        TransformsIntoPassengerInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            cargo_type: None,
            weight: 1,
            voice: "Action".into(),
            requires_force_move: false,
            enter_cursor: "enter".into(),
            enter_blocked_cursor: "enter-blocked".into(),
        }
    }
}

pub struct TransformsIntoPassenger {
    info: Rc<TransformsIntoPassengerInfo>,
    state: ConditionalState,
    self_: ActorRef,
    me: Weak<TransformsIntoPassenger>,
    transforms: RefCell<Vec<Rc<Transforms>>>,
}

register_traits!(TransformsIntoPassenger: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TransformsIntoPassengerInfo => |info, init| Some(Rc::new_cyclic(|me| TransformsIntoPassenger {
    state: ConditionalState::new(&info.base),
    info,
    self_: init.self_.clone(),
    me: me.clone(),
    transforms: RefCell::new(Vec::new()),
})));

impl ConditionalHooks for TransformsIntoPassenger {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(TransformsIntoPassenger);

impl INotifyCreated for TransformsIntoPassenger {
    fn created(&self, self_: &ActorRef) {
        *self.transforms.borrow_mut() = self_.traits_implementing::<Transforms>();
        conditional_created(self, self_);
    }
}

impl TransformsIntoPassenger {
    fn is_correct_cargo_type(&self, target: &ActorRef) -> bool {
        let info = target.info.trait_info::<CargoInfo>().expect("target has cargo");
        self.info.cargo_type.as_ref().is_some_and(|c| info.types.contains(c))
    }

    fn can_enter(&self, target: &ActorRef) -> bool {
        can_transform(&self.self_, &self.transforms.borrow())
            && target.trait_or_default::<Cargo>().is_some_and(|c| c.has_space(self.info.weight))
    }
}

impl IIssueOrder for TransformsIntoPassenger {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let (a, b) = (self.me.upgrade().expect("alive"), self.me.upgrade().expect("alive"));
        vec![Box::new(UnitOrderTargeter::enter_allied(
            "EnterTransport",
            5,
            &self.info.enter_cursor,
            &self.info.enter_blocked_cursor,
            |t| t.info.has_trait_info::<CargoInfo>(),
            Box::new(move |t, modifiers| {
                (!a.info.requires_force_move || modifiers.has_modifier(TargetModifiers::FORCE_MOVE)) && a.is_correct_cargo_type(t)
            }),
            Box::new(move |t| b.can_enter(t)),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "EnterTransport").then(|| Order::new("EnterTransport", Some(self_.clone()), target.clone(), queued))
    }
}

impl IResolveOrder for TransformsIntoPassenger {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.state.is_trait_disabled() || order.order_string != "EnterTransport" || order.target.target_type() != TargetType::Actor {
            return;
        }
        let target = order.target.actor().expect("type was checked");
        if !self.can_enter(target) || !self.is_correct_cargo_type(target) {
            return;
        }
        let transforms = self.transforms.borrow().clone();
        queue_after_transform(self_, order, &transforms, "EnterTransport");
    }
}

impl IOrderVoice for TransformsIntoPassenger {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        if self.state.is_trait_disabled() || order.order_string != "EnterTransport" {
            return None;
        }
        let target = order.target.actor().filter(|_| order.target.target_type() == TargetType::Actor)?;
        self.can_enter(target).then(|| self.info.voice.clone())
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformsIntoRepairableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub repair_actors: IndexSet<String>,
    pub voice: String,
    pub requires_force_move: bool,
    pub enter_cursor: String,
    pub enter_blocked_cursor: String,
}

impl Default for TransformsIntoRepairableInfo {
    fn default() -> Self {
        TransformsIntoRepairableInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            repair_actors: IndexSet::new(),
            voice: "Action".into(),
            requires_force_move: false,
            enter_cursor: "enter".into(),
            enter_blocked_cursor: "enter-blocked".into(),
        }
    }
}

pub struct TransformsIntoRepairable {
    info: Rc<TransformsIntoRepairableInfo>,
    state: ConditionalState,
    self_: ActorRef,
    me: Weak<TransformsIntoRepairable>,
    transforms: RefCell<Vec<Rc<Transforms>>>,
}

register_traits!(TransformsIntoRepairable: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TransformsIntoRepairableInfo => |info, init| Some(Rc::new_cyclic(|me| TransformsIntoRepairable {
    state: ConditionalState::new(&info.base),
    info,
    self_: init.self_.clone(),
    me: me.clone(),
    transforms: RefCell::new(Vec::new()),
})));

impl ConditionalHooks for TransformsIntoRepairable {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(TransformsIntoRepairable);

impl INotifyCreated for TransformsIntoRepairable {
    fn created(&self, self_: &ActorRef) {
        *self.transforms.borrow_mut() = self_.traits_implementing::<Transforms>();
        conditional_created(self, self_);
    }
}

impl TransformsIntoRepairable {
    fn can_repair(&self) -> bool {
        can_transform(&self.self_, &self.transforms.borrow()) && self.self_.trait_::<dyn IHealth>().damage_state() > DamageState::UNDAMAGED
    }

    fn can_repair_at(&self, target: &ActorRef) -> bool {
        self.info.repair_actors.contains(&target.info.name)
    }
}

impl IIssueOrder for TransformsIntoRepairable {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let (a, b) = (self.me.upgrade().expect("alive"), self.me.upgrade().expect("alive"));
        vec![Box::new(UnitOrderTargeter::enter_allied(
            "Repair",
            5,
            &self.info.enter_cursor,
            &self.info.enter_blocked_cursor,
            |t| t.info.has_trait_info::<BuildingInfo>(),
            Box::new(move |t, modifiers| {
                (!a.info.requires_force_move || modifiers.has_modifier(TargetModifiers::FORCE_MOVE)) && a.can_repair_at(t)
            }),
            Box::new(move |_| b.can_repair()),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "Repair").then(|| Order::new("Repair", Some(self_.clone()), target.clone(), queued))
    }
}

impl IResolveOrder for TransformsIntoRepairable {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.state.is_trait_disabled() || order.order_string != "Repair" || order.target.target_type() != TargetType::Actor {
            return;
        }
        if !self.can_repair_at(order.target.actor().expect("type was checked")) || !self.can_repair() {
            return;
        }
        let transforms = self.transforms.borrow().clone();
        queue_after_transform(self_, order, &transforms, "Repair");
    }
}

impl IOrderVoice for TransformsIntoRepairable {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "Repair" && !self.state.is_trait_disabled() && self.can_repair()).then(|| self.info.voice.clone())
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformsIntoTransformsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub voice: String,
}

impl Default for TransformsIntoTransformsInfo {
    fn default() -> Self {
        TransformsIntoTransformsInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            voice: "Action".into(),
        }
    }
}

pub struct TransformsIntoTransforms {
    info: Rc<TransformsIntoTransformsInfo>,
    state: ConditionalState,
}

register_traits!(TransformsIntoTransforms: IResolveOrder, IOrderVoice, IIssueDeployOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TransformsIntoTransformsInfo => |info, _init| Some(Rc::new(TransformsIntoTransforms { state: ConditionalState::new(&info.base), info })));
conditional_simple!(TransformsIntoTransforms);

impl IResolveOrder for TransformsIntoTransforms {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.state.is_trait_disabled() || order.order_string != "AfterDeployTransform" || !order.queued {
            return;
        }
        if let Some(current) = self_.current_activity().filter(|a| a.as_any().is::<Transform>()) {
            rustora_engine::activity::queue(&current, Some(IssueOrderAfterTransform::new("DeployTransform", &order.target)));
        }
    }
}

impl IOrderVoice for TransformsIntoTransforms {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "DeployTransform" && !self.state.is_trait_disabled()).then(|| self.info.voice.clone())
    }
}

impl IIssueDeployOrder for TransformsIntoTransforms {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order> {
        Some(Order::new("AfterDeployTransform", Some(self_.clone()), Target::Invalid, queued))
    }
    fn can_issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> bool {
        queued && self_.current_activity().is_some_and(|a| a.as_any().is::<Transform>())
    }
}

/// `TransferTimedExternalConditionOnTransform`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct TransferTimedExternalConditionOnTransformInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
}

pub struct TransferTimedExternalConditionOnTransform {
    info: Rc<TransferTimedExternalConditionOnTransformInfo>,
    duration: Cell<i32>,
    remaining: Cell<i32>,
}

register_traits!(TransferTimedExternalConditionOnTransform: IConditionTimerWatcher, INotifyTransform);
info_impl!(TransferTimedExternalConditionOnTransformInfo => |info, _init| Some(Rc::new(TransferTimedExternalConditionOnTransform {
    info,
    duration: Cell::new(0),
    remaining: Cell::new(0),
})));

impl IConditionTimerWatcher for TransferTimedExternalConditionOnTransform {
    fn update(&self, duration: i32, remaining: i32) {
        self.duration.set(duration);
        self.remaining.set(remaining);
    }
    fn condition(&self) -> &str {
        &self.info.condition
    }
}

impl INotifyTransform for TransferTimedExternalConditionOnTransform {
    fn before_transform(&self, _self_: &ActorRef) {}
    fn on_transform(&self, _self_: &ActorRef) {}
    fn after_transform(&self, to_actor: &ActorRef) {
        if self.remaining.get() <= 0 {
            return;
        }
        let source = self as *const Self as usize;
        let external = to_actor
            .traits_implementing::<ExternalCondition>()
            .into_iter()
            .find(|t| t.info.condition == self.info.condition && t.can_grant_condition(source));
        if let Some(e) = external {
            e.grant_condition(to_actor, source, self.duration.get(), self.remaining.get());
        }
    }
}

pub struct TransformArgs {
    pub offset: CVec,
    pub facing: WAngle,
    pub force_health_percentage: i32,
    pub skip_make_anims: bool,
    pub faction: Option<String>,
    pub sounds: Vec<String>,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
}

impl Default for TransformArgs {
    fn default() -> Self {
        TransformArgs {
            offset: CVec::ZERO,
            facing: WAngle::new(384),
            force_health_percentage: 0,
            skip_make_anims: false,
            faction: None,
            sounds: Vec::new(),
            notification: None,
            text_notification: None,
        }
    }
}

/// `Activities/Transform.cs`.
pub struct Transform {
    base: ActivityBase,
    me: Weak<Transform>,
    to_actor: String,
    args: TransformArgs,
}

impl Transform {
    pub fn new(to_actor: &str, args: TransformArgs) -> ActivityRef {
        Rc::new_cyclic(|me| Transform {
            base: ActivityBase::default(),
            me: me.clone(),
            to_actor: to_actor.to_string(),
            args,
        })
    }

    fn do_transform(&self, self_: &ActorRef, transforms: Option<Rc<Transforms>>, make_animation: Option<Rc<WithMakeAnimation>>) {
        let current = self_.current_activity();
        let me = self.me.upgrade().expect("activity alive");
        let a = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| {
            if a.is_dead() || a.will_dispose() {
                return;
            }
            if transforms.as_ref().is_some_and(|t| !t.can_deploy()) {
                match make_animation.as_ref().filter(|_| !me.args.skip_make_anims) {
                    Some(m) => {
                        let (me2, a2) = (me.clone(), a.clone());
                        m.forward(
                            &a,
                            Rc::new(move || {
                                me2.base.is_interruptible.set(true);
                                me2.cancel(&a2, true);
                            }),
                        );
                    }
                    None => {
                        me.base.is_interruptible.set(true);
                        me.cancel(&a, true);
                    }
                }
                return;
            }
            for nt in a.traits_implementing::<dyn INotifyTransform>() {
                nt.on_transform(&a);
            }
            a.dispose();
            rustora_engine::sound::play_all_to_owner(&a, rustora_engine::sound::SoundType::World, &me.args.sounds, a.center_position());
            rustora_engine::sound::notify_owner(&a, "Speech", me.args.notification.as_deref(), me.args.text_notification.as_deref());
            let mut init = ActorInits::new()
                .with(LocationInit::new(a.location() + me.args.offset))
                .with(OwnerInit::from_player(&a.owner()))
                .with(FacingInit::new(me.args.facing));
            if me.args.skip_make_anims {
                init.push(Rc::new(SkipMakeAnimsInit));
            }
            if let Some(f) = &me.args.faction {
                init.push(Rc::new(FactionInit::new(f.clone())));
            }
            if let Some(h) = a.trait_or_default::<dyn IHealth>() {
                let new_hp = if me.args.force_health_percentage > 0 {
                    me.args.force_health_percentage
                } else {
                    (h.hp() as i64 * 100 / h.max_hp() as i64) as i32
                };
                init.push(Rc::new(HealthInit::new(new_hp)));
            }
            for m in a.traits_implementing::<dyn ITransformActorInitModifier>() {
                m.modify_transform_actor_init(&a, &mut init);
            }
            let n = w.create_actor(&me.to_actor, init);
            for nt in a.traits_implementing::<dyn INotifyTransform>() {
                nt.after_transform(&n);
            }
            let mut transfers = Vec::new();
            for_each_activity(current.clone(), false, &mut |act| {
                if act.as_any().downcast_ref::<IssueOrderAfterTransform>().is_some() {
                    transfers.push(act.clone());
                }
            });
            for t in transfers {
                let tr = t.as_any().downcast_ref::<IssueOrderAfterTransform>().expect("checked");
                if tr.base.is_canceling() {
                    continue;
                }
                let order = Order::new(&tr.order_string, Some(n.clone()), tr.target.clone(), true);
                for r in n.traits_implementing::<dyn IResolveOrder>() {
                    r.resolve_order(&n, &order);
                }
            }
            *a.replaced_by_actor.borrow_mut() = Some(n);
        }));
    }
}

impl Activity for Transform {
    activity_common!("Transform");

    fn on_first_run(&self, self_: &ActorRef) {
        if self_.info.has_trait_info::<crate::mobile::MobileInfo>()
            || self_.info.has_trait_info_named("Husk")
            || self_.info.has_trait_info_named("Aircraft")
        {
            self.base.queue_child(crate::activities::Turn::new(self_, self.args.facing));
        }
        if self_.info.has_trait_info_named("Aircraft") {
            self.base.queue_child(crate::aircraft::Land::new(self_));
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let transforms = self_.trait_or_default::<Transforms>();
        if transforms.as_ref().is_some_and(|t| !t.can_deploy()) {
            return true;
        }
        for nt in self_.traits_implementing::<dyn INotifyTransform>() {
            nt.before_transform(self_);
        }
        let make_animation = self_.trait_or_default::<WithMakeAnimation>();
        if let Some(m) = make_animation.as_ref().filter(|_| !self.args.skip_make_anims) {
            self.base.is_interruptible.set(false);
            self.base.queue_child(crate::activities::WaitFor::new(Box::new(|| false), true));
            let (me, a, t, m2) = (
                self.me.upgrade().expect("activity alive"),
                self_.clone(),
                transforms.clone(),
                m.clone(),
            );
            m.reverse(self_, Rc::new(move || me.do_transform(&a, t.clone(), Some(m2.clone()))));
            return false;
        }
        self.do_transform(self_, transforms, None);
        true
    }
}

/// `IssueOrderAfterTransform`.
pub struct IssueOrderAfterTransform {
    base: ActivityBase,
    order_string: String,
    target: Target,
}

impl IssueOrderAfterTransform {
    pub fn new(order_string: &str, target: &Target) -> ActivityRef {
        Rc::new(IssueOrderAfterTransform {
            base: ActivityBase::default(),
            order_string: order_string.to_string(),
            target: target.clone(),
        })
    }
}

impl Activity for IssueOrderAfterTransform {
    activity_common!("IssueOrderAfterTransform");
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SellableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub sell_sounds: Vec<String>,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub refund_percent: i32,
    pub show_ticks: bool,
    pub skip_make_animation: bool,
    pub show_tooltip_text: bool,
    pub cursor: String,
}

impl Default for SellableInfo {
    fn default() -> Self {
        SellableInfo {
            sell_sounds: Vec::new(),
            notification: None,
            text_notification: None,
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            refund_percent: 50,
            show_ticks: true,
            skip_make_animation: false,
            show_tooltip_text: true,
            cursor: "sell".into(),
        }
    }
}

pub struct Sellable {
    pub info: Rc<SellableInfo>,
    state: ConditionalState,
}

impl IProvideTooltipInfo for Sellable {
    fn is_tooltip_visible(&self, self_: &ActorRef, for_player: Option<&rustora_engine::player::PlayerRef>, sell_mode: bool) -> bool {
        self.info.show_tooltip_text
            && !self.state.is_trait_disabled()
            && sell_mode
            && for_player.is_some_and(|p| Rc::ptr_eq(p, &self_.owner()))
    }

    fn tooltip_text(&self, self_: &ActorRef, _message: &dyn Fn(&str) -> String) -> String {
        let sell_value = get_sell_value(self_) as i64;
        let (hp, max_hp) = self_
            .trait_or_default::<dyn IHealth>()
            .map_or((1, 1), |h| (h.hp() as i64, h.max_hp() as i64));
        format!(
            "Refund: ${}",
            (sell_value * self.info.refund_percent as i64 * hp / (100 * max_hp)) as i32
        )
    }
}

impl Sellable {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

register_traits!(Sellable: IResolveOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync, IProvideTooltipInfo);
info_impl!(SellableInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Sellable { info, state }))
});

conditional_simple!(Sellable);

impl IResolveOrder for Sellable {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Sell" {
            self.sell(self_);
        }
    }
}

impl Sellable {
    pub fn sell(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        self_.cancel_activity();
        rustora_engine::sound::play_all_to_owner(
            self_,
            rustora_engine::sound::SoundType::Ui,
            &self.info.sell_sounds,
            self_.center_position(),
        );
        for ns in self_.traits_implementing::<dyn INotifySold>() {
            ns.selling(self_);
        }
        if !self.info.skip_make_animation
            && let Some(m) = self_.trait_or_default::<WithMakeAnimation>()
        {
            m.reverse_then_activity(self_, Sell::new(self_), false);
            return;
        }
        self_.queue_activity_queued(false, Sell::new(self_));
    }
}

/// `Activities/Sell.cs`.
pub struct Sell {
    base: ActivityBase,
    health: Option<Rc<dyn IHealth>>,
    sellable_info: Rc<SellableInfo>,
    player_resources: Rc<PlayerResources>,
}

impl Sell {
    pub fn new(self_: &ActorRef) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        Rc::new(Sell {
            base,
            health: self_.trait_or_default::<dyn IHealth>(),
            sellable_info: self_.info.trait_info::<SellableInfo>().expect("Sellable"),
            player_resources: self_.owner().player_actor().trait_::<PlayerResources>(),
        })
    }
}

impl Activity for Sell {
    activity_common!("Sell");

    fn tick(&self, self_: &ActorRef) -> bool {
        let sell_value = get_sell_value(self_) as i64;
        let (hp, max_hp) = self.health.as_ref().map_or((1i64, 1i64), |h| (h.hp() as i64, h.max_hp() as i64));
        let refund = (sell_value * self.sellable_info.refund_percent as i64 * hp / (100 * max_hp)) as i32;
        self.player_resources.change_cash(refund);
        for ns in self_.traits_implementing::<dyn INotifySold>() {
            ns.sold(self_);
        }
        rustora_engine::sound::notify_owner(
            self_,
            "Speech",
            self.sellable_info.notification.as_deref(),
            self.sellable_info.text_notification.as_deref(),
        );
        self_.dispose();
        false
    }
}
