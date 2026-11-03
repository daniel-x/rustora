// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the death traits: `FireWarheadOnDeath.cs`, `SpawnActorOnDeath.cs`, `Husk.cs` and
//! `Mods.Cnc/Traits/HarvesterHuskModifier.cs`.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use indexmap::IndexSet;
use rustora_core::field_enum;
use rustora_core::world_types::{CPos, CVec, WAngle, WPos, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInit, ActorInits, CenterPositionInit, FacingInit, FactionInit, LocationInit, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::damage::{AttackInfo, DamageTypes};
use rustora_engine::player::{PlayerRef, WinState};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::{ANY, FULL_CELL, INVALID};
use rustora_engine::traits::*;
use rustora_engine::weapon::{WeaponInfo, weapon_impact};
use rustora_engine::{activity_common, conditional_plumbing, info_impl, load_info, register_traits};

use crate::activities::Drag;
use crate::actor_map::WorldExt;
use crate::actor_reference::SkipMakeAnimsInit;
use crate::attack::Armament;
use crate::building::BuildingInfo;
use crate::harvester::Harvester;
use crate::interfaces::*;
use crate::locomotor::{BlockedByActor, LocomotorInfo};
use crate::mobile::HuskSpeedInit;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("FireWarheadsOnDeath", |n| load_info!(FireWarheadsOnDeathInfo, n));
    r.register("SpawnActorOnDeath", |n| load_info!(SpawnActorOnDeathInfo, n));
    r.register("Husk", |n| load_info!(HuskInfo, n));
    r.register("HarvesterHuskModifier", |n| load_info!(HarvesterHuskModifierInfo, n));
}

/// `IHuskModifier`.
pub trait IHuskModifier {
    fn husk_actor(&self, self_: &ActorRef) -> Option<String>;
}

/// `EffectiveOwnerInit`.
pub struct EffectiveOwnerInit {
    pub value: PlayerRef,
}

impl ActorInit for EffectiveOwnerInit {
    fn instance_name(&self) -> Option<&str> {
        None
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `ParentActorInit`.
pub struct ParentActorInit {
    pub value: ActorRef,
}

impl ActorInit for ParentActorInit {
    fn instance_name(&self) -> Option<&str> {
        None
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplosionType {
    Footprint,
    CenterPosition,
}
field_enum!(ExplosionType, [Footprint, CenterPosition]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageSource {
    Self_,
    Killer,
}

impl rustora_core::fieldloader::FieldValue for DamageSource {
    const TYPE_NAME: &'static str = "DamageSource";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&rustora_core::miniyaml::MiniYaml>) -> Result<Self, rustora_core::fieldloader::LoadError> {
        match value.unwrap_or("").trim() {
            s if s.eq_ignore_ascii_case("Self") => Ok(DamageSource::Self_),
            s if s.eq_ignore_ascii_case("Killer") => Ok(DamageSource::Killer),
            _ => rustora_core::fieldloader::invalid(value, "DamageSource", field),
        }
    }
}
impl rustora_core::fieldloader::ArrayAware for DamageSource {}

#[derive(Clone, Debug, FieldLoad)]
pub struct FireWarheadsOnDeathInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub weapon: String,
    pub empty_weapon: Option<String>,
    pub loaded_chance: i32,
    pub chance: i32,
    pub damage_threshold: i32,
    pub death_types: DamageTypes,
    pub damage_source: DamageSource,
    #[field(name = "Type")]
    pub ty: ExplosionType,
    pub offset: WVec,
}

impl Default for FireWarheadsOnDeathInfo {
    fn default() -> Self {
        FireWarheadsOnDeathInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            weapon: String::new(),
            empty_weapon: Some("UnitExplode".into()),
            loaded_chance: 100,
            chance: 100,
            damage_threshold: 0,
            death_types: DamageTypes::default(),
            damage_source: DamageSource::Self_,
            ty: ExplosionType::CenterPosition,
            offset: WVec::ZERO,
        }
    }
}

pub struct FireWarheadsOnDeath {
    info: Rc<FireWarheadsOnDeathInfo>,
    state: ConditionalState,
    health: Rc<dyn IHealth>,
    building_info: OnceCell<Option<Rc<BuildingInfo>>>,
    armaments: OnceCell<Vec<Rc<Armament>>>,
    weapons: OnceCell<(Option<Rc<WeaponInfo>>, Option<Rc<WeaponInfo>>)>,
}

register_traits!(FireWarheadsOnDeath: INotifyKilled, INotifyDamage, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(FireWarheadsOnDeathInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(FireWarheadsOnDeath {
        health: init.self_.trait_::<dyn IHealth>(),
        info,
        state,
        building_info: OnceCell::new(),
        armaments: OnceCell::new(),
        weapons: OnceCell::new(),
    }))
});

impl ConditionalHooks for FireWarheadsOnDeath {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}
conditional_plumbing!(FireWarheadsOnDeath);

impl INotifyCreated for FireWarheadsOnDeath {
    fn created(&self, self_: &ActorRef) {
        let _ = self.building_info.set(self_.info.trait_info::<BuildingInfo>());
        let _ = self.armaments.set(self_.traits_implementing::<Armament>());
        let world = self_.world();
        let rules = world.rules();
        let lookup = |name: &str| {
            Some(rules.weapon(&name.to_lowercase()).unwrap_or_else(|| panic!("Weapons Ruleset does not contain an entry '{}'", name.to_lowercase())))
        };
        let weapon = Some(self.info.weapon.as_str()).filter(|w| !w.is_empty()).and_then(lookup);
        let empty = self.info.empty_weapon.as_deref().filter(|w| !w.is_empty()).and_then(lookup);
        let _ = self.weapons.set((weapon, empty));
        conditional_created(self, self_);
    }
}

impl FireWarheadsOnDeath {
    fn source(&self, self_: &ActorRef, e: &AttackInfo) -> Option<ActorRef> {
        match self.info.damage_source {
            DamageSource::Self_ => Some(self_.clone()),
            DamageSource::Killer => e.attacker.clone(),
        }
    }

