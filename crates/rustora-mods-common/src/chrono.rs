// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Chronoshiftable`, `Teleport`, `ConyardChronoReturn`, `ConyardChronoVortex`, `PortableChrono` and `MadTank`.
//!
//! Not ported: the time bars (`ISelectionBar`), the vortex renderer, and the
//! `PortableChronoOrderGenerator` of the deploy order; force move still targets the teleport.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::expression::BooleanExpression;
use rustora_core::world_types::{CPos, WAngle, WDist};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::activity::{LINE_CRIMSON, TargetLineNode};
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_init::{ActorInit, ActorInits, FacingInit, FactionInit, LocationInit, OwnerInit};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageTypeTag};
use rustora_engine::effect::Effect;
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, IOrderVoice, SoundType};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::*;
use rustora_engine::weapon::{WeaponInfo, weapon_impact};
use rustora_engine::world::World;
use rustora_engine::{activity_common, conditional_observers_impl, info_impl, load_info, register_traits};

use crate::activities::Nudge;
use crate::activities::{Move, MoveAdjacentTo};
use crate::actor_reference::HealthInit;
use crate::cargo::Cargo;
use crate::health::Health;
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::MobileInfo;
use crate::orders::{DeployOrderTargeter, IIssueDeployOrder, IIssueOrder, IOrderTargeter, TargetModifiers, UnitOrderTargeter};
use crate::render::WithSpriteBody;

pub fn register(r: &mut TraitRegistry) {
    r.register("Chronoshiftable", |n| load_info!(ChronoshiftableInfo, n));
    r.register("ConyardChronoReturn", |n| load_info!(ConyardChronoReturnInfo, n));
    r.register("PortableChrono", |n| load_info!(PortableChronoInfo, n));
    r.register("MadTank", |n| load_info!(MadTankInfo, n));
}

/// `MapGrid.MaximumTileSearchRange`; RA uses the default.
const MAXIMUM_TILE_SEARCH_RANGE: i32 = 50;

fn kill_actor(a: &ActorRef, attacker: Option<&ActorRef>, damage_types: &BitSet<DamageTypeTag>) {
    if let Some(h) = a.trait_or_default::<dyn IHealth>() {
        h.kill(a, attacker, damage_types);
    }
}

fn play_active_animation(chronosphere: Option<&ActorRef>, self_: &ActorRef) {
    let Some(c) = chronosphere.filter(|c| c.id != self_.id && !c.disposed()) else {
        return;
    };
    if let Some(body) = c.trait_or_default::<WithSpriteBody>()
        && body.default_animation.has_sequence("active")
    {
        body.play_custom_animation(c, "active", None);
    }
}

/// `ChronoshiftReturnInit`.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
pub struct ChronoshiftReturnInit {
    pub ticks: i32,
    pub duration: i32,
    pub origin: CPos,
    pub chronosphere: Option<ActorRef>,
}

impl ActorInit for ChronoshiftReturnInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ChronoshiftableInfo {
    pub time_bar_color: rustora_core::fieldloader::Color,
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub explode_instead: bool,
    pub damage_types: BitSet<DamageTypeTag>,
    pub chronoshift_sound: String,
    pub return_to_origin: bool,
}

