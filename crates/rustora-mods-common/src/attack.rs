// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Armament.cs`, `Traits/Attack`, `Traits/Air/AttackAircraft.cs`, `Traits/Air/AttackBomber.cs`,
//! `Traits/Attack/AttackLeap.cs`, `Traits/Attack/AttackTesla.cs` and their activities.
//!
//! The C# subclasses of `AttackBase` become one struct with an [`AttackKind`], and their infos one union struct.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::{WAngle, WDist, WPos, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::LINE_CRIMSON;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::actor_ext::enabled_target_types;
use rustora_engine::actor_info::TraitNode;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::order::Order;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, SoundType};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::*;
use rustora_engine::weapon::{ProjectileArgs, WeaponInfo};
use rustora_engine::{activity_common, info_impl, load_info};

use crate::activities::{MoveCooldownHelper, Turn, Wait};
use crate::aircraft::{Aircraft, AircraftInfo, Fly, FlyArgs, FlyForward, ReturnToBase, TakeOff};
use crate::auto_target::{AutoTarget, INotifyStanceChanged, IOverrideAutoTarget, UnitStance};
use crate::cargo::Cargo;
use crate::interfaces::*;
use crate::locomotor::BlockedByActor;
use crate::mobile::Mobile;
use crate::render::BodyOrientation;
use crate::resupply::{Rearmable, Resupply};
use crate::turreted::Turreted;
use crate::util;
use crate::visibility::AffectsShroud;

pub fn register(r: &mut TraitRegistry) {
    r.register("Armament", |n| load_info!(ArmamentInfo, n));
    r.register("AttackFrontal", |n| load_attack(n, AttackKind::Frontal));
    r.register("AttackOmni", |n| load_attack(n, AttackKind::Omni));
    r.register("AttackFollow", |n| load_attack(n, AttackKind::Follow));
    r.register("AttackTurreted", |n| load_attack(n, AttackKind::Turreted));
    r.register("AttackCharges", |n| load_attack(n, AttackKind::Charges));
    r.register("AttackGarrisoned", |n| load_attack(n, AttackKind::Garrisoned));
    r.register("AttackAircraft", |n| load_attack(n, AttackKind::Aircraft));
    r.register("AttackBomber", |n| load_attack(n, AttackKind::Bomber));
    r.register("AttackLeap", |n| load_attack(n, AttackKind::Leap));
    r.register("EdibleByLeap", |n| load_info!(EdibleByLeapInfo, n));
    r.register("AttackTesla", |n| load_attack(n, AttackKind::Tesla));
}

#[derive(Clone)]
pub struct Barrel {
    pub offset: WVec,
    pub yaw: WAngle,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ArmamentInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub name: String,
    #[field(require)]
    pub weapon: String,
    pub turret: String,
    pub fire_delay: i32,
    pub local_offset: Vec<WVec>,
    pub local_yaw: Vec<WAngle>,
    pub recoil: WDist,
    pub recoil_recovery: WDist,
    pub reloading_condition: Option<String>,
    pub target_relationships: PlayerRelationship,
    pub force_target_relationships: PlayerRelationship,
    pub ammo_usage: i32,
    pub cursor: String,
    pub outside_range_cursor: String,
    pub muzzle_sequence: Option<String>,
    pub muzzle_palette: String,
}

impl Default for ArmamentInfo {
    fn default() -> Self {
        ArmamentInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            name: "primary".into(),
            weapon: String::new(),
            turret: "primary".into(),
            muzzle_sequence: None,
            muzzle_palette: "effect".into(),
            fire_delay: 0,
            local_offset: Vec::new(),
            local_yaw: Vec::new(),
            recoil: WDist::ZERO,
            recoil_recovery: WDist::new(9),
            reloading_condition: None,
            target_relationships: PlayerRelationship::ENEMY,
            force_target_relationships: PlayerRelationship(
                PlayerRelationship::ENEMY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ALLY.0,
            ),
            ammo_usage: 1,
            cursor: "attack".into(),
            outside_range_cursor: "attackoutsiderange".into(),
        }
    }
}

type DelayedAction = (i32, i32, Box<dyn Fn(i32)>);

pub struct Armament {
    pub info: Rc<ArmamentInfo>,
    state: ConditionalState,
    me: Weak<Armament>,
    pub actor: ActorRef,
    pub weapon: Rc<WeaponInfo>,
    pub barrels: Vec<Barrel>,
    turret: OnceCell<Option<Rc<Turreted>>>,
    hovers: OnceCell<Option<Rc<crate::conditions::Hovers>>>,
    coords: OnceCell<Rc<BodyOrientation>>,
    notify_burst_complete: RefCell<Vec<Rc<dyn INotifyBurstComplete>>>,
    notify_attacks: RefCell<Vec<(ActorRef, Rc<dyn INotifyAttack>)>>,
    condition_token: Cell<i32>,
    range_modifiers: RefCell<Vec<Rc<dyn IRangeModifier>>>,
    reload_modifiers: RefCell<Vec<Rc<dyn IReloadModifier>>>,
    damage_modifiers: RefCell<Vec<Rc<dyn IFirepowerModifier>>>,
    inaccuracy_modifiers: RefCell<Vec<Rc<dyn IInaccuracyModifier>>>,
    ticks_since_last_shot: Cell<i32>,
    current_barrel: Cell<usize>,
    delayed_actions: RefCell<Vec<DelayedAction>>,
    pub recoil: Cell<WDist>,
    fire_delay: Cell<i32>,
    burst: Cell<i32>,
}

rustora_engine::register_traits!(Armament: ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ArmamentInfo => |info, init| {
    let w = init.world();
    let weapon = w.rules().weapon(&info.weapon.to_lowercase()).unwrap_or_else(|| panic!("Weapons Ruleset does not contain an entry '{}'", info.weapon));
    let mut barrels: Vec<Barrel> = info.local_offset.iter().enumerate().map(|(i, o)| Barrel { offset: *o, yaw: info.local_yaw.get(i).copied().unwrap_or(WAngle::ZERO) }).collect();
    if barrels.is_empty() {
        barrels.push(Barrel { offset: WVec::ZERO, yaw: WAngle::ZERO });
    }
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new_cyclic(|me| Armament {
        state,
        me: me.clone(),
        actor: init.self_.clone(),
        burst: Cell::new(weapon.burst),
        weapon,
        barrels,
        turret: OnceCell::new(),
        hovers: OnceCell::new(),
        coords: OnceCell::new(),
        notify_burst_complete: RefCell::new(Vec::new()),
        notify_attacks: RefCell::new(Vec::new()),
        condition_token: Cell::new(INVALID_CONDITION_TOKEN),
        range_modifiers: RefCell::new(Vec::new()),
        reload_modifiers: RefCell::new(Vec::new()),
        damage_modifiers: RefCell::new(Vec::new()),
        inaccuracy_modifiers: RefCell::new(Vec::new()),
        ticks_since_last_shot: Cell::new(0),
        current_barrel: Cell::new(0),
        delayed_actions: RefCell::new(Vec::new()),
        recoil: Cell::new(WDist::ZERO),
        fire_delay: Cell::new(0),
        info,
    }))
});

impl ConditionalHooks for Armament {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_plumbing!(Armament);

impl INotifyCreated for Armament {
    fn created(&self, self_: &ActorRef) {
        let _ = self.turret.set(
            self_
                .traits_implementing::<Turreted>()
                .into_iter()
                .find(|t| t.name() == self.info.turret),
        );
        let _ = self.hovers.set(self_.trait_or_default::<crate::conditions::Hovers>());
        let _ = self.coords.set(self_.trait_::<BodyOrientation>());
        *self.notify_burst_complete.borrow_mut() = self_.traits_implementing::<dyn INotifyBurstComplete>();
        *self.notify_attacks.borrow_mut() = self_
            .traits_implementing::<dyn INotifyAttack>()
            .into_iter()
            .map(|a| (self_.clone(), a))
            .collect();
        *self.range_modifiers.borrow_mut() = self_.traits_implementing::<dyn IRangeModifier>();
        *self.reload_modifiers.borrow_mut() = self_.traits_implementing::<dyn IReloadModifier>();
        *self.damage_modifiers.borrow_mut() = self_.traits_implementing::<dyn IFirepowerModifier>();
        *self.inaccuracy_modifiers.borrow_mut() = self_.traits_implementing::<dyn IInaccuracyModifier>();
        conditional_created(self, self_);
    }
}

impl ITick for Armament {
    fn tick(&self, self_: &ActorRef) {
        self.update_condition(self_);
        if self.is_trait_disabled() {
            return;
        }
        if self.ticks_since_last_shot.get() < self.weapon.reload_delay {
            self.ticks_since_last_shot.set(self.ticks_since_last_shot.get() + 1);
        }
        if self.fire_delay.get() > 0 {
            self.fire_delay.set(self.fire_delay.get() - 1);
        }
        self.recoil
            .set(WDist::new(0.max(self.recoil.get().length - self.info.recoil_recovery.length)));
        let mut list = std::mem::take(&mut *self.delayed_actions.borrow_mut());
        for x in list.iter_mut() {
            x.0 -= 1;
            if x.0 <= 0 {
                (x.2)(x.1);
            }
        }
        list.retain(|a| a.0 > 0);
        let mut slot = self.delayed_actions.borrow_mut();
        list.append(&mut slot);
        *slot = list;
    }
}

impl Armament {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    /// The owning `Rc`, for keeping the armament in frame end tasks.
    pub fn me_rc(&self) -> Rc<Armament> {
        self.me.upgrade().expect("armament")
    }

    pub fn burst(&self) -> i32 {
        self.burst.get()
    }

    pub fn fire_delay(&self) -> i32 {
        self.fire_delay.get()
    }

    pub fn max_range(&self) -> WDist {
        WDist::new(util::apply_percentage_modifiers(
            self.weapon.range.length,
            self.range_modifiers.borrow().iter().map(|m| m.get_range_modifier()),
        ))
    }

    pub fn add_notify_attacks(&self, attacker: &ActorRef, list: &[Rc<dyn INotifyAttack>]) {
        self.notify_attacks
            .borrow_mut()
            .extend(list.iter().map(|a| (attacker.clone(), a.clone())));
    }

    pub fn remove_notify_attacks(&self, list: &[Rc<dyn INotifyAttack>]) {
        self.notify_attacks
            .borrow_mut()
            .retain(|(_, n)| !list.iter().any(|x| Rc::ptr_eq(x, n)));
    }

    fn update_condition(&self, self_: &ActorRef) {
        let Some(c) = self.info.reloading_condition.as_deref().filter(|c| !c.is_empty()) else {
            return;
        };
        let enabled = !self.is_trait_disabled() && self.is_reloading();
        if enabled && self.condition_token.get() == INVALID_CONDITION_TOKEN {
            self.condition_token.set(self_.grant_condition(c));
        } else if !enabled && self.condition_token.get() != INVALID_CONDITION_TOKEN {
            self.condition_token.set(self_.revoke_condition(self.condition_token.get()));
        }
    }

    fn schedule_delayed_action(&self, t: i32, b: i32, a: Box<dyn Fn(i32)>) {
        if t > 0 {
            self.delayed_actions.borrow_mut().push((t, b, a));
        } else {
            a(b);
        }
    }

    fn can_fire(&self, self_: &ActorRef, target: &Target) -> bool {
        if self.is_reloading() || self.is_trait_paused() {
            return false;
        }
        if self
            .turret
            .get()
            .and_then(|t| t.as_ref())
            .is_some_and(|t| !t.has_achieved_desired_facing())
        {
            return false;
        }
        let pos = self_.center_position();
        if !target.is_in_range(pos, self.max_range())
            || (self.weapon.min_range != WDist::ZERO && target.is_in_range(pos, self.weapon.min_range))
        {
            return false;
        }
        self.weapon.is_valid_against(target, &self_.world(), self_)
    }

    pub fn check_fire(&self, self_: &ActorRef, target: &Target) -> bool {
        if !self.can_fire(self_, target) {
            return false;
        }
        if self.ticks_since_last_shot.get() >= self.weapon.reload_delay {
            self.burst.set(self.weapon.burst);
        }
        self.ticks_since_last_shot.set(0);
        loop {
            self.current_barrel.set(self.current_barrel.get() % self.barrels.len());
            let barrel = if self.weapon.burst == 1 {
                self.barrels[self.current_barrel.get()].clone()
            } else {
                self.barrels[(self.burst.get() as usize) % self.barrels.len()].clone()
            };
            self.current_barrel.set(self.current_barrel.get() + 1);
            self.fire_barrel(self_, target, barrel);
            self.update_burst(self_, target);
            if !(self.fire_delay.get() == 0 && self.can_fire(self_, target)) {
                break;
            }
        }
        true
    }

