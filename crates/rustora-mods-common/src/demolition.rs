// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Demolition`, `Demolishable` and `Demolish`.
//!
//! Not ported: the white `FlashTarget` on the target.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageTypeTag};
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::IOrderVoice;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::{conditional_plumbing, conditional_simple, info_impl, load_info, register_traits};

use crate::cargo::{Enter, EnterHooks};
use crate::cash_repair::EnterBehaviour;
use crate::interfaces::*;
use crate::orders::{IIssueOrder, IOrderTargeter, TargetModifiers, UnitOrderTargeter};
use crate::util::apply_percentage_modifiers;

pub fn register(r: &mut TraitRegistry) {
    r.register("Demolition", |n| load_info!(DemolitionInfo, n));
    r.register("Demolishable", |n| load_info!(DemolishableInfo, n));
}

pub trait IDemolishable {
    fn is_valid_target(&self, self_: &ActorRef, saboteur: &ActorRef) -> bool;
    fn demolish(&self, self_: &ActorRef, saboteur: &ActorRef, delay: i32, damage_types: BitSet<DamageTypeTag>);
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DemolitionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub detonation_delay: i32,
    pub flashes: i32,
    pub flashes_delay: i32,
    pub flash_interval: i32,
    pub enter_behaviour: EnterBehaviour,
    pub damage_types: BitSet<DamageTypeTag>,
    pub voice: String,
    pub target_relationships: PlayerRelationship,
    pub force_target_relationships: PlayerRelationship,
    pub cursor: String,
}

