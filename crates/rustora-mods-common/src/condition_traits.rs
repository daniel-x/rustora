// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of small condition and multiplier traits: `ToggleConditionOnOrder`, `GrantConditionOnDeploy`, `KillsSelf`,
//! `ProximityExternalCondition`, `GrantExternalConditionToProduced`, `GrantConditionOnCombatantOwner`, `PowerMultiplier`,
//! the handicap multipliers, `GivesBounty`, `SpawnActorsOnSell`, `GrantConditionOnMovement`, `GrantRandomCondition`, `ProductionTimeMultiplier` and
//! `ProductionCostMultiplier`.
//!
//! Not ported: the floating bounty text, `IDelayCarryallPickup` because no RA actor uses a carryall, and the ramp check
//! of `GrantConditionOnDeploy` because RA tilesets have no ramps.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::world_types::{CPos, WAngle, WDist, WPos};
use rustora_derive::FieldLoad;
use rustora_engine::activity::TargetLineNode;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_init::{ActorInits, LocationInit, OwnerInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, Damage, DamageTypeTag};
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, IOrderVoice, SoundType};
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{
    activity_common, conditional_observers_impl, conditional_plumbing, conditional_simple, info_impl, load_info, register_traits,
};

use crate::activities::Turn;
use crate::actor_map::WorldExt;
use crate::base::PowerManager;
use crate::building::BuildingInfo;
use crate::experience::ExternalCondition;
use crate::interfaces::*;
use crate::orders::{DeployOrderTargeter, IIssueDeployOrder, IIssueOrder, IOrderTargeter};
use crate::player::PlayerResources;
use crate::production::{CustomSellValueInfo, TechTree, ValuedInfo, get_sell_value};
use crate::util::random_in_range;

pub fn register(r: &mut TraitRegistry) {
    r.register("ToggleConditionOnOrder", |n| load_info!(ToggleConditionOnOrderInfo, n));
    r.register("GrantConditionOnDeploy", |n| load_info!(GrantConditionOnDeployInfo, n));
    r.register("KillsSelf", |n| load_info!(KillsSelfInfo, n));
    r.register("ProximityExternalCondition", |n| load_info!(ProximityExternalConditionInfo, n));
    r.register("GrantExternalConditionToProduced", |n| {
        load_info!(GrantExternalConditionToProducedInfo, n)
    });
    r.register("GrantConditionOnCombatantOwner", |n| {
        load_info!(GrantConditionOnCombatantOwnerInfo, n)
    });
    r.register("PowerMultiplier", |n| load_info!(PowerMultiplierInfo, n));
    r.register("HandicapFirepowerMultiplier", |n| load_info!(HandicapFirepowerMultiplierInfo, n));
    r.register("HandicapDamageMultiplier", |n| load_info!(HandicapDamageMultiplierInfo, n));
    r.register("HandicapProductionTimeMultiplier", |n| {
        load_info!(HandicapProductionTimeMultiplierInfo, n)
    });
    r.register("GivesBounty", |n| load_info!(GivesBountyInfo, n));
    r.register("SpawnActorsOnSell", |n| load_info!(SpawnActorsOnSellInfo, n));
    r.register("GrantConditionOnMovement", |n| load_info!(GrantConditionOnMovementInfo, n));
    r.register("GrantRandomCondition", |n| load_info!(GrantRandomConditionInfo, n));
    r.register("ProductionTimeMultiplier", |n| load_info!(ProductionTimeMultiplierInfo, n));
    r.register("ProductionCostMultiplier", |n| load_info!(ProductionCostMultiplierInfo, n));
}

/// The fields of `ProductionTimeMultiplier` and `ProductionCostMultiplier`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ProductionMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub multiplier: i32,
    pub prerequisites: Vec<String>,
    pub queue: Vec<String>,
}

impl Default for ProductionMultiplierInfo {
    fn default() -> Self {
        ProductionMultiplierInfo {
            instance_name: None,
            multiplier: 100,
            prerequisites: Vec::new(),
            queue: Vec::new(),
        }
    }
}