    fn fire_barrel(&self, self_: &ActorRef, target: &Target, barrel: Barrel) {
        let notifies = self.notify_attacks.borrow().clone();
        for (a, n) in &notifies {
            n.preparing_attack(a, target, self, &barrel);
        }
        let me = self.me.upgrade().expect("armament");
        let muzzle_position: Rc<dyn Fn() -> WPos> = {
            let (m, s, b) = (me.clone(), self_.clone(), barrel.clone());
            Rc::new(move || s.center_position() + m.muzzle_offset(&s, &b))
        };
        let muzzle_facing: Rc<dyn Fn() -> WAngle> = {
            let (m, s, b) = (me.clone(), self_.clone(), barrel.clone());
            Rc::new(move || m.muzzle_orientation(&s, &b).yaw)
        };
        let muzzle_orientation = WRot::from_yaw(muzzle_facing());
        let mut passive_target = if self.weapon.target_actor_center {
            target.center_position()
        } else {
            util::closest_position(&target.positions(), muzzle_position()).unwrap_or(WPos::ZERO)
        };
        let initial = self.weapon.first_burst_target_offset;
        if initial != WVec::ZERO {
            passive_target = passive_target + WVec::new(initial.y, -initial.x, initial.z).rotate(&muzzle_orientation);
        }
        let following = self.weapon.following_burst_target_offset;
        if following != WVec::ZERO {
            let f = WVec::new(following.y, -following.x, following.z);
            passive_target = passive_target + (f * (self.weapon.burst - self.burst.get())).rotate(&muzzle_orientation);
        }
        let make_args = {
            let weapon = self.weapon.clone();
            let damage: Vec<i32> = self.damage_modifiers.borrow().iter().map(|m| m.get_firepower_modifier()).collect();
            let inaccuracy: Vec<i32> = self
                .inaccuracy_modifiers
                .borrow()
                .iter()
                .map(|m| m.get_inaccuracy_modifier())
                .collect();
            let range: Vec<i32> = self.range_modifiers.borrow().iter().map(|m| m.get_range_modifier()).collect();
            let facing = muzzle_facing();
            let source = muzzle_position();
            let (cur_f, cur_s) = (muzzle_facing.clone(), muzzle_position.clone());
            let (s, t) = (self_.clone(), target.clone());
            move || ProjectileArgs {
                weapon: weapon.clone(),
                damage_modifiers: damage.clone(),
                inaccuracy_modifiers: inaccuracy.clone(),
                range_modifiers: range.clone(),
                facing,
                current_muzzle_facing: cur_f.clone(),
                source,
                current_source: cur_s.clone(),
                source_actor: s.clone(),
                passive_target,
                guided_target: t.clone(),
            }
        };
        let delayed_target = target.clone();
        let (m, s) = (me, self_.clone());
        self.schedule_delayed_action(
            self.info.fire_delay,
            self.burst.get(),
            Box::new(move |burst| {
                let args = make_args();
                if let Some(p) = &args.weapon.projectile.clone()
                    && let Some(projectile) = p.create(args)
                {
                    s.world().add_effect(projectile);
                    let world = s.world();
                    let center = s.center_position();
                    sound::play_random_at(&world, SoundType::World, &m.weapon.report, center, 1.0);
                    if burst == m.weapon.burst {
                        sound::play_random_at(&world, SoundType::World, &m.weapon.start_burst_report, center, 1.0);
                    }
                }
                m.recoil.set(m.info.recoil);
                let notifies = m.notify_attacks.borrow().clone();
                for (a, n) in &notifies {
                    n.attacking(a, &delayed_target, &m, &barrel);
                }
            }),
        );
    }

    fn update_burst(&self, self_: &ActorRef, target: &Target) {
        self.burst.set(self.burst.get() - 1);
        if self.burst.get() > 0 {
            if self.weapon.burst_delays.len() == 1 {
                self.fire_delay.set(self.weapon.burst_delays[0]);
            } else {
                self.fire_delay
                    .set(self.weapon.burst_delays[(self.weapon.burst - (self.burst.get() + 1)) as usize]);
            }
        } else {
            let d = util::apply_percentage_modifiers(
                self.weapon.reload_delay,
                self.reload_modifiers.borrow().iter().map(|m| m.get_reload_modifier()),
            );
            self.fire_delay.set(if d <= 0 { 1 } else { d });
            self.burst.set(self.weapon.burst);
            if !self.weapon.after_fire_sound.is_empty() {
                let s = self_.clone();
                let weapon = self.weapon.clone();
                self.schedule_delayed_action(
                    weapon.after_fire_sound_delay,
                    self.burst.get(),
                    Box::new(move |_burst| {
                        sound::play_random_at(&s.world(), SoundType::World, &weapon.after_fire_sound, s.center_position(), 1.0);
                    }),
                );
            }
            let list = self.notify_burst_complete.borrow().clone();
            for nbc in list {
                nbc.fired_burst(self_, target, self);
            }
        }
    }

    pub fn is_reloading(&self) -> bool {
        self.fire_delay.get() > 0 || self.is_trait_disabled()
    }

    pub fn muzzle_offset(&self, self_: &ActorRef, b: &Barrel) -> WVec {
        let mut local = b.offset + WVec::new(-self.recoil.get().length, 0, 0);
        if let Some(h) = self.hovers.get().and_then(|h| h.as_ref()) {
            local = local + h.world_visual_offset();
        }
        let coords = self.coords.get().expect("coords");
        let body = coords.quantize_orientation(self_.orientation());
        local = match self.turret.get().and_then(|t| t.as_ref()) {
            Some(t) => local.rotate(&t.world_orientation()) + t.offset().rotate(&body),
            None => local.rotate(&body),
        };
        coords.local_to_world(local)
    }