impl Default for DemolitionInfo {
    fn default() -> Self {
        let (a, n, e) = (
            PlayerRelationship::ALLY.0,
            PlayerRelationship::NEUTRAL.0,
            PlayerRelationship::ENEMY.0,
        );
        DemolitionInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            detonation_delay: 45,
            flashes: 3,
            flashes_delay: 4,
            flash_interval: 4,
            enter_behaviour: EnterBehaviour::Exit,
            damage_types: BitSet::default(),
            voice: "Action".into(),
            target_relationships: PlayerRelationship(e | n),
            force_target_relationships: PlayerRelationship(e | n | a),
            cursor: "c4".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Demolition {
    info: Rc<DemolitionInfo>,
    state: ConditionalState,
}

register_traits!(Demolition: IIssueOrder, IResolveOrder, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(DemolitionInfo => |info, _init| Some(Rc::new(Demolition { state: ConditionalState::new(&info.base), info })));
conditional_simple!(Demolition);

fn relationship_allowed(info: &DemolitionInfo, relationship: PlayerRelationship, modifiers: TargetModifiers) -> bool {
    let force = modifiers.has_modifier(TargetModifiers::FORCE_ATTACK);
    if !info.target_relationships.has_relationship(relationship) && !force {
        return false;
    }
    info.force_target_relationships.has_relationship(relationship) || !force
}

fn demolishable_info_valid(info: &ActorInfo) -> bool {
    info.has_trait_info::<DemolishableInfo>()
}

impl IIssueOrder for Demolition {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let (a, f) = (self.info.clone(), self.info.clone());
        vec![Box::new(UnitOrderTargeter::new(
            "C4",
            6,
            &self.info.cursor,
            true,
            true,
            Box::new(move |self_, target, modifiers, _| {
                if modifiers.has_modifier(TargetModifiers::FORCE_MOVE) {
                    return false;
                }
                relationship_allowed(&a, target.owner().relationship_with(Some(&self_.owner())), modifiers)
                    && target
                        .traits_implementing::<dyn IDemolishable>()
                        .iter()
                        .any(|d| d.is_valid_target(target, self_))
            }),
            Box::new(move |self_, target, modifiers, _| {
                target
                    .owner()
                    .is_some_and(|o| relationship_allowed(&f, o.relationship_with(Some(&self_.owner())), modifiers))
                    && demolishable_info_valid(target.info())
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "C4" && !self.state.is_trait_disabled()).then(|| Order::new("C4", Some(self_.clone()), target.clone(), queued))
    }
}

impl IResolveOrder for Demolition {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "C4" || self.state.is_trait_disabled() {
            return;
        }
        if order.target.target_type() == TargetType::Actor {
            let target = order.target.actor().expect("type was checked");
            if !target
                .traits_implementing::<dyn IDemolishable>()
                .iter()
                .any(|d| d.is_valid_target(target, self_))
            {
                return;
            }
        }
        let hooks = DemolishHooks {
            info: self.info.clone(),
            notifiers: self_.traits_implementing::<dyn INotifyDemolition>(),
            enter: Rc::new(RefCell::new(None)),
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

impl IOrderVoice for Demolition {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (!self.state.is_trait_disabled() && order.order_string == "C4").then(|| self.info.voice.clone())
    }
}

/// The actor being entered and its `IDemolishable` traits.
type EnterTarget = (ActorRef, Vec<Rc<dyn IDemolishable>>);

/// `Demolish`.
impl Demolition {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    /// `GetDemolishActivity(self, target, null)`.
    pub fn demolish_activity(&self, self_: &ActorRef, target: &Target) -> rustora_engine::activity::ActivityRef {
        let hooks = DemolishHooks {
            info: self.info.clone(),
            notifiers: self_.traits_implementing::<dyn INotifyDemolition>(),
            enter: Rc::new(RefCell::new(None)),
        };
        Enter::new(self_, target, Box::new(hooks))
    }
}

#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
struct DemolishHooks {
    info: Rc<DemolitionInfo>,
    notifiers: Vec<Rc<dyn INotifyDemolition>>,
    enter: Rc<RefCell<Option<EnterTarget>>>,
}

impl EnterHooks for DemolishHooks {
    fn type_name(&self) -> &'static str {
        "Demolish"
    }

    fn try_start_enter(&self, enter: &Enter, self_: &ActorRef, target_actor: &ActorRef) -> bool {
        let demolishables = target_actor.traits_implementing::<dyn IDemolishable>();
        let valid = demolishables.iter().any(|d| d.is_valid_target(target_actor, self_));
        *self.enter.borrow_mut() = Some((target_actor.clone(), demolishables));
        if !valid {
            enter.cancel_keep_queue(self_);
        }
        valid
    }

    fn on_enter_complete(&self, self_: &ActorRef, target_actor: &ActorRef) {
        let (enter, info, notifiers) = (self.enter.clone(), self.info.clone(), self.notifiers.clone());
        let (saboteur, target_id) = (self_.clone(), target_actor.id);
        self_.world().add_frame_end_task(Box::new(move |_| {
            let Some((enter_actor, demolishables)) = enter.borrow().clone().filter(|(a, _)| a.id == target_id) else {
                return;
            };
            if !demolishables.iter().any(|d| d.is_valid_target(&enter_actor, &saboteur)) {
                return;
            }
            for n in &notifiers {
                n.demolishing(&saboteur);
            }
            for d in &demolishables {
                d.demolish(&enter_actor, &saboteur, info.detonation_delay, info.damage_types);
            }
            match info.enter_behaviour {
                EnterBehaviour::Dispose => saboteur.dispose(),
                EnterBehaviour::Suicide => {
                    if let Some(h) = saboteur.trait_or_default::<dyn IHealth>() {
                        h.kill(&saboteur, Some(&saboteur), &Default::default());
                    }
                }
                EnterBehaviour::Exit => {}
            }
        }));
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct DemolishableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub condition: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
#[persist(shared)]
struct DemolishAction {
    saboteur: ActorRef,
    token: i32,
    delay: Cell<i32>,
    damage_types: BitSet<DamageTypeTag>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct Demolishable {
    info: Rc<DemolishableInfo>,
    state: ConditionalState,
    actions: RefCell<Vec<Rc<DemolishAction>>>,
    damage_modifiers: RefCell<Vec<Rc<dyn IDamageModifier>>>,
}

register_traits!(Demolishable: IDemolishable, ITick, INotifyOwnerChanged, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(DemolishableInfo => |info, _init| Some(Rc::new(Demolishable {
    state: ConditionalState::new(&info.base),
    info,
    actions: RefCell::new(Vec::new()),
    damage_modifiers: RefCell::new(Vec::new()),
})));

impl ConditionalHooks for Demolishable {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_plumbing!(Demolishable);

impl Demolishable {
    fn refresh_modifiers(&self, self_: &ActorRef, owner: &PlayerRef) {
        let mut modifiers = self_.traits_implementing::<dyn IDamageModifier>();
        modifiers.extend(owner.player_actor().traits_implementing::<dyn IDamageModifier>());
        *self.damage_modifiers.borrow_mut() = modifiers;
    }
}

impl INotifyCreated for Demolishable {
    /// `Created` of C# does not call the base method, so conditions are only observed through `IObservesVariables`.
    fn created(&self, self_: &ActorRef) {
        self.refresh_modifiers(self_, &self_.owner());
    }
}

impl INotifyOwnerChanged for Demolishable {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        self.refresh_modifiers(self_, new_owner);
    }
}

impl IDemolishable for Demolishable {
    fn is_valid_target(&self, _self_: &ActorRef, _saboteur: &ActorRef) -> bool {
        !self.state.is_trait_disabled()
    }

    fn demolish(&self, self_: &ActorRef, saboteur: &ActorRef, delay: i32, damage_types: BitSet<DamageTypeTag>) {
        if self.state.is_trait_disabled() {
            return;
        }
        let token = match &self.info.condition {
            Some(c) => self_.grant_condition(c),
            None => INVALID_CONDITION_TOKEN,
        };
        self.actions.borrow_mut().push(Rc::new(DemolishAction {
            saboteur: saboteur.clone(),
            token,
            delay: Cell::new(delay),
            damage_types,
        }));
    }
}

impl ITick for Demolishable {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() || self.actions.borrow().is_empty() {
            return;
        }
        let no_damage = Damage::new(0, Default::default());
        let mut remove = Vec::new();
        for a in self.actions.borrow().clone() {
            let delay = a.delay.get();
            a.delay.set(delay - 1);
            if delay > 0 {
                continue;
            }
            let modifier = apply_percentage_modifiers(
                100,
                self.damage_modifiers
                    .borrow()
                    .iter()
                    .map(|t| t.get_damage_modifier(Some(self_), &no_damage)),
            );
            if modifier > 0 {
                if let Some(h) = self_.trait_or_default::<dyn IHealth>() {
                    h.kill(self_, Some(&a.saboteur), &a.damage_types);
                }
            } else if a.token != INVALID_CONDITION_TOKEN {
                self_.revoke_condition(a.token);
                remove.push(a);
            }
        }
        self.actions.borrow_mut().retain(|a| !remove.iter().any(|r| Rc::ptr_eq(r, a)));
    }
}
rustora_engine::persist_trait_refs!(dyn IDemolishable);

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_values!(dyn crate::cargo::EnterHooks: DemolishHooks);
}