impl ProductionMultiplierInfo {
    fn modifier(&self, tech_tree: &TechTree, queue: &str) -> i32 {
        if (self.queue.is_empty() || self.queue.iter().any(|q| q == queue))
            && (self.prerequisites.is_empty() || tech_tree.has_prerequisites(&self.prerequisites))
        {
            self.multiplier
        } else {
            100
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProductionTimeMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ProductionMultiplierInfo,
}

info_impl!(ProductionTimeMultiplierInfo => |_info, _init| None, [IProductionTimeModifierInfo]);

impl IProductionTimeModifierInfo for ProductionTimeMultiplierInfo {
    fn get_production_time_modifier(&self, tech_tree: &TechTree, queue: &str) -> i32 {
        self.base.modifier(tech_tree, queue)
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ProductionCostMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ProductionMultiplierInfo,
}

info_impl!(ProductionCostMultiplierInfo => |_info, _init| None, [IProductionCostModifierInfo]);

impl IProductionCostModifierInfo for ProductionCostMultiplierInfo {
    fn get_production_cost_modifier(&self, tech_tree: &TechTree, queue: &str) -> i32 {
        self.base.modifier(tech_tree, queue)
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantRandomConditionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub conditions: Vec<String>,
}

/// Grants a random condition of `Conditions` when the actor is created.
pub struct GrantRandomCondition {
    info: Rc<GrantRandomConditionInfo>,
}

register_traits!(GrantRandomCondition: INotifyCreated);
info_impl!(GrantRandomConditionInfo => |info, _| Some(Rc::new(GrantRandomCondition { info })));

impl INotifyCreated for GrantRandomCondition {
    fn created(&self, self_: &ActorRef) {
        let conditions = &self.info.conditions;
        if conditions.is_empty() {
            return;
        }
        let i = self_.world().shared_random.borrow_mut().next_max(conditions.len() as i32) as usize;
        self_.grant_condition(&conditions[i]);
    }
}

/// The `MovementType` flags of a field, like `Horizontal, Turn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovementTypes(pub u8);

impl rustora_core::fieldloader::FieldValue for MovementTypes {
    const TYPE_NAME: &'static str = "MovementType";
    fn parse(
        field: &str,
        value: Option<&str>,
        _yaml: Option<&rustora_core::miniyaml::MiniYaml>,
    ) -> Result<Self, rustora_core::fieldloader::LoadError> {
        let mut r = 0;
        for p in value.unwrap_or("").split(',').map(str::trim).filter(|p| !p.is_empty()) {
            r |= match p.to_ascii_lowercase().as_str() {
                "none" => crate::locomotor::movement_type::NONE,
                "horizontal" => crate::locomotor::movement_type::HORIZONTAL,
                "vertical" => crate::locomotor::movement_type::VERTICAL,
                "turn" => crate::locomotor::movement_type::TURN,
                _ => return rustora_core::fieldloader::invalid(value, "MovementType", field),
            };
        }
        Ok(MovementTypes(r))
    }
}

impl rustora_core::fieldloader::ArrayAware for MovementTypes {}

#[derive(Clone, Debug, FieldLoad)]
pub struct GrantConditionOnMovementInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub condition: String,
    pub valid_movement_types: MovementTypes,
}

impl Default for GrantConditionOnMovementInfo {
    fn default() -> Self {
        GrantConditionOnMovementInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            condition: String::new(),
            valid_movement_types: MovementTypes(crate::locomotor::movement_type::HORIZONTAL),
        }
    }
}

/// Grants a condition while the actor moves in one of `ValidMovementTypes`.
pub struct GrantConditionOnMovement {
    info: Rc<GrantConditionOnMovementInfo>,
    state: ConditionalState,
    token: Cell<i32>,
}

register_traits!(GrantConditionOnMovement: INotifyMoving, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GrantConditionOnMovementInfo => |info, _| Some(Rc::new(GrantConditionOnMovement {
    state: ConditionalState::new(&info.base),
    info,
    token: Cell::new(INVALID_CONDITION_TOKEN),
})));

impl GrantConditionOnMovement {
    fn update_condition(&self, self_: &ActorRef, types: u8) {
        let valid = !self.state.is_trait_disabled() && types & self.info.valid_movement_types.0 != 0;
        if !valid && self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        } else if valid && self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }

    fn current_types(self_: &ActorRef) -> u8 {
        self_.trait_::<dyn IMove>().current_movement_types()
    }
}

impl ConditionalHooks for GrantConditionOnMovement {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        self.update_condition(self_, Self::current_types(self_));
    }

    fn trait_disabled(&self, self_: &ActorRef) {
        self.update_condition(self_, Self::current_types(self_));
    }
}
rustora_engine::conditional_plumbing!(GrantConditionOnMovement);

impl INotifyCreated for GrantConditionOnMovement {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifyMoving for GrantConditionOnMovement {
    fn movement_type_changed(&self, self_: &ActorRef, ty: u8) {
        self.update_condition(self_, ty);
    }
}

fn notify_owner_faction(self_: &ActorRef, kind: &str, notification: Option<&str>) {
    if let Some(n) = notification {
        let owner = self_.owner();
        sound::play_notification(&self_.world(), Some(&owner), kind, n, Some(owner.faction.as_str()));
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ToggleConditionOnOrderInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    #[field(require)]
    pub condition: String,
    #[field(require)]
    pub order_name: String,
    pub enabled_sound: Option<String>,
    pub enabled_speech: Option<String>,
    pub enabled_text_notification: Option<String>,
    pub disabled_sound: Option<String>,
    pub disabled_speech: Option<String>,
    pub disabled_text_notification: Option<String>,
}

pub struct ToggleConditionOnOrder {
    info: Rc<ToggleConditionOnOrderInfo>,
    state: ConditionalState,
    token: Cell<i32>,
    enabled: Cell<bool>,
}

register_traits!(ToggleConditionOnOrder: IResolveOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ToggleConditionOnOrderInfo => |info, _init| Some(Rc::new(ToggleConditionOnOrder {
    state: ConditionalState::new_pausable(&info.base),
    info,
    token: Cell::new(INVALID_CONDITION_TOKEN),
    enabled: Cell::new(false),
})));

impl ToggleConditionOnOrder {
    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }

    pub fn order_name(&self) -> &str {
        &self.info.order_name
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    fn set_condition(&self, self_: &ActorRef, granted: bool) {
        let info = &self.info;
        let world = self_.world();
        if granted && self.token.get() == INVALID_CONDITION_TOKEN {
            self.token.set(self_.grant_condition(&info.condition));
            notify_owner_faction(self_, "Sounds", info.enabled_sound.as_deref());
            notify_owner_faction(self_, "Speech", info.enabled_speech.as_deref());
            world
                .sound
                .add_transient_line(Some(&self_.owner()), info.enabled_text_notification.as_deref());
        } else if !granted && self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
            notify_owner_faction(self_, "Sounds", info.disabled_sound.as_deref());
            notify_owner_faction(self_, "Speech", info.disabled_speech.as_deref());
            world
                .sound
                .add_transient_line(Some(&self_.owner()), info.disabled_text_notification.as_deref());
        }
    }
}

impl ConditionalHooks for ToggleConditionOnOrder {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.enabled.set(false);
        self.set_condition(self_, false);
    }
    fn trait_paused(&self, self_: &ActorRef) {
        self.set_condition(self_, false);
    }
    fn trait_resumed(&self, self_: &ActorRef) {
        self.set_condition(self_, self.enabled.get());
    }
}