    pub fn muzzle_orientation(&self, self_: &ActorRef, b: &Barrel) -> WRot {
        let base = self
            .turret
            .get()
            .and_then(|t| t.as_ref())
            .map_or_else(|| self_.orientation(), |t| t.world_orientation());
        WRot::from_yaw(b.yaw).rotate(&base)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttackSource {
    #[default]
    Default,
    AutoTarget,
    AttackMove,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttackKind {
    #[default]
    Frontal,
    Omni,
    Follow,
    Turreted,
    Charges,
    Garrisoned,
    Aircraft,
    Bomber,
    Leap,
    Tesla,
}

impl AttackKind {
    fn class_name(self) -> &'static str {
        match self {
            AttackKind::Frontal => "AttackFrontal",
            AttackKind::Omni => "AttackOmni",
            AttackKind::Follow => "AttackFollow",
            AttackKind::Turreted => "AttackTurreted",
            AttackKind::Charges => "AttackCharges",
            AttackKind::Garrisoned => "AttackGarrisoned",
            AttackKind::Aircraft => "AttackAircraft",
            AttackKind::Bomber => "AttackBomber",
            AttackKind::Leap => "AttackLeap",
            AttackKind::Tesla => "AttackTesla",
        }
    }

    fn is_follow(self) -> bool {
        matches!(
            self,
            AttackKind::Follow | AttackKind::Turreted | AttackKind::Garrisoned | AttackKind::Aircraft
        )
    }

    fn is_frontal(self) -> bool {
        matches!(self, AttackKind::Frontal | AttackKind::Leap)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AirAttackType {
    #[default]
    Default,
    Hover,
    Strafe,
}

rustora_core::field_enum!(AirAttackType, [Default, Hover, Strafe]);

fn load_attack(node: &TraitNode, kind: AttackKind) -> Result<Rc<dyn TraitInfo>, String> {
    let mut info = AttackBaseInfo::default();
    rustora_core::fieldloader::FieldLoad::load(&mut info, &node.yaml).map_err(|e| format!("Trait name {}: {}", node.display_name(), e))?;
    info.instance_name = node.instance_name.clone();
    info.kind = kind;
    Ok(Rc::new(info))
}

/// Union of `AttackBaseInfo` and the info fields of its subclasses.
#[derive(Clone, Debug, FieldLoad)]
pub struct AttackBaseInfo {
    pub voice: String,
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(ignore)]
    pub kind: AttackKind,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub armaments: IndexSet<String>,
    pub attack_requires_entering_cell: bool,
    pub target_frozen_actors: bool,
    pub force_fire_ignores_actors: bool,
    pub outside_range_requires_force_fire: bool,
    pub facing_tolerance: WAngle,
    pub target_terrain_without_force_fire: bool,
    pub opportunity_fire: bool,
    pub persistent_targeting: bool,
    pub range_margin: WDist,
    pub abort_on_resupply: bool,
    pub turrets: Vec<String>,
    pub charge_level: i32,
    pub charge_rate: i32,
    pub discharge_rate: i32,
    pub charging_condition: Option<String>,
    pub port_offsets: Vec<WVec>,
    pub port_yaws: Vec<WAngle>,
    pub port_cones: Vec<WAngle>,
    pub attack_type: AirAttackType,
    pub strafe_run_length: WDist,
    pub speed: WDist,
    pub leap_condition: Option<String>,
    pub max_charges: i32,
    pub reload_delay: i32,
    pub initial_charge_delay: i32,
    pub charge_delay: i32,
    pub cursor: Option<String>,
    pub outside_range_cursor: Option<String>,
}

impl Default for AttackBaseInfo {
    fn default() -> Self {
        AttackBaseInfo {
            voice: "Action".into(),
            instance_name: None,
            kind: AttackKind::Frontal,
            base: PausableConditionalTraitInfo::default(),
            armaments: IndexSet::from(["primary".to_string(), "secondary".to_string()]),
            attack_requires_entering_cell: false,
            target_frozen_actors: false,
            force_fire_ignores_actors: false,
            outside_range_requires_force_fire: false,
            facing_tolerance: WAngle::new(512),
            target_terrain_without_force_fire: false,
            opportunity_fire: true,
            persistent_targeting: true,
            range_margin: WDist::from_cells(1),
            abort_on_resupply: true,
            turrets: vec!["primary".to_string()],
            charge_level: 25,
            charge_rate: 1,
            discharge_rate: 1,
            charging_condition: None,
            port_offsets: Vec::new(),
            port_yaws: Vec::new(),
            port_cones: Vec::new(),
            attack_type: AirAttackType::Default,
            strafe_run_length: WDist::ZERO,
            speed: WDist::new(426),
            leap_condition: Some("attacking".to_string()),
            max_charges: 1,
            reload_delay: 120,
            initial_charge_delay: 22,
            charge_delay: 3,
            cursor: None,
            outside_range_cursor: None,
        }
    }
}

pub struct AttackBase {
    pub info: Rc<AttackBaseInfo>,
    state: ConditionalState,
    self_: ActorRef,
    me: Weak<AttackBase>,
    is_aiming: Cell<bool>,
    facing: OnceCell<Option<Rc<dyn IFacing>>>,
    positionable: OnceCell<Option<Rc<dyn IPositionable>>>,
    notify_aiming: RefCell<Vec<Rc<dyn INotifyAiming>>>,
    armaments: RefCell<Vec<Rc<Armament>>>,
    was_aiming: Cell<bool>,
    // AttackFollow
    requested_target: RefCell<Target>,
    opportunity_target: RefCell<Target>,
    mobile: OnceCell<Option<Rc<Mobile>>>,
    auto_target: OnceCell<Option<Rc<AutoTarget>>>,
    requested_force_attack: Cell<bool>,
    requested_target_preset: RefCell<Option<ActivityRef>>,
    opportunity_force_attack: Cell<bool>,
    opportunity_target_is_persistent: Cell<bool>,
    turrets: RefCell<Vec<Rc<Turreted>>>,
    // AttackCharges
    charging_token: Cell<i32>,
    charging: Cell<bool>,
    charge_level: Cell<i32>,
    // AttackGarrisoned
    notify_attacks: RefCell<Vec<Rc<dyn INotifyAttack>>>,
    pax: RefCell<IndexMap<u32, (Rc<dyn IFacing>, Rc<dyn IPositionable>)>>,
    // AttackBomber
    bomber_target: RefCell<Target>,
    in_attack_range: Cell<bool>,
    facing_target: Cell<bool>,
    pub on_removed_from_world: RefCell<Vec<Rc<dyn Fn(&ActorRef)>>>,
    pub on_entered_attack_range: RefCell<Vec<Rc<dyn Fn(&ActorRef)>>>,
    pub on_exited_attack_range: RefCell<Vec<Rc<dyn Fn(&ActorRef)>>>,
    // AttackLeap
    leap_token: Cell<i32>,
    // AttackTesla
    charges: Cell<i32>,
    time_to_recharge: Cell<i32>,
}

impl TraitObject for AttackBase {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<AttackBase>(actor, self.clone());
        dict.add::<dyn ITick>(actor, self.clone());
        dict.add::<dyn IResolveOrder>(actor, self.clone());
        dict.add::<dyn sound::IOrderVoice>(actor, self.clone());
        dict.add::<dyn crate::orders::IIssueOrder>(actor, self.clone());
        dict.add::<dyn INotifyCreated>(actor, self.clone());
        dict.add::<dyn IObservesVariables>(actor, self.clone());
        dict.add::<dyn IDisabledTrait>(actor, self.clone());
        match self.info.kind {
            AttackKind::Charges | AttackKind::Tesla => dict.add::<dyn INotifyAttack>(actor, self.clone()),
            AttackKind::Bomber => dict.add::<dyn INotifyRemovedFromWorld>(actor, self.clone()),
            _ => {}
        }
        if self.info.kind == AttackKind::Charges {
            dict.add::<dyn INotifySold>(actor, self.clone());
        }
        if self.info.kind == AttackKind::Garrisoned {
            dict.add::<dyn INotifyPassengerEntered>(actor, self.clone());
            dict.add::<dyn INotifyPassengerExited>(actor, self.clone());
        }
        if self.info.kind.is_follow() {
            dict.add::<dyn INotifyOwnerChanged>(actor, self.clone());
            dict.add::<dyn IOverrideAutoTarget>(actor, self.clone());
            dict.add::<dyn INotifyStanceChanged>(actor, self.clone());
        }
        dict.add::<dyn ISync>(actor, self);
    }

    fn type_name(&self) -> &'static str {
        self.info.kind.class_name()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

info_impl!(AttackBaseInfo => |info, init| {
    let state = ConditionalState::new_pausable(&info.base);
    let self_ = init.self_.clone();
    let turrets = if info.kind == AttackKind::Turreted {
        self_.traits_implementing::<Turreted>().into_iter().filter(|t| info.turrets.contains(&t.info.turret)).collect()
    } else {
        Vec::new()
    };
    let charges = info.max_charges;
    Some(Rc::new_cyclic(|me| AttackBase {
        state,
        self_,
        me: me.clone(),
        is_aiming: Cell::new(false),
        facing: OnceCell::new(),
        positionable: OnceCell::new(),
        notify_aiming: RefCell::new(Vec::new()),
        armaments: RefCell::new(Vec::new()),
        was_aiming: Cell::new(false),
        requested_target: RefCell::new(Target::Invalid),
        opportunity_target: RefCell::new(Target::Invalid),
        mobile: OnceCell::new(),
        auto_target: OnceCell::new(),
        requested_force_attack: Cell::new(false),
        requested_target_preset: RefCell::new(None),
        opportunity_force_attack: Cell::new(false),
        opportunity_target_is_persistent: Cell::new(false),
        turrets: RefCell::new(turrets),
        charging_token: Cell::new(INVALID_CONDITION_TOKEN),
        charging: Cell::new(false),
        charge_level: Cell::new(0),
        notify_attacks: RefCell::new(Vec::new()),
        pax: RefCell::new(IndexMap::new()),
        bomber_target: RefCell::new(Target::Invalid),
        in_attack_range: Cell::new(false),
        facing_target: Cell::new(true),
        on_removed_from_world: RefCell::new(Vec::new()),
        on_entered_attack_range: RefCell::new(Vec::new()),
        on_exited_attack_range: RefCell::new(Vec::new()),
        leap_token: Cell::new(INVALID_CONDITION_TOKEN),
        charges: Cell::new(charges),
        time_to_recharge: Cell::new(0),
        info,
    }))
});

impl ConditionalHooks for AttackBase {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
rustora_engine::conditional_observers_impl!(AttackBase);

impl ISync for AttackBase {
    fn sync_hash(&self) -> i32 {
        let mut h = self.state.sync_hash() ^ sync_bool(self.is_aiming.get());
        if self.info.kind == AttackKind::Bomber {
            h ^= self.bomber_target.borrow().sync_hash() ^ sync_bool(self.in_attack_range.get()) ^ sync_bool(self.facing_target.get());
        }
        if self.info.kind == AttackKind::Tesla {
            h ^= self.charges.get() ^ self.time_to_recharge.get();
        }
        h
    }
}

impl INotifyCreated for AttackBase {
    fn created(&self, self_: &ActorRef) {
        if self.info.kind == AttackKind::Garrisoned {
            *self.notify_attacks.borrow_mut() = self_.traits_implementing::<dyn INotifyAttack>();
        }
        if self.info.kind.is_follow() {
            let _ = self.mobile.set(self_.trait_or_default::<Mobile>());
            let _ = self.auto_target.set(self_.trait_or_default::<AutoTarget>());
        }
        let _ = self.facing.set(self_.trait_or_default::<dyn IFacing>());
        let _ = self.positionable.set(self_.trait_or_default::<dyn IPositionable>());
        *self.notify_aiming.borrow_mut() = self_.traits_implementing::<dyn INotifyAiming>();
        if self.info.kind != AttackKind::Garrisoned {
            *self.armaments.borrow_mut() = self_
                .traits_implementing::<Armament>()
                .into_iter()
                .filter(|a| self.info.armaments.contains(&a.info.name))
                .collect();
        }
        conditional_created(self, self_);
    }
}

impl ITick for AttackBase {
    fn tick(&self, self_: &ActorRef) {
        match self.info.kind {
            AttackKind::Bomber => return self.tick_bomber(self_),
            AttackKind::Tesla => {
                self.time_to_recharge.set(self.time_to_recharge.get() - 1);
                if self.time_to_recharge.get() <= 0 {
                    self.charges.set(self.info.max_charges);
                }
                return;
            }
            AttackKind::Charges => {
                self.charging
                    .set(self.charging.get() && self_.current_activity().is_some_and(|c| c.type_name() == "SetTarget"));
                let delta = if self.charging.get() {
                    self.info.charge_rate
                } else {
                    -self.info.discharge_rate
                };
                self.charge_level
                    .set((self.charge_level.get() + delta).clamp(0, self.info.charge_level));
                if self.charge_level.get() > 0 && self.charging_token.get() == INVALID_CONDITION_TOKEN {
                    self.charging_token
                        .set(self_.grant_condition(self.info.charging_condition.as_deref().unwrap_or("")));
                }
                if self.charge_level.get() == 0 && self.charging_token.get() != INVALID_CONDITION_TOKEN {
                    self.charging_token.set(self_.revoke_condition(self.charging_token.get()));
                }
            }
            k if k.is_follow() && self.tick_follow(self_) => {
                return;
            }
            _ => {}
        }
        let list = self.notify_aiming.borrow().clone();
        if !self.was_aiming.get() && self.is_aiming.get() {
            for n in &list {
                n.started_aiming(self_, self);
            }
        } else if self.was_aiming.get() && !self.is_aiming.get() {
            for n in &list {
                n.stopped_aiming(self_, self);
            }
        }
        self.was_aiming.set(self.is_aiming.get());
    }
}

impl AttackBase {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    pub fn is_aiming(&self) -> bool {
        self.is_aiming.get()
    }

    pub fn set_is_aiming(&self, v: bool) {
        self.is_aiming.set(v);
    }

    pub fn armaments(&self) -> Vec<Rc<Armament>> {
        self.armaments.borrow().clone()
    }

    pub fn is_turreted_with(&self, turret: &str) -> bool {
        self.info.kind == AttackKind::Turreted && self.info.turrets.iter().any(|t| t == turret)
    }

    pub fn requested_target(&self) -> Target {
        self.requested_target.borrow().clone()
    }

    /// True when `AttackFollow.Tick` returned early, skipping the base tick.
    fn tick_follow(&self, self_: &ActorRef) -> bool {
        if self.is_trait_disabled() {
            *self.requested_target.borrow_mut() = Target::Invalid;
            *self.opportunity_target.borrow_mut() = Target::Invalid;
            self.opportunity_target_is_persistent.set(false);
        }
        let preset = self.requested_target_preset.borrow().clone();
        if let Some(p) = preset {
            let is_next = self_
                .current_activity()
                .and_then(|c| c.base().next_activity())
                .is_some_and(|n| Rc::ptr_eq(&n, &p));
            if is_next {
                let t = self.requested_target.borrow().recalculate(&self_.owner()).0;
                *self.requested_target.borrow_mut() = t;
            } else {
                self.clear_requested_target();
            }
        }
        if self
            .mobile
            .get()
            .and_then(|m| m.as_ref())
            .is_some_and(|m| !m.can_interact_with_ground_layer(self_))
        {
            return true;
        }
        let requested = self.requested_target();
        if requested.is_valid_for(self_) {
            self.is_aiming
                .set(self.can_aim_at_target(self_, &requested, self.requested_force_attack.get()));
            if self.is_aiming.get() {
                self.do_attack(self_, &requested);
            }
        } else {
            self.is_aiming.set(false);
            let opp = self.opportunity_target.borrow().clone();
            if opp.is_valid_for(self_) {
                self.is_aiming
                    .set(self.can_aim_at_target(self_, &opp, self.opportunity_force_attack.get()));
            }
            if !self.is_aiming.get()
                && self.info.opportunity_fire
                && let Some(at) = self.auto_target.get().and_then(|a| a.as_ref())
                && !at.is_trait_disabled()
                && at.stance() >= UnitStance::Defend
            {
                let t = at.scan_for_target(self_, false, false, false);
                *self.opportunity_target.borrow_mut() = t.clone();
                self.opportunity_force_attack.set(false);
                self.opportunity_target_is_persistent.set(false);
                if t.is_valid_for(self_) {
                    self.is_aiming
                        .set(self.can_aim_at_target(self_, &t, self.opportunity_force_attack.get()));
                }
            }
            if self.is_aiming.get() {
                let t = self.opportunity_target.borrow().clone();
                self.do_attack(self_, &t);
            }
        }
        false
    }

    fn tick_bomber(&self, self_: &ActorRef) {
        let was_in_attack_range = self.in_attack_range.get();
        self.in_attack_range.set(false);
        if self_.is_in_world() {
            let w = self_.world();
            let c = self.bomber_target.borrow().center_position();
            let dat = w.map().distance_above_terrain(c);
            let target = Target::from_pos(c - WVec::from_dists(WDist::ZERO, WDist::ZERO, dat));
            *self.bomber_target.borrow_mut() = target.clone();
            let was_facing_target = self.facing_target.get();
            self.facing_target
                .set(self.target_in_firing_arc(self_, &target, self.info.facing_tolerance));
            let armaments = self.armaments();
            for a in &armaments {
                if !target.is_in_range(self_.center_position(), a.max_range()) {
                    continue;
                }
                self.in_attack_range.set(true);
                a.check_fire(self_, &target);
            }
            if armaments.is_empty() {
                self.in_attack_range
                    .set(!was_in_attack_range && !self.facing_target.get() && was_facing_target);
            }
        }
        if self.in_attack_range.get() && !was_in_attack_range {
            let e = self.on_entered_attack_range.borrow().clone();
            e.iter().for_each(|f| f(self_));
        }
        if !self.in_attack_range.get() && was_in_attack_range {
            let e = self.on_exited_attack_range.borrow().clone();
            e.iter().for_each(|f| f(self_));
        }
    }

    pub fn set_bomber_target(&self, pos: WPos) {
        *self.bomber_target.borrow_mut() = Target::from_pos(pos);
    }

    pub fn set_requested_target(&self, target: &Target, is_force_attack: bool, preset: Option<ActivityRef>) {
        *self.requested_target.borrow_mut() = target.clone();
        self.requested_force_attack.set(is_force_attack);
        *self.requested_target_preset.borrow_mut() = preset;
    }

    pub fn clear_requested_target(&self) {
        if self.info.persistent_targeting {
            *self.opportunity_target.borrow_mut() = self.requested_target();
            self.opportunity_force_attack.set(self.requested_force_attack.get());
            self.opportunity_target_is_persistent.set(true);
        }
        *self.requested_target.borrow_mut() = Target::Invalid;
        *self.requested_target_preset.borrow_mut() = None;
    }

    fn can_aim_at_target(&self, self_: &ActorRef, target: &Target, force_attack: bool) -> bool {
        if target.target_type() == TargetType::Actor && !target.actor().unwrap().can_be_viewed_by_player(Some(&self_.owner())) {
            return false;
        }
        if target.target_type() == TargetType::FrozenActor && !target.frozen_actor().unwrap().is_valid() {
            return false;
        }
        let pos = self_.center_position();
        for a in self.choose_armaments_for_target(target, force_attack) {
            if target.is_in_range(pos, a.max_range())
                && (a.weapon.min_range == WDist::ZERO || !target.is_in_range(pos, a.weapon.min_range))
                && self.target_in_firing_arc(self_, target, self.info.facing_tolerance)
            {
                return true;
            }
        }
        false
    }

    pub fn target_in_firing_arc(&self, self_: &ActorRef, target: &Target, tolerance: WAngle) -> bool {
        let Some(facing) = self.facing.get().and_then(|f| f.as_ref()) else {
            return true;
        };
        let pos = self_.center_position();
        let delta = self.get_target_position(pos, target) - pos;
        if delta.horizontal_length_squared() == 0 {
            return true;
        }
        util::facing_within_tolerance(facing.facing(), delta.yaw(), tolerance)
    }

    /// The base `AttackBase.CanAttack`.
    fn can_attack_base(&self, self_: &ActorRef, target: &Target) -> bool {
        if !self_.is_in_world()
            || self.is_trait_disabled()
            || self.is_trait_paused()
            || !target.is_valid_for(self_)
            || !self.has_any_valid_weapons(target, false, true)
        {
            return false;
        }
        !self.positionable.get().and_then(|p| p.as_ref()).is_some()
            || self_
                .trait_or_default::<Mobile>()
                .is_none_or(|m| m.can_interact_with_ground_layer(self_))
    }

    pub fn can_attack(&self, self_: &ActorRef, target: &Target) -> bool {
        match self.info.kind {
            AttackKind::Frontal => self.can_attack_frontal(self_, target),
            AttackKind::Leap => {
                if target.target_type() != TargetType::Actor {
                    return false;
                }
                if self_.location() == target.actor().unwrap().location() && self.has_any_valid_weapons(target, false, false) {
                    return true;
                }
                self.can_attack_frontal(self_, target)
            }
            AttackKind::Turreted => {
                if target.target_type() == TargetType::Invalid {
                    return false;
                }
                let mut ready = false;
                for t in self.turrets.borrow().iter() {
                    if t.face_target(self_, target) {
                        ready = true;
                    }
                }
                ready && self.can_attack_base(self_, target)
            }
            AttackKind::Charges => {
                self.charging
                    .set(self.can_attack_base(self_, target) && self.is_reachable_target(target, true));
                self.charge_level.get() >= self.info.charge_level && self.charging.get()
            }
            AttackKind::Aircraft => {
                let w = self_.world();
                let min_alt = self_.info.trait_info::<AircraftInfo>().expect("aircraft").min_airborne_altitude;
                if w.map().distance_above_terrain(self_.center_position()).length < min_alt || !w.map().contains(self_.location()) {
                    return false;
                }
                self.can_attack_base(self_, target) && self.target_in_firing_arc(self_, target, self.info.facing_tolerance)
            }
            AttackKind::Tesla => self.is_reachable_target(target, true) && self.can_attack_base(self_, target),
            _ => self.can_attack_base(self_, target),
        }
    }

    fn can_attack_frontal(&self, self_: &ActorRef, target: &Target) -> bool {
        self.can_attack_base(self_, target) && self.target_in_firing_arc(self_, target, self.info.facing_tolerance)
    }

    pub fn do_attack(&self, self_: &ActorRef, target: &Target) {
        if !self.can_attack(self_, target) {
            return;
        }
        if self.info.kind == AttackKind::Garrisoned {
            return self.do_attack_garrisoned(self_, target);
        }
        for a in self.armaments() {
            a.check_fire(self_, target);
        }
    }

    fn select_fire_port(&self, self_: &ActorRef, target_yaw: WAngle) -> Option<usize> {
        let body_yaw = self.facing.get().and_then(|f| f.as_ref()).map_or(WAngle::ZERO, |f| f.facing());
        let mut indices: Vec<usize> = (0..self.info.port_offsets.len()).collect();
        let w = self_.world();
        let n = indices.len();
        for i in 0..n {
            let j = i + w.shared_random.borrow_mut().next_max((n - i) as i32) as usize;
            indices.swap(i, j);
            let idx = indices[i];
            let yaw = body_yaw + self.info.port_yaws[idx];
            let left = (yaw - target_yaw).angle;
            let right = (target_yaw - yaw).angle;
            if left.min(right) <= self.info.port_cones[idx].angle {
                return Some(idx);
            }
        }
        None
    }

    fn do_attack_garrisoned(&self, self_: &ActorRef, target: &Target) {
        let pos = self_.center_position();
        let target_yaw = (self.get_target_position(pos, target) - pos).yaw();
        let coords = self_.trait_::<BodyOrientation>();
        for a in self.armaments() {
            if a.is_trait_disabled() {
                continue;
            }
            let Some(port) = self.select_fire_port(self_, target_yaw) else {
                return;
            };
            let (f, p) = self.pax.borrow().get(&a.actor.id).cloned().expect("passenger entry");
            f.set_facing(target_yaw);
            let body = coords.quantize_orientation(self_.orientation());
            let offset = coords.local_to_world(self.info.port_offsets[port].rotate(&body));
            p.set_center_position(&a.actor, pos + offset);
            a.check_fire(&a.actor, target);
        }
    }

    pub fn on_stop_order(&self, self_: &ActorRef) {
        if self.info.kind.is_follow() {
            *self.requested_target.borrow_mut() = Target::Invalid;
            *self.opportunity_target.borrow_mut() = Target::Invalid;
            self.opportunity_target_is_persistent.set(false);
        }
        let Some(current) = self_.current_activity() else { return };
        if matches!(current.type_name(), "Resupply" | "ReturnToBase") {
            return;
        }
        self_.cancel_activity();
    }

    pub fn get_attack_activity(
        &self,
        self_: &ActorRef,
        source: AttackSource,
        target: &Target,
        allow_move: bool,
        force_attack: bool,
    ) -> ActivityRef {
        let me = self.me.upgrade().expect("attack");
        match self.info.kind {
            AttackKind::Frontal => Attack::new(self_, target, allow_move, force_attack),
            AttackKind::Omni | AttackKind::Charges => Rc::new(SetTarget {
                base: ActivityBase::default(),
                attack: me,
                allow_move,
                force_attack,
                target: RefCell::new(target.clone()),
            }),
            AttackKind::Follow | AttackKind::Turreted | AttackKind::Garrisoned => {
                let force = force_attack
                    || (self.opportunity_target_is_persistent.get()
                        && self.opportunity_force_attack.get()
                        && self.opportunity_target.borrow().same_as(target));
                AttackActivity::new(self_, me, source, target, allow_move, force)
            }
            AttackKind::Aircraft => FlyAttack::new(self_, me, source, target, force_attack),
            AttackKind::Bomber => panic!("AttackBomber requires a scripted target"),
            AttackKind::Leap => LeapAttack::new(self_, target, allow_move, force_attack, me),
            AttackKind::Tesla => Rc::new(ChargeAttack {
                base: ActivityBase::default(),
                attack: me,
                target: target.clone(),
                force_attack,
            }),
        }
    }

    pub fn has_any_valid_weapons(&self, t: &Target, check_for_center_targeting_weapons: bool, reloading_is_invalid: bool) -> bool {
        if self.is_trait_disabled() {
            return false;
        }
        if self.info.attack_requires_entering_cell {
            let can_enter = self
                .positionable
                .get()
                .and_then(|p| p.as_ref())
                .is_some_and(|p| p.can_enter_cell(t.actor().expect("target actor").location(), None, BlockedByActor::None));
            if !can_enter {
                return false;
            }
        }
        let w = self.self_.world();
        self.armaments.borrow().iter().any(|a| {
            let check = if check_for_center_targeting_weapons {
                a.weapon.target_actor_center
            } else {
                !a.is_trait_paused()
            };
            check && (!reloading_is_invalid || !a.is_reloading()) && !a.is_trait_disabled() && a.weapon.is_valid_against(t, &w, &self.self_)
        })
    }

    pub fn get_target_position(&self, pos: WPos, target: &Target) -> WPos {
        if self.has_any_valid_weapons(target, true, false) {
            target.center_position()
        } else {
            util::closest_position(&target.positions(), pos).unwrap_or(WPos::ZERO)
        }
    }

    pub fn get_minimum_range(&self) -> WDist {
        if self.is_trait_disabled() {
            return WDist::ZERO;
        }
        self.armaments
            .borrow()
            .iter()
            .filter(|a| !a.is_trait_disabled() && !a.is_trait_paused())
            .map(|a| a.weapon.min_range)
            .min()
            .unwrap_or(WDist::ZERO)
    }

    pub fn get_maximum_range(&self) -> WDist {
        if self.is_trait_disabled() {
            return WDist::ZERO;
        }
        self.armaments
            .borrow()
            .iter()
            .filter(|a| !a.is_trait_disabled() && !a.is_trait_paused())
            .map(|a| a.max_range())
            .fold(WDist::ZERO, WDist::max)
    }

    pub fn get_minimum_range_versus_target(&self, target: &Target) -> WDist {
        if self.is_trait_disabled() {
            return WDist::ZERO;
        }
        let w = self.self_.world();
        self.armaments
            .borrow()
            .iter()
            .filter(|a| !a.is_trait_disabled() && !a.is_trait_paused() && a.weapon.is_valid_against(target, &w, &self.self_))
            .map(|a| a.weapon.min_range)
            .min()
            .unwrap_or(WDist::ZERO)
    }

    pub fn get_maximum_range_versus_target(&self, target: &Target) -> WDist {
        if self.is_trait_disabled() {
            return WDist::ZERO;
        }
        let w = self.self_.world();
        let (mut max, mut fallback) = (WDist::ZERO, WDist::ZERO);
        for a in self.armaments.borrow().iter() {
            if a.is_trait_disabled() || !a.weapon.is_valid_against(target, &w, &self.self_) {
                continue;
            }
            let range = a.max_range();
            fallback = fallback.max(range);
            if !a.is_trait_paused() {
                max = max.max(range);
            }
        }
        if max != WDist::ZERO { max } else { fallback }
    }

    pub fn choose_armaments_for_target(&self, t: &Target, force_attack: bool) -> Vec<Rc<Armament>> {
        let tt = t.target_type();
        if !force_attack
            && ((tt == TargetType::Terrain && !self.info.target_terrain_without_force_fire)
                || tt == TargetType::Invalid
                || t.requires_force_fire())
        {
            return Vec::new();
        }
        let owner = match tt {
            TargetType::FrozenActor => t.frozen_actor().unwrap().owner(),
            TargetType::Actor => Some(t.actor().unwrap().owner()),
            _ => None,
        };
        let w = self.self_.world();
        let me = self.self_.owner();
        self.armaments
            .borrow()
            .iter()
            .filter(|a| {
                !a.is_trait_disabled()
                    && owner.as_ref().is_none_or(|o| {
                        let r = if force_attack {
                            a.info.force_target_relationships
                        } else {
                            a.info.target_relationships
                        };
                        r.has_relationship(me.relationship_with(Some(o)))
                    })
                    && a.weapon.is_valid_against(t, &w, &self.self_)
            })
            .cloned()
            .collect()
    }

    pub fn attack_target(
        &self,
        target: &Target,
        source: AttackSource,
        queued: bool,
        allow_move: bool,
        force_attack: bool,
        line_color: Option<Color>,
    ) {
        let self_ = self.self_.clone();
        if self.is_trait_disabled() || !target.is_valid_for(&self_) {
            return;
        }
        let activity = self.get_attack_activity(&self_, source, target, allow_move, force_attack);
        activity.base().target_line_color.set(line_color);
        self_.queue_activity_queued(queued, activity.clone());
        if self.info.kind.is_follow() && !queued {
            self.set_requested_target(target, force_attack, Some(activity));
        }
    }

    pub fn is_reachable_target(&self, target: &Target, allow_move: bool) -> bool {
        self.has_any_valid_weapons(target, false, false)
            && (target.is_in_range(self.self_.center_position(), self.get_maximum_range_versus_target(target))
                || (allow_move && self.self_.trait_or_default::<dyn IMove>().is_some()))
    }

    pub fn unforced_attack_target_stances(&self) -> PlayerRelationship {
        let mut s = PlayerRelationship::NONE;
        for a in self.armaments.borrow().iter() {
            if !a.is_trait_disabled() {
                s = PlayerRelationship(s.0 | a.info.target_relationships.0);
            }
        }
        s
    }

    pub fn grant_leap_condition(&self, self_: &ActorRef) {
        self.leap_token
            .set(self_.grant_condition(self.info.leap_condition.as_deref().unwrap_or("")));
    }

    pub fn revoke_leap_condition(&self, self_: &ActorRef) {
        if self.leap_token.get() != INVALID_CONDITION_TOKEN {
            self.leap_token.set(self_.revoke_condition(self.leap_token.get()));
        }
    }
}

impl sound::IOrderVoice for AttackBase {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        matches!(order.order_string.as_str(), "Attack" | "ForceAttack").then(|| self.info.voice.clone())
    }
}

impl IResolveOrder for AttackBase {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        let force_attack = order.order_string == "ForceAttack";
        if force_attack || order.order_string == "Attack" {
            if order.target.is_valid_for(self_) {
                self.attack_target(
                    &order.target,
                    AttackSource::Default,
                    order.queued,
                    true,
                    force_attack,
                    Some(LINE_CRIMSON),
                );
                crate::target_lines::show_target_lines(&self.self_);
            }
        } else if order.order_string == "Stop" {
            self.on_stop_order(self_);
        }
    }
}

impl INotifyAttack for AttackBase {
    fn attacking(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {
        if self.info.kind == AttackKind::Charges {
            self.charge_level.set(0);
        } else {
            self.charges.set(self.charges.get() - 1);
            self.time_to_recharge.set(self.info.reload_delay);
        }
    }

    fn preparing_attack(&self, _self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {}
}

impl INotifySold for AttackBase {
    fn selling(&self, _self_: &ActorRef) {
        self.charge_level.set(0);
    }

    fn sold(&self, _self_: &ActorRef) {}
}

impl INotifyRemovedFromWorld for AttackBase {
    fn removed_from_world(&self, self_: &ActorRef) {
        let e = self.on_removed_from_world.borrow().clone();
        e.iter().for_each(|f| f(self_));
    }
}

impl INotifyOwnerChanged for AttackBase {
    fn on_owner_changed(&self, _self_: &ActorRef, _old_owner: &PlayerRef, _new_owner: &PlayerRef) {
        *self.requested_target.borrow_mut() = Target::Invalid;
        *self.opportunity_target.borrow_mut() = Target::Invalid;
        self.opportunity_target_is_persistent.set(false);
    }
}

impl IOverrideAutoTarget for AttackBase {
    fn try_get_auto_target_override(&self, _self_: &ActorRef) -> Option<Target> {
        if self.requested_target.borrow().target_type() != TargetType::Invalid {
            return Some(self.requested_target());
        }
        if self.opportunity_target_is_persistent.get() && self.opportunity_target.borrow().target_type() != TargetType::Invalid {
            return Some(self.opportunity_target.borrow().clone());
        }
        None
    }
}

impl INotifyStanceChanged for AttackBase {
    fn stance_changed(&self, self_: &ActorRef, auto_target: &AutoTarget, old_stance: UnitStance, new_stance: UnitStance) {
        if new_stance > old_stance || self.opportunity_force_attack.get() {
            return;
        }
        let opp = self.opportunity_target.borrow().clone();
        if !has_valid_priority(&opp, self_, auto_target) {
            *self.opportunity_target.borrow_mut() = Target::Invalid;
        }
    }
}

/// True unless `target` is an actor or frozen actor without a valid target priority.
fn has_valid_priority(target: &Target, self_: &ActorRef, auto_target: &AutoTarget) -> bool {
    match target.target_type() {
        TargetType::Actor => {
            let a = target.actor().unwrap();
            auto_target.has_valid_target_priority(self_, Some(&a.owner()), &enabled_target_types(a))
        }
        TargetType::FrozenActor => {
            let f = target.frozen_actor().unwrap();
            auto_target.has_valid_target_priority(self_, f.owner().as_ref(), &f.target_types())
        }
        _ => true,
    }
}

impl INotifyPassengerEntered for AttackBase {
    fn on_passenger_entered(&self, self_: &ActorRef, passenger: &ActorRef) {
        self.pax.borrow_mut().insert(
            passenger.id,
            (passenger.trait_::<dyn IFacing>(), passenger.trait_::<dyn IPositionable>()),
        );
        let notify = self.notify_attacks.borrow().clone();
        for a in passenger.traits_implementing::<Armament>() {
            if self.info.armaments.contains(&a.info.name) {
                a.add_notify_attacks(self_, &notify);
                self.armaments.borrow_mut().push(a);
            }
        }
    }
}

impl INotifyPassengerExited for AttackBase {
    fn on_passenger_exited(&self, _self_: &ActorRef, passenger: &ActorRef) {
        self.pax.borrow_mut().shift_remove(&passenger.id);
        let notify = self.notify_attacks.borrow().clone();
        self.armaments.borrow_mut().retain(|a| {
            if a.actor.id == passenger.id {
                a.remove_notify_attacks(&notify);
                false
            } else {
                true
            }
        });
    }
}

/// Dispatches `IActivityNotifyStanceChanged.StanceChanged` to the activities that implement it.
pub fn activity_stance_changed(a: &ActivityRef, self_: &ActorRef, auto_target: &AutoTarget, old: UnitStance, new: UnitStance) {
    let any = a.as_any();
    if let Some(x) = any.downcast_ref::<Attack>() {
        if !(new > old || x.force_attack) && !x.last_visible.has_valid_priority(self_, auto_target) {
            *x.target.borrow_mut() = Target::Invalid;
        }
    } else if let Some(x) = any.downcast_ref::<AttackActivity>() {
        if !(new > old || x.force_attack) && !x.last_visible.has_valid_priority(self_, auto_target) {
            x.attack.clear_requested_target();
        }
    } else if let Some(x) = any.downcast_ref::<FlyAttack>() {
        if !(new > old || x.force_attack) && !x.last_visible.has_valid_priority(self_, auto_target) {
            x.attack.clear_requested_target();
        }
    } else if let Some(x) = any.downcast_ref::<SetTarget>() {
        if !(new > old || x.force_attack) {
            let t = x.target.borrow().clone();
            if !has_valid_priority(&t, self_, auto_target) {
                *x.target.borrow_mut() = Target::Invalid;
            }
        }
    } else if let Some(x) = any.downcast_ref::<LeapAttack>() {
        if !(new > old || x.force_attack) && !x.last_visible.has_valid_priority(self_, auto_target) {
            *x.target.borrow_mut() = Target::Invalid;
        }
    } else if let Some(x) = any.downcast_ref::<ChargeAttack>()
        && !(new > old || x.force_attack)
        && !has_valid_priority(&x.target, self_, auto_target)
    {
        x.cancel(self_, true);
    }
}

/// The `lastVisible*` state shared by the attack activities.
struct LastVisible {
    target: RefCell<Target>,
    maximum_range: Cell<WDist>,
    target_types: RefCell<BitSet<TargetableType>>,
    owner: RefCell<Option<PlayerRef>>,
}

impl LastVisible {
    fn new() -> Self {
        LastVisible {
            target: RefCell::new(Target::Invalid),
            maximum_range: Cell::new(WDist::ZERO),
            target_types: RefCell::new(BitSet::default()),
            owner: RefCell::new(None),
        }
    }

    fn visible_on_creation(self_: &ActorRef, target: &Target) -> bool {
        let tt = target.target_type();
        (tt == TargetType::Actor && target.actor().unwrap().can_be_viewed_by_player(Some(&self_.owner())))
            || tt == TargetType::FrozenActor
            || tt == TargetType::Terrain
    }

    fn set_owner_types(&self, target: &Target) {
        match target.target_type() {
            TargetType::Actor => {
                let a = target.actor().unwrap();
                *self.owner.borrow_mut() = Some(a.owner());
                *self.target_types.borrow_mut() = enabled_target_types(a);
            }
            TargetType::FrozenActor => {
                let f = target.frozen_actor().unwrap();
                *self.owner.borrow_mut() = f.owner();
                *self.target_types.borrow_mut() = f.target_types();
            }
            _ => {}
        }
    }

    fn has_valid_priority(&self, self_: &ActorRef, auto_target: &AutoTarget) -> bool {
        self.target.borrow().is_valid_for(self_)
            && auto_target.has_valid_target_priority(self_, self.owner.borrow().as_ref(), &self.target_types.borrow())
    }
}

fn sight_range(self_: &ActorRef) -> WDist {
    let mut best: Option<WDist> = None;
    for s in self_.traits_implementing::<AffectsShroud>() {
        if s.is_reveals_shroud() && !s.is_trait_disabled() && best.is_none_or(|b| s.range() > b) {
            best = Some(s.range());
        }
    }
    best.unwrap_or(WDist::from_cells(2))
}

const STATUS_NEEDS_TO_TURN: i32 = 1;
const STATUS_NEEDS_TO_MOVE: i32 = 2;
const STATUS_ATTACKING: i32 = 3;

/// `Activities/Attack.cs`.
pub struct Attack {
    base: ActivityBase,
    attack_traits: Vec<Rc<AttackBase>>,
    mv: Option<Rc<dyn IMove>>,
    mobile: Option<Rc<Mobile>>,
    facing: Rc<dyn IFacing>,
    positionable: Rc<dyn IPositionable>,
    force_attack: bool,
    helper: MoveCooldownHelper,
    target: RefCell<Target>,
    last_visible: LastVisible,
    use_last_visible_target: Cell<bool>,
    attack_status: Cell<i32>,
}

impl Attack {
    pub fn new(self_: &ActorRef, target: &Target, allow_movement: bool, force_attack: bool) -> ActivityRef {
        let attack_traits: Vec<Rc<AttackBase>> = self_
            .traits_implementing::<AttackBase>()
            .into_iter()
            .filter(|a| a.info.kind.is_frontal())
            .collect();
        let imove = self_.trait_or_default::<dyn IMove>();
        let mobile = self_.trait_or_default::<Mobile>();
        let a = Attack {
            base: ActivityBase::default(),
            mv: if allow_movement { imove } else { None },
            facing: self_.trait_::<dyn IFacing>(),
            positionable: self_.trait_::<dyn IPositionable>(),
            force_attack,
            helper: MoveCooldownHelper::new(mobile.clone()),
            mobile,
            target: RefCell::new(target.clone()),
            last_visible: LastVisible::new(),
            use_last_visible_target: Cell::new(false),
            attack_status: Cell::new(0),
            attack_traits,
        };
        if LastVisible::visible_on_creation(self_, target) {
            *a.last_visible.target.borrow_mut() = Target::from_pos(target.center_position());
            a.last_visible.maximum_range.set(a.min_max_range(target));
            a.last_visible.set_owner_types(target);
        }
        Rc::new(a)
    }

    fn active_traits(&self) -> Vec<Rc<AttackBase>> {
        self.attack_traits.iter().filter(|t| !t.is_trait_disabled()).cloned().collect()
    }

    fn min_max_range(&self, target: &Target) -> WDist {
        self.active_traits()
            .iter()
            .map(|x| x.get_maximum_range_versus_target(target))
            .min()
            .expect("Attack without enabled attack traits")
    }

    fn queue_move(&self, min_range: WDist, max_range: WDist, initial: WPos) {
        self.helper.notify_move_queued();
        let target = self.target.borrow().clone();
        self.base.queue_child(
            self.mv
                .as_ref()
                .unwrap()
                .move_within_range(&target, min_range, max_range, Some(initial)),
        );
    }

    fn add_status(&self, s: i32) {
        self.attack_status.set(self.attack_status.get() | s);
    }

    fn tick_attack(&self, self_: &ActorRef, attack: &Rc<AttackBase>) -> i32 {
        let target = self.target.borrow().clone();
        if !target.is_valid_for(self_) {
            return 0;
        }
        if attack.info.attack_requires_entering_cell
            && !self
                .positionable
                .can_enter_cell(target.actor().expect("target actor").location(), None, BlockedByActor::None)
        {
            return 0;
        }
        if !attack.info.target_frozen_actors && !self.force_attack && target.target_type() == TargetType::FrozenActor {
            if self.mv.is_none() {
                return 0;
            }
            self.add_status(STATUS_NEEDS_TO_MOVE);
            self.queue_move(WDist::ZERO, sight_range(self_), target.center_position());
            return STATUS_NEEDS_TO_MOVE;
        }
        let armaments = attack.choose_armaments_for_target(&target, self.force_attack);
        if armaments.is_empty() {
            return 0;
        }
        let active: Vec<&Rc<Armament>> = armaments.iter().filter(|x| !x.is_trait_paused()).collect();
        let (min_range, max_range) = if !active.is_empty() {
            (
                active.iter().map(|a| a.weapon.min_range).max().unwrap(),
                active.iter().map(|a| a.max_range()).min().unwrap(),
            )
        } else {
            (WDist::ZERO, armaments.iter().map(|a| a.max_range()).max().unwrap())
        };
        let pos = self_.center_position();
        if !target.is_in_range(pos, max_range)
            || (min_range.length != 0 && target.is_in_range(pos, min_range))
            || self.mobile.as_ref().is_some_and(|m| !m.can_interact_with_ground_layer(self_))
        {
            if self.mv.is_none() {
                return 0;
            }
            self.add_status(STATUS_NEEDS_TO_MOVE);
            let check = if self.use_last_visible_target.get() {
                self.last_visible.target.borrow().clone()
            } else {
                target
            };
            self.queue_move(min_range, max_range, check.center_position());
            return STATUS_NEEDS_TO_MOVE;
        }
        if !attack.target_in_firing_arc(self_, &target, attack.info.facing_tolerance) {
            if self.mobile.as_ref().is_none_or(|m| !m.is_trait_disabled() && !m.is_trait_paused()) {
                self.facing.set_facing(util::tick_facing(
                    self.facing.facing(),
                    (attack.get_target_position(pos, &target) - pos).yaw(),
                    self.facing.turn_speed(),
                ));
                if !attack.target_in_firing_arc(self_, &target, attack.info.facing_tolerance) {
                    self.add_status(STATUS_NEEDS_TO_TURN);
                    return STATUS_NEEDS_TO_TURN;
                }
            } else {
                self.add_status(STATUS_NEEDS_TO_TURN);
                return STATUS_NEEDS_TO_TURN;
            }
        }
        self.add_status(STATUS_ATTACKING);
        if !attack.is_trait_paused() {
            for a in &armaments {
                a.check_fire(self_, &target);
            }
        }
        STATUS_ATTACKING
    }
}

impl Activity for Attack {
    activity_common!("Attack");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        let t = if self.use_last_visible_target.get() {
            &self.last_visible.target
        } else {
            &self.target
        };
        Some(t.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let active = self.active_traits();
        if active.is_empty() {
            self.cancel(self_, false);
            return false;
        }
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        if !hidden && target.target_type() == TargetType::Actor {
            *self.last_visible.target.borrow_mut() = Target::from_target_positions(&target);
            self.last_visible.maximum_range.set(self.min_max_range(&target));
            self.last_visible.set_owner_types(&target);
        }
        let use_last = hidden || !target.is_valid_for(self_);
        self.use_last_visible_target.set(use_last);
        if let Some(r) = self.helper.tick(&self_.world(), hidden) {
            return r;
        }
        if use_last && !self.last_visible.target.borrow().is_valid_for(self_) {
            return true;
        }
        let pos = self_.center_position();
        if use_last {
            let check = self.last_visible.target.borrow().clone();
            let range = self.last_visible.maximum_range.get();
            if check.is_in_range(pos, range) || self.mv.is_none() || range == WDist::ZERO {
                return true;
            }
            self.queue_move(WDist::ZERO, range, check.center_position());
            return false;
        }
        self.attack_status.set(0);
        for attack in &active {
            let status = self.tick_attack(self_, attack);
            attack.set_is_aiming(status == STATUS_ATTACKING || status == STATUS_NEEDS_TO_TURN);
        }
        self.attack_status.get() < STATUS_NEEDS_TO_TURN
    }

    fn on_last_run(&self, _self_: &ActorRef) {
        for attack in self.active_traits() {
            attack.set_is_aiming(false);
        }
    }
}

/// `AttackFollow.AttackActivity`.
pub struct AttackActivity {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    mv: Option<Rc<dyn IMove>>,
    force_attack: bool,
    rearmable: Option<Rc<Rearmable>>,
    source: AttackSource,
    is_aircraft: bool,
    helper: MoveCooldownHelper,
    target: RefCell<Target>,
    last_visible: LastVisible,
    last_visible_minimum_range: Cell<WDist>,
    has_ticked: Cell<bool>,
}

impl AttackActivity {
    fn new(
        self_: &ActorRef,
        attack: Rc<AttackBase>,
        source: AttackSource,
        target: &Target,
        allow_move: bool,
        force_attack: bool,
    ) -> ActivityRef {
        let mv = if allow_move { self_.trait_or_default::<dyn IMove>() } else { None };
        let helper = MoveCooldownHelper::new(if mv.is_some() { self_.trait_or_default::<Mobile>() } else { None });
        helper.retry_if_destination_blocked.set(true);
        let a = AttackActivity {
            base: ActivityBase::default(),
            mv,
            force_attack,
            rearmable: self_.trait_or_default::<Rearmable>(),
            source,
            is_aircraft: self_.info.has_trait_info::<AircraftInfo>(),
            helper,
            target: RefCell::new(target.clone()),
            last_visible: LastVisible::new(),
            last_visible_minimum_range: Cell::new(WDist::ZERO),
            has_ticked: Cell::new(false),
            attack,
        };
        if LastVisible::visible_on_creation(self_, target) {
            *a.last_visible.target.borrow_mut() = Target::from_pos(target.center_position());
            a.last_visible.maximum_range.set(a.attack.get_maximum_range_versus_target(target));
            a.last_visible_minimum_range.set(a.attack.get_minimum_range_versus_target(target));
            a.last_visible.set_owner_types(target);
        }
        Rc::new(a)
    }
}

impl Activity for AttackActivity {
    activity_common!("AttackActivity");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let attack = &self.attack;
        if self.has_ticked.get() && attack.requested_target.borrow().target_type() == TargetType::Invalid {
            return true;
        }
        if attack.is_trait_paused() {
            return false;
        }
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        attack.set_requested_target(&target, self.force_attack, None);
        self.has_ticked.set(true);
        let tt = target.target_type();
        if !hidden && tt == TargetType::Actor {
            *self.last_visible.target.borrow_mut() = Target::from_target_positions(&target);
            let mut max = attack.get_maximum_range_versus_target(&target);
            let min = attack.get_minimum_range();
            self.last_visible.set_owner_types(&target);
            let leeway = attack.info.range_margin.length;
            if leeway != 0 && self.mv.is_some() && target.actor().unwrap().trait_or_default::<dyn IMove>().is_some() {
                let prefer_min = (min.length + leeway).min(max.length);
                let prefer_max = (max.length - leeway).max(min.length);
                let v = max.length - leeway;
                max = WDist::new(if v < prefer_min {
                    prefer_min
                } else if v > prefer_max {
                    prefer_max
                } else {
                    v
                });
            }
            self.last_visible.maximum_range.set(max);
            self.last_visible_minimum_range.set(min);
        } else if tt == TargetType::FrozenActor && !self.last_visible.target.borrow().is_valid_for(self_) {
            *self.last_visible.target.borrow_mut() = Target::from_target_positions(&target);
            self.last_visible.maximum_range.set(attack.get_maximum_range_versus_target(&target));
            self.last_visible.set_owner_types(&target);
        }
        let mut max_range = self.last_visible.maximum_range.get();
        let min_range = self.last_visible_minimum_range.get();
        let use_last = hidden || !target.is_valid_for(self_);
        if tt == TargetType::FrozenActor && !attack.info.target_frozen_actors && !self.force_attack {
            max_range = max_range.min(sight_range(self_));
        }
        if let Some(r) = self.helper.tick(&self_.world(), hidden) {
            return r;
        }
        if use_last && !self.last_visible.target.borrow().is_valid_for(self_) {
            return true;
        }
        let w = self_.world();
        if let Some(rearmable) = &self.rearmable
            && !use_last
            && attack
                .armaments()
                .iter()
                .all(|x| x.is_trait_paused() || !x.weapon.is_valid_against(&target, &w, self_))
        {
            if self.source == AttackSource::AttackMove {
                return true;
            }
            if attack.info.abort_on_resupply
                && let Some(n) = self.base.next_activity()
            {
                n.cancel(self_, false);
            }
            if self.is_aircraft {
                self.base.queue_child(ReturnToBase::new(self_, None, false));
            } else {
                let mut hosts: Vec<ActorRef> = w
                    .actors_having_trait::<crate::aircraft::Reservable>()
                    .into_iter()
                    .filter(|a| {
                        !a.is_dead()
                            && a.is_in_world()
                            && a.owner().is_allied_with(Some(&self_.owner()))
                            && rearmable.info.rearm_actors.contains(&a.info.name)
                    })
                    .collect();
                hosts.sort_by_key(|a| {
                    (
                        if Rc::ptr_eq(&a.owner(), &self_.owner()) { 0 } else { 1 },
                        (self_.location() - a.location()).length_squared(),
                    )
                });
                if let Some(h) = hosts.first() {
                    self.base.queue_child(Resupply::new(self_, h, WDist::new(512), false));
                }
            }
            return attack.info.abort_on_resupply;
        }
        let pos = self_.center_position();
        let check = if use_last {
            self.last_visible.target.borrow().clone()
        } else {
            target.clone()
        };
        if check.is_in_range(pos, max_range) && !check.is_in_range(pos, min_range) {
            return use_last;
        }
        let Some(mv) = &self.mv else { return true };
        if max_range == WDist::ZERO || max_range < min_range {
            return true;
        }
        self.helper.notify_move_queued();
        self.base
            .queue_child(mv.move_within_range(&target, min_range, max_range, Some(check.center_position())));
        false
    }

    fn on_last_run(&self, _self_: &ActorRef) {
        self.attack.clear_requested_target();
    }
}

/// `AttackOmni.SetTarget`.
pub struct SetTarget {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    allow_move: bool,
    force_attack: bool,
    target: RefCell<Target>,
}

impl Activity for SetTarget {
    activity_common!("SetTarget");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let t = self.target.borrow().recalculate_invalidating_hidden(&self_.owner());
        *self.target.borrow_mut() = t.clone();
        if self.base.is_canceling() || !t.is_valid_for(self_) || !self.attack.is_reachable_target(&t, self.allow_move) {
            return true;
        }
        self.attack.do_attack(self_, &t);
        false
    }
}

/// `AttackTesla.ChargeAttack`.
pub struct ChargeAttack {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    target: Target,
    force_attack: bool,
}

impl Activity for ChargeAttack {
    activity_common!("ChargeAttack");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() || !self.attack.can_attack(self_, &self.target) {
            return true;
        }
        if self.attack.charges.get() == 0 {
            return false;
        }
        for n in self_.traits_implementing::<dyn INotifyTeslaCharging>() {
            n.charging(self_, &self.target);
        }
        self.base.queue_child(Wait::new(self.attack.info.initial_charge_delay));
        self.base.queue_child(Rc::new(ChargeFire {
            base: ActivityBase::default(),
            attack: self.attack.clone(),
            target: self.target.clone(),
        }));
        false
    }
}

struct ChargeFire {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    target: Target,
}

impl Activity for ChargeFire {
    activity_common!("ChargeFire");

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() || !self.attack.can_attack(self_, &self.target) || self.attack.charges.get() == 0 {
            return true;
        }
        self.attack.do_attack(self_, &self.target);
        self.base.queue_child(Wait::new(self.attack.info.charge_delay));
        false
    }
}

/// `Activities/Air/FlyAttack.cs`.
pub struct FlyAttack {
    base: ActivityBase,
    aircraft: Rc<Aircraft>,
    attack: Rc<AttackBase>,
    rearmable: Option<Rc<Rearmable>>,
    source: AttackSource,
    force_attack: bool,
    strafe_distance: WDist,
    target: RefCell<Target>,
    last_visible: LastVisible,
    has_ticked: Cell<bool>,
}

impl FlyAttack {
    fn new(self_: &ActorRef, attack: Rc<AttackBase>, source: AttackSource, target: &Target, force_attack: bool) -> ActivityRef {
        let a = FlyAttack {
            base: ActivityBase::default(),
            aircraft: self_.trait_::<Aircraft>(),
            rearmable: self_.trait_or_default::<Rearmable>(),
            source,
            force_attack,
            strafe_distance: attack.info.strafe_run_length,
            target: RefCell::new(target.clone()),
            last_visible: LastVisible::new(),
            has_ticked: Cell::new(false),
            attack,
        };
        if LastVisible::visible_on_creation(self_, target) {
            *a.last_visible.target.borrow_mut() = Target::from_pos(target.center_position());
            a.last_visible.maximum_range.set(a.attack.get_maximum_range_versus_target(target));
            a.last_visible.set_owner_types(target);
        }
        Rc::new(a)
    }
}

impl Activity for FlyAttack {
    activity_common!("FlyAttack");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        Some(self.target.borrow().clone())
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        let (aircraft, attack) = (&self.aircraft, &self.attack);
        if aircraft.force_landing() {
            self.cancel(self_, false);
        }
        if self.base.is_canceling() {
            return true;
        }
        if self.has_ticked.get() && attack.requested_target.borrow().target_type() == TargetType::Invalid {
            return true;
        }
        if attack.is_trait_paused() {
            return false;
        }
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        attack.set_requested_target(&target, self.force_attack, None);
        self.has_ticked.set(true);
        let tt = target.target_type();
        if (!hidden && tt == TargetType::Actor) || (tt == TargetType::FrozenActor && !self.last_visible.target.borrow().is_valid_for(self_))
        {
            *self.last_visible.target.borrow_mut() = Target::from_target_positions(&target);
            self.last_visible.maximum_range.set(attack.get_maximum_range_versus_target(&target));
            self.last_visible.set_owner_types(&target);
        }
        let use_last = hidden || !target.is_valid_for(self_);
        if use_last && !self.last_visible.target.borrow().is_valid_for(self_) {
            return true;
        }
        let w = self_.world();
        if self.rearmable.is_some()
            && !use_last
            && attack
                .armaments()
                .iter()
                .all(|x| x.is_trait_paused() || !x.weapon.is_valid_against(&target, &w, self_))
        {
            if self.source == AttackSource::AttackMove {
                return true;
            }
            if attack.info.abort_on_resupply
                && let Some(n) = self.base.next_activity()
            {
                n.cancel(self_, false);
            }
            self.base.queue_child(ReturnToBase::new(self_, None, false));
            return attack.info.abort_on_resupply;
        }
        let pos = self_.center_position();
        let check = if use_last {
            self.last_visible.target.borrow().clone()
        } else {
            target.clone()
        };
        let minimum_range = if attack.info.attack_type == AirAttackType::Strafe {
            WDist::ZERO
        } else {
            attack.get_minimum_range_versus_target(&target)
        };
        let last_max = self.last_visible.maximum_range.get();
        if last_max == WDist::ZERO || last_max < minimum_range {
            return true;
        }
        let delta = attack.get_target_position(pos, &target) - pos;
        let desired_facing = if delta.horizontal_length_squared() != 0 {
            delta.yaw()
        } else {
            aircraft.facing()
        };
        self.base.queue_child(TakeOff::new(self_));
        if !check.is_in_range(pos, last_max) || check.is_in_range(pos, minimum_range) {
            self.base
                .queue_child(aircraft.move_within_range(&target, minimum_range, last_max, Some(check.center_position())));
        } else if use_last {
            return true;
        } else if attack.info.attack_type == AirAttackType::Strafe {
            let exit_range = if self.strafe_distance != WDist::ZERO {
                self.strafe_distance
            } else {
                last_max
            };
            let run = StrafeAttackRun {
                base: ActivityBase::default(),
                attack: attack.clone(),
                aircraft: aircraft.clone(),
                exit_range,
                target: RefCell::new(target.clone()),
            };
            run.base.child_has_priority.set(false);
            self.base.queue_child(Rc::new(run));
        } else if attack.info.attack_type == AirAttackType::Default && !aircraft.info.can_hover {
            let run = FlyAttackRun {
                base: ActivityBase::default(),
                attack: attack.clone(),
                exit_range: last_max,
                target: RefCell::new(target.clone()),
                target_is_visible_actor: Cell::new(false),
            };
            run.base.child_has_priority.set(false);
            self.base.queue_child(Rc::new(run));
        } else if !attack.target_in_firing_arc(self_, &target, attack.info.facing_tolerance) {
            aircraft.set_facing(util::tick_facing(aircraft.facing(), desired_facing, aircraft.turn_speed()));
        }
        false
    }