    fn choose_weapon_for_explosion(&self, self_: &ActorRef) -> Option<Rc<WeaponInfo>> {
        let (weapon, empty) = self.weapons.get().expect("created");
        let armaments = self.armaments.get().expect("created");
        if armaments.is_empty() {
            return weapon.clone();
        }
        if self_.world().shared_random.borrow_mut().next_max(100) > self.info.loaded_chance {
            return empty.clone();
        }
        if armaments.iter().any(|a| !a.is_reloading()) {
            return weapon.clone();
        }
        empty.clone()
    }
}

impl INotifyKilled for FireWarheadsOnDeath {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.state.is_trait_disabled() || !self_.is_in_world() {
            return;
        }
        let w = self_.world();
        if w.shared_random.borrow_mut().next_max(100) > self.info.chance {
            return;
        }
        if !self.info.death_types.is_empty() && !e.damage.damage_types.overlaps(&self.info.death_types) {
            return;
        }
        let Some(weapon) = self.choose_weapon_for_explosion(self_) else { return };
        let source = self.source(self_, e).expect("explosion source");
        match (self.info.ty, self.building_info.get().and_then(|b| b.as_ref())) {
            (ExplosionType::Footprint, Some(bi)) => {
                for c in bi.occupied_tiles(self_.location()) {
                    weapon_impact(&weapon, &Target::from_pos(w.map().center_of_cell(c) + self.info.offset), &source);
                }
            }
            _ => weapon_impact(&weapon, &Target::from_pos(self_.center_position() + self.info.offset), &source),
        }
    }
}

impl INotifyDamage for FireWarheadsOnDeath {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.info.damage_threshold == 0 || self.state.is_trait_disabled() || !self_.is_in_world() {
            return;
        }
        if !self.info.death_types.is_empty() && !e.damage.damage_types.overlaps(&self.info.death_types) {
            return;
        }
        let source = self.source(self_, e);
        if (self.health.hp() as i64) * 100 < self.info.damage_threshold as i64 * self.health.max_hp() as i64 {
            let (a, types) = (self_.clone(), e.damage.damage_types.clone());
            self_.world().add_frame_end_task(Box::new(move |_| {
                if let Some(h) = a.trait_or_default::<dyn IHealth>() {
                    h.kill(&a, source.as_ref(), &types);
                }
            }));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerType {
    Victim,
    Killer,
    InternalName,
}
field_enum!(OwnerType, [Victim, Killer, InternalName]);

#[derive(Clone, Debug, FieldLoad)]
pub struct SpawnActorOnDeathInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub actor: String,
    pub probability: i32,
    pub owner_type: OwnerType,
    pub internal_owner: Option<String>,
    pub effective_owner_from_owner: bool,
    pub death_type: Option<String>,
    pub skip_make_animations: bool,
    pub requires_lobby_creeps: bool,
    pub offset: CVec,
    pub spawn_after_defeat: bool,
}

impl Default for SpawnActorOnDeathInfo {
    fn default() -> Self {
        SpawnActorOnDeathInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            actor: String::new(),
            probability: 100,
            owner_type: OwnerType::Victim,
            internal_owner: Some("Neutral".into()),
            effective_owner_from_owner: false,
            death_type: None,
            skip_make_animations: true,
            requires_lobby_creeps: false,
            offset: CVec::ZERO,
            spawn_after_defeat: true,
        }
    }
}