conditional_observers_impl!(ToggleConditionOnOrder);

impl ISync for ToggleConditionOnOrder {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ sync_bool(self.enabled.get())
    }
}

impl INotifyCreated for ToggleConditionOnOrder {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IResolveOrder for ToggleConditionOnOrder {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if !self.state.is_trait_disabled() && !self.state.is_trait_paused() && order.order_string == self.info.order_name {
            self.enabled.set(!self.enabled.get());
            self.set_condition(self_, self.enabled.get());
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GrantConditionOnDeployInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub undeployed_condition: Option<String>,
    #[field(require)]
    pub deployed_condition: String,
    pub allowed_terrain_types: IndexSet<String>,
    pub can_deploy_on_ramps: bool,
    pub deploy_cursor: String,
    pub deploy_blocked_cursor: String,
    pub facing: Option<WAngle>,
    pub deploy_sounds: Vec<String>,
    pub undeploy_sounds: Vec<String>,
    pub skip_make_animation: bool,
    pub undeploy_on_move: bool,
    pub undeploy_on_pickup: bool,
    pub voice: String,
}

impl Default for GrantConditionOnDeployInfo {
    fn default() -> Self {
        GrantConditionOnDeployInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            undeployed_condition: None,
            deployed_condition: String::new(),
            allowed_terrain_types: IndexSet::new(),
            can_deploy_on_ramps: false,
            deploy_cursor: "deploy".into(),
            deploy_blocked_cursor: "deploy-blocked".into(),
            facing: None,
            deploy_sounds: Vec::new(),
            undeploy_sounds: Vec::new(),
            skip_make_animation: false,
            undeploy_on_move: false,
            undeploy_on_pickup: false,
            voice: "Action".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeployState {
    Undeployed,
    Deploying,
    Deployed,
    Undeploying,
}

const DEPLOY_ORDER: &str = "GrantConditionOnDeploy";

pub struct GrantConditionOnDeploy {
    info: Rc<GrantConditionOnDeployInfo>,
    state: ConditionalState,
    self_: ActorRef,
    me: Weak<GrantConditionOnDeploy>,
    notify: RefCell<Vec<Rc<dyn INotifyDeployTriggered>>>,
    deployed_token: Cell<i32>,
    undeployed_token: Cell<i32>,
    deploy_state: Cell<DeployState>,
}

register_traits!(GrantConditionOnDeploy: IResolveOrder, IIssueOrder, INotifyDeployComplete, IIssueDeployOrder, IOrderVoice, IWrapMove, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GrantConditionOnDeployInfo => |info, init| Some(Rc::new_cyclic(|me| GrantConditionOnDeploy {
    state: ConditionalState::new_pausable(&info.base),
    info,
    self_: init.self_.clone(),
    me: me.clone(),
    notify: RefCell::new(Vec::new()),
    deployed_token: Cell::new(INVALID_CONDITION_TOKEN),
    undeployed_token: Cell::new(INVALID_CONDITION_TOKEN),
    deploy_state: Cell::new(DeployState::Undeployed),
})));

impl ConditionalHooks for GrantConditionOnDeploy {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(GrantConditionOnDeploy);

impl INotifyCreated for GrantConditionOnDeploy {
    fn created(&self, self_: &ActorRef) {
        *self.notify.borrow_mut() = self_.traits_implementing::<dyn INotifyDeployTriggered>();
        conditional_created(self, self_);
        self.on_undeploy_completed();
    }
}

impl GrantConditionOnDeploy {
    pub fn deploy_state(&self) -> DeployState {
        self.deploy_state.get()
    }

    fn can_deploy(&self) -> bool {
        if self.state.is_trait_paused() || self.state.is_trait_disabled() {
            return false;
        }
        self.is_valid_terrain(self.self_.location()) || self.deploy_state.get() == DeployState::Deployed
    }

    pub fn is_valid_terrain(&self, location: CPos) -> bool {
        let map = self.self_.world().map().clone();
        if !map.contains(location) {
            return false;
        }
        self.info.allowed_terrain_types.is_empty() || self.info.allowed_terrain_types.contains(&map.terrain_type_info(location).ty)
    }

    pub fn deploy(&self) {
        if self.deploy_state.get() != DeployState::Undeployed || !self.is_valid_terrain(self.self_.location()) {
            return;
        }
        let self_ = &self.self_;
        sound::play_random_at(
            &self_.world(),
            SoundType::World,
            &self.info.deploy_sounds,
            self_.center_position(),
            1.0,
        );
        if self.undeployed_token.get() != INVALID_CONDITION_TOKEN {
            self.undeployed_token.set(self_.revoke_condition(self.undeployed_token.get()));
        }
        self.deploy_state.set(DeployState::Deploying);
        let notify = self.notify.borrow().clone();
        if notify.is_empty() {
            self.on_deploy_completed();
        } else {
            for n in notify {
                n.deploy(self_, self.info.skip_make_animation);
            }
        }
    }

    pub fn undeploy(&self) {
        if self.deploy_state.get() != DeployState::Deployed {
            return;
        }
        let self_ = &self.self_;
        sound::play_random_at(
            &self_.world(),
            SoundType::World,
            &self.info.undeploy_sounds,
            self_.center_position(),
            1.0,
        );
        if self.deployed_token.get() != INVALID_CONDITION_TOKEN {
            self.deployed_token.set(self_.revoke_condition(self.deployed_token.get()));
        }
        self.deploy_state.set(DeployState::Deploying);
        let notify = self.notify.borrow().clone();
        if notify.is_empty() {
            self.on_undeploy_completed();
        } else {
            for n in notify {
                n.undeploy(self_, self.info.skip_make_animation);
            }
        }
    }

    fn on_deploy_completed(&self) {
        if self.deployed_token.get() == INVALID_CONDITION_TOKEN {
            self.deployed_token.set(self.self_.grant_condition(&self.info.deployed_condition));
        }
        self.deploy_state.set(DeployState::Deployed);
    }

    fn on_undeploy_completed(&self) {
        if self.undeployed_token.get() == INVALID_CONDITION_TOKEN
            && let Some(c) = &self.info.undeployed_condition
        {
            self.undeployed_token.set(self.self_.grant_condition(c));
        }
        self.deploy_state.set(DeployState::Undeployed);
    }
}

impl INotifyDeployComplete for GrantConditionOnDeploy {
    fn finished_deploy(&self, _self_: &ActorRef) {
        self.on_deploy_completed();
    }
    fn finished_undeploy(&self, _self_: &ActorRef) {
        self.on_undeploy_completed();
    }
}

impl IWrapMove for GrantConditionOnDeploy {
    fn is_trait_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }
    fn wrap_move(&self, inner: ActivityRef) -> ActivityRef {
        if !self.info.undeploy_on_move {
            return inner;
        }
        let activity = DeployForGrantedCondition::new(&self.self_, self.me.upgrade().expect("trait is alive"), true);
        rustora_engine::activity::queue(&activity, Some(inner));
        activity
    }
}

impl IIssueOrder for GrantConditionOnDeploy {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let cursor = if self.can_deploy() {
            &self.info.deploy_cursor
        } else {
            &self.info.deploy_blocked_cursor
        }
        .clone();
        vec![Box::new(DeployOrderTargeter::new(
            DEPLOY_ORDER,
            5,
            Box::new(move || cursor.clone()),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == DEPLOY_ORDER).then(|| Order::new(DEPLOY_ORDER, Some(self_.clone()), target.clone(), queued))
    }
}

impl IIssueDeployOrder for GrantConditionOnDeploy {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order> {
        Some(Order::new(DEPLOY_ORDER, Some(self_.clone()), Target::Invalid, queued))
    }
    fn can_issue_deploy_order(&self, _self_: &ActorRef, _queued: bool) -> bool {
        !self.state.is_trait_paused() && !self.state.is_trait_disabled()
    }
}

impl IResolveOrder for GrantConditionOnDeploy {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if self.state.is_trait_disabled() || self.state.is_trait_paused() || order.order_string != DEPLOY_ORDER {
            return;
        }
        let activity = DeployForGrantedCondition::new(self_, self.me.upgrade().expect("trait is alive"), false);
        self_.queue_activity_queued(order.queued, activity);
    }
}

impl IOrderVoice for GrantConditionOnDeploy {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == DEPLOY_ORDER).then(|| self.info.voice.clone())
    }
}

/// `DeployForGrantedCondition`.
pub struct DeployForGrantedCondition {
    base: ActivityBase,
    deploy: Rc<GrantConditionOnDeploy>,
    can_turn: bool,
    moving: bool,
}

impl DeployForGrantedCondition {
    pub fn new(self_: &ActorRef, deploy: Rc<GrantConditionOnDeploy>, moving: bool) -> ActivityRef {
        Rc::new(DeployForGrantedCondition {
            base: ActivityBase::default(),
            deploy,
            can_turn: self_.trait_or_default::<dyn IFacing>().is_some(),
            moving,
        })
    }
}

impl Activity for DeployForGrantedCondition {
    activity_common!("DeployForGrantedCondition");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        self.base.next_activity().map_or_else(Vec::new, |n| n.target_line_nodes(self_))
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if self.deploy.deploy_state() == DeployState::Undeployed
            && self.can_turn
            && !self.moving
            && let Some(f) = self.deploy.info.facing
        {
            self.base.queue_child(Turn::new(self_, f));
        }
    }