    fn on_last_run(&self, _self_: &ActorRef) {
        self.attack.clear_requested_target();
    }
}

struct FlyAttackRun {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    exit_range: WDist,
    target: RefCell<Target>,
    target_is_visible_actor: Cell<bool>,
}

impl Activity for FlyAttackRun {
    activity_common!("FlyAttackRun");

    fn on_first_run(&self, self_: &ActorRef) {
        let target = self.target.borrow().clone();
        if target.is_valid_for(self_) {
            let c = Some(target.center_position());
            self.base.queue_child(Fly::full(
                self_,
                target.clone(),
                FlyArgs {
                    initial: c,
                    ..Default::default()
                },
            ));
            self.base.queue_child(FlyForward::new_ticks(self_, 1));
            self.base.queue_child(Fly::full(
                self_,
                target,
                FlyArgs {
                    min_range: self.exit_range,
                    max_range: WDist::MAX,
                    initial: c,
                    ..Default::default()
                },
            ));
        } else {
            self.cancel(self_, false);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.tick_child(self_) || self.base.is_canceling() {
            return true;
        }
        let was_visible = self.target_is_visible_actor.get();
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        self.target_is_visible_actor
            .set(target.target_type() == TargetType::Actor && !hidden);
        if was_visible && (!target.is_valid_for(self_) || !self.attack.has_any_valid_weapons(&target, false, false)) {
            self.cancel(self_, false);
        }
        false
    }
}

struct StrafeAttackRun {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    aircraft: Rc<Aircraft>,
    exit_range: WDist,
    target: RefCell<Target>,
}

impl Activity for StrafeAttackRun {
    activity_common!("StrafeAttackRun");