pub struct SpawnActorOnDeath {
    info: Rc<SpawnActorOnDeathInfo>,
    state: ConditionalState,
    faction: String,
    attacking_player: RefCell<Option<PlayerRef>>,
}

register_traits!(SpawnActorOnDeath: INotifyKilled, INotifyRemovedFromWorld, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SpawnActorOnDeathInfo => |info, init| {
    assert!(!info.requires_lobby_creeps, "SpawnActorOnDeath: MapCreeps is not ported");
    let state = ConditionalState::new(&info.base);
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    Some(Rc::new(SpawnActorOnDeath { info, state, faction, attacking_player: RefCell::new(None) }))
});

rustora_engine::conditional_simple!(SpawnActorOnDeath);

impl INotifyKilled for SpawnActorOnDeath {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.state.is_trait_disabled() || !self_.is_in_world() {
            return;
        }
        if self_.world().shared_random.borrow_mut().next_max(100) > self.info.probability {
            return;
        }
        if self.info.death_type.as_deref().is_some_and(|d| !e.damage.damage_types.contains(d)) {
            return;
        }
        *self.attacking_player.borrow_mut() = Some(e.attacker.as_ref().expect("attacker").owner());
    }
}

impl INotifyRemovedFromWorld for SpawnActorOnDeath {
    fn removed_from_world(&self, self_: &ActorRef) {
        let Some(attacking_player) = self.attacking_player.borrow().clone() else { return };
        let w = self_.world();
        let owner = self_.owner();
        let defeated = owner.win_state.get() == WinState::Lost;
        if defeated && !self.info.spawn_after_defeat {
            return;
        }
        let internal_owner = || {
            let name = self.info.internal_owner.as_deref().unwrap_or("");
            w.players().into_iter().find(|p| p.internal_name == name).expect("internal owner")
        };
        let mut td = ActorInits::new()
            .with(ParentActorInit { value: self_.clone() })
            .with(LocationInit::new(self_.location() + self.info.offset))
            .with(CenterPositionInit::new(self_.center_position()))
            .with(FactionInit::new(self.faction.clone()));
        let mut has_effective_owner = true;
        match self_.cache().effective_owner.as_ref().filter(|eo| eo.disguised()) {
            Some(eo) => td.push(Rc::new(EffectiveOwnerInit { value: eo.owner().expect("effective owner") })),
            None if self.info.effective_owner_from_owner => td.push(Rc::new(EffectiveOwnerInit { value: owner.clone() })),
            None => has_effective_owner = false,
        }
        match self.info.owner_type {
            OwnerType::Victim => {
                if !defeated || self.info.internal_owner.as_deref().is_none_or(str::is_empty) {
                    td.push(Rc::new(OwnerInit::from_player(&owner)));
                } else {
                    td.push(Rc::new(OwnerInit::from_player(&internal_owner())));
                    if !has_effective_owner {
                        td.push(Rc::new(EffectiveOwnerInit { value: owner.clone() }));
                    }
                }
            }
            OwnerType::Killer => td.push(Rc::new(OwnerInit::from_player(&attacking_player))),
            OwnerType::InternalName => td.push(Rc::new(OwnerInit::from_player(&internal_owner()))),
        }
        if self.info.skip_make_animations {
            td.push(Rc::new(SkipMakeAnimsInit));
        }
        for m in self_.traits_implementing::<dyn IDeathActorInitModifier>() {
            m.modify_death_actor_init(self_, &mut td);
        }
        let husk = self_.traits_implementing::<dyn IHuskModifier>().into_iter().find_map(|m| m.husk_actor(self_));
        let actor = husk.unwrap_or_else(|| self.info.actor.clone());
        w.add_frame_end_task(Box::new(move |w| {
            w.create_actor(&actor, td);
        }));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HuskInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub allowed_terrain: IndexSet<String>,
    pub preview_facing: WAngle,
    pub locomotor: Option<String>,
}

impl Default for HuskInfo {
    fn default() -> Self {
        HuskInfo { instance_name: None, allowed_terrain: IndexSet::new(), preview_facing: WAngle::new(384), locomotor: None }
    }
}

impl HuskInfo {
    fn locomotor_info(&self, self_: &ActorRef) -> Option<Rc<LocomotorInfo>> {
        let name = self.locomotor.as_deref().filter(|l| !l.is_empty())?;
        self_.world().rules().actor("world").expect("world").trait_infos::<LocomotorInfo>().into_iter().find(|l| l.name == name)
    }
}

pub struct Husk {
    info: Rc<HuskInfo>,
    self_: ActorRef,
    effective_owner: Option<PlayerRef>,
    drag_speed: i32,
    final_position: WPos,
    notify_center_position_changed: RefCell<Option<Vec<Rc<dyn INotifyCenterPositionChanged>>>>,
    top_left: Cell<CPos>,
    center_position: Cell<WPos>,
    orientation: Cell<WRot>,
}

register_traits!(
    Husk: IOccupySpace,
    IPositionable,
    IFacing,
    ISync,
    INotifyCreated,
    INotifyAddedToWorld,
    INotifyRemovedFromWorld,
    IDeathActorInitModifier,
    IEffectiveOwner
);
info_impl!(HuskInfo => |info, init| {
    let w = init.world();
    let top_left = init.inits.get::<LocationInit>().expect("Husk needs a LocationInit").value;
    let center = init.inits.get::<CenterPositionInit>().map_or_else(|| w.map().center_of_cell(top_left), |c| c.value);
    let facing = init.inits.get::<FacingInit>().map_or(WAngle::new(512), |f| f.value);
    Some(Rc::new(Husk {
        info,
        self_: init.self_.clone(),
        effective_owner: Some(init.inits.get::<EffectiveOwnerInit>().map_or_else(|| init.self_.owner(), |e| e.value.clone())),
        drag_speed: init.inits.get::<HuskSpeedInit>().map_or(0, |s| s.value),
        final_position: w.map().center_of_cell(top_left),
        notify_center_position_changed: RefCell::new(None),
        top_left: Cell::new(top_left),
        center_position: Cell::new(center),
        orientation: Cell::new(WRot::from_yaw(facing)),
    }))
});

impl ISync for Husk {
    fn sync_hash(&self) -> i32 {
        self.top_left.get().hash() ^ self.center_position.get().hash() ^ self.facing().hash()
    }
}

impl INotifyCreated for Husk {
    fn created(&self, self_: &ActorRef) {
        let activity = DragAndCrush::new(self_, self.info.locomotor_info(self_), self.drag_speed, self.final_position);
        self_.queue_activity(activity);
        *self.notify_center_position_changed.borrow_mut() = Some(self_.traits_implementing::<dyn INotifyCenterPositionChanged>());
    }
}

impl IOccupySpace for Husk {
    fn center_position(&self) -> WPos {
        self.center_position.get()
    }