    fn tick(&self, _self_: &ActorRef) -> bool {
        if self.base.is_canceling() || (self.deploy.deploy_state() != DeployState::Deployed && self.moving) {
            return true;
        }
        self.base.queue_child(DeployInner::new(self.deploy.clone()));
        true
    }
}

/// `DeployInner`.
pub struct DeployInner {
    base: ActivityBase,
    deployment: Rc<GrantConditionOnDeploy>,
    initiated: Cell<bool>,
}

impl DeployInner {
    fn new(deployment: Rc<GrantConditionOnDeploy>) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        Rc::new(DeployInner {
            base,
            deployment,
            initiated: Cell::new(false),
        })
    }
}

impl Activity for DeployInner {
    activity_common!("DeployInner");

    fn tick(&self, _self_: &ActorRef) -> bool {
        let state = self.deployment.deploy_state();
        if state == DeployState::Deploying || state == DeployState::Undeploying {
            return false;
        }
        if self.initiated.get() {
            return true;
        }
        if state == DeployState::Undeployed {
            self.deployment.deploy();
        } else {
            self.deployment.undeploy();
        }
        self.initiated.set(true);
        false
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct KillsSelfInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub remove_instead: bool,
    pub delay: Vec<i32>,
    pub damage_types: BitSet<DamageTypeTag>,
    pub grants_condition: Option<String>,
}

impl Default for KillsSelfInfo {
    fn default() -> Self {
        KillsSelfInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            remove_instead: false,
            delay: vec![0],
            damage_types: BitSet::default(),
            grants_condition: None,
        }
    }
}