    fn on_first_run(&self, self_: &ActorRef) {
        let target = self.target.borrow().clone();
        if target.is_valid_for(self_) {
            let c = Some(target.center_position());
            self.base.queue_child(Fly::full(
                self_,
                target.clone(),
                FlyArgs {
                    initial: c,
                    ..Default::default()
                },
            ));
            self.base.queue_child(FlyForward::new_distance(self_, self.exit_range));
            let distance_to_turn = WDist::new(self.aircraft.info.speed * 256 / self.aircraft.info.turn_speed.angle);
            self.base.queue_child(Fly::full(
                self_,
                target,
                FlyArgs {
                    min_range: self.exit_range + distance_to_turn,
                    max_range: WDist::MAX,
                    initial: c,
                    ..Default::default()
                },
            ));
        } else {
            self.cancel(self_, false);
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.tick_child(self_) || self.base.is_canceling() {
            return true;
        }
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        if !hidden && target.target_type() == TargetType::Actor {
            self.attack
                .set_requested_target(&Target::from_target_positions(&target), true, None);
        }
        false
    }
}

/// `Activities/Hunt.cs`.
pub struct Hunt {
    base: ActivityBase,
    mv: Rc<dyn IMove>,
    attack: Rc<AttackBase>,
}

impl Hunt {
    pub fn new(self_: &ActorRef) -> ActivityRef {
        Rc::new(Hunt {
            base: ActivityBase::default(),
            mv: self_.trait_::<dyn IMove>(),
            attack: self_.trait_::<AttackBase>(),
        })
    }
}

impl Activity for Hunt {
    activity_common!("Hunt");

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let w = self_.world();
        let candidates = w.actors_having_trait::<crate::conditions::Huntable>().into_iter().filter(|a| {
            a.id != self_.id
                && !a.is_dead()
                && a.is_in_world()
                && util::appears_hostile_to(a, self_)
                && rustora_engine::actor_ext::is_targetable_by(a, self_)
                && self.attack.has_any_valid_weapons(&Target::from_actor(Some(a)), false, false)
        });
        let Some(target_actor) = util::closest_to_with_path_from(candidates, self_) else {
            return false;
        };
        let (mv, world) = (self.mv.clone(), w.clone());
        self.base.queue_child(crate::attack_move::AttackMoveActivity::new(
            self_,
            Rc::new(move || {
                mv.move_within_range(
                    &Target::from_cell(&world, target_actor.location(), rustora_engine::traits::subcell::FULL_CELL),
                    WDist::ZERO,
                    WDist::from_cells(2),
                    None,
                )
            }),
            false,
        ));
        self.base.queue_child(Wait::new(25));
        false
    }
}

/// `Turn` needs no attack state; re-exported for the leap activities once ported.
pub type TurnActivity = Turn;

/// `Cargo` passengers armed through `AttackGarrisoned`.
pub fn garrison_cargo(self_: &ActorRef) -> Option<Rc<Cargo>> {
    self_.trait_or_default::<Cargo>()
}

/// `Mods.Cnc/Traits/EdibleByLeap.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct EdibleByLeapInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(Default)]
pub struct EdibleByLeap {
    leaper: RefCell<Option<ActorRef>>,
}

rustora_engine::register_traits!(EdibleByLeap);
info_impl!(EdibleByLeapInfo => |_, _| Some(Rc::new(EdibleByLeap::default())));

impl EdibleByLeap {
    pub fn can_leap(&self, targeter: &ActorRef) -> bool {
        self.leaper.borrow().as_ref().is_none_or(|l| l.is_dead() || l.id == targeter.id)
    }