impl Default for ChronoshiftableInfo {
    fn default() -> Self {
        ChronoshiftableInfo {
            time_bar_color: rustora_core::fieldloader::Color::from_argb(255, 255, 255, 255),
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            explode_instead: false,
            damage_types: BitSet::default(),
            chronoshift_sound: "chrono2.aud".into(),
            return_to_origin: true,
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct Chronoshiftable {
    info: Rc<ChronoshiftableInfo>,
    state: ConditionalState,
    self_: ActorRef,
    chronosphere: RefCell<Option<ActorRef>>,
    kill_cargo: Cell<bool>,
    duration: Cell<i32>,
    positionable: RefCell<Option<Rc<dyn IPositionable>>>,
    origin: Cell<CPos>,
    return_ticks: Cell<i32>,
}

register_traits!(Chronoshiftable: ISelectionBar, ITick, IDeathActorInitModifier, ITransformActorInitModifier, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ChronoshiftableInfo => |info, init| {
    let return_init = init.inits.get::<ChronoshiftReturnInit>();
    Some(Rc::new(Chronoshiftable {
        state: ConditionalState::new(&info.base),
        info,
        self_: init.self_.clone(),
        chronosphere: RefCell::new(return_init.and_then(|r| r.chronosphere.clone())),
        kill_cargo: Cell::new(false),
        duration: Cell::new(return_init.map_or(0, |r| r.duration)),
        positionable: RefCell::new(None),
        origin: Cell::new(return_init.map_or(CPos::ZERO, |r| r.origin)),
        return_ticks: Cell::new(return_init.map_or(0, |r| r.ticks)),
    }))
});

impl ConditionalHooks for Chronoshiftable {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

conditional_observers_impl!(Chronoshiftable);

impl ISync for Chronoshiftable {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.origin.get().hash() ^ self.return_ticks.get()
    }
}

impl INotifyCreated for Chronoshiftable {
    fn created(&self, self_: &ActorRef) {
        *self.positionable.borrow_mut() = self_
            .trait_or_default::<dyn IPositionable>()
            .filter(|_| self_.cache().occupies_space.is_some());
        conditional_created(self, self_);
    }
}

impl ITick for Chronoshiftable {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() || !self.info.return_to_origin || self.return_ticks.get() <= 0 {
            return;
        }
        self.return_ticks.set(self.return_ticks.get() - 1);
        if self.return_ticks.get() != 0 {
            return;
        }
        if self_.current_activity().is_some_and(|a| a.as_any().is::<Move>()) {
            self_.clear_current_activity();
        }
        let chronosphere = self.chronosphere.borrow().clone().unwrap_or_else(|| self_.clone());
        let teleport = Teleport::new(
            Some(chronosphere),
            self.origin.get(),
            None,
            TeleportOptions {
                kill_cargo: true,
                screen_flash: self.kill_cargo.get(),
                sound: Some(self.info.chronoshift_sound.clone()),
                interruptable: false,
                kill_on_failure: true,
                kill_damage_types: self.info.damage_types,
            },
        );
        self_.queue_activity_queued(false, teleport);
    }
}

impl Chronoshiftable {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn return_ticks(&self) -> i32 {
        self.return_ticks.get()
    }

    pub fn can_chronoshift_to(&self, target_location: CPos) -> bool {
        !self.state.is_trait_disabled()
            && self
                .positionable
                .borrow()
                .as_ref()
                .is_some_and(|p| p.can_enter_cell(target_location, None, BlockedByActor::All))
    }

    pub fn teleport(&self, target_location: CPos, duration: i32, kill_cargo: bool, chronosphere: &ActorRef) -> bool {
        if self.state.is_trait_disabled() {
            return false;
        }
        let self_ = &self.self_;
        if self.info.explode_instead {
            let (a, c, types) = (self_.clone(), chronosphere.clone(), self.info.damage_types);
            self_.world().add_frame_end_task(Box::new(move |_| {
                if !a.disposed() {
                    kill_actor(&a, Some(&c), &types);
                }
            }));
            return true;
        }
        if self.return_ticks.get() <= 0 {
            self.origin.set(self_.location());
            self.return_ticks.set(duration);
        }
        self.duration.set(duration);
        *self.chronosphere.borrow_mut() = Some(chronosphere.clone());
        self.kill_cargo.set(kill_cargo);
        let teleport = Teleport::new(
            Some(chronosphere.clone()),
            target_location,
            None,
            TeleportOptions {
                kill_cargo,
                screen_flash: true,
                sound: Some(self.info.chronoshift_sound.clone()),
                interruptable: true,
                kill_on_failure: false,
                kill_damage_types: BitSet::default(),
            },
        );
        self_.queue_activity_queued(false, teleport);
        true
    }

    fn modify_actor_init(&self, init: &mut ActorInits) {
        if self.state.is_trait_disabled() || !self.info.return_to_origin || self.return_ticks.get() <= 0 {
            return;
        }
        init.push(Rc::new(ChronoshiftReturnInit {
            ticks: self.return_ticks.get(),
            duration: self.duration.get(),
            origin: self.origin.get(),
            chronosphere: self.chronosphere.borrow().clone(),
        }));
    }
}

impl IDeathActorInitModifier for Chronoshiftable {
    fn modify_death_actor_init(&self, _self_: &ActorRef, init: &mut ActorInits) {
        self.modify_actor_init(init);
    }
}

impl ITransformActorInitModifier for Chronoshiftable {
    fn modify_transform_actor_init(&self, _self_: &ActorRef, init: &mut ActorInits) {
        self.modify_actor_init(init);
    }
}

/// Optional arguments of the `Teleport` constructor.
#[derive(rustora_engine::persist::Persist)]
#[persist(value)]
pub struct TeleportOptions {
    pub kill_cargo: bool,
    pub screen_flash: bool,
    pub sound: Option<String>,
    pub interruptable: bool,
    pub kill_on_failure: bool,
    pub kill_damage_types: BitSet<DamageTypeTag>,
}

/// `Teleport`.
#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
pub struct Teleport {
    base: ActivityBase,
    teleporter: Option<ActorRef>,
    maximum_distance: Option<i32>,
    options: TeleportOptions,
    destination: Cell<CPos>,
}

impl Teleport {
    pub fn new(teleporter: Option<ActorRef>, destination: CPos, maximum_distance: Option<i32>, options: TeleportOptions) -> ActivityRef {
        let base = ActivityBase::default();
        if !options.interruptable {
            base.is_interruptible.set(false);
        }
        Rc::new(Teleport {
            base,
            teleporter,
            maximum_distance,
            options,
            destination: Cell::new(destination),
        })
    }

    fn choose_best_destination_cell(&self, self_: &ActorRef, teleporter: &ActorRef, destination: CPos) -> Option<CPos> {
        let world = self_.world();
        let map = world.map();
        let restrict_to: Option<Vec<CPos>> = self
            .maximum_distance
            .map(|d| map.find_tiles_in_circle(self_.location(), d, false).collect());
        let mut destination = destination;
        if let Some(r) = &restrict_to {
            let mut best: Option<(CPos, i32)> = None;
            for c in r {
                let d = (*c - destination).length_squared();
                if best.is_none_or(|(_, b)| d < b) {
                    best = Some((*c, d));
                }
            }
            if let Some((c, _)) = best {
                destination = c;
            }
        }
        let pos = self_.trait_::<dyn IPositionable>();
        let shroud = teleporter.owner().shroud();
        if pos.can_enter_cell(destination, None, BlockedByActor::All) && shroud.is_explored(destination) {
            return Some(destination);
        }
        let max = self.maximum_distance.unwrap_or(MAXIMUM_TILE_SEARCH_RANGE);
        map.find_tiles_in_circle(destination, max, false).find(|tile| {
            shroud.is_explored(*tile)
                && restrict_to.as_ref().is_none_or(|r| r.contains(tile))
                && pos.can_enter_cell(*tile, None, BlockedByActor::All)
        })
    }
}

impl Activity for Teleport {
    activity_common!("Teleport");

    fn tick(&self, self_: &ActorRef) -> bool {
        let teleporter = self.teleporter.as_ref();
        let is_self = teleporter.is_some_and(|t| t.id == self_.id);
        let pc = self_.trait_or_default::<PortableChrono>();
        if is_self
            && let Some(pc) = &pc
            && (!pc.can_teleport() || self.base.is_canceling())
        {
            if self.options.kill_on_failure {
                kill_actor(self_, teleporter, &self.options.kill_damage_types);
            }
            return true;
        }
        let best = teleporter.and_then(|t| self.choose_best_destination_cell(self_, t, self.destination.get()));
        let Some(destination) = best else {
            if self.options.kill_on_failure {
                kill_actor(self_, teleporter, &self.options.kill_damage_types);
            }
            return true;
        };
        self.destination.set(destination);
        let world = self_.world();
        if let Some(s) = &self.options.sound {
            sound::play_at(&world, SoundType::World, s, self_.center_position(), 1.0);
            sound::play_at(&world, SoundType::World, s, world.map().center_of_cell(destination), 1.0);
        }
        self_
            .trait_::<dyn IPositionable>()
            .set_position_cell(self_, destination, subcell::ANY);
        self_.generation.set(self_.generation.get() + 1);
        if self.options.kill_cargo
            && let (Some(cargo), Some(t)) = (self_.trait_or_default::<Cargo>(), teleporter)
        {
            while !cargo.is_empty() {
                let a = cargo.unload(self_, None);
                kill_actor(&a, Some(t), &BitSet::default());
            }
        }
        if is_self && let Some(pc) = &pc {
            pc.reset_charge_time();
        }
        if self.options.screen_flash {
            crate::post_effects::chronoshift_flash(&world);
        }
        play_active_animation(teleporter, self_);
        true
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ConyardChronoReturnInfo {
    pub time_bar_color: rustora_core::fieldloader::Color,
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub condition: Option<String>,
    pub damage: i32,
    pub damage_types: BitSet<DamageTypeTag>,
    pub return_original_actor_on_condition: Option<BooleanExpression>,
    pub original_actor: String,
    pub facing: WAngle,
    pub chronoshift_sound: String,
}

impl Default for ConyardChronoReturnInfo {
    fn default() -> Self {
        ConyardChronoReturnInfo {
            time_bar_color: rustora_core::fieldloader::Color::from_argb(255, 255, 255, 255),
            instance_name: None,
            condition: None,
            damage: 1000,
            damage_types: BitSet::default(),
            return_original_actor_on_condition: None,
            original_actor: "mcv".into(),
            facing: WAngle::new(384),
            chronoshift_sound: "chrono2.aud".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct ConyardChronoReturn {
    info: Rc<ConyardChronoReturnInfo>,
    self_: ActorRef,
    me: Weak<ConyardChronoReturn>,
    faction: String,
    condition_token: Cell<i32>,
    chronosphere: RefCell<Option<ActorRef>>,
    duration: i32,
    return_original: Cell<bool>,
    selling: Cell<bool>,
    return_ticks: Cell<i32>,
    origin: CPos,
    triggered: Cell<bool>,
}

register_traits!(ConyardChronoReturn: ISelectionBar, ITick, ISync, IObservesVariables, INotifySold, IDeathActorInitModifier, ITransformActorInitModifier);
info_impl!(ConyardChronoReturnInfo => |info, init| {
    let return_init = init.inits.get::<ChronoshiftReturnInit>();
    Some(Rc::new_cyclic(|me| ConyardChronoReturn {
        info,
        self_: init.self_.clone(),
        me: me.clone(),
        faction: init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone()),
        condition_token: Cell::new(INVALID_CONDITION_TOKEN),
        chronosphere: RefCell::new(return_init.and_then(|r| r.chronosphere.clone())),
        duration: return_init.map_or(0, |r| r.duration),
        return_original: Cell::new(false),
        selling: Cell::new(false),
        return_ticks: Cell::new(return_init.map_or(0, |r| r.ticks)),
        origin: return_init.map_or(CPos::ZERO, |r| r.origin),
        triggered: Cell::new(false),
    }))
});

impl ISync for ConyardChronoReturn {
    fn sync_hash(&self) -> i32 {
        self.return_ticks.get() ^ self.origin.hash() ^ sync_bool(self.triggered.get())
    }
}

impl IObservesVariables for ConyardChronoReturn {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        let Some(expression) = self.info.return_original_actor_on_condition.clone() else {
            return Vec::new();
        };
        let variables: Vec<String> = expression.variables().iter().cloned().collect();
        let me = Rc::downgrade(&self);
        vec![VariableObserver {
            notifier: Rc::new(move |_self_, conditions| {
                if let Some(t) = me.upgrade() {
                    t.return_original.set(expression.evaluate(conditions));
                }
            }),
            variables,
        }]
    }
}

impl ConyardChronoReturn {
    fn trigger_vortex(&self) {
        let self_ = &self.self_;
        if self.condition_token.get() == INVALID_CONDITION_TOKEN
            && let Some(c) = &self.info.condition
        {
            self.condition_token.set(self_.grant_condition(c));
        }
        self.triggered.set(true);
        let me = self.me.clone();
        let launcher = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |w| {
            w.add_effect(ConyardChronoVortex::new(me, launcher));
        }));
    }

    fn choose_best_destination_cell(&self, mobile_info: &MobileInfo, destination: CPos) -> Option<CPos> {
        let chronosphere = self.chronosphere.borrow().clone()?;
        let world = self.self_.world();
        if mobile_info.can_enter_cell(&world, None, destination, subcell::ANY, None, BlockedByActor::All) {
            return Some(destination);
        }
        let shroud = chronosphere.owner().shroud();
        let max = MAXIMUM_TILE_SEARCH_RANGE;
        world
            .map()
            .find_tiles_in_circle(destination, max, false)
            .find(|t| shroud.is_explored(*t) && mobile_info.can_enter_cell(&world, None, *t, subcell::ANY, None, BlockedByActor::All))
    }

    fn return_to_origin(&self, self_: &ActorRef) {
        let world = self_.world();
        let actor_info = world.rules().actor(&self.info.original_actor).expect("original actor exists");
        let mobile_info = actor_info.trait_info::<MobileInfo>().expect("original actor has Mobile");
        let Some(destination) = self.choose_best_destination_cell(&mobile_info, self.origin) else {
            return;
        };
        for nt in self_.traits_implementing::<dyn INotifyTransform>() {
            nt.on_transform(self_);
        }
        let health = self_.trait_::<Health>();
        let mut init = ActorInits::new()
            .with(LocationInit::new(destination))
            .with(OwnerInit::from_player(&self_.owner()))
            .with(FacingInit::new(self.info.facing))
            .with(FactionInit::new(self.faction.clone()))
            .with(HealthInit::new((health.hp() as i64 * 100 / health.max_hp() as i64) as i32));
        for m in self_.traits_implementing::<dyn ITransformActorInitModifier>() {
            m.modify_transform_actor_init(self_, &mut init);
        }
        let a = world.create_actor(&self.info.original_actor, init);
        for nt in self_.traits_implementing::<dyn INotifyTransform>() {
            nt.after_transform(&a);
        }
        if let Some(selection) = crate::selection::Selection::of(&world).filter(|s| s.contains(self_)) {
            selection.add(&a);
        }
        if let Some(groups) = crate::selection::ControlGroups::of(&world)
            && let Some(group) = groups.control_group_for_actor(self_)
        {
            groups.add_to_control_group(&a, group);
        }
        *self_.replaced_by_actor.borrow_mut() = Some(a);
        sound::play_at(
            &world,
            SoundType::World,
            &self.info.chronoshift_sound,
            world.map().center_of_cell(destination),
            1.0,
        );
        self_.dispose();
    }

    fn modify_actor_init(&self, init: &mut ActorInits) {
        if self.return_ticks.get() <= 0 {
            return;
        }
        init.push(Rc::new(ChronoshiftReturnInit {
            ticks: self.return_ticks.get(),
            duration: self.duration,
            origin: self.origin,
            chronosphere: self.chronosphere.borrow().clone(),
        }));
    }
}

impl ITick for ConyardChronoReturn {
    fn tick(&self, self_: &ActorRef) {
        if self_.will_dispose() {
            return;
        }
        if self.triggered.get() {
            let chronosphere = self.chronosphere.borrow().clone();
            let damage = Damage::new(self.info.damage, self.info.damage_types);
            self_.trait_::<Health>().inflict_damage(self_, chronosphere.as_ref(), &damage, true);
        }
        if self.return_ticks.get() <= 0 {
            return;
        }
        self.return_ticks.set(self.return_ticks.get() - 1);
        if self.return_ticks.get() > 0 {
            return;
        }
        if self.return_original.get() && !self.selling.get() {
            self.return_to_origin(self_);
        } else {
            self.trigger_vortex();
        }
        crate::post_effects::chronoshift_flash(&self_.world());
        sound::play_at(
            &self_.world(),
            SoundType::World,
            &self.info.chronoshift_sound,
            self_.center_position(),
            1.0,
        );
        play_active_animation(self.chronosphere.borrow().as_ref(), self_);
    }
}

impl INotifySold for ConyardChronoReturn {
    fn selling(&self, _self_: &ActorRef) {
        self.selling.set(true);
    }
    fn sold(&self, _self_: &ActorRef) {}
}

impl IDeathActorInitModifier for ConyardChronoReturn {
    fn modify_death_actor_init(&self, _self_: &ActorRef, init: &mut ActorInits) {
        self.modify_actor_init(init);
    }
}

impl ITransformActorInitModifier for ConyardChronoReturn {
    fn modify_transform_actor_init(&self, _self_: &ActorRef, init: &mut ActorInits) {
        self.modify_actor_init(init);
    }
}

/// `ConyardChronoVortex`, without its renderable: only the end of the animation affects the simulation.
#[derive(rustora_engine::persist::Persist)]
#[persist(effect)]
struct ConyardChronoVortex {
    /// The trait that started the vortex and its actor, notified on completion.
    on_completion: RefCell<Option<(Weak<ConyardChronoReturn>, ActorRef)>>,
    #[persist(me)]
    me: Weak<ConyardChronoVortex>,
    loops: Cell<i32>,
    frame: Cell<i32>,
}

impl ConyardChronoVortex {
    fn new(trait_: Weak<ConyardChronoReturn>, launcher: ActorRef) -> Rc<ConyardChronoVortex> {
        Rc::new_cyclic(|me| ConyardChronoVortex {
            on_completion: RefCell::new(Some((trait_, launcher))),
            me: me.clone(),
            loops: Cell::new(3),
            frame: Cell::new(0),
        })
    }
}

impl Effect for ConyardChronoVortex {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn tick(&self, world: &Rc<World>) {
        self.frame.set(self.frame.get() + 1);
        if self.frame.get() == 32 {
            self.loops.set(self.loops.get() - 1);
            if self.loops.get() > 0 {
                self.frame.set(16);
            }
        }
        if self.frame.get() == 48 {
            let me = self.me.upgrade();
            let on_completion = self.on_completion.borrow_mut().take();
            world.add_frame_end_task(Box::new(move |w| {
                if let Some(me) = me {
                    w.remove_effect(&(me as Rc<dyn Effect>));
                }
                if let Some((t, launcher)) = on_completion
                    && let Some(t) = t.upgrade()
                {
                    t.triggered.set(false);
                    if t.condition_token.get() != INVALID_CONDITION_TOKEN && !launcher.disposed() {
                        t.condition_token.set(launcher.revoke_condition(t.condition_token.get()));
                    }
                }
            }));
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PortableChronoInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub charge_delay: i32,
    pub has_distance_limit: bool,
    pub max_distance: i32,
    pub chronoshift_sound: String,
    pub deploy_cursor: String,
    pub deploy_blocked_cursor: String,
    pub target_cursor: String,
    pub target_blocked_cursor: String,
    pub kill_cargo: bool,
    pub flash_screen: bool,
    pub voice: String,
}

impl Default for PortableChronoInfo {
    fn default() -> Self {
        PortableChronoInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            charge_delay: 500,
            has_distance_limit: true,
            max_distance: 12,
            chronoshift_sound: "chrotnk1.aud".into(),
            deploy_cursor: "deploy".into(),
            deploy_blocked_cursor: "deploy-blocked".into(),
            target_cursor: "chrono-target".into(),
            target_blocked_cursor: "move-blocked".into(),
            kill_cargo: true,
            flash_screen: false,
            voice: "Action".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct PortableChrono {
    info: Rc<PortableChronoInfo>,
    state: ConditionalState,
    mv: Rc<dyn IMove>,
    charge_tick: Cell<i32>,
}

register_traits!(PortableChrono: ISelectionBar, IIssueOrder, IResolveOrder, ITick, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(PortableChronoInfo => |info, init| Some(Rc::new(PortableChrono {
    state: ConditionalState::new_pausable(&info.base),
    info,
    mv: init.self_.trait_::<dyn IMove>(),
    charge_tick: Cell::new(0),
})));

impl ConditionalHooks for PortableChrono {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
    fn trait_disabled(&self, _self_: &ActorRef) {
        self.charge_tick.set(0);
    }
}

conditional_observers_impl!(PortableChrono);

impl ISync for PortableChrono {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.charge_tick.get()
    }
}

impl INotifyCreated for PortableChrono {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl PortableChrono {
    pub fn can_teleport(&self) -> bool {
        !self.state.is_trait_disabled() && !self.state.is_trait_paused() && self.charge_tick.get() <= 0
    }

    pub fn reset_charge_time(&self) {
        self.charge_tick.set(self.info.charge_delay);
    }
}

impl ITick for PortableChrono {
    fn tick(&self, _self_: &ActorRef) {
        if !self.state.is_trait_disabled() && !self.state.is_trait_paused() && self.charge_tick.get() > 0 {
            self.charge_tick.set(self.charge_tick.get() - 1);
        }
    }
}

impl IIssueOrder for PortableChrono {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let cursor = if self.can_teleport() {
            &self.info.deploy_cursor
        } else {
            &self.info.deploy_blocked_cursor
        }
        .clone();
        vec![
            Box::new(PortableChronoOrderTargeter {
                target_cursor: self.info.target_cursor.clone(),
                is_queued: Cell::new(false),
            }),
            Box::new(DeployOrderTargeter::new(
                "PortableChronoDeploy",
                5,
                Box::new(move || cursor.clone()),
            )),
        ]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        match order.order_id() {
            "PortableChronoDeploy" => Some(Order::new("PortableChronoDeploy", Some(self_.clone()), Target::Invalid, queued)),
            "PortableChronoTeleport" => Some(Order::new("PortableChronoTeleport", Some(self_.clone()), target.clone(), queued)),
            _ => None,
        }
    }
}

impl IResolveOrder for PortableChrono {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "PortableChronoTeleport" || order.target.target_type() == TargetType::Invalid {
            return;
        }
        let max_distance = self.info.has_distance_limit.then_some(self.info.max_distance);
        if !order.queued {
            self_.cancel_activity();
        }
        let cell = self_.world().map().cell_containing(order.target.center_position());
        if let Some(d) = max_distance {
            self_.queue_activity(self.mv.move_within_range(&order.target, WDist::ZERO, WDist::from_cells(d), None));
        }
        let options = TeleportOptions {
            kill_cargo: self.info.kill_cargo,
            screen_flash: self.info.flash_screen,
            sound: Some(self.info.chronoshift_sound.clone()),
            interruptable: true,
            kill_on_failure: false,
            kill_damage_types: BitSet::default(),
        };
        self_.queue_activity(Teleport::new(Some(self_.clone()), cell, max_distance, options));
        self_.queue_activity(self.mv.move_to(cell, 5, None, false));
    }
}

impl IOrderVoice for PortableChrono {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == "PortableChronoTeleport").then(|| self.info.voice.clone())
    }
}

struct PortableChronoOrderTargeter {
    target_cursor: String,
    is_queued: Cell<bool>,
}

impl IOrderTargeter for PortableChronoOrderTargeter {
    fn order_id(&self) -> &str {
        "PortableChronoTeleport"
    }

    fn order_priority(&self) -> i32 {
        5
    }

    fn can_target(&self, self_: &ActorRef, target: &Target, modifiers: &mut TargetModifiers, cursor: &mut Option<String>) -> bool {
        if !modifiers.has_modifier(TargetModifiers::FORCE_MOVE) {
            return false;
        }
        let xy = self_.world().map().cell_containing(target.center_position());
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        if self_.is_in_world() && self_.owner().shroud().is_explored(xy) {
            *cursor = Some(self.target_cursor.clone());
            return true;
        }
        false
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(
        &self,
        _self_: &ActorRef,
        _target: &Target,
        _actors_at: &[ActorRef],
        _xy: CPos,
        _modifiers: TargetModifiers,
    ) -> bool {
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MadTankInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub thump_sequence: Option<String>,
    pub thump_interval: i32,
    pub thump_damage_weapon: Option<String>,
    pub charge_delay: i32,
    pub charge_sound: String,
    pub detonation_delay: i32,
    pub detonation_sound: String,
    pub detonation_weapon: Option<String>,
    pub driver_actor: String,
    pub voice: String,
    pub deployed_condition: Option<String>,
    pub damage_types: BitSet<DamageTypeTag>,
    pub attack_cursor: String,
    pub deploy_cursor: String,
}

impl Default for MadTankInfo {
    fn default() -> Self {
        MadTankInfo {
            instance_name: None,
            thump_sequence: Some("piston".into()),
            thump_interval: 8,
            thump_damage_weapon: Some("MADTankThump".into()),
            charge_delay: 96,
            charge_sound: "madchrg2.aud".into(),
            detonation_delay: 42,
            detonation_sound: "madexplo.aud".into(),
            detonation_weapon: Some("MADTankDetonate".into()),
            driver_actor: "e1".into(),
            voice: "Action".into(),
            deployed_condition: None,
            damage_types: BitSet::default(),
            attack_cursor: "attack".into(),
            deploy_cursor: "deploy".into(),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct MadTank {
    info: Rc<MadTankInfo>,
    me: Weak<MadTank>,
    initiated: Cell<bool>,
}

register_traits!(MadTank: IIssueOrder, IResolveOrder, IOrderVoice, IIssueDeployOrder);
info_impl!(MadTankInfo => |info, _init| Some(Rc::new_cyclic(|me| MadTank { info, me: me.clone(), initiated: Cell::new(false) })));

fn lookup_weapon(world: &World, name: &Option<String>) -> Option<Rc<WeaponInfo>> {
    name.as_ref()
        .map(|n| world.rules().weapon(&n.to_lowercase()).expect("weapon exists"))
}

impl IIssueOrder for MadTank {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        let mut attack = UnitOrderTargeter::target_types(
            BitSet::<TargetableType>::new(&["DetonateAttack"]),
            "DetonateAttack",
            5,
            &self.info.attack_cursor,
            true,
            false,
        );
        attack.force_attack = Some(false);
        let mut orders: Vec<Box<dyn IOrderTargeter>> = vec![Box::new(attack)];
        if !self.initiated.get() {
            let cursor = self.info.deploy_cursor.clone();
            orders.push(Box::new(DeployOrderTargeter::new("Detonate", 5, Box::new(move || cursor.clone()))));
        }
        orders
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        matches!(order.order_id(), "DetonateAttack" | "Detonate")
            .then(|| Order::new(order.order_id(), Some(self_.clone()), target.clone(), queued))
    }
}

impl IIssueDeployOrder for MadTank {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order> {
        Some(Order::new("Detonate", Some(self_.clone()), Target::Invalid, queued))
    }
    fn can_issue_deploy_order(&self, _self_: &ActorRef, _queued: bool) -> bool {
        true
    }
}

impl IOrderVoice for MadTank {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        matches!(order.order_string.as_str(), "DetonateAttack" | "Detonate").then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for MadTank {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        let mad = self.me.upgrade().expect("alive");
        match order.order_string.as_str() {
            "DetonateAttack" => self_.queue_activity_queued(order.queued, DetonationSequence::new(self_, mad, order.target.clone(), false)),
            "Detonate" => self_.queue_activity_queued(order.queued, DetonationSequence::new(self_, mad, Target::Invalid, true)),
            _ => {}
        }
    }
}

/// `MadTank.DetonationSequence`.
#[derive(rustora_engine::persist::Persist)]
#[persist(activity)]
struct DetonationSequence {
    base: ActivityBase,
    mad: Rc<MadTank>,
    mv: Rc<dyn IMove>,
    body: Rc<WithSpriteBody>,
    assign_target_on_first_run: bool,
    ticks: Cell<i32>,
    target: RefCell<Target>,
}

impl DetonationSequence {
    fn new(self_: &ActorRef, mad: Rc<MadTank>, target: Target, assign_target_on_first_run: bool) -> ActivityRef {
        Rc::new(DetonationSequence {
            base: ActivityBase::default(),
            mad,
            mv: self_.trait_::<dyn IMove>(),
            body: self_.trait_::<WithSpriteBody>(),
            assign_target_on_first_run,
            ticks: Cell::new(0),
            target: RefCell::new(target),
        })
    }
}

impl Activity for DetonationSequence {
    activity_common!("DetonationSequence");

    fn target_line_nodes(&self, self_: &ActorRef) -> Vec<TargetLineNode> {
        let _ = self_;
        vec![TargetLineNode {
            target: self.target.borrow().clone(),
            color: LINE_CRIMSON,
        }]
    }

    fn on_first_run(&self, self_: &ActorRef) {
        if self.assign_target_on_first_run {
            *self.target.borrow_mut() = Target::from_cell(&self_.world(), self_.location(), 0);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let target = self.target.borrow().clone();
        if target.target_type() != TargetType::Invalid && !self.mv.can_enter_target_now(self_, &target) {
            self.base.queue_child(MoveAdjacentTo::new_adjacent(self_, &target, None));
            return false;
        }
        let info = &self.mad.info;
        if !self.mad.initiated.get() {
            if target.target_type() == TargetType::Invalid {
                return true;
            }
            if let Some(c) = &info.deployed_condition {
                self_.grant_condition(c);
            }
            let (a, driver) = (self_.clone(), info.driver_actor.to_lowercase());
            self_.world().add_frame_end_task(Box::new(move |w| {
                let inits = ActorInits::new()
                    .with(LocationInit::new(a.location()))
                    .with(OwnerInit::from_player(&a.owner()));
                let d = w.create_actor(&driver, inits);
                d.queue_activity_queued(false, Nudge::new(&d));
            }));
            if let Some(s) = &info.thump_sequence {
                self.body.play_custom_animation_repeating(self_, s);
            }
            self.base.is_interruptible.set(false);
            self.mad.initiated.set(true);
        }
        self.ticks.set(self.ticks.get() + 1);
        let world = self_.world();
        if self.ticks.get() % info.thump_interval == 0
            && let Some(w) = lookup_weapon(&world, &info.thump_damage_weapon)
        {
            weapon_impact(&w, &Target::from_pos(self_.center_position()), self_);
        }
        if self.ticks.get() == info.charge_delay {
            sound::play_at(&world, SoundType::World, &info.charge_sound, self_.center_position(), 1.0);
        }
        self.ticks.get() == info.charge_delay + info.detonation_delay
    }

    fn on_last_run(&self, self_: &ActorRef) {
        if !self.mad.initiated.get() {
            return;
        }
        let info = self.mad.info.clone();
        sound::play_at(
            &self_.world(),
            SoundType::World,
            &info.detonation_sound,
            self_.center_position(),
            1.0,
        );
        let a = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |w| {
            if let Some(weapon) = lookup_weapon(w, &info.detonation_weapon) {
                weapon_impact(&weapon, &Target::from_pos(a.center_position()), &a);
            }
            kill_actor(&a, Some(&a), &info.damage_types);
        }));
    }
}

impl ISelectionBar for Chronoshiftable {
    fn value(&self, self_: &ActorRef, viewer: Option<&rustora_engine::player::PlayerRef>) -> f32 {
        let ticks = self.return_ticks.get();
        if self.state.is_trait_disabled()
            || !self.info.return_to_origin
            || ticks == 0
            || !self_.owner().is_allied_with(viewer.map(|v| &**v))
        {
            return 0.0;
        }
        ticks as f32 / self.duration.get() as f32
    }

    fn color(&self) -> rustora_core::fieldloader::Color {
        self.info.time_bar_color
    }
}

impl ISelectionBar for ConyardChronoReturn {
    fn value(&self, self_: &ActorRef, viewer: Option<&rustora_engine::player::PlayerRef>) -> f32 {
        let ticks = self.return_ticks.get();
        if ticks == 0 || !self_.owner().is_allied_with(viewer.map(|v| &**v)) {
            return 0.0;
        }
        ticks as f32 / self.duration as f32
    }

    fn color(&self) -> rustora_core::fieldloader::Color {
        self.info.time_bar_color
    }
}

impl ISelectionBar for PortableChrono {
    fn value(&self, _self_: &ActorRef, _viewer: Option<&rustora_engine::player::PlayerRef>) -> f32 {
        if self.state.is_trait_disabled() {
            return 0.0;
        }
        (self.info.charge_delay - self.charge_tick.get()) as f32 / self.info.charge_delay as f32
    }

    fn color(&self) -> rustora_core::fieldloader::Color {
        crate::cosmetic::magenta()
    }
}

/// Registers the save game loaders of this module.
pub(crate) fn register_persist() {
    rustora_engine::persist_activities!(Teleport, DetonationSequence);
    rustora_engine::persist_effects!(ConyardChronoVortex);
    rustora_engine::persist_values!(dyn rustora_engine::actor_init::ActorInit: ChronoshiftReturnInit);
}
