// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `IIssueOrder`, `IOrderTargeter` and `TargetModifiers` of `TraitsInterfaces.cs`. Targeters run only on the
//! client that gives the order, so their state may change freely; the resulting `Order` is what gets synced.

use std::any::Any;
use std::rc::Rc;

use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::bitset::BitSet;
use rustora_engine::frozen::FrozenActorRef;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("CommandBarBlacklist", |n| load_info!(CommandBarBlacklistInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CommandBarBlacklistInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub disable_stop: bool,
    pub disable_waypoint_mode: bool,
}

impl Default for CommandBarBlacklistInfo {
    fn default() -> Self {
        CommandBarBlacklistInfo {
            instance_name: None,
            disable_stop: true,
            disable_waypoint_mode: true,
        }
    }
}

pub struct CommandBarBlacklist;

register_traits!(CommandBarBlacklist);
info_impl!(CommandBarBlacklistInfo => |_info, _init| Some(Rc::new(CommandBarBlacklist)));

/// `TargetModifiers` flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetModifiers(pub u8);

impl TargetModifiers {
    pub const NONE: TargetModifiers = TargetModifiers(0);
    pub const FORCE_ATTACK: TargetModifiers = TargetModifiers(1);
    pub const FORCE_QUEUE: TargetModifiers = TargetModifiers(2);
    pub const FORCE_MOVE: TargetModifiers = TargetModifiers(4);

    pub fn has_modifier(self, m: TargetModifiers) -> bool {
        self.0 & m.0 == m.0
    }

    pub fn with(self, m: TargetModifiers, on: bool) -> TargetModifiers {
        if on { TargetModifiers(self.0 | m.0) } else { self }
    }

    pub fn without(self, m: TargetModifiers) -> TargetModifiers {
        TargetModifiers(self.0 & !m.0)
    }
}

pub trait IOrderTargeter: Any {
    fn order_id(&self) -> &str;
    fn order_priority(&self) -> i32;
    fn can_target(&self, self_: &ActorRef, target: &Target, modifiers: &mut TargetModifiers, cursor: &mut Option<String>) -> bool;
    fn is_queued(&self) -> bool;
    fn target_overrides_selection(
        &self,
        self_: &ActorRef,
        target: &Target,
        actors_at: &[ActorRef],
        xy: CPos,
        modifiers: TargetModifiers,
    ) -> bool;
    fn as_any(&self) -> &dyn Any;
}

pub trait IIssueOrder {
    fn orders(&self, self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>>;
    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order>;
}

pub trait IIssueDeployOrder {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order>;
    fn can_issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> bool;
}

/// `DeployOrderTargeter`: targets the actor itself.
pub struct DeployOrderTargeter {
    pub order_id: String,
    pub order_priority: i32,
    cursor: Box<dyn Fn() -> String>,
    is_queued: std::cell::Cell<bool>,
}

impl DeployOrderTargeter {
    pub fn new(order_id: &str, order_priority: i32, cursor: Box<dyn Fn() -> String>) -> DeployOrderTargeter {
        DeployOrderTargeter {
            order_id: order_id.into(),
            order_priority,
            cursor,
            is_queued: std::cell::Cell::new(false),
        }
    }
}

impl IOrderTargeter for DeployOrderTargeter {
    fn order_id(&self) -> &str {
        &self.order_id
    }

    fn order_priority(&self) -> i32 {
        self.order_priority
    }

    fn can_target(&self, self_: &ActorRef, target: &Target, modifiers: &mut TargetModifiers, cursor: &mut Option<String>) -> bool {
        let Some(a) = target.actor().filter(|_| target.target_type() == TargetType::Actor) else {
            return false;
        };
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        *cursor = Some((self.cursor)());
        std::rc::Rc::ptr_eq(self_, a)
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(&self, _: &ActorRef, _: &Target, _: &[ActorRef], _: CPos, _: TargetModifiers) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `CanTargetActor` and `CanTargetFrozenActor` of a [`UnitOrderTargeter`]: they get the ordered actor, the target and
/// the modifiers, and may replace the cursor.
pub type CanTargetActor = Box<dyn Fn(&ActorRef, &ActorRef, TargetModifiers, &mut Option<String>) -> bool>;
pub type CanTargetFrozenActor = Box<dyn Fn(&ActorRef, &FrozenActorRef, TargetModifiers, &mut Option<String>) -> bool>;

/// `UnitOrderTargeter` and its subclasses: targets actors and frozen actors by relationship.
pub struct UnitOrderTargeter {
    pub order_id: String,
    pub order_priority: i32,
    cursor: String,
    target_enemy_units: bool,
    target_ally_units: bool,
    pub force_attack: Option<bool>,
    can_target_actor: CanTargetActor,
    can_target_frozen_actor: CanTargetFrozenActor,
    is_queued: std::cell::Cell<bool>,
}

impl UnitOrderTargeter {
    pub fn new(
        order_id: &str,
        order_priority: i32,
        cursor: &str,
        target_enemy_units: bool,
        target_ally_units: bool,
        can_target_actor: CanTargetActor,
        can_target_frozen_actor: CanTargetFrozenActor,
    ) -> UnitOrderTargeter {
        UnitOrderTargeter {
            order_id: order_id.into(),
            order_priority,
            cursor: cursor.into(),
            target_enemy_units,
            target_ally_units,
            force_attack: None,
            can_target_actor,
            can_target_frozen_actor,
            is_queued: std::cell::Cell::new(false),
        }
    }

    /// `TargetTypeOrderTargeter`.
    pub fn target_types(
        target_types: BitSet<TargetableType>,
        order_id: &str,
        order_priority: i32,
        cursor: &str,
        target_enemy_units: bool,
        target_ally_units: bool,
    ) -> UnitOrderTargeter {
        let frozen_types = target_types;
        UnitOrderTargeter::new(
            order_id,
            order_priority,
            cursor,
            target_enemy_units,
            target_ally_units,
            Box::new(move |_, t, _, _| target_types.overlaps(&rustora_engine::actor_ext::enabled_target_types(t))),
            Box::new(move |_, t, _, _| t.target_types().overlaps(&frozen_types)),
        )
    }
}

impl UnitOrderTargeter {
    /// `EnterAlliedActorTargeter<T>`, where `has_info` checks for the `T` trait info on the target.
    pub fn enter_allied(
        order_id: &str,
        order_priority: i32,
        enter_cursor: &str,
        enter_blocked_cursor: &str,
        has_info: fn(&ActorRef) -> bool,
        can_target: Box<dyn Fn(&ActorRef, TargetModifiers) -> bool>,
        use_enter_cursor: Box<dyn Fn(&ActorRef) -> bool>,
    ) -> UnitOrderTargeter {
        let (enter, blocked) = (enter_cursor.to_string(), enter_blocked_cursor.to_string());
        UnitOrderTargeter::new(
            order_id,
            order_priority,
            enter_cursor,
            false,
            true,
            Box::new(move |self_, target, modifiers, cursor| {
                if !self_.owner().is_allied_with(Some(&target.owner())) || !has_info(target) || !can_target(target, modifiers) {
                    return false;
                }
                *cursor = Some(if use_enter_cursor(target) { enter.clone() } else { blocked.clone() });
                true
            }),
            Box::new(|_, _, _, _| false),
        )
    }
}

impl IOrderTargeter for UnitOrderTargeter {
    fn order_id(&self) -> &str {
        &self.order_id
    }

    fn order_priority(&self) -> i32 {
        self.order_priority
    }

    fn can_target(&self, self_: &ActorRef, target: &Target, modifiers: &mut TargetModifiers, cursor: &mut Option<String>) -> bool {
        let ty = target.target_type();
        if ty != TargetType::Actor && ty != TargetType::FrozenActor {
            return false;
        }
        *cursor = Some(self.cursor.clone());
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        let force_attack = modifiers.has_modifier(TargetModifiers::FORCE_ATTACK);
        if self.force_attack.is_some_and(|f| f != force_attack) {
            return false;
        }
        let owner = match (target.frozen_actor(), target.actor()) {
            (Some(fa), _) if ty == TargetType::FrozenActor => fa.owner(),
            (_, Some(a)) => Some(a.owner()),
            _ => None,
        };
        let relationship = self_.owner().relationship_with(owner.as_deref());
        if !force_attack && relationship == PlayerRelationship::ALLY && !self.target_ally_units {
            return false;
        }
        if !force_attack && relationship == PlayerRelationship::ENEMY && !self.target_enemy_units {
            return false;
        }
        match (target.frozen_actor(), target.actor()) {
            (Some(fa), _) if ty == TargetType::FrozenActor => (self.can_target_frozen_actor)(self_, fa, *modifiers, cursor),
            (_, Some(a)) => (self.can_target_actor)(self_, a, *modifiers, cursor),
            _ => false,
        }
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(&self, _: &ActorRef, _: &Target, _: &[ActorRef], _: CPos, _: TargetModifiers) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