    pub fn get_leap_at_by(&self, targeter: &ActorRef) -> bool {
        if self.leaper.borrow().as_ref().is_some_and(|l| !l.is_dead() && l.id != targeter.id) {
            return false;
        }
        *self.leaper.borrow_mut() = Some(targeter.clone());
        true
    }
}

/// `Mods.Cnc/Activities/LeapAttack.cs`.
pub struct LeapAttack {
    base: ActivityBase,
    attack: Rc<AttackBase>,
    mobile: Rc<Mobile>,
    allow_movement: bool,
    force_attack: bool,
    helper: MoveCooldownHelper,
    target: RefCell<Target>,
    last_visible: LastVisible,
    last_visible_min_range: Cell<WDist>,
    use_last_visible_target: Cell<bool>,
}

impl LeapAttack {
    pub fn new(self_: &ActorRef, target: &Target, allow_movement: bool, force_attack: bool, attack: Rc<AttackBase>) -> ActivityRef {
        let mobile = self_.trait_::<Mobile>();
        let a = LeapAttack {
            base: ActivityBase::default(),
            helper: MoveCooldownHelper::new(Some(mobile.clone())),
            mobile,
            allow_movement,
            force_attack,
            target: RefCell::new(target.clone()),
            last_visible: LastVisible::new(),
            last_visible_min_range: Cell::new(WDist::ZERO),
            use_last_visible_target: Cell::new(false),
            attack,
        };
        if LastVisible::visible_on_creation(self_, target) {
            *a.last_visible.target.borrow_mut() = Target::from_pos(target.center_position());
            a.update_ranges(target);
            a.last_visible.set_owner_types(target);
        }
        Rc::new(a)
    }