pub struct KillsSelf {
    info: Rc<KillsSelfInfo>,
    state: ConditionalState,
    lifetime: Cell<i32>,
}

register_traits!(KillsSelf: INotifyAddedToWorld, ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(KillsSelfInfo => |info, init| {
    let lifetime = random_in_range(&mut init.self_.world().shared_random.borrow_mut(), &info.delay);
    Some(Rc::new(KillsSelf { state: ConditionalState::new(&info.base), info, lifetime: Cell::new(lifetime) }))
});

impl KillsSelf {
    fn schedule_kill(&self, self_: &ActorRef) {
        let info = self.info.clone();
        let a = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |_| {
            if a.is_dead() {
                return;
            }
            if let Some(c) = &info.grants_condition {
                a.grant_condition(c);
            }
            match a.trait_or_default::<dyn IHealth>() {
                Some(h) if !info.remove_instead => h.kill(&a, Some(&a), &info.damage_types),
                _ => a.dispose(),
            }
        }));
    }
}

impl ConditionalHooks for KillsSelf {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        if self.lifetime.get() == 0 && self_.is_in_world() {
            self.schedule_kill(self_);
        }
    }
}

conditional_plumbing!(KillsSelf);

impl INotifyCreated for KillsSelf {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifyAddedToWorld for KillsSelf {
    fn added_to_world(&self, self_: &ActorRef) {
        if !self.state.is_trait_disabled() {
            self.trait_enabled(self_);
        }
    }
}

impl ITick for KillsSelf {
    fn tick(&self, self_: &ActorRef) {
        if !self_.is_in_world() || self_.is_dead() || self.state.is_trait_disabled() {
            return;
        }
        if !self_.world().map().contains(self_.location()) {
            return;
        }
        let lifetime = self.lifetime.get();
        self.lifetime.set(lifetime - 1);
        if lifetime <= 0 {
            self.schedule_kill(self_);
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ProximityExternalConditionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub condition: String,
    pub range: WDist,
    pub maximum_vertical_offset: WDist,
    pub valid_relationships: PlayerRelationship,
    pub affects_parent: bool,
    pub enable_sound: Option<String>,
    pub disable_sound: Option<String>,
}

impl Default for ProximityExternalConditionInfo {
    fn default() -> Self {
        ProximityExternalConditionInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            condition: String::new(),
            range: WDist::from_cells(3),
            maximum_vertical_offset: WDist::ZERO,
            valid_relationships: PlayerRelationship::ALLY,
            affects_parent: false,
            enable_sound: None,
            disable_sound: None,
        }
    }
}

pub struct ProximityExternalCondition {
    info: Rc<ProximityExternalConditionInfo>,
    state: ConditionalState,
    self_: ActorRef,
    me: Weak<ProximityExternalCondition>,
    tokens: RefCell<IndexMap<u32, i32>>,
    trigger: Cell<i32>,
    cached_position: Cell<WPos>,
    cached_range: Cell<WDist>,
    desired_range: Cell<WDist>,
    cached_v_range: Cell<WDist>,
    desired_v_range: Cell<WDist>,
}

register_traits!(ProximityExternalCondition: ITick, INotifyAddedToWorld, INotifyRemovedFromWorld, INotifyOtherProduction, INotifyProximityOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ProximityExternalConditionInfo => |info, init| Some(Rc::new_cyclic(|me| ProximityExternalCondition {
    state: ConditionalState::new(&info.base),
    info,
    self_: init.self_.clone(),
    me: me.clone(),
    tokens: RefCell::new(IndexMap::new()),
    trigger: Cell::new(0),
    cached_position: Cell::new(WPos::ZERO),
    cached_range: Cell::new(WDist::ZERO),
    desired_range: Cell::new(WDist::ZERO),
    cached_v_range: Cell::new(WDist::ZERO),
    desired_v_range: Cell::new(WDist::ZERO),
})));

impl ProximityExternalCondition {
    fn try_grant(&self, a: &ActorRef) {
        let source = self.self_.id as usize;
        let external = a
            .traits_implementing::<ExternalCondition>()
            .into_iter()
            .find(|t| t.info.condition == self.info.condition && t.can_grant_condition(source));
        if let Some(e) = external {
            let token = e.grant_condition(a, source, 0, 0);
            self.tokens.borrow_mut().insert(a.id, token);
        }
    }