    fn top_left(&self) -> CPos {
        self.top_left.get()
    }

    fn occupied_cells(&self) -> Vec<(CPos, u8)> {
        vec![(self.top_left.get(), FULL_CELL)]
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl IPositionable for Husk {
    fn can_exist_in_cell(&self, cell: CPos) -> bool {
        let w = self.self_.world();
        w.map().contains(cell) && self.info.allowed_terrain.contains(&w.map().terrain_type_info(cell).ty)
    }

    fn can_enter_cell(&self, location: CPos, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> bool {
        self.get_available_subcell(location, ANY, ignore_actor, check) != INVALID
    }

    fn get_valid_subcell(&self, _preferred: u8) -> u8 {
        FULL_CELL
    }

    fn get_available_subcell(&self, cell: CPos, _preferred: u8, ignore_actor: Option<&ActorRef>, check: BlockedByActor) -> u8 {
        if !self.can_exist_in_cell(cell) {
            return INVALID;
        }
        if check == BlockedByActor::None {
            return FULL_CELL;
        }
        let w = self.self_.world();
        if w.actor_map().get_actors_at(cell).iter().all(|x| ignore_actor.is_some_and(|i| i.id == x.id)) { FULL_CELL } else { INVALID }
    }

    fn set_position_cell(&self, self_: &ActorRef, cell: CPos, _sub: u8) {
        self.set_position(self_, self_.world().map().center_of_cell(cell));
    }

    fn set_position(&self, self_: &ActorRef, pos: WPos) {
        let w = self_.world();
        let m = w.actor_map();
        m.remove_influence(self_, self);
        self.center_position.set(pos);
        self.top_left.set(w.map().cell_containing(pos));
        m.add_influence(self_, self);
        w.update_maps(self_);
    }

    fn set_center_position(&self, self_: &ActorRef, pos: WPos) {
        self.center_position.set(pos);
        let list = self.notify_center_position_changed.borrow().clone();
        for n in list.iter().flatten() {
            n.center_position_changed(self_, 0, 0);
        }
    }
}

impl IFacing for Husk {
    fn turn_speed(&self) -> WAngle {
        WAngle::ZERO
    }

    fn facing(&self) -> WAngle {
        self.orientation.get().yaw
    }

    fn set_facing(&self, f: WAngle) {
        self.orientation.set(self.orientation.get().with_yaw(f));
    }

    fn orientation(&self) -> WRot {
        self.orientation.get()
    }
}

impl INotifyAddedToWorld for Husk {
    fn added_to_world(&self, self_: &ActorRef) {
        self_.world().add_to_maps(self_, self);
    }
}

impl INotifyRemovedFromWorld for Husk {
    fn removed_from_world(&self, self_: &ActorRef) {
        self_.world().remove_from_maps(self_, self);
    }
}

impl IDeathActorInitModifier for Husk {
    fn modify_death_actor_init(&self, _self_: &ActorRef, init: &mut ActorInits) {
        init.push(Rc::new(FacingInit::new(self.facing())));
    }
}

impl IEffectiveOwner for Husk {
    fn disguised(&self) -> bool {
        true
    }

    fn owner(&self) -> Option<PlayerRef> {
        self.effective_owner.clone()
    }
}

/// `DragAndCrush` of `Husk.cs`.
pub struct DragAndCrush {
    base: ActivityBase,
    info: Option<Rc<LocomotorInfo>>,
}

impl DragAndCrush {
    /// Queues the `Drag` before the returned activity, like the C# constructor.
    pub fn new(self_: &ActorRef, info: Option<Rc<LocomotorInfo>>, drag_speed: i32, final_position: WPos) -> ActivityRef {
        let distance = (final_position - self_.center_position()).length();
        if drag_speed > 0 && distance > 0 {
            self_.queue_activity(Drag::new(self_, self_.center_position(), final_position, distance / drag_speed, None));
        }
        Rc::new(DragAndCrush { base: ActivityBase::default(), info })
    }
}

impl Activity for DragAndCrush {
    activity_common!("DragAndCrush");

    fn on_first_run(&self, self_: &ActorRef) {
        if !util::is_at_ground_level(self_) {
            return;
        }
        let Some(info) = self.info.as_ref().filter(|i| !i.crushes.is_empty()) else { return };
        let w = self_.world();
        for a in w.actor_map().get_actors_at(w.map().cell_containing(self_.center_position())) {
            if a.id == self_.id {
                continue;
            }
            for c in a.cache().crushables.clone() {
                if c.crushable_by_actor(&a, self_, info.crushes.clone()) && util::is_at_ground_level(&a) {
                    for n in a.traits_implementing::<dyn INotifyCrushed>() {
                        n.on_crush(&a, self_, info.crushes.clone());
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HarvesterHuskModifierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub full_husk_actor: Option<String>,
    pub fullness_threshold: i32,
}

impl Default for HarvesterHuskModifierInfo {
    fn default() -> Self {
        HarvesterHuskModifierInfo { instance_name: None, full_husk_actor: None, fullness_threshold: 50 }
    }
}

pub struct HarvesterHuskModifier {
    info: Rc<HarvesterHuskModifierInfo>,
}

register_traits!(HarvesterHuskModifier: IHuskModifier);
info_impl!(HarvesterHuskModifierInfo => |info, _| Some(Rc::new(HarvesterHuskModifier { info })));

impl IHuskModifier for HarvesterHuskModifier {
    fn husk_actor(&self, self_: &ActorRef) -> Option<String> {
        if self_.trait_::<Harvester>().fullness() > self.info.fullness_threshold { self.info.full_husk_actor.clone() } else { None }
    }
}