    fn update_ranges(&self, target: &Target) {
        self.last_visible_min_range.set(self.attack.get_minimum_range_versus_target(target));
        self.last_visible
            .maximum_range
            .set(self.attack.get_maximum_range_versus_target(target));
    }
}

impl Activity for LeapAttack {
    activity_common!("LeapAttack");

    fn target_line_target(&self, _self_: &ActorRef) -> Option<Target> {
        let t = if self.use_last_visible_target.get() {
            &self.last_visible.target
        } else {
            &self.target
        };
        Some(t.borrow().clone())
    }

    fn on_first_run(&self, _self_: &ActorRef) {
        self.attack.set_is_aiming(true);
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let (target, hidden) = self.target.borrow().recalculate(&self_.owner());
        *self.target.borrow_mut() = target.clone();
        if !hidden && target.target_type() == TargetType::Actor {
            *self.last_visible.target.borrow_mut() = Target::from_target_positions(&target);
            self.update_ranges(&target);
            self.last_visible.set_owner_types(&target);
        }
        self.use_last_visible_target.set(hidden || !target.is_valid_for(self_));
        let w = self_.world();
        if let Some(r) = self.helper.tick(&w, hidden) {
            return r;
        }
        let last_visible = self.last_visible.target.borrow().clone();
        if self.use_last_visible_target.get() && !last_visible.is_valid_for(self_) {
            return true;
        }
        let pos = self_.center_position();
        let check = if self.use_last_visible_target.get() {
            last_visible
        } else {
            target.clone()
        };
        let (min_range, max_range) = (self.last_visible_min_range.get(), self.last_visible.maximum_range.get());
        if !check.is_in_range(pos, max_range) || check.is_in_range(pos, min_range) {
            if !self.allow_movement || max_range == WDist::ZERO || max_range < min_range {
                return true;
            }
            self.helper.notify_move_queued();
            self.base.queue_child(
                self.mobile
                    .move_within_range(&target, min_range, max_range, Some(check.center_position())),
            );
            return false;
        }
        if hidden || target.target_type() != TargetType::Actor {
            return true;
        }
        if !target.is_valid_for(self_) || !self.attack.has_any_valid_weapons(&target, false, false) {
            return true;
        }
        let victim = target.actor().unwrap().clone();
        let Some(edible) = victim.trait_or_default::<EdibleByLeap>().filter(|e| e.can_leap(self_)) else {
            return true;
        };
        if self.attack.armaments().iter().all(|a| a.is_reloading()) {
            return false;
        }
        let target_mobile = victim.trait_or_default::<Mobile>();
        let target_subcell = target_mobile
            .as_ref()
            .map_or(rustora_engine::traits::subcell::ANY, |m| m.to_subcell.get());
        let destination = w.map().center_of_subcell(victim.location(), target_subcell);
        let origin = w.map().center_of_subcell(self_.location(), self.mobile.from_subcell.get());
        let desired_facing = (destination - origin).yaw();
        if self.mobile.facing() != desired_facing {
            self.base.queue_child(Turn::new(self_, desired_facing));
            return false;
        }
        self.base.queue_child(Leap::new(
            &target,
            self.mobile.clone(),
            target_mobile,
            self.attack.info.speed.length,
            self.attack.clone(),
            edible,
        ));
        false
    }