    fn try_revoke(&self, a: &ActorRef, token: i32) {
        let source = self.self_.id as usize;
        for e in a.traits_implementing::<ExternalCondition>() {
            if e.try_revoke_condition(a, source, token) {
                break;
            }
        }
    }

    fn actor_entered(&self, a: &ActorRef) {
        if a.disposed() || self.self_.disposed() {
            return;
        }
        if (a.id == self.self_.id && !self.info.affects_parent) || self.tokens.borrow().contains_key(&a.id) {
            return;
        }
        let relationship = self.self_.owner().relationship_with(Some(&a.owner()));
        if self.info.valid_relationships.has_relationship(relationship) {
            self.try_grant(a);
        }
    }

    fn actor_exited(&self, a: &ActorRef) {
        if a.disposed() {
            return;
        }
        let token = self.tokens.borrow_mut().shift_remove(&a.id);
        if let Some(token) = token {
            self.try_revoke(a, token);
        }
    }

    fn in_range(&self, self_: &ActorRef, a: &ActorRef) -> bool {
        (a.center_position() - self_.center_position()).horizontal_length_squared() <= self.info.range.length_squared()
    }
}

impl ConditionalHooks for ProximityExternalCondition {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        if let Some(s) = &self.info.enable_sound {
            sound::play_at(&self_.world(), SoundType::World, s, self_.center_position(), 1.0);
        }
        self.desired_range.set(self.info.range);
        self.desired_v_range.set(self.info.maximum_vertical_offset);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        if let Some(s) = &self.info.disable_sound {
            sound::play_at(&self_.world(), SoundType::World, s, self_.center_position(), 1.0);
        }
        self.desired_range.set(WDist::ZERO);
        self.desired_v_range.set(WDist::ZERO);
    }
}

conditional_plumbing!(ProximityExternalCondition);

impl INotifyCreated for ProximityExternalCondition {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl INotifyAddedToWorld for ProximityExternalCondition {
    fn added_to_world(&self, self_: &ActorRef) {
        self.cached_position.set(self_.center_position());
        let entered = self.me.clone();
        let exited = self.me.clone();
        let id = self_.world().actor_map().add_proximity_trigger(
            self.cached_position.get(),
            self.cached_range.get(),
            self.cached_v_range.get(),
            Some(Rc::new(move |a: &ActorRef| {
                if let Some(t) = entered.upgrade() {
                    t.actor_entered(a);
                }
            })),
            Some(Rc::new(move |a: &ActorRef| {
                if let Some(t) = exited.upgrade() {
                    t.actor_exited(a);
                }
            })),
        );
        self.trigger.set(id);
    }
}

impl INotifyRemovedFromWorld for ProximityExternalCondition {
    fn removed_from_world(&self, self_: &ActorRef) {
        self_.world().actor_map().remove_proximity_trigger(self.trigger.get());
    }
}

impl ITick for ProximityExternalCondition {
    fn tick(&self, self_: &ActorRef) {
        let pos = self_.center_position();
        if pos != self.cached_position.get()
            || self.desired_range.get() != self.cached_range.get()
            || self.desired_v_range.get() != self.cached_v_range.get()
        {
            self.cached_position.set(pos);
            self.cached_range.set(self.desired_range.get());
            self.cached_v_range.set(self.desired_v_range.get());
            self_
                .world()
                .actor_map()
                .update_proximity_trigger(self.trigger.get(), pos, self.cached_range.get(), self.cached_v_range.get());
        }
    }
}

impl INotifyOtherProduction for ProximityExternalCondition {
    fn unit_produced_by_other(
        &self,
        self_: &ActorRef,
        _producer: &ActorRef,
        produced: &ActorRef,
        _production_type: &str,
        _init: &ActorInits,
    ) {
        if produced.cache().occupies_space.is_none() || self.state.is_trait_disabled() || !self.in_range(self_, produced) {
            return;
        }
        let relationship = self_.owner().relationship_with(Some(&produced.owner()));
        if self.info.valid_relationships.has_relationship(relationship) {
            self.try_grant(produced);
        }
    }
}

impl INotifyProximityOwnerChanged for ProximityExternalCondition {
    fn on_proximity_owner_changed(&self, self_: &ActorRef, actor: &ActorRef, _old_owner: &PlayerRef, _new_owner: &PlayerRef) {
        if actor.cache().occupies_space.is_none() || self.state.is_trait_disabled() || !self.in_range(self_, actor) {
            return;
        }
        let relationship = self_.owner().relationship_with(Some(&actor.owner()));
        let has_relationship = self.info.valid_relationships.has_relationship(relationship);
        let token = self.tokens.borrow().get(&actor.id).copied();
        match token {
            None if has_relationship => self.try_grant(actor),
            Some(token) if !has_relationship => {
                self.tokens.borrow_mut().shift_remove(&actor.id);
                self.try_revoke(actor, token);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantExternalConditionToProducedInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub condition: String,
    pub duration: i32,
}

pub struct GrantExternalConditionToProduced {
    info: Rc<GrantExternalConditionToProducedInfo>,
    state: ConditionalState,
}

register_traits!(GrantExternalConditionToProduced: INotifyProduction, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GrantExternalConditionToProducedInfo => |info, _init| Some(Rc::new(GrantExternalConditionToProduced { state: ConditionalState::new(&info.base), info })));
conditional_simple!(GrantExternalConditionToProduced);

impl INotifyProduction for GrantExternalConditionToProduced {
    fn unit_produced(&self, self_: &ActorRef, other: &ActorRef, _exit: CPos) {
        if self.state.is_trait_disabled() || other.is_dead() {
            return;
        }
        let external = other
            .traits_implementing::<ExternalCondition>()
            .into_iter()
            .find(|t| t.info.condition == self.info.condition && t.can_grant_condition(other.id as usize));
        if let Some(e) = external {
            e.grant_condition(other, self_.id as usize, self.info.duration, 0);
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantConditionOnCombatantOwnerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
}

pub struct GrantConditionOnCombatantOwner {
    info: Rc<GrantConditionOnCombatantOwnerInfo>,
    token: Cell<i32>,
}

register_traits!(GrantConditionOnCombatantOwner: INotifyCreated, INotifyOwnerChanged);
info_impl!(GrantConditionOnCombatantOwnerInfo => |info, _init| Some(Rc::new(GrantConditionOnCombatantOwner { info, token: Cell::new(INVALID_CONDITION_TOKEN) })));

impl INotifyCreated for GrantConditionOnCombatantOwner {
    fn created(&self, self_: &ActorRef) {
        if !self_.owner().non_combatant {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }
}

impl INotifyOwnerChanged for GrantConditionOnCombatantOwner {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
        if !new_owner.non_combatant {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PowerMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub modifier: i32,
}

pub struct PowerMultiplier {
    info: Rc<PowerMultiplierInfo>,
    state: ConditionalState,
    power: RefCell<Rc<PowerManager>>,
}

register_traits!(PowerMultiplier: IPowerModifier, INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(PowerMultiplierInfo => |info, init| Some(Rc::new(PowerMultiplier {
    state: ConditionalState::new(&info.base),
    info,
    power: RefCell::new(init.self_.owner().player_actor().trait_::<PowerManager>()),
})));

impl ConditionalHooks for PowerMultiplier {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_enabled(&self, self_: &ActorRef) {
        self.power.borrow().clone().update_actor(self_);
    }
    fn trait_disabled(&self, self_: &ActorRef) {
        self.power.borrow().clone().update_actor(self_);
    }
}

conditional_plumbing!(PowerMultiplier);

impl INotifyCreated for PowerMultiplier {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IPowerModifier for PowerMultiplier {
    fn get_power_modifier(&self) -> i32 {
        if self.state.is_trait_disabled() { 100 } else { self.info.modifier }
    }
}

impl INotifyOwnerChanged for PowerMultiplier {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        *self.power.borrow_mut() = new_owner.player_actor().trait_::<PowerManager>();
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct HandicapFirepowerMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct HandicapFirepowerMultiplier {
    self_: ActorRef,
}

register_traits!(HandicapFirepowerMultiplier: IFirepowerModifier);
info_impl!(HandicapFirepowerMultiplierInfo => |_info, init| Some(Rc::new(HandicapFirepowerMultiplier { self_: init.self_.clone() })));

impl IFirepowerModifier for HandicapFirepowerMultiplier {
    fn get_firepower_modifier(&self) -> i32 {
        100 - self.self_.owner().handicap
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct HandicapDamageMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct HandicapDamageMultiplier {
    self_: ActorRef,
}

register_traits!(HandicapDamageMultiplier: IDamageModifier);
info_impl!(HandicapDamageMultiplierInfo => |_info, init| Some(Rc::new(HandicapDamageMultiplier { self_: init.self_.clone() })));

impl IDamageModifier for HandicapDamageMultiplier {
    fn get_damage_modifier(&self, _attacker: Option<&ActorRef>, _damage: &Damage) -> i32 {
        10000 / (100 - self.self_.owner().handicap)
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct HandicapProductionTimeMultiplierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

info_impl!(HandicapProductionTimeMultiplierInfo => |_info, _init| None, [IProductionTimeModifierInfo]);

impl IProductionTimeModifierInfo for HandicapProductionTimeMultiplierInfo {
    fn get_production_time_modifier(&self, tech_tree: &TechTree, _queue: &str) -> i32 {
        100 + tech_tree.owner().handicap
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GivesBountyInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub percentage: i32,
    pub valid_relationships: PlayerRelationship,
    pub show_bounty: bool,
    pub death_types: BitSet<DamageTypeTag>,
}

impl Default for GivesBountyInfo {
    fn default() -> Self {
        GivesBountyInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            percentage: 10,
            valid_relationships: PlayerRelationship(PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0),
            show_bounty: true,
            death_types: BitSet::default(),
        }
    }
}

pub struct GivesBounty {
    info: Rc<GivesBountyInfo>,
    state: ConditionalState,
}

register_traits!(GivesBounty: INotifyKilled, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GivesBountyInfo => |info, _init| Some(Rc::new(GivesBounty { state: ConditionalState::new(&info.base), info })));
conditional_simple!(GivesBounty);

impl INotifyKilled for GivesBounty {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        let Some(attacker) = e.attacker.as_ref().filter(|a| !a.disposed()) else {
            return;
        };
        if self.state.is_trait_disabled() {
            return;
        }
        let relationship = attacker.owner().relationship_with(Some(&self_.owner()));
        if !self.info.valid_relationships.has_relationship(relationship) {
            return;
        }
        if !self.info.death_types.is_empty() && !e.damage.damage_types.overlaps(&self.info.death_types) {
            return;
        }
        let bounty = get_sell_value(self_) * self.info.percentage / 100;
        attacker.owner().player_actor().trait_::<PlayerResources>().change_cash(bounty);
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SpawnActorsOnSellInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub value_percent: i32,
    pub min_hp_percent: i32,
    #[field(require)]
    pub actor_types: Vec<String>,
    pub guaranteed_actor_types: Vec<String>,
    pub factions: IndexSet<String>,
}

impl Default for SpawnActorsOnSellInfo {
    fn default() -> Self {
        SpawnActorsOnSellInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            value_percent: 40,
            min_hp_percent: 30,
            actor_types: Vec::new(),
            guaranteed_actor_types: Vec::new(),
            factions: IndexSet::new(),
        }
    }
}

pub struct SpawnActorsOnSell {
    info: Rc<SpawnActorsOnSellInfo>,
    state: ConditionalState,
    correct_faction: bool,
}

register_traits!(SpawnActorsOnSell: INotifySold, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SpawnActorsOnSellInfo => |info, init| {
    let correct_faction = info.factions.is_empty() || info.factions.contains(&init.self_.owner().faction);
    Some(Rc::new(SpawnActorsOnSell { state: ConditionalState::new(&info.base), info, correct_faction }))
});
conditional_simple!(SpawnActorsOnSell);

impl INotifySold for SpawnActorsOnSell {
    fn selling(&self, _self_: &ActorRef) {}

    fn sold(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() || !self.correct_faction {
            return;
        }
        let Some(building) = self_.info.trait_info::<BuildingInfo>() else {
            return;
        };
        let world = self_.world();
        let rules = world.rules().clone();
        let actor_cost = |name: &String| {
            let cost = rules.actor(name).and_then(|a| a.trait_info::<ValuedInfo>()).map_or(0, |v| v.cost);
            (name.clone(), cost)
        };
        let cost = match self_.info.trait_info::<CustomSellValueInfo>() {
            Some(c) => c.value,
            None => self_.info.trait_info::<ValuedInfo>().map_or(0, |v| v.cost),
        };
        let mut dudes_value = self.info.value_percent * cost / 100;
        if let Some(h) = self_.trait_or_default::<dyn IHealth>() {
            if 100i64 * h.hp() as i64 >= self.info.min_hp_percent as i64 * h.max_hp() as i64 {
                dudes_value = (h.hp() as i64 * dudes_value as i64 / h.max_hp() as i64) as i32;
            } else {
                dudes_value = 0;
            }
        }
        let mut eligible = building.tiles(self_.location());
        if eligible.is_empty() {
            return;
        }
        let owner = self_.owner();
        let spawn = |name: String, loc: CPos| {
            let owner = owner.clone();
            world.add_frame_end_task(Box::new(move |w| {
                let mut td = ActorInits::new();
                td.push(Rc::new(LocationInit::new(loc)));
                td.push(Rc::new(OwnerInit::from_player(&owner)));
                w.create_actor(&name, td);
            }));
        };
        if !self.info.guaranteed_actor_types.is_empty() {
            let mut guaranteed: Vec<(String, i32)> = self.info.guaranteed_actor_types.iter().map(actor_cost).collect();
            while !eligible.is_empty() && !guaranteed.is_empty() {
                let mut random = world.shared_random.borrow_mut();
                let at = guaranteed.remove(random.next_max(guaranteed.len() as i32) as usize);
                let loc = eligible.remove(random.next_max(eligible.len() as i32) as usize);
                drop(random);
                dudes_value -= at.1;
                spawn(at.0, loc);
            }
            if eligible.is_empty() {
                return;
            }
        }
        let actor_types: Vec<(String, i32)> = self.info.actor_types.iter().map(actor_cost).collect();
        loop {
            let affordable: Vec<&(String, i32)> = actor_types.iter().filter(|a| a.1 <= dudes_value).collect();
            if eligible.is_empty() || affordable.is_empty() {
                break;
            }
            let mut random = world.shared_random.borrow_mut();
            let at = affordable[random.next_max(affordable.len() as i32) as usize];
            let loc = eligible.remove(random.next_max(eligible.len() as i32) as usize);
            drop(random);
            dudes_value -= at.1;
            spawn(at.0.clone(), loc);
        }
    }
}