    fn on_last_run(&self, _self_: &ActorRef) {
        self.attack.set_is_aiming(false);
    }
}

/// `Mods.Cnc/Activities/Leap.cs`.
pub struct Leap {
    base: ActivityBase,
    mobile: Rc<Mobile>,
    target_mobile: Option<Rc<Mobile>>,
    speed: i32,
    attack: Rc<AttackBase>,
    edible: Rc<EdibleByLeap>,
    target: Target,
    destination_cell: Cell<rustora_core::world_types::CPos>,
    destination_subcell: Cell<u8>,
    origin: Cell<WPos>,
    destination: Cell<WPos>,
    length: Cell<i32>,
    canceled: Cell<bool>,
    jump_complete: Cell<bool>,
    ticks: Cell<i32>,
    target_position: Cell<WPos>,
}

impl Leap {
    pub fn new(
        target: &Target,
        mobile: Rc<Mobile>,
        target_mobile: Option<Rc<Mobile>>,
        speed: i32,
        attack: Rc<AttackBase>,
        edible: Rc<EdibleByLeap>,
    ) -> ActivityRef {
        Rc::new(Leap {
            base: ActivityBase::default(),
            mobile,
            target_mobile,
            speed,
            attack,
            edible,
            target: target.clone(),
            destination_cell: Cell::new(rustora_core::world_types::CPos::ZERO),
            destination_subcell: Cell::new(rustora_engine::traits::subcell::ANY),
            origin: Cell::new(WPos::ZERO),
            destination: Cell::new(WPos::ZERO),
            length: Cell::new(0),
            canceled: Cell::new(false),
            jump_complete: Cell::new(false),
            ticks: Cell::new(0),
            target_position: Cell::new(WPos::ZERO),
        })
    }
}

impl Activity for Leap {
    activity_common!("Leap");

    fn on_first_run(&self, self_: &ActorRef) {
        self.destination_cell.set(self.target.actor().expect("leap target").location());
        if let Some(m) = &self.target_mobile {
            self.destination_subcell.set(m.to_subcell.get());
        }
        let w = self_.world();
        self.origin.set(self_.center_position());
        self.destination.set(
            w.map()
                .center_of_subcell(self.destination_cell.get(), self.destination_subcell.get()),
        );
        self.length
            .set(((self.origin.get() - self.destination.get()).length() / self.speed).max(1));
        self.canceled
            .set(!self.edible.get_leap_at_by(self_) || self.target.target_type() != TargetType::Actor);
        self.base.is_interruptible.set(false);
        if self.canceled.get() {
            return;
        }
        self.target_position.set(self.target.center_position());
        self.attack.grant_leap_condition(self_);
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.canceled.get() || self.jump_complete.get() {
            return true;
        }
        if self.target.target_type() != TargetType::Invalid {
            self.target_position.set(self.target.center_position());
        }
        let length = self.length.get();
        let position = if length > 1 {
            WPos::lerp(self.origin.get(), self.target_position.get(), self.ticks.get(), length - 1)
        } else {
            self.target_position.get()
        };
        self.mobile.set_center_position(self_, position);
        self.ticks.set(self.ticks.get() + 1);
        if self.ticks.get() >= length {
            self.attack.set_is_aiming(false);
            let (cell, sub) = (self.destination_cell.get(), self.destination_subcell.get());
            self.mobile.set_location(cell, sub, cell, sub);
            self.mobile.update_movement();
            self.attack.revoke_leap_condition(self_);
            self.attack.do_attack(self_, &self.target);
            self.jump_complete.set(true);
            let to = self_.world().map().center_of_subcell(cell, sub);
            self.base.queue_child(self.mobile.local_move(self_, position, to));
        }
        false
    }

    fn on_last_run(&self, self_: &ActorRef) {
        self.attack.revoke_leap_condition(self_);
    }

    fn on_actor_dispose(&self, self_: &ActorRef) {
        self.attack.revoke_leap_condition(self_);
    }

    fn get_targets(&self, _self_: &ActorRef) -> Vec<Target> {
        vec![Target::from_pos(if self.ticks.get() < self.length.get() / 2 {
            self.origin.get()
        } else {
            self.destination.get()
        })]
    }
}

impl crate::orders::IIssueOrder for AttackBase {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn crate::orders::IOrderTargeter>> {
        if self.is_trait_disabled() || self.armaments().is_empty() {
            return Vec::new();
        }
        vec![Box::new(AttackOrderTargeter {
            ab: self.me.upgrade().expect("attack base"),
            force: Cell::new(false),
            order_priority: 6,
            is_queued: Cell::new(false),
        })]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn crate::orders::IOrderTargeter, target: &Target, queued: bool) -> Option<Order> {
        order
            .as_any()
            .is::<AttackOrderTargeter>()
            .then(|| Order::new(order.order_id(), Some(self_.clone()), target.clone(), queued))
    }
}

/// `AttackBase.AttackOrderTargeter`. Its order id turns into `ForceAttack` once it targets with force attack.
struct AttackOrderTargeter {
    ab: Rc<AttackBase>,
    force: Cell<bool>,
    order_priority: i32,
    is_queued: Cell<bool>,
}

impl AttackOrderTargeter {
    /// The valid armament with the highest range out of those that have ammo, else the one with the highest range.
    fn armament_for(&self, target: &Target, force_attack: bool) -> Option<Rc<Armament>> {
        let mut armaments = self.ab.choose_armaments_for_target(target, force_attack);
        armaments.sort_by_key(|a| (a.is_trait_paused(), std::cmp::Reverse(a.max_range().length)));
        armaments.into_iter().next()
    }

    fn cursor_for(&self, a: &Armament, out_of_range: bool) -> String {
        let info = &self.ab.info;
        if out_of_range {
            info.outside_range_cursor
                .clone()
                .unwrap_or_else(|| a.info.outside_range_cursor.clone())
        } else {
            info.cursor.clone().unwrap_or_else(|| a.info.cursor.clone())
        }
    }

    fn can_target_actor(
        &self,
        self_: &ActorRef,
        target: &Target,
        modifiers: &mut crate::orders::TargetModifiers,
        cursor: &mut Option<String>,
    ) -> bool {
        use crate::orders::TargetModifiers;
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        if modifiers.has_modifier(TargetModifiers::FORCE_MOVE) {
            return false;
        }
        let info = &self.ab.info;
        if info.force_fire_ignores_actors && modifiers.has_modifier(TargetModifiers::FORCE_ATTACK) {
            return false;
        }
        if let Some(t) = target.actor().filter(|_| target.target_type() == TargetType::Actor) {
            let disguised = t.trait_or_default::<dyn IEffectiveOwner>().is_some_and(|e| e.disguised());
            if disguised && self_.owner().relationship_with(Some(&t.owner())) == PlayerRelationship::ENEMY {
                *modifiers = modifiers.with(TargetModifiers::FORCE_ATTACK, true);
            }
        }
        let force_attack = modifiers.has_modifier(TargetModifiers::FORCE_ATTACK);
        let Some(a) = self.armament_for(target, force_attack) else {
            return false;
        };
        let out_of_range = !target.is_in_range(self_.center_position(), a.max_range())
            || (!force_attack && target.target_type() == TargetType::FrozenActor && !info.target_frozen_actors);
        if out_of_range && info.outside_range_requires_force_fire && !force_attack {
            return false;
        }
        *cursor = Some(self.cursor_for(&a, out_of_range));
        if force_attack {
            self.force.set(true);
        }
        true
    }

    fn can_target_location(
        &self,
        self_: &ActorRef,
        location: rustora_core::world_types::CPos,
        modifiers: crate::orders::TargetModifiers,
        cursor: &mut Option<String>,
    ) -> bool {
        use crate::orders::TargetModifiers;
        let world = self_.world();
        if !world.map().contains(location) {
            return false;
        }
        self.is_queued.set(modifiers.has_modifier(TargetModifiers::FORCE_QUEUE));
        if modifiers.has_modifier(TargetModifiers::FORCE_MOVE)
            || !(self.ab.info.target_terrain_without_force_fire || modifiers.has_modifier(TargetModifiers::FORCE_ATTACK))
        {
            return false;
        }
        let target = Target::from_cell(&world, location, rustora_engine::traits::subcell::FULL_CELL);
        let Some(a) = self.armament_for(&target, true) else {
            return false;
        };
        *cursor = Some(self.cursor_for(&a, !target.is_in_range(self_.center_position(), a.max_range())));
        self.force.set(true);
        true
    }
}

impl crate::orders::IOrderTargeter for AttackOrderTargeter {
    fn order_id(&self) -> &str {
        if self.force.get() { "ForceAttack" } else { "Attack" }
    }

    fn order_priority(&self) -> i32 {
        self.order_priority
    }

    fn can_target(
        &self,
        self_: &ActorRef,
        target: &Target,
        modifiers: &mut crate::orders::TargetModifiers,
        cursor: &mut Option<String>,
    ) -> bool {
        match target.target_type() {
            TargetType::Actor | TargetType::FrozenActor => self.can_target_actor(self_, target, modifiers, cursor),
            TargetType::Terrain => {
                let cell = self_.world().map().cell_containing(target.center_position());
                self.can_target_location(self_, cell, *modifiers, cursor)
            }
            TargetType::Invalid => false,
        }
    }

    fn is_queued(&self) -> bool {
        self.is_queued.get()
    }

    fn target_overrides_selection(
        &self,
        _: &ActorRef,
        _: &Target,
        _: &[ActorRef],
        _: rustora_core::world_types::CPos,
        _: crate::orders::TargetModifiers,
    ) -> bool {
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
