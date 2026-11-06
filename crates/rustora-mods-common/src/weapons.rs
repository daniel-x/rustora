// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Projectiles`, `Warheads`, `Traits/JamsMissiles.cs`, `Traits/BlocksProjectiles.cs`, `Traits/Armor.cs` and the
//! line and circle queries of `WorldExtensions.cs`. Only simulation behavior is kept: sprites, sounds and smudges return
//! with the client. Warheads without simulation effect (`LeaveSmudge`, `ShakeScreen`, flashes) stay unregistered.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{CPos, CVec, Int2, MersenneTwister, WAngle, WDist, WPos, WRot, WVec, isqrt_i32};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_ext::{enabled_target_types, targetable_positions};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::damage::{Damage, DamageTypes};
use rustora_engine::effect::{Effect, EffectRef};
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::render::{PaletteRef, Renderable, SpriteRenderable, fog_obscures, render_animation};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::{Animation, Sequence};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::terrain::TargetableType;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::traits::*;
use rustora_engine::weapon::{IProjectileInfo, IWarhead, ProjectileArgs, WarheadArgs, WeaponRegistry, weapon_impact_args};
use rustora_engine::world::World;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

use crate::actor_map::WorldExt;
use crate::effects::{ContrailFader, ContrailFields, SharedContrail, SpriteEffect};
use crate::health::HealthInfo;
use crate::hit_shape::{ArmorType, HitShape};
use crate::interfaces::{IFirepowerModifier, IInaccuracyModifier, IRangeModifier};
use crate::resources::ResourceLayer;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("Armor", |n| load_info!(ArmorInfo, n));
    r.register("BlocksProjectiles", |n| load_info!(BlocksProjectilesInfo, n));
    r.register("JamsMissiles", |n| load_info!(JamsMissilesInfo, n));
    r.register("FireProjectilesOnDeath", |n| load_info!(FireProjectilesOnDeathInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct FireProjectilesOnDeathInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(require)]
    pub weapons: Vec<String>,
    pub pieces: Vec<i32>,
    pub range: Vec<WDist>,
}

impl Default for FireProjectilesOnDeathInfo {
    fn default() -> Self {
        FireProjectilesOnDeathInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            weapons: Vec::new(),
            pieces: vec![3, 10],
            range: vec![WDist::from_cells(2), WDist::from_cells(5)],
        }
    }
}

/// Fires a random number of projectiles of each weapon in random directions when the actor dies.
pub struct FireProjectilesOnDeath {
    info: Rc<FireProjectilesOnDeathInfo>,
    state: ConditionalState,
}

register_traits!(FireProjectilesOnDeath: crate::interfaces::INotifyKilled, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(FireProjectilesOnDeathInfo => |info, _| Some(Rc::new(FireProjectilesOnDeath {
    state: ConditionalState::new(&info.base),
    info,
})));
conditional_simple!(FireProjectilesOnDeath);

impl crate::interfaces::INotifyKilled for FireProjectilesOnDeath {
    fn killed(&self, self_: &ActorRef, _e: &rustora_engine::damage::AttackInfo) {
        if self.state.is_trait_disabled() {
            return;
        }
        let w = self_.world();
        let info = &self.info;
        for name in &info.weapons {
            let weapon = w
                .rules()
                .weapon(&name.to_lowercase())
                .unwrap_or_else(|| panic!("Weapons Ruleset does not contain an entry '{}'", name.to_lowercase()));
            let (pieces, range) = {
                let mut r = w.shared_random.borrow_mut();
                let pieces = r.next_range(info.pieces[0], info.pieces[1]);
                (pieces, r.next_range(info.range[0].length, info.range[1].length))
            };
            for _ in 0..pieces {
                let (rotation, facing) = {
                    let mut r = w.shared_random.borrow_mut();
                    let rotation = WRot::from_yaw(WAngle::new(r.next_max(1024)));
                    (rotation, WAngle::new(r.next_max(1024)))
                };
                let center = self_.center_position();
                let me = self_.clone();
                let args = ProjectileArgs {
                    weapon: weapon.clone(),
                    damage_modifiers: self_
                        .traits_implementing::<dyn IFirepowerModifier>()
                        .iter()
                        .map(|m| m.get_firepower_modifier())
                        .collect(),
                    inaccuracy_modifiers: self_
                        .traits_implementing::<dyn IInaccuracyModifier>()
                        .iter()
                        .map(|m| m.get_inaccuracy_modifier())
                        .collect(),
                    range_modifiers: self_
                        .traits_implementing::<dyn IRangeModifier>()
                        .iter()
                        .map(|m| m.get_range_modifier())
                        .collect(),
                    facing,
                    current_muzzle_facing: Rc::new(|| WAngle::ZERO),
                    source: center,
                    current_source: Rc::new(move || me.center_position()),
                    source_actor: self_.clone(),
                    passive_target: center + WVec::new(range, 0, 0).rotate(&rotation),
                    guided_target: Target::Invalid,
                };
                let me = self_.clone();
                w.add_frame_end_task(Box::new(move |w| {
                    let weapon = args.weapon.clone();
                    let Some(projectile) = weapon.projectile.as_ref() else { return };
                    if let Some(p) = projectile.create(args) {
                        w.add_effect(p);
                    }
                    if !weapon.report.is_empty() {
                        rustora_engine::sound::play_random_at(
                            w,
                            rustora_engine::sound::SoundType::World,
                            &weapon.report,
                            me.center_position(),
                            1.0,
                        );
                    }
                }));
            }
        }
    }
}

macro_rules! projectile_loader {
    ($info:ty) => {
        |y: &MiniYaml| {
            let mut i = <$info>::default();
            rustora_core::fieldloader::FieldLoad::load(&mut i, y).map_err(|e| e.to_string())?;
            Ok(Rc::new(i) as Rc<dyn IProjectileInfo>)
        }
    };
}

macro_rules! warhead_loader {
    ($info:ty) => {
        |y: &MiniYaml| {
            let mut i = <$info>::default();
            rustora_core::fieldloader::FieldLoad::load(&mut i, y).map_err(|e| e.to_string())?;
            Ok(Rc::new(i) as Rc<dyn IWarhead>)
        }
    };
}

fn load_damage(y: &MiniYaml, kind: DamageKind) -> Result<Rc<dyn IWarhead>, String> {
    let mut i = DamageWarhead::default();
    rustora_core::fieldloader::FieldLoad::load(&mut i, y).map_err(|e| e.to_string())?;
    i.kind = kind;
    Ok(Rc::new(i))
}

/// Registers the projectile and warhead types used by weapon rules.
pub fn register_weapon_parts(r: &mut WeaponRegistry) {
    r.projectiles.insert("Bullet", projectile_loader!(BulletInfo));
    r.projectiles.insert("InstantHit", projectile_loader!(InstantHitInfo));
    r.projectiles.insert("GravityBomb", projectile_loader!(GravityBombInfo));
    r.projectiles.insert("Missile", projectile_loader!(MissileInfo));
    r.projectiles.insert("TeslaZap", projectile_loader!(TeslaZapInfo));
    r.projectiles.insert("LaserZap", projectile_loader!(LaserZapInfo));
    r.projectiles.insert("Railgun", projectile_loader!(RailgunInfo));
    r.warheads.insert("SpreadDamage", |y| load_damage(y, DamageKind::Spread));
    r.warheads.insert("TargetDamage", |y| load_damage(y, DamageKind::Target));
    r.warheads
        .insert("HealthPercentageDamage", |y| load_damage(y, DamageKind::HealthPercentage));
    r.warheads.insert("CreateEffect", warhead_loader!(CreateEffectWarhead));
    r.warheads.insert("LeaveSmudge", warhead_loader!(LeaveSmudgeWarhead));
    r.warheads.insert("ShakeScreen", warhead_loader!(ShakeScreenWarhead));
    r.warheads.insert("CreateResource", warhead_loader!(CreateResourceWarhead));
    r.warheads.insert("DestroyResource", warhead_loader!(DestroyResourceWarhead));
    r.warheads.insert("FireCluster", warhead_loader!(FireClusterWarhead));
    r.warheads
        .insert("GrantExternalCondition", warhead_loader!(GrantExternalConditionWarhead));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InaccuracyType {
    #[default]
    Maximum,
    PerCellIncrement,
    Absolute,
}

rustora_core::field_enum!(InaccuracyType, [Maximum, PerCellIncrement, Absolute]);

fn get_projectile_inaccuracy(base: i32, ty: InaccuracyType, args: &ProjectileArgs) -> i32 {
    let inaccuracy = util::apply_percentage_modifiers(base, args.inaccuracy_modifiers.iter().copied());
    match ty {
        InaccuracyType::Maximum => {
            let max_range = util::apply_percentage_modifiers(args.weapon.range.length, args.range_modifiers.iter().copied());
            inaccuracy.wrapping_mul((args.passive_target - args.source).length()) / max_range
        }
        InaccuracyType::PerCellIncrement => inaccuracy.wrapping_mul((args.passive_target - args.source).length()) / 1024,
        InaccuracyType::Absolute => inaccuracy,
    }
}

/// `WorldExtensions.FindActorsOnCircle`.
pub fn find_actors_on_circle(w: &World, origin: WPos, r: WDist) -> Vec<ActorRef> {
    w.find_actors_in_circle(origin, r + w.actor_map().largest_actor_radius.get())
}

fn minimum_point_line_projection(line_start: WPos, line_end: WPos, point: WPos) -> WPos {
    let squared_length = (line_end - line_start).horizontal_length_squared();
    if squared_length == 0 {
        return line_end;
    }
    let x = (point.x as i64 - line_end.x as i64) * (line_start.x as i64 - line_end.x as i64);
    let y = (point.y as i64 - line_end.y as i64) * (line_start.y as i64 - line_end.y as i64);
    let t = x + y;
    if t < 0 {
        return line_end;
    }
    if t > squared_length {
        return line_start;
    }
    WPos::lerp_long(line_end, line_start, t, squared_length)
}

/// `WorldExtensions.FindBlockingActorsOnLine`.
fn find_blocking_actors_on_line(w: &World, start: WPos, end: WPos, width: WDist) -> Vec<ActorRef> {
    find_actors_on_line(w, start, end, width, true)
}

/// `WorldExtensions.FindActorsOnLine`.
fn find_actors_on_line(w: &World, start: WPos, end: WPos, width: WDist, only_blockers: bool) -> Vec<ActorRef> {
    let dir = WVec::new(if end.x < start.x { -1 } else { 1 }, if end.y < start.y { -1 } else { 1 }, 0);
    let m = w.actor_map();
    let radius = if only_blockers {
        m.largest_blocking_actor_radius.get()
    } else {
        m.largest_actor_radius.get()
    };
    let overselect = dir * (1024 + width.length + radius.length);
    let mut out = Vec::new();
    for a in m.actors_in_box(end + overselect, start - overselect) {
        let mut actor_width = 0;
        for s in a.traits_implementing::<HitShape>() {
            if !s.is_trait_disabled() {
                actor_width = actor_width.max(s.info.ty.outer_radius().length);
            }
        }
        let projection = minimum_point_line_projection(start, end, a.center_position());
        if (a.center_position() - projection).horizontal_length() <= actor_width + width.length {
            out.push(a);
        }
    }
    out
}

fn any_blocking_actors_between(w: &World, owner: &PlayerRef, start: WPos, end: WPos, width: WDist) -> Option<WPos> {
    let length = (end - start).length();
    for a in find_blocking_actors_on_line(w, start, end, width) {
        let blockers: Vec<Rc<BlocksProjectiles>> = a
            .traits_implementing::<BlocksProjectiles>()
            .into_iter()
            .filter(|t| {
                !t.state.is_trait_disabled()
                    && t.info
                        .valid_relationships
                        .has_relationship(a.owner().relationship_with(Some(owner)))
            })
            .collect();
        if blockers.is_empty() {
            continue;
        }
        let hit = minimum_point_line_projection(start, end, a.center_position());
        let dat = w.map().distance_above_terrain(hit);
        if (hit - start).length() < length && blockers.iter().any(|t| t.info.height > dat) {
            return Some(hit);
        }
    }
    None
}

/// The closest enabled hit shape by `DistanceFromEdge`, first one on ties.
fn closest_active_shape(victim: &ActorRef, pos: WPos) -> Option<(Rc<HitShape>, i32)> {
    let mut best: Option<(Rc<HitShape>, i32)> = None;
    for h in victim.traits_implementing::<HitShape>() {
        if h.is_trait_disabled() {
            continue;
        }
        let d = h.distance_from_edge(victim, pos).length;
        if best.as_ref().is_none_or(|b| d < b.1) {
            best = Some((h, d));
        }
    }
    best
}

fn all_relationships() -> PlayerRelationship {
    PlayerRelationship(PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0)
}

#[derive(Clone, Debug, FieldLoad, Default)]
pub struct ArmorInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    #[field(name = "Type")]
    pub ty: Option<String>,
}

pub struct Armor {
    pub info: Rc<ArmorInfo>,
    state: ConditionalState,
}

register_traits!(Armor: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ArmorInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Armor { info, state }))
});
conditional_simple!(Armor);

#[derive(Clone, Debug, FieldLoad)]
pub struct BlocksProjectilesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub height: WDist,
    pub valid_relationships: PlayerRelationship,
}

impl Default for BlocksProjectilesInfo {
    fn default() -> Self {
        BlocksProjectilesInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            height: WDist::from_cells(1),
            valid_relationships: all_relationships(),
        }
    }
}

pub struct BlocksProjectiles {
    pub info: Rc<BlocksProjectilesInfo>,
    state: ConditionalState,
}

register_traits!(BlocksProjectiles: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(BlocksProjectilesInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(BlocksProjectiles { info, state }))
});
conditional_simple!(BlocksProjectiles);

#[derive(Clone, Debug, FieldLoad)]
pub struct JamsMissilesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub range: WDist,
    pub deflection_relationships: PlayerRelationship,
    pub chance: i32,
}

impl Default for JamsMissilesInfo {
    fn default() -> Self {
        JamsMissilesInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            range: WDist::ZERO,
            deflection_relationships: all_relationships(),
            chance: 100,
        }
    }
}

pub struct JamsMissiles {
    pub info: Rc<JamsMissilesInfo>,
    state: ConditionalState,
}

register_traits!(JamsMissiles: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(JamsMissilesInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(JamsMissiles { info, state }))
});
conditional_simple!(JamsMissiles);

/// Fields of the abstract `Warhead`.
#[derive(Clone, Debug, FieldLoad)]
pub struct WarheadBase {
    pub valid_targets: BitSet<TargetableType>,
    pub invalid_targets: BitSet<TargetableType>,
    pub valid_relationships: PlayerRelationship,
    pub affects_parent: bool,
    pub air_threshold: WDist,
    pub delay: i32,
}

impl Default for WarheadBase {
    fn default() -> Self {
        WarheadBase {
            valid_targets: BitSet::new(&["Ground", "Water"]),
            invalid_targets: BitSet::default(),
            valid_relationships: all_relationships(),
            affects_parent: false,
            air_threshold: WDist::new(128),
            delay: 0,
        }
    }
}

impl WarheadBase {
    fn is_valid_target(&self, types: &BitSet<TargetableType>) -> bool {
        self.valid_targets.overlaps(types) && !self.invalid_targets.overlaps(types)
    }

    fn relationship_and_types_valid(&self, victim: &ActorRef, fired_by: &ActorRef) -> bool {
        self.valid_relationships
            .has_relationship(fired_by.owner().relationship_with(Some(&victim.owner())))
            && self.is_valid_target(&enabled_target_types(victim))
    }

    fn is_valid_against(&self, victim: &ActorRef, fired_by: &ActorRef) -> bool {
        (self.affects_parent || victim.id != fired_by.id) && self.relationship_and_types_valid(victim, fired_by)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DamageKind {
    #[default]
    Spread,
    Target,
    HealthPercentage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DamageCalculationType {
    #[default]
    HitShape,
    ClosestTargetablePosition,
    CenterPosition,
}

rustora_core::field_enum!(DamageCalculationType, [HitShape, ClosestTargetablePosition, CenterPosition]);

/// `SpreadDamageWarhead`, `TargetDamageWarhead` and `HealthPercentageDamageWarhead`.
#[derive(Clone, Debug, FieldLoad)]
pub struct DamageWarhead {
    #[field(ignore)]
    pub kind: DamageKind,
    #[field(flatten)]
    pub base: WarheadBase,
    pub damage: i32,
    pub damage_types: DamageTypes,
    pub versus: IndexMap<String, i32>,
    #[field(name = "Spread")]
    pub spread_value: Option<WDist>,
    pub falloff: Vec<i32>,
    pub range: Option<Vec<WDist>>,
    pub damage_calculation_type: DamageCalculationType,
}

impl Default for DamageWarhead {
    fn default() -> Self {
        DamageWarhead {
            kind: DamageKind::Spread,
            base: WarheadBase::default(),
            damage: 0,
            damage_types: DamageTypes::default(),
            versus: IndexMap::new(),
            spread_value: None,
            falloff: vec![100, 37, 14, 5, 0],
            range: None,
            damage_calculation_type: DamageCalculationType::HitShape,
        }
    }
}

impl DamageWarhead {
    fn is_valid_against(&self, victim: &ActorRef, fired_by: &ActorRef) -> bool {
        victim.info.has_trait_info::<HealthInfo>() && self.base.is_valid_against(victim, fired_by)
    }

    fn damage_versus(&self, victim: &ActorRef, shape: &HitShape) -> i32 {
        if self.versus.is_empty() {
            return 100;
        }
        let shape_types: &BitSet<ArmorType> = &shape.info.armor_types;
        let armor = victim.traits_implementing::<Armor>().into_iter().filter_map(|a| {
            if a.state.is_trait_disabled() {
                return None;
            }
            let t = a.info.ty.as_ref()?;
            let v = self.versus.get(t)?;
            (shape_types.is_empty() || shape_types.contains(t)).then_some(*v)
        });
        util::apply_percentage_modifiers(100, armor.collect::<Vec<_>>())
    }

    fn inflict_damage(&self, victim: &ActorRef, fired_by: &ActorRef, shape: &HitShape, modifiers: &[i32]) {
        let versus = self.damage_versus(victim, shape);
        let damage = if self.kind == DamageKind::HealthPercentage {
            let hp = victim.info.trait_info::<HealthInfo>().expect("health info").hp;
            util::apply_percentage_modifiers(hp, modifiers.iter().copied().chain([self.damage, versus]))
        } else {
            util::apply_percentage_modifiers(self.damage, modifiers.iter().copied().chain([versus]))
        };
        if !victim.disposed()
            && let Some(h) = &victim.cache().health
        {
            h.inflict_damage(victim, Some(fired_by), &Damage::new(damage, self.damage_types), false);
        }
    }

    fn spread_or(&self, default: WDist) -> WDist {
        self.spread_value.unwrap_or(default)
    }

    fn effective_range(&self) -> Vec<WDist> {
        match &self.range {
            Some(r) => r.clone(),
            None => {
                let spread = self.spread_or(WDist::new(43));
                (0..self.falloff.len() as i32).map(|i| spread * i).collect()
            }
        }
    }

    fn damage_falloff(&self, range: &[WDist], distance: i32) -> i32 {
        let mut inner = range[0].length;
        for (i, r) in range.iter().enumerate().skip(1) {
            let outer = r.length;
            if outer > distance {
                return Int2::lerp_i32(self.falloff[i - 1], self.falloff[i], distance - inner, outer - inner);
            }
            inner = outer;
        }
        0
    }

    fn impact_pos(&self, pos: WPos, fired_by: &ActorRef, args: &WarheadArgs) {
        let w = fired_by.world();
        if self.kind == DamageKind::Spread {
            let range = self.effective_range();
            let last = *range.last().expect("spread range");
            for victim in find_actors_on_circle(&w, pos, last) {
                if !self.is_valid_against(&victim, fired_by) {
                    continue;
                }
                let Some((shape, closest)) = closest_active_shape(&victim, pos) else {
                    continue;
                };
                let falloff = match self.damage_calculation_type {
                    DamageCalculationType::HitShape => closest,
                    DamageCalculationType::ClosestTargetablePosition => targetable_positions(&victim)
                        .iter()
                        .map(|x| (*x - pos).length())
                        .min()
                        .expect("targetable position"),
                    DamageCalculationType::CenterPosition => (victim.center_position() - pos).length(),
                };
                if falloff > last.length {
                    continue;
                }
                let mut modifiers = args.damage_modifiers.clone();
                modifiers.push(self.damage_falloff(&range, falloff));
                self.inflict_damage(&victim, fired_by, &shape, &modifiers);
            }
        } else {
            let spread = self.spread_or(WDist::ZERO);
            if spread == WDist::ZERO {
                return;
            }
            for victim in find_actors_on_circle(&w, pos, spread) {
                if !self.is_valid_against(&victim, fired_by) {
                    continue;
                }
                let Some((shape, closest)) = closest_active_shape(&victim, pos) else {
                    continue;
                };
                if closest > spread.length {
                    continue;
                }
                self.inflict_damage(&victim, fired_by, &shape, &args.damage_modifiers);
            }
        }
    }
}

impl IWarhead for DamageWarhead {
    fn damage_value(&self) -> Option<i32> {
        Some(self.damage)
    }

    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        let fired_by = &args.source_actor;
        match target.target_type() {
            TargetType::Actor => {
                let victim = target.actor().unwrap();
                if !self.is_valid_against(victim, fired_by) {
                    return;
                }
                let Some((shape, _)) = closest_active_shape(victim, victim.center_position()) else {
                    return;
                };
                self.inflict_damage(victim, fired_by, &shape, &args.damage_modifiers);
            }
            TargetType::Invalid => {}
            _ => self.impact_pos(target.center_position(), fired_by, args),
        }
    }
}

/// `CreateEffectWarhead`: only the checks and the shared random draw of `Inaccuracy` affect the simulation.
#[derive(Clone, Debug, FieldLoad)]
pub struct CreateEffectWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    pub explosions: Vec<String>,
    pub image: Option<String>,
    pub explosion_palette: String,
    pub use_player_palette: bool,
    pub force_display_at_ground_level: bool,
    pub impact_sounds: Vec<String>,
    pub impact_sound_chance: i32,
    pub impact_actors: bool,
    pub inaccuracy: WDist,
}

impl Default for CreateEffectWarhead {
    fn default() -> Self {
        CreateEffectWarhead {
            base: WarheadBase::default(),
            explosions: Vec::new(),
            image: Some("explosion".into()),
            explosion_palette: "effect".into(),
            use_player_palette: false,
            force_display_at_ground_level: false,
            impact_sounds: Vec::new(),
            impact_sound_chance: 100,
            impact_actors: true,
            inaccuracy: WDist::ZERO,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImpactActorType {
    None,
    Invalid,
    Valid,
}

impl CreateEffectWarhead {
    fn actor_type_at_impact(&self, w: &World, pos: WPos, fired_by: &ActorRef) -> ImpactActorType {
        let mut any_invalid = false;
        for victim in find_actors_on_circle(w, pos, WDist::ZERO) {
            if !self.base.affects_parent && victim.id == fired_by.id {
                continue;
            }
            let hit = victim
                .traits_implementing::<HitShape>()
                .iter()
                .any(|s| !s.is_trait_disabled() && s.distance_from_edge(&victim, pos).length <= 0);
            if !hit {
                continue;
            }
            if self.base.relationship_and_types_valid(&victim, fired_by) {
                return ImpactActorType::Valid;
            }
            any_invalid = true;
        }
        if any_invalid {
            ImpactActorType::Invalid
        } else {
            ImpactActorType::None
        }
    }

    fn is_valid_against_terrain(&self, w: &World, pos: WPos) -> bool {
        let cell = w.map().cell_containing(pos);
        if !w.map().contains(cell) {
            return false;
        }
        if w.map().distance_above_terrain(pos) > self.base.air_threshold {
            self.base.is_valid_target(&BitSet::new(&["Air"]))
        } else {
            self.base.is_valid_target(&w.map().terrain_type_info(cell).target_types)
        }
    }
}

impl IWarhead for CreateEffectWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        if target.target_type() == TargetType::Invalid {
            return;
        }
        let fired_by = &args.source_actor;
        let mut pos = target.center_position();
        let w = fired_by.world();
        let at_impact = if self.impact_actors {
            self.actor_type_at_impact(&w, pos, fired_by)
        } else {
            ImpactActorType::None
        };
        if at_impact == ImpactActorType::Invalid || (at_impact == ImpactActorType::None && !self.is_valid_against_terrain(&w, pos)) {
            return;
        }
        let explosion = random_or_default(&w, &self.explosions);
        if let (Some(image), Some(explosion)) = (self.image.clone(), explosion) {
            if self.inaccuracy.length > 0 {
                pos = pos + WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * self.inaccuracy.length / 1024;
            }
            if self.force_display_at_ground_level {
                pos = pos - WVec::new(0, 0, w.map().distance_above_terrain(pos).length);
            }
            let palette = PaletteRef::resolve(Some(&self.explosion_palette), self.use_player_palette, &fired_by.owner());
            w.add_frame_end_task(Box::new(move |w| {
                let e: EffectRef = SpriteEffect::at(w, pos, WAngle::ZERO, &image, &explosion, palette);
                w.add_effect(e);
            }));
        }
        if let Some(sound) = rustora_engine::sound::random_name(&w, &self.impact_sounds)
            && w.local_random.borrow_mut().next_range(0, 100) < self.impact_sound_chance
        {
            rustora_engine::sound::play_at(&w, rustora_engine::sound::SoundType::World, sound, pos, 1.0);
        }
    }
}

/// `Warheads/ShakeScreenWarhead.cs`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ShakeScreenWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    pub duration: i32,
    pub intensity: i32,
    pub multiplier: Vec<f32>,
}

impl IWarhead for ShakeScreenWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        let world = args.source_actor.world();
        if let Some(shaker) = world.world_actor().trait_or_default::<crate::cosmetic::ScreenShaker>() {
            let multiplier = (
                self.multiplier.first().copied().unwrap_or(0.0),
                self.multiplier.get(1).copied().unwrap_or(0.0),
            );
            shaker.add_effect(self.duration, target.center_position(), self.intensity, multiplier);
        }
    }
}

/// `Warheads/LeaveSmudgeWarhead.cs`.
#[derive(Clone, Debug, FieldLoad)]
pub struct LeaveSmudgeWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    pub size: Vec<i32>,
    pub smudge_type: IndexSet<String>,
    pub chance: i32,
}

impl Default for LeaveSmudgeWarhead {
    fn default() -> Self {
        LeaveSmudgeWarhead {
            base: WarheadBase::default(),
            size: vec![0, 0],
            smudge_type: IndexSet::new(),
            chance: 100,
        }
    }
}

impl IWarhead for LeaveSmudgeWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        if target.target_type() == TargetType::Invalid {
            return;
        }
        let fired_by = &args.source_actor;
        let w = fired_by.world();
        if self.chance < w.local_random.borrow_mut().next_max(100) {
            return;
        }
        let pos = target.center_position();
        if w.map().distance_above_terrain(pos) > self.base.air_threshold {
            return;
        }
        let layers = w.world_actor().traits_implementing::<crate::smudge::SmudgeLayer>();
        for cell in annulus_cells(&w, w.map().cell_containing(pos), &self.size) {
            let terrain = w.map().terrain_type_info(cell);
            let Some(kind) = terrain.accepts_smudge_type.iter().find(|t| self.smudge_type.contains(*t)) else {
                continue;
            };
            if w.actor_map()
                .get_actors_at(cell)
                .iter()
                .any(|a| !self.base.is_valid_against(a, fired_by))
            {
                continue;
            }
            if let Some(layer) = layers.iter().find(|l| &l.info.smudge_type == kind) {
                layer.add_smudge(cell);
            }
        }
    }
}

/// `RandomOrDefault(world.LocalRandom)`.
fn random_or_default(w: &World, items: &[String]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let i = w.local_random.borrow_mut().next_max(items.len() as i32);
    Some(items[i as usize].clone())
}

fn annulus_cells(w: &World, center: CPos, size: &[i32]) -> Vec<CPos> {
    let min_range = if size.len() > 1 && size[1] > 0 { size[1] } else { 0 };
    w.map().find_tiles_in_annulus(center, min_range, size[0], false).collect()
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CreateResourceWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    pub size: Vec<i32>,
    #[field(require)]
    pub adds_resource_type: String,
}

impl Default for CreateResourceWarhead {
    fn default() -> Self {
        CreateResourceWarhead {
            base: WarheadBase::default(),
            size: vec![0, 0],
            adds_resource_type: String::new(),
        }
    }
}

impl IWarhead for CreateResourceWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        if target.target_type() == TargetType::Invalid {
            return;
        }
        let w = args.source_actor.world();
        let pos = target.center_position();
        if w.map().distance_above_terrain(pos) > self.base.air_threshold {
            return;
        }
        let layer = w.world_actor().trait_::<ResourceLayer>();
        let ty = &self.adds_resource_type;
        let max_density = layer.get_max_density(ty);
        for cell in annulus_cells(&w, w.map().cell_containing(pos), &self.size) {
            if !layer.can_add_resource(ty, cell, 1) {
                continue;
            }
            let splash = w
                .shared_random
                .borrow_mut()
                .next_range(1, max_density - layer.get_resource(cell).density);
            layer.add_resource(ty, cell, splash);
        }
    }
}

/// Grants an external condition to the hit actors.
#[derive(Clone, Debug, FieldLoad)]
pub struct GrantExternalConditionWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    #[field(require)]
    pub condition: String,
    pub duration: i32,
    pub range: WDist,
}

impl Default for GrantExternalConditionWarhead {
    fn default() -> Self {
        GrantExternalConditionWarhead {
            base: WarheadBase::default(),
            condition: String::new(),
            duration: 0,
            range: WDist::from_cells(1),
        }
    }
}

impl IWarhead for GrantExternalConditionWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        let fired_by = &args.source_actor;
        let actors = match target.target_type() {
            TargetType::Invalid => return,
            TargetType::Actor => vec![target.actor().expect("actor target").clone()],
            _ => fired_by.world().find_actors_in_circle(target.center_position(), self.range),
        };
        let source = fired_by.id as usize;
        for a in actors {
            if !self.base.is_valid_against(&a, fired_by) {
                continue;
            }
            if let Some(e) = a
                .traits_implementing::<crate::experience::ExternalCondition>()
                .into_iter()
                .find(|t| t.info.condition == self.condition && t.can_grant_condition(source))
            {
                e.grant_condition(&a, source, self.duration, 0);
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct DestroyResourceWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    pub size: Vec<i32>,
    pub resource_amount: i32,
    pub resource_types: IndexSet<String>,
}

impl Default for DestroyResourceWarhead {
    fn default() -> Self {
        DestroyResourceWarhead {
            base: WarheadBase::default(),
            size: vec![0, 0],
            resource_amount: 0,
            resource_types: IndexSet::new(),
        }
    }
}

impl IWarhead for DestroyResourceWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        if target.target_type() == TargetType::Invalid {
            return;
        }
        let w = args.source_actor.world();
        let pos = target.center_position();
        if w.map().distance_above_terrain(pos) > self.base.air_threshold {
            return;
        }
        let layer = w.world_actor().trait_::<ResourceLayer>();
        for cell in annulus_cells(&w, w.map().cell_containing(pos), &self.size) {
            let contents = layer.get_resource(cell);
            let ty = contents.ty.as_deref();
            if self.resource_types.is_empty() || ty.is_some_and(|t| self.resource_types.contains(t)) {
                if self.resource_amount <= 0 {
                    layer.clear_resources(cell);
                } else if let Some(t) = ty {
                    layer.remove_resource(t, cell, self.resource_amount);
                }
            }
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct FireClusterWarhead {
    #[field(flatten)]
    pub base: WarheadBase,
    #[field(require)]
    pub weapon: String,
    pub random_cluster_count: i32,
    #[field(require)]
    pub dimensions: CVec,
    #[field(require)]
    pub footprint: String,
}

impl Default for FireClusterWarhead {
    fn default() -> Self {
        FireClusterWarhead {
            base: WarheadBase::default(),
            weapon: String::new(),
            random_cluster_count: -1,
            dimensions: CVec::ZERO,
            footprint: String::new(),
        }
    }
}

impl FireClusterWarhead {
    fn cells_matching(&self, location: CPos, random: bool) -> Vec<CPos> {
        let cell_type = if random { 'x' } else { 'X' };
        let footprint: Vec<char> = self.footprint.chars().filter(|c| !c.is_whitespace()).collect();
        let x = location.x() - (self.dimensions.x - 1) / 2;
        let y = location.y() - (self.dimensions.y - 1) / 2;
        let mut out = Vec::new();
        let mut index = 0;
        for j in 0..self.dimensions.y {
            for i in 0..self.dimensions.x {
                if footprint[index] == cell_type {
                    out.push(CPos::new(x + i, y + j));
                }
                index += 1;
            }
        }
        out
    }

    fn fire_projectile_at_cell(&self, fired_by: &ActorRef, target: &Target, cell: CPos, args: &WarheadArgs) {
        let w = fired_by.world();
        let weapon = w.rules().weapon(&self.weapon.to_lowercase()).expect("cluster weapon");
        let tc = Target::from_cell(&w, cell, FULL_CELL);
        if !weapon.is_valid_against(&tc, &w, fired_by) {
            return;
        }
        let center = w.map().center_of_cell(cell);
        let source = target.center_position();
        let facing = (center - source).yaw();
        let t2 = target.clone();
        let t3 = target.clone();
        let projectile_args = ProjectileArgs {
            weapon: weapon.clone(),
            damage_modifiers: args.damage_modifiers.clone(),
            inaccuracy_modifiers: Vec::new(),
            range_modifiers: Vec::new(),
            facing,
            current_muzzle_facing: Rc::new(move || (center - t2.center_position()).yaw()),
            source,
            current_source: Rc::new(move || t3.center_position()),
            source_actor: fired_by.clone(),
            passive_target: center,
            guided_target: tc,
        };
        if let Some(projectile) = weapon.projectile.as_ref().and_then(|p| p.create(projectile_args)) {
            w.add_frame_end_task(Box::new(move |w| w.add_effect(projectile)));
        }
    }
}

impl IWarhead for FireClusterWarhead {
    fn delay(&self) -> i32 {
        self.base.delay
    }

    fn do_impact(&self, target: &Target, args: &WarheadArgs) {
        if target.target_type() == TargetType::Invalid {
            return;
        }
        let fired_by = &args.source_actor;
        let w = fired_by.world();
        let cell = w.map().cell_containing(target.center_position());
        for c in self.cells_matching(cell, false) {
            self.fire_projectile_at_cell(fired_by, target, c, args);
        }
        if self.random_cluster_count != 0 {
            let cells = self.cells_matching(cell, true);
            let count = if self.random_cluster_count < 0 {
                cells.len() as i32
            } else {
                self.random_cluster_count
            };
            if !cells.is_empty() {
                for _ in 0..count {
                    let i = w.shared_random.borrow_mut().next_max(cells.len() as i32) as usize;
                    self.fire_projectile_at_cell(fired_by, target, cells[i], args);
                }
            }
        }
    }
}

/// `sequences.Random(world.SharedRandom)`.
fn random_sequence(w: &World, sequences: &[String]) -> String {
    let i = w.shared_random.borrow_mut().next_max(sequences.len() as i32);
    sequences[i as usize].clone()
}

/// The palette and shadow fields of `BulletInfo`, `MissileInfo` and `GravityBombInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ProjectileVisuals {
    pub palette: String,
    pub is_player_palette: bool,
    pub shadow: bool,
    pub shadow_color: Color,
    pub trail_palette: String,
    pub trail_use_player_palette: bool,
}

impl Default for ProjectileVisuals {
    fn default() -> Self {
        ProjectileVisuals {
            palette: "effect".into(),
            is_player_palette: false,
            shadow: false,
            shadow_color: Color::from_argb(140, 0, 0, 0),
            trail_palette: "effect".into(),
            trail_use_player_palette: false,
        }
    }
}

impl ProjectileVisuals {
    fn trail_palette(&self, owner: &PlayerRef) -> PaletteRef {
        PaletteRef::resolve(Some(&self.trail_palette), self.trail_use_player_palette, owner)
    }

    /// `RenderAnimation` of the projectiles: the optional shadow on the ground, then the projectile.
    fn render(&self, anim: &Animation, pos: WPos, args: &ProjectileArgs, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if fog_obscures(viewer, pos) {
            return;
        }
        let palette = PaletteRef::resolve(Some(&self.palette), self.is_player_palette, &args.source_actor.owner());
        if self.shadow {
            let w = args.source_actor.world();
            let shadow_pos = pos - WVec::new(0, 0, w.map().distance_above_terrain(pos).length);
            let first = out.len();
            render_animation(anim, shadow_pos, WVec::ZERO, 0, &palette, out);
            for r in &mut out[first..] {
                if let Renderable::Sprite(s) = r {
                    let c = self.shadow_color;
                    s.replace_color = Some([c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]);
                    s.alpha = c.a() as f32 / 255.0;
                }
            }
        }
        render_animation(anim, pos, WVec::ZERO, 0, &palette, out);
    }
}

fn projectile_animation(w: &World, image: Option<&str>, sequences: &[String], facing: Rc<dyn Fn() -> WAngle>) -> Option<Rc<Animation>> {
    let image = image.filter(|i| !i.is_empty())?;
    let anim = Animation::new(w.map().sequences().clone(), image, Some(facing), None);
    anim.play_repeating(&random_sequence(w, sequences));
    Some(anim)
}

fn add_contrail_fader(world: &World, pos: WPos, contrail: &Option<SharedContrail>) {
    if let Some(c) = contrail {
        let fader: EffectRef = ContrailFader::new(pos, c.clone());
        world.add_frame_end_task(Box::new(move |w| w.add_effect(fader)));
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct BulletInfo {
    pub bounce_sound: Option<String>,
    #[field(flatten)]
    pub visuals: ProjectileVisuals,
    #[field(flatten)]
    pub contrail: ContrailFields,
    pub speed: Vec<WDist>,
    pub image: Option<String>,
    pub sequences: Vec<String>,
    pub inaccuracy: WDist,
    pub inaccuracy_type: InaccuracyType,
    pub trail_image: Option<String>,
    pub trail_sequences: Vec<String>,
    pub trail_interval: i32,
    pub trail_delay: i32,
    pub blockable: bool,
    pub width: WDist,
    pub launch_angle: Vec<WAngle>,
    pub bounce_count: i32,
    pub bounce_range_modifier: i32,
    pub invalid_bounce_terrain: IndexSet<String>,
    pub valid_bounce_blocker_relationships: PlayerRelationship,
    pub airburst_altitude: WDist,
}

impl Default for BulletInfo {
    fn default() -> Self {
        BulletInfo {
            bounce_sound: None,
            visuals: ProjectileVisuals::default(),
            contrail: ContrailFields::default(),
            speed: vec![WDist::new(17)],
            image: None,
            sequences: vec!["idle".to_string()],
            inaccuracy: WDist::ZERO,
            inaccuracy_type: InaccuracyType::Maximum,
            trail_image: None,
            trail_sequences: vec!["idle".to_string()],
            trail_interval: 2,
            trail_delay: 1,
            blockable: true,
            width: WDist::new(1),
            launch_angle: vec![WAngle::ZERO],
            bounce_count: 0,
            bounce_range_modifier: 60,
            invalid_bounce_terrain: IndexSet::new(),
            valid_bounce_blocker_relationships: PlayerRelationship(PlayerRelationship::ENEMY.0 | PlayerRelationship::NEUTRAL.0),
            airburst_altitude: WDist::ZERO,
        }
    }
}

impl IProjectileInfo for BulletInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        let info = Rc::new(self.clone());
        let w = args.source_actor.world();
        let angle = if info.launch_angle.len() > 1 {
            WAngle::new(
                w.shared_random
                    .borrow_mut()
                    .next_range(info.launch_angle[0].angle, info.launch_angle[1].angle),
            )
        } else {
            info.launch_angle[0]
        };
        let speed = if info.speed.len() > 1 {
            WDist::new(w.shared_random.borrow_mut().next_range(info.speed[0].length, info.speed[1].length))
        } else {
            info.speed[0]
        };
        let mut target = args.passive_target;
        if info.inaccuracy.length > 0 {
            let max = get_projectile_inaccuracy(info.inaccuracy.length, info.inaccuracy_type, &args);
            target = target + WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * max / 1024;
        }
        if info.airburst_altitude > WDist::ZERO {
            target = target + WVec::new(0, 0, info.airburst_altitude.length);
        }
        let pos = args.source;
        let facing_ticks = Rc::new(Cell::new(0));
        let length = 1.max((target - pos).length() / speed.length);
        let yaw = (target - pos).yaw();
        let f = facing_ticks.clone();
        let anim = projectile_animation(
            &w,
            info.image.as_deref(),
            &info.sequences,
            Rc::new(move || effective_facing(f.get(), length, angle, yaw)),
        );
        let contrail = info.contrail.create(&args.source_actor);
        Some(Rc::new_cyclic(|me: &Weak<Bullet>| Bullet {
            me: me.clone(),
            anim,
            contrail,
            yaw,
            facing_ticks,
            speed,
            angle,
            pos: Cell::new(pos),
            last_pos: Cell::new(WPos::ZERO),
            target: Cell::new(target),
            source: Cell::new(pos),
            length: Cell::new(length),
            ticks: Cell::new(0),
            smoke_ticks: Cell::new(info.trail_delay),
            remaining_bounces: Cell::new(info.bounce_count),
            info,
            args,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `Bullet.GetEffectiveFacing`.
fn effective_facing(ticks: i32, length: i32, angle: WAngle, facing: WAngle) -> WAngle {
    let at = ticks as f32 / (length - 1) as f32;
    let attitude = angle.tan() as f32 * (1.0 - 2.0 * at) / (4.0 * 1024.0);
    let u = (facing.angle % 512) as f32 / 512.0;
    let scale = 2048.0 * u * (1.0 - u);
    let f = facing.angle as f32;
    WAngle::new(if facing.angle < 512 {
        f - scale * attitude
    } else {
        f + scale * attitude
    } as i32)
}

struct Bullet {
    me: Weak<Bullet>,
    anim: Option<Rc<Animation>>,
    contrail: Option<SharedContrail>,
    yaw: WAngle,
    /// A copy of `ticks` for the facing function of the animation.
    facing_ticks: Rc<Cell<i32>>,
    info: Rc<BulletInfo>,
    args: ProjectileArgs,
    speed: WDist,
    angle: WAngle,
    pos: Cell<WPos>,
    last_pos: Cell<WPos>,
    target: Cell<WPos>,
    source: Cell<WPos>,
    length: Cell<i32>,
    ticks: Cell<i32>,
    smoke_ticks: Cell<i32>,
    remaining_bounces: Cell<i32>,
}

impl Bullet {
    fn should_explode(&self, w: &Rc<World>) -> bool {
        let info = &self.info;
        if info.blockable
            && let Some(hit) =
                any_blocking_actors_between(w, &self.args.source_actor.owner(), self.last_pos.get(), self.pos.get(), info.width)
        {
            self.pos.set(hit);
            return true;
        }
        if info.trail_image.as_deref().is_some_and(|i| !i.is_empty()) {
            self.smoke_ticks.set(self.smoke_ticks.get() - 1);
            if self.smoke_ticks.get() < 0 {
                let info2 = info.clone();
                let delayed = WPos::lerp_quadratic(
                    self.source.get(),
                    self.target.get(),
                    self.angle,
                    self.ticks.get() - info.trail_delay,
                    self.length.get(),
                );
                let facing = effective_facing(self.ticks.get(), self.length.get(), self.angle, self.yaw);
                let palette = info.visuals.trail_palette(&self.args.source_actor.owner());
                w.add_frame_end_task(Box::new(move |w| {
                    let seq = random_sequence(w, &info2.trail_sequences);
                    let image = info2.trail_image.as_deref().unwrap_or_default();
                    let e: EffectRef = SpriteEffect::at(w, delayed, facing, image, &seq, palette);
                    w.add_effect(e);
                }));
                self.smoke_ticks.set(info.trail_interval);
            }
        }
        if let Some(c) = &self.contrail {
            c.borrow_mut().update(self.pos.get());
        }
        let t = self.ticks.get();
        self.ticks.set(t + 1);
        let flight_length_reached = t >= self.length.get();
        let should_bounce = self.remaining_bounces.get() > 0;
        if flight_length_reached && should_bounce {
            let pos = self.pos.get();
            let cell = w.map().cell_containing(pos);
            if !w.map().contains(cell)
                || info.invalid_bounce_terrain.contains(&w.map().terrain_type_info(cell).ty)
                || self.any_valid_targets_in_radius(w, pos)
            {
                return true;
            }
            let mut target = self.target.get() + (pos - self.source.get()) * info.bounce_range_modifier / 100;
            let dat = w.map().distance_above_terrain(target);
            target = target + WVec::new(0, 0, -dat.length);
            self.target.set(target);
            self.length.set(1.max((target - pos).length() / self.speed.length));
            self.ticks.set(0);
            self.source.set(pos);
            if let Some(s) = &info.bounce_sound {
                rustora_engine::sound::play_at(w, rustora_engine::sound::SoundType::World, s, pos, 1.0);
            }
            self.remaining_bounces.set(self.remaining_bounces.get() - 1);
        }
        if flight_length_reached && !should_bounce {
            return true;
        }
        let pos = self.pos.get();
        if w.map().distance_above_terrain(pos).length < 0 {
            return true;
        }
        self.remaining_bounces.get() < info.bounce_count && self.any_valid_targets_in_radius(w, pos)
    }

    fn any_valid_targets_in_radius(&self, w: &World, pos: WPos) -> bool {
        let fired_by = &self.args.source_actor;
        for victim in find_actors_on_circle(w, pos, self.info.width) {
            if !Target::from_actor(Some(&victim)).is_valid_for(fired_by) {
                continue;
            }
            let is_guided = self.args.guided_target.actor().is_some_and(|g| g.id == victim.id);
            if !is_guided
                && !self
                    .info
                    .valid_bounce_blocker_relationships
                    .has_relationship(fired_by.owner().relationship_with(Some(&victim.owner())))
            {
                continue;
            }
            if victim
                .traits_implementing::<HitShape>()
                .iter()
                .any(|h| !h.is_trait_disabled() && h.distance_from_edge(&victim, pos).length <= 0)
            {
                return true;
            }
        }
        false
    }
}

impl Effect for Bullet {
    fn tick(&self, world: &Rc<World>) {
        if let Some(a) = &self.anim {
            a.tick();
        }
        self.facing_ticks.set(self.ticks.get());
        self.last_pos.set(self.pos.get());
        self.pos.set(WPos::lerp_quadratic(
            self.source.get(),
            self.target.get(),
            self.angle,
            self.ticks.get(),
            self.length.get(),
        ));
        if self.should_explode(world) {
            add_contrail_fader(world, self.pos.get(), &self.contrail);
            let me: EffectRef = self.me.upgrade().expect("bullet");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
            let pos = self.pos.get();
            let mut a = WarheadArgs::from_projectile(&self.args);
            a.impact_orientation = WRot::new(WAngle::ZERO, util::get_vertical_angle(self.last_pos.get(), pos), self.args.facing);
            a.impact_position = pos;
            weapon_impact_args(&self.args.weapon, &Target::from_pos(pos), &a);
        }
    }

    fn is_sync(&self) -> bool {
        true
    }

    fn sync_hash(&self) -> i32 {
        self.pos.get().hash() ^ self.last_pos.get().hash() ^ self.target.get().hash() ^ self.source.get().hash()
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if let Some(c) = &self.contrail {
            c.borrow().render(viewer, out);
        }
        if self.ticks.get() >= self.length.get() {
            return;
        }
        if let Some(a) = &self.anim {
            self.info.visuals.render(a, self.pos.get(), &self.args, viewer, out);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct InstantHitInfo {
    pub inaccuracy: WDist,
    pub inaccuracy_type: InaccuracyType,
    pub blockable: bool,
    pub width: WDist,
}

impl Default for InstantHitInfo {
    fn default() -> Self {
        InstantHitInfo {
            inaccuracy: WDist::ZERO,
            inaccuracy_type: InaccuracyType::Maximum,
            blockable: false,
            width: WDist::new(1),
        }
    }
}

impl IProjectileInfo for InstantHitInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        let target = if args.weapon.target_actor_center {
            args.guided_target.clone()
        } else if self.inaccuracy.length > 0 {
            let max = get_projectile_inaccuracy(self.inaccuracy.length, self.inaccuracy_type, &args);
            let w = args.source_actor.world();
            let offset = WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * max / 1024;
            Target::from_pos(args.passive_target + offset)
        } else {
            Target::from_pos(args.passive_target)
        };
        Some(Rc::new_cyclic(|me: &Weak<InstantHit>| InstantHit {
            me: me.clone(),
            info: self.clone(),
            args,
            target: RefCell::new(target),
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct InstantHit {
    me: Weak<InstantHit>,
    info: InstantHitInfo,
    args: ProjectileArgs,
    target: RefCell<Target>,
}

impl Effect for InstantHit {
    fn tick(&self, world: &Rc<World>) {
        let args = &self.args;
        if self.target.borrow().target_type() == TargetType::Invalid {
            *self.target.borrow_mut() = Target::from_pos(args.passive_target);
        }
        let center = self.target.borrow().center_position();
        if self.info.blockable
            && let Some(hit) = any_blocking_actors_between(world, &args.source_actor.owner(), args.source, center, self.info.width)
        {
            *self.target.borrow_mut() = Target::from_pos(hit);
        }
        let target = self.target.borrow().clone();
        let center = target.center_position();
        let mut a = WarheadArgs::from_projectile(args);
        a.impact_orientation = WRot::new(WAngle::ZERO, util::get_vertical_angle(args.source, center), args.facing);
        a.impact_position = center;
        weapon_impact_args(&args.weapon, &target, &a);
        let me: EffectRef = self.me.upgrade().expect("instant hit");
        world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GravityBombInfo {
    #[field(flatten)]
    pub visuals: ProjectileVisuals,
    pub image: Option<String>,
    pub sequences: Vec<String>,
    pub open_sequence: Option<String>,
    pub velocity: WVec,
    pub acceleration: WVec,
}

impl Default for GravityBombInfo {
    fn default() -> Self {
        GravityBombInfo {
            visuals: ProjectileVisuals::default(),
            image: None,
            sequences: vec!["idle".to_string()],
            open_sequence: None,
            velocity: WVec::ZERO,
            acceleration: WVec::new(0, 0, -15),
        }
    }
}

impl IProjectileInfo for GravityBombInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        let w = args.source_actor.world();
        let facing = args.facing;
        let anim = self.image.as_deref().filter(|i| !i.is_empty()).map(|image| {
            let a = Animation::new(w.map().sequences().clone(), image, Some(Rc::new(move || facing)), None);
            match self.open_sequence.as_deref().filter(|s| !s.is_empty()) {
                Some(open) => {
                    let (world, sequences, weak) = (w.clone(), self.sequences.clone(), Rc::downgrade(&a));
                    a.play_then(
                        open,
                        Some(Rc::new(move || {
                            let seq = random_sequence(&world, &sequences);
                            if let Some(a) = weak.upgrade() {
                                a.play_repeating(&seq);
                            }
                        })),
                    );
                }
                None => a.play_repeating(&random_sequence(&w, &self.sequences)),
            }
            a
        });
        let v = self.velocity;
        let a = self.acceleration;
        let velocity = WVec::new(v.y, -v.x, v.z).rotate(&WRot::from_yaw(args.facing));
        Some(Rc::new_cyclic(|me: &Weak<GravityBomb>| GravityBomb {
            me: me.clone(),
            visuals: self.visuals.clone(),
            anim,
            acceleration: WVec::new(a.y, -a.x, a.z),
            velocity: Cell::new(velocity),
            pos: Cell::new(args.source),
            last_pos: Cell::new(WPos::ZERO),
            args,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct GravityBomb {
    me: Weak<GravityBomb>,
    visuals: ProjectileVisuals,
    anim: Option<Rc<Animation>>,
    acceleration: WVec,
    velocity: Cell<WVec>,
    pos: Cell<WPos>,
    last_pos: Cell<WPos>,
    args: ProjectileArgs,
}

impl Effect for GravityBomb {
    fn tick(&self, world: &Rc<World>) {
        self.last_pos.set(self.pos.get());
        self.pos.set(self.pos.get() + self.velocity.get());
        self.velocity.set(self.velocity.get() + self.acceleration);
        let z = self.args.passive_target.z;
        if self.pos.get().z <= z {
            self.pos.set(self.pos.get() + WVec::new(0, 0, z - self.pos.get().z));
            let me: EffectRef = self.me.upgrade().expect("bomb");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
            let pos = self.pos.get();
            let mut a = WarheadArgs::from_projectile(&self.args);
            a.impact_orientation = WRot::new(WAngle::ZERO, util::get_vertical_angle(self.last_pos.get(), pos), self.args.facing);
            a.impact_position = pos;
            weapon_impact_args(&self.args.weapon, &Target::from_pos(pos), &a);
        }
        if let Some(a) = &self.anim {
            a.tick();
        }
    }

    fn is_sync(&self) -> bool {
        true
    }

    fn sync_hash(&self) -> i32 {
        self.pos.get().hash() ^ self.last_pos.get().hash()
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if let Some(a) = &self.anim {
            self.visuals.render(a, self.pos.get(), &self.args, viewer, out);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TeslaZapInfo {
    pub image: String,
    pub bright_sequence: String,
    pub dim_sequence: String,
    pub palette: String,
    pub bright_zaps: i32,
    pub dim_zaps: i32,
    pub duration: i32,
    pub damage_duration: i32,
    pub track_target: bool,
}

impl Default for TeslaZapInfo {
    fn default() -> Self {
        TeslaZapInfo {
            image: "litning".into(),
            bright_sequence: "bright".into(),
            dim_sequence: "dim".into(),
            palette: "effect".into(),
            bright_zaps: 1,
            dim_zaps: 2,
            duration: 2,
            damage_duration: 1,
            track_target: true,
        }
    }
}

impl IProjectileInfo for TeslaZapInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        Some(Rc::new_cyclic(|me: &Weak<TeslaZap>| TeslaZap {
            me: me.clone(),
            info: self.clone(),
            ticks_until_remove: Cell::new(self.duration),
            damage_duration: Cell::new(self.damage_duration.min(self.duration)),
            target: Cell::new(args.passive_target),
            args,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

const ZAP_STEPS: [[i32; 5]; 8] = [
    [8, 8, 4, 4, 0],
    [-8, -8, -4, -4, 0],
    [8, 0, 4, 4, 1],
    [-8, 0, -4, 4, 1],
    [0, 8, 4, 4, 2],
    [0, -8, 4, -4, 2],
    [-8, 8, -4, 4, 3],
    [8, -8, 4, -4, 3],
];

/// `WorldRenderer.ScreenPosition` with the RA tile size.
fn zap_screen_position(p: WPos) -> [f32; 2] {
    [24.0 * p.x as f32 / 1024.0, 24.0 * (p.y - p.z) as f32 / 1024.0]
}

/// `TeslaZapRenderable.DrawZapWandering`.
fn draw_zap_wandering(
    random: &mut MersenneTwister,
    from: [f32; 2],
    to: [f32; 2],
    seq: &Rc<Sequence>,
    palette: &PaletteRef,
    items: &mut Vec<Renderable>,
) {
    let dist = [to[0] - from[0], to[1] - from[1]];
    let len = (dist[0] * dist[0] + dist[1] * dist[1]).sqrt();
    if len == 0.0 {
        return;
    }
    let norm = [-dist[1] / len, dist[0] / len];
    let wander = |random: &mut MersenneTwister, t: f32| {
        let d = WDist::from_pdf(random, 2).length as f32 * len / 4096.0;
        [from[0] + t * dist[0] + d * norm[0], from[1] + t * dist[1] + d * norm[1]]
    };
    if random.next_max(2) != 0 {
        let p1 = wander(random, 1.0 / 3.0);
        let p2 = wander(random, 2.0 / 3.0);
        let p1 = draw_zap(from, p1, seq, palette, items);
        let p2 = draw_zap(p1, p2, seq, palette, items);
        draw_zap(p2, to, seq, palette, items);
    } else {
        let p1 = wander(random, 0.5);
        let p1 = draw_zap(from, p1, seq, palette, items);
        draw_zap(p1, to, seq, palette, items);
    }
}

/// `TeslaZapRenderable.DrawZap`. Returns the end of the drawn path.
fn draw_zap(from: [f32; 2], to: [f32; 2], seq: &Rc<Sequence>, palette: &PaletteRef, items: &mut Vec<Renderable>) -> [f32; 2] {
    let dist = [to[0] - from[0], to[1] - from[1]];
    let q = [-dist[1], dist[0]];
    let c = -(from[0] * q[0] + from[1] * q[1]);
    let mut z = from;
    let mut count = 0;
    while (to[0] - z[0]).abs() > 5.0 || (to[1] - z[1]).abs() > 5.0 {
        let cur = (to[0] - z[0]).powi(2) + (to[1] - z[1]).powi(2);
        let moved = |t: &[i32; 5]| [z[0] + t[0] as f32, z[1] + t[1] as f32];
        let step = ZAP_STEPS
            .iter()
            .filter(|t| {
                let m = moved(t);
                (to[0] - m[0]).powi(2) + (to[1] - m[1]).powi(2) < cur
            })
            .min_by(|a, b| {
                let key = |t: &[i32; 5]| {
                    let m = moved(t);
                    (m[0] * q[0] + m[1] * q[1] + c).abs()
                };
                key(a).total_cmp(&key(b))
            })
            .expect("a zap step approaches the target");
        let (x, y) = ((z[0] + step[2] as f32) as i32, (z[1] + step[3] as f32) as i32);
        items.push(Renderable::Sprite(SpriteRenderable {
            sequence: seq.clone(),
            frame: step[4],
            facing: WAngle::ZERO,
            pos: WPos::new(1024 * x / 24, 1024 * y / 24, 0),
            offset: WVec::ZERO,
            z_offset: 0,
            palette: palette.clone(),
            alpha: 1.0,
            replace_color: None,
            is_decoration: true,
        }));
        z = [z[0] + step[0] as f32, z[1] + step[1] as f32];
        count += 1;
        if count >= 1000 {
            break;
        }
    }
    z
}

struct TeslaZap {
    me: Weak<TeslaZap>,
    info: TeslaZapInfo,
    args: ProjectileArgs,
    ticks_until_remove: Cell<i32>,
    damage_duration: Cell<i32>,
    target: Cell<WPos>,
}

impl Effect for TeslaZap {
    fn tick(&self, world: &Rc<World>) {
        let t = self.ticks_until_remove.get();
        self.ticks_until_remove.set(t - 1);
        if t <= 0 {
            let me: EffectRef = self.me.upgrade().expect("zap");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
        let args = &self.args;
        if self.info.track_target && args.guided_target.is_valid_for(&args.source_actor) {
            let p = if args.weapon.target_actor_center {
                args.guided_target.center_position()
            } else {
                util::closest_position(&args.guided_target.positions(), args.source).unwrap_or(WPos::ZERO)
            };
            self.target.set(p);
        }
        let d = self.damage_duration.get();
        self.damage_duration.set(d - 1);
        if d > 0 {
            weapon_impact_args(
                &args.weapon,
                &Target::from_pos(self.target.get()),
                &WarheadArgs::from_projectile(args),
            );
        }
    }

    fn is_sync(&self) -> bool {
        true
    }

    fn sync_hash(&self) -> i32 {
        self.target.get().hash()
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        let source = self.args.source;
        let target = self.target.get();
        if fog_obscures(viewer, source) && fog_obscures(viewer, target) {
            return;
        }
        let w = self.args.source_actor.world();
        let seqs = w.map().sequences();
        let info = &self.info;
        let (bright, dim) = (
            seqs.get_sequence(&info.image, &info.bright_sequence),
            seqs.get_sequence(&info.image, &info.dim_sequence),
        );
        let palette = PaletteRef::named(&info.palette);
        let (from, to) = (zap_screen_position(source), zap_screen_position(target));
        let mut items = Vec::new();
        let mut random = w.local_random.borrow_mut();
        for (seq, count) in [(&dim, info.dim_zaps), (&bright, info.bright_zaps)] {
            for _ in 0..count {
                draw_zap_wandering(&mut random, from, to, seq, &palette, &mut items);
            }
        }
        out.push(Renderable::Group {
            pos: source,
            z_offset: 0,
            items,
        });
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `LaserZapInfo`. `Shape` and `SecondaryBeamShape` only affect rendering and are kept as text.
#[derive(Clone, Debug, FieldLoad)]
pub struct LaserZapInfo {
    pub width: WDist,
    pub shape: String,
    pub z_offset: i32,
    pub duration: i32,
    pub damage_duration: i32,
    pub damage_interval: i32,
    pub use_player_color: bool,
    pub color: Color,
    pub track_target: bool,
    pub inaccuracy: WDist,
    pub inaccuracy_type: InaccuracyType,
    pub blockable: bool,
    pub secondary_beam: bool,
    pub secondary_beam_width: WDist,
    pub secondary_beam_shape: String,
    pub secondary_beam_z_offset: i32,
    pub secondary_beam_use_player_color: bool,
    pub secondary_beam_color: Color,
    pub hit_anim: Option<String>,
    pub hit_anim_sequence: String,
    pub hit_anim_palette: String,
    pub launch_effect_image: Option<String>,
    pub launch_effect_sequence: Option<String>,
    pub launch_effect_palette: String,
}

impl Default for LaserZapInfo {
    fn default() -> Self {
        let red = Color::from_argb(255, 255, 0, 0);
        LaserZapInfo {
            width: WDist::new(86),
            shape: "Cylindrical".into(),
            z_offset: 0,
            duration: 10,
            damage_duration: 1,
            damage_interval: 1,
            use_player_color: false,
            color: red,
            track_target: true,
            inaccuracy: WDist::ZERO,
            inaccuracy_type: InaccuracyType::Maximum,
            blockable: false,
            secondary_beam: false,
            secondary_beam_width: WDist::new(86),
            secondary_beam_shape: "Cylindrical".into(),
            secondary_beam_z_offset: 0,
            secondary_beam_use_player_color: false,
            secondary_beam_color: red,
            hit_anim: None,
            hit_anim_sequence: "idle".into(),
            hit_anim_palette: "effect".into(),
            launch_effect_image: None,
            launch_effect_sequence: None,
            launch_effect_palette: "effect".into(),
        }
    }
}

impl IProjectileInfo for LaserZapInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        let w = args.source_actor.world();
        let owner_color = args.source_actor.owner().color;
        let color = if self.use_player_color { owner_color } else { self.color };
        let secondary_color = if self.secondary_beam_use_player_color {
            owner_color
        } else {
            self.secondary_beam_color
        };
        let mut target = args.passive_target;
        if self.inaccuracy.length > 0 {
            let max = get_projectile_inaccuracy(self.inaccuracy.length, self.inaccuracy_type, &args);
            target = target + WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * max / 1024;
        }
        let hit_anim = self
            .hit_anim
            .as_deref()
            .filter(|i| !i.is_empty())
            .map(|image| Animation::new(w.map().sequences().clone(), image, None, None));
        Some(Rc::new_cyclic(|me: &Weak<LaserZap>| LaserZap {
            me: me.clone(),
            info: self.clone(),
            color,
            secondary_color,
            show_hit_anim: Rc::new(Cell::new(hit_anim.is_some())),
            hit_anim,
            ticks: Cell::new(0),
            interval: Cell::new(0),
            target: Cell::new(target),
            source: Cell::new(args.source),
            args,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `LaserZap`: a beam that deals damage for `DamageDuration` ticks and fades over `Duration` ticks.
struct LaserZap {
    me: Weak<LaserZap>,
    info: LaserZapInfo,
    args: ProjectileArgs,
    color: Color,
    secondary_color: Color,
    hit_anim: Option<Rc<Animation>>,
    show_hit_anim: Rc<Cell<bool>>,
    ticks: Cell<i32>,
    interval: Cell<i32>,
    target: Cell<WPos>,
    source: Cell<WPos>,
}

impl LaserZap {
    fn beam(&self, color: Color, width: WDist, out: &mut Vec<Renderable>) {
        let info = &self.info;
        let a = (info.duration - self.ticks.get()) * color.a() as i32 / info.duration;
        let c = crate::effects::premultiplied(Color::from_argb(a.clamp(0, 255) as u8, color.r(), color.g(), color.b()));
        out.push(Renderable::Line {
            from: self.source.get(),
            to: self.target.get(),
            width: width.length as f32,
            from_color: c,
            to_color: c,
        });
    }
}

impl Effect for LaserZap {
    fn tick(&self, world: &Rc<World>) {
        let args = &self.args;
        let info = &self.info;
        self.source.set((args.current_source)());
        if self.ticks.get() == 0
            && let (Some(image), Some(sequence)) = (info.launch_effect_image.as_deref(), info.launch_effect_sequence.as_deref())
            && !image.is_empty()
            && !sequence.is_empty()
        {
            let (pos, facing) = (self.source.get(), (args.current_muzzle_facing)());
            let (image, sequence, palette) = (image.to_string(), sequence.to_string(), info.launch_effect_palette.clone());
            world.add_frame_end_task(Box::new(move |w| {
                w.add_effect(SpriteEffect::at(w, pos, facing, &image, &sequence, PaletteRef::named(&palette)));
            }));
        }
        if info.track_target && args.guided_target.is_valid_for(&args.source_actor) {
            let p = if args.weapon.target_actor_center {
                args.guided_target.center_position()
            } else {
                util::closest_position(&args.guided_target.positions(), self.source.get()).unwrap_or(WPos::ZERO)
            };
            self.target.set(p);
        }
        if info.blockable
            && let Some(hit) =
                any_blocking_actors_between(world, &args.source_actor.owner(), self.source.get(), self.target.get(), info.width)
        {
            self.target.set(hit);
        }
        if self.ticks.get() < info.damage_duration {
            self.interval.set(self.interval.get() - 1);
            if self.interval.get() <= 0 {
                let target = self.target.get();
                let mut a = WarheadArgs::from_projectile(args);
                a.impact_orientation = WRot::new(
                    WAngle::ZERO,
                    util::get_vertical_angle(self.source.get(), target),
                    (args.current_muzzle_facing)(),
                );
                a.impact_position = target;
                weapon_impact_args(&args.weapon, &Target::from_pos(target), &a);
                self.interval.set(info.damage_interval);
            }
        }
        if self.show_hit_anim.get()
            && let Some(anim) = &self.hit_anim
        {
            if self.ticks.get() == 0 {
                let show = self.show_hit_anim.clone();
                anim.play_then(&info.hit_anim_sequence, Some(Rc::new(move || show.set(false))));
            }
            anim.tick();
        }
        self.ticks.set(self.ticks.get() + 1);
        if self.ticks.get() >= info.duration && !self.show_hit_anim.get() {
            let me: EffectRef = self.me.upgrade().expect("laser zap");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
    }

    fn is_sync(&self) -> bool {
        true
    }

    fn sync_hash(&self) -> i32 {
        self.target.get().hash() ^ self.source.get().hash()
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        let (source, target) = (self.source.get(), self.target.get());
        if fog_obscures(viewer, target) && fog_obscures(viewer, source) {
            return;
        }
        let info = &self.info;
        if self.ticks.get() < info.duration {
            self.beam(self.color, info.width, out);
            if info.secondary_beam {
                self.beam(self.secondary_color, info.secondary_beam_width, out);
            }
        }
        if self.show_hit_anim.get()
            && let Some(anim) = &self.hit_anim
        {
            render_animation(anim, target, WVec::ZERO, 0, &PaletteRef::named(&info.hit_anim_palette), out);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `RailgunInfo`. The helix is not drawn; the beam is drawn as a fading line.
#[derive(Clone, Debug, FieldLoad)]
pub struct RailgunInfo {
    pub damage_actors_in_line: bool,
    pub inaccuracy: WDist,
    pub inaccuracy_type: InaccuracyType,
    pub blockable: bool,
    pub duration: i32,
    pub z_offset: i32,
    pub beam_width: WDist,
    pub beam_shape: String,
    pub beam_color: Color,
    pub beam_player_color: bool,
    pub beam_alpha_delta_per_tick: i32,
    pub helix_thickness: WDist,
    pub helix_radius: WDist,
    pub helix_pitch: WDist,
    pub helix_radius_delta_per_tick: i32,
    pub helix_alpha_delta_per_tick: i32,
    pub helix_angle_delta_per_tick: WAngle,
    pub quantization_count: i32,
    pub helix_color: Color,
    pub helix_player_color: bool,
    pub hit_anim: Option<String>,
    pub hit_anim_sequence: String,
    pub hit_anim_palette: String,
}

impl Default for RailgunInfo {
    fn default() -> Self {
        RailgunInfo {
            damage_actors_in_line: false,
            inaccuracy: WDist::ZERO,
            inaccuracy_type: InaccuracyType::Maximum,
            blockable: false,
            duration: 15,
            z_offset: 0,
            beam_width: WDist::new(86),
            beam_shape: "Cylindrical".into(),
            beam_color: Color::from_argb(128, 255, 255, 255),
            beam_player_color: false,
            beam_alpha_delta_per_tick: -8,
            helix_thickness: WDist::new(32),
            helix_radius: WDist::new(64),
            helix_pitch: WDist::new(512),
            helix_radius_delta_per_tick: 8,
            helix_alpha_delta_per_tick: -8,
            helix_angle_delta_per_tick: WAngle::new(16),
            quantization_count: 16,
            helix_color: Color::from_argb(128, 255, 255, 255),
            helix_player_color: false,
            hit_anim: None,
            hit_anim_sequence: "idle".into(),
            hit_anim_palette: "effect".into(),
        }
    }
}

impl IProjectileInfo for RailgunInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        let w = args.source_actor.world();
        let c = args.source_actor.owner().color;
        let beam_color = if self.beam_player_color {
            Color::from_argb(self.beam_color.a(), c.r(), c.g(), c.b())
        } else {
            self.beam_color
        };
        let mut target = args.passive_target;
        if self.inaccuracy.length > 0 {
            let max = get_projectile_inaccuracy(self.inaccuracy.length, self.inaccuracy_type, &args);
            target = target + WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * max / 1024;
        }
        let hit_anim = self
            .hit_anim
            .as_deref()
            .filter(|i| !i.is_empty())
            .map(|image| Animation::new(w.map().sequences().clone(), image, None, None));
        if self.blockable
            && let Some(hit) = any_blocking_actors_between(&w, &args.source_actor.owner(), target, args.source, self.beam_width)
        {
            target = hit;
        }
        Some(Rc::new_cyclic(|me: &Weak<Railgun>| Railgun {
            me: me.clone(),
            info: self.clone(),
            beam_color,
            hit_anim,
            animation_complete: Rc::new(Cell::new(false)),
            ticks: Cell::new(0),
            target,
            args,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `Railgun`: hits the target, or every actor on the line, on its first tick.
struct Railgun {
    me: Weak<Railgun>,
    info: RailgunInfo,
    args: ProjectileArgs,
    beam_color: Color,
    hit_anim: Option<Rc<Animation>>,
    animation_complete: Rc<Cell<bool>>,
    ticks: Cell<i32>,
    target: WPos,
}

impl Effect for Railgun {
    fn tick(&self, world: &Rc<World>) {
        let args = &self.args;
        let target = self.target;
        if self.ticks.get() == 0 {
            match &self.hit_anim {
                Some(anim) => {
                    let done = self.animation_complete.clone();
                    anim.play_then(&self.info.hit_anim_sequence, Some(Rc::new(move || done.set(true))));
                }
                None => self.animation_complete.set(true),
            }
            let orientation = WRot::new(WAngle::ZERO, util::get_vertical_angle(args.source, target), args.facing);
            if !self.info.damage_actors_in_line {
                let mut a = WarheadArgs::from_projectile(args);
                a.impact_orientation = orientation;
                a.impact_position = target;
                weapon_impact_args(&args.weapon, &Target::from_pos(target), &a);
            } else {
                for actor in find_actors_on_line(world, args.source, target, self.info.beam_width, false) {
                    let mut a = WarheadArgs::from_projectile(args);
                    a.impact_orientation = orientation;
                    a.impact_position = actor.center_position();
                    weapon_impact_args(&args.weapon, &Target::from_actor(Some(&actor)), &a);
                }
            }
        }
        if let Some(anim) = &self.hit_anim {
            anim.tick();
        }
        let t = self.ticks.get();
        self.ticks.set(t + 1);
        if t > self.info.duration && self.animation_complete.get() {
            let me: EffectRef = self.me.upgrade().expect("railgun");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
    }

    fn is_sync(&self) -> bool {
        true
    }

    fn sync_hash(&self) -> i32 {
        self.target.hash()
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        let source = self.args.source;
        if fog_obscures(viewer, self.target) && fog_obscures(viewer, source) {
            return;
        }
        let info = &self.info;
        if self.ticks.get() < info.duration {
            let c = self.beam_color;
            let a = (c.a() as i32 + info.beam_alpha_delta_per_tick * self.ticks.get()).clamp(0, 255) as u8;
            let color = crate::effects::premultiplied(Color::from_argb(a, c.r(), c.g(), c.b()));
            out.push(Renderable::Line {
                from: source,
                to: self.target,
                width: info.beam_width.length as f32,
                from_color: color,
                to_color: color,
            });
        }
        if let Some(anim) = &self.hit_anim {
            render_animation(anim, self.target, WVec::ZERO, 0, &PaletteRef::named(&info.hit_anim_palette), out);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct MissileInfo {
    #[field(flatten)]
    pub visuals: ProjectileVisuals,
    #[field(flatten)]
    pub contrail: ContrailFields,
    pub image: Option<String>,
    pub sequences: Vec<String>,
    pub minimum_launch_angle: WAngle,
    pub maximum_launch_angle: WAngle,
    pub minimum_launch_speed: WDist,
    pub maximum_launch_speed: WDist,
    pub speed: WDist,
    pub acceleration: WDist,
    pub arm: i32,
    pub blockable: bool,
    pub width: WDist,
    pub inaccuracy: WDist,
    pub inaccuracy_type: InaccuracyType,
    pub lock_on_inaccuracy: WDist,
    pub lock_on_probability: i32,
    pub horizontal_rate_of_turn: WAngle,
    pub vertical_rate_of_turn: WAngle,
    pub gravity: i32,
    pub range_limit: WDist,
    pub explode_when_empty: bool,
    pub airburst_altitude: WDist,
    pub cruise_altitude: WDist,
    pub homing_activation_delay: i32,
    pub trail_image: Option<String>,
    pub trail_sequences: Vec<String>,
    pub trail_interval: i32,
    pub trail_when_deactivated: bool,
    pub jammable: bool,
    pub jammed_diversion_range: i32,
    pub bound_to_terrain_type: String,
    pub allow_snapping: bool,
    pub close_enough: WDist,
}

impl Default for MissileInfo {
    fn default() -> Self {
        MissileInfo {
            visuals: ProjectileVisuals::default(),
            contrail: ContrailFields::default(),
            image: None,
            sequences: vec!["idle".to_string()],
            minimum_launch_angle: WAngle::new(-64),
            maximum_launch_angle: WAngle::new(128),
            minimum_launch_speed: WDist::new(-1),
            maximum_launch_speed: WDist::new(-1),
            speed: WDist::new(384),
            acceleration: WDist::new(5),
            arm: 0,
            blockable: true,
            width: WDist::new(1),
            inaccuracy: WDist::ZERO,
            inaccuracy_type: InaccuracyType::Absolute,
            lock_on_inaccuracy: WDist::new(-1),
            lock_on_probability: 100,
            horizontal_rate_of_turn: WAngle::new(20),
            vertical_rate_of_turn: WAngle::new(24),
            gravity: 10,
            range_limit: WDist::ZERO,
            explode_when_empty: true,
            airburst_altitude: WDist::ZERO,
            cruise_altitude: WDist::new(512),
            homing_activation_delay: 0,
            trail_image: None,
            trail_sequences: vec!["idle".to_string()],
            trail_interval: 2,
            trail_when_deactivated: false,
            jammable: true,
            jammed_diversion_range: 20,
            bound_to_terrain_type: String::new(),
            allow_snapping: false,
            close_enough: WDist::new(298),
        }
    }
}

fn loop_radius(speed: i32, rot: i32) -> i32 {
    speed.wrapping_mul(6400) / (157 * rot)
}

fn exts_clamp(v: i32, min: i32, max: i32) -> i32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

fn sbyte_angle(a: WAngle) -> i32 {
    (a.angle >> 2) as i8 as i32
}

fn tick_facing_int(facing: i32, desired: i32, rot: i32) -> i32 {
    let left = (facing - desired) & 0xFF;
    if left < rot {
        return desired & 0xFF;
    }
    let right = (desired - facing) & 0xFF;
    if right < rot {
        return desired & 0xFF;
    }
    if right < left {
        return (facing + rot) & 0xFF;
    }
    (facing - rot) & 0xFF
}

fn normalize_facing(f: i32) -> i32 {
    if f >= 0 {
        return f & 0xFF;
    }
    let negative = -f & 0xFF;
    if negative == 0 { 0 } else { 256 - negative }
}

fn facing_rotated(v: WVec, v_facing: i32, h_facing: i32) -> WVec {
    v.rotate(&WRot::new(WAngle::from_facing(v_facing), WAngle::ZERO, WAngle::ZERO))
        .rotate(&WRot::new(WAngle::ZERO, WAngle::ZERO, WAngle::from_facing(h_facing)))
}

impl IProjectileInfo for MissileInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef> {
        let info = Rc::new(self.clone());
        let w = args.source_actor.world();
        let limit = if info.range_limit != WDist::ZERO {
            info.range_limit
        } else {
            args.weapon.range
        };
        let range_limit = WDist::new(util::apply_percentage_modifiers(limit.length, args.range_modifiers.iter().copied()));
        let lock_on = w.shared_random.borrow_mut().next_max(100) <= info.lock_on_probability;
        let inaccuracy = if lock_on && info.lock_on_inaccuracy.length > -1 {
            info.lock_on_inaccuracy.length
        } else {
            info.inaccuracy.length
        };
        let mut offset = WVec::ZERO;
        if inaccuracy > 0 {
            let max = get_projectile_inaccuracy(inaccuracy, info.inaccuracy_type, &args);
            offset = WVec::from_pdf(&mut w.shared_random.borrow_mut(), 2) * max / 1024;
        }
        let max_launch_speed = if info.maximum_launch_speed.length > -1 {
            info.maximum_launch_speed.length
        } else {
            info.speed.length
        };
        let speed = max_launch_speed;
        let radius = loop_radius(speed, info.vertical_rate_of_turn.facing());
        let pos = args.source;
        let tar_dist = args.passive_target + offset - pos;
        let v_dist = WVec::new(-tar_dist.z, -tar_dist.horizontal_length(), 0);
        let mut v_facing = v_dist.yaw().facing() as i8 as i32;
        if v_facing == -1 {
            v_facing = 0;
        }
        v_facing = exts_clamp(
            v_facing,
            sbyte_angle(info.minimum_launch_angle),
            sbyte_angle(info.maximum_launch_angle),
        );
        let h_facing = args.facing.facing();
        let velocity = facing_rotated(WVec::new(0, -speed, 0), v_facing, h_facing);
        let render_facing = Rc::new(Cell::new(args.facing));
        let f = render_facing.clone();
        let anim = projectile_animation(&w, info.image.as_deref(), &info.sequences, Rc::new(move || f.get()));
        let contrail = info.contrail.create(&args.source_actor);
        Some(Rc::new_cyclic(|me: &Weak<Missile>| Missile {
            me: me.clone(),
            anim,
            contrail,
            render_facing,
            gravity: WVec::new(0, 0, -info.gravity),
            max_speed: info.speed.length,
            ticks: Cell::new(0),
            ticks_to_next_smoke: Cell::new(0),
            state: Cell::new(MissileState::Freefall),
            target_passed_by: Cell::new(false),
            lock_on,
            allow_pass_by: Cell::new(false),
            target_position: Cell::new(args.passive_target),
            offset,
            tar_vel: Cell::new(WVec::ZERO),
            pred_vel: Cell::new(WVec::ZERO),
            pos: Cell::new(pos),
            velocity: Cell::new(velocity),
            speed: Cell::new(speed),
            loop_radius: Cell::new(radius),
            distance_covered: Cell::new(WDist::ZERO),
            range_limit,
            h_facing: Cell::new(h_facing),
            v_facing: Cell::new(v_facing),
            info,
            args,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MissileState {
    Freefall,
    Homing,
    Hitting,
}

/// `Missile`, without the `TerrainHeightAware` code paths that Red Alert does not use.
struct Missile {
    me: Weak<Missile>,
    anim: Option<Rc<Animation>>,
    contrail: Option<SharedContrail>,
    render_facing: Rc<Cell<WAngle>>,
    info: Rc<MissileInfo>,
    args: ProjectileArgs,
    gravity: WVec,
    max_speed: i32,
    ticks: Cell<i32>,
    ticks_to_next_smoke: Cell<i32>,
    state: Cell<MissileState>,
    target_passed_by: Cell<bool>,
    lock_on: bool,
    allow_pass_by: Cell<bool>,
    target_position: Cell<WPos>,
    offset: WVec,
    tar_vel: Cell<WVec>,
    pred_vel: Cell<WVec>,
    pos: Cell<WPos>,
    velocity: Cell<WVec>,
    speed: Cell<i32>,
    loop_radius: Cell<i32>,
    distance_covered: Cell<WDist>,
    range_limit: WDist,
    h_facing: Cell<i32>,
    v_facing: Cell<i32>,
}

impl Missile {
    fn vrot(&self) -> i32 {
        self.info.vertical_rate_of_turn.facing()
    }

    fn change_speed(&self, sign: i32) {
        self.speed
            .set((self.speed.get() + sign * self.info.acceleration.length).clamp(0, self.max_speed));
        self.loop_radius.set(loop_radius(self.speed.get(), self.vrot()));
    }

    fn jammed_by(&self, a: &ActorRef, t: &JamsMissiles) -> bool {
        let range = if t.state.is_trait_disabled() { WDist::ZERO } else { t.info.range };
        if (a.center_position() - self.pos.get()).horizontal_length_squared() > range.length_squared() {
            return false;
        }
        if !t
            .info
            .deflection_relationships
            .has_relationship(a.owner().relationship_with(Some(&self.args.source_actor.owner())))
        {
            return false;
        }
        a.world().shared_random.borrow_mut().next_max(100) < t.info.chance
    }

    fn freefall_tick(&self) -> WVec {
        let v = self.velocity.get();
        let mv = v + self.gravity / 2;
        let mut nv = v + self.gravity;
        let ratio = self.max_speed.wrapping_mul(1024) / nv.length();
        if ratio < 1024 {
            nv = nv * ratio / 1024;
        }
        self.velocity.set(nv);
        mv
    }

    fn cruise_facing(&self, diff: i32) -> i32 {
        let cruise = self.info.cruise_altitude.length;
        let v_dist = WVec::new(-diff - cruise, -self.speed.get(), 0);
        let mut d = if v_dist.horizontal_length_squared() as i8 != 0 {
            v_dist.yaw().facing()
        } else {
            self.v_facing.get()
        };
        if -diff > cruise {
            d = -d;
        }
        d = exts_clamp(d, -self.vrot(), self.vrot());
        self.change_speed(1);
        d
    }

    fn homing_inner_tick(&self, diff: i32, rel_hor: i32, rel_hgt: i32, passed: bool) -> i32 {
        let v_facing = self.v_facing.get();
        let radius = self.loop_radius.get();
        if !(rel_hor <= 3 * radius || self.state.get() == MissileState::Hitting) {
            return self.cruise_facing(diff);
        }
        self.state.set(MissileState::Hitting);
        const LAST_HT: i32 = 0;
        const LAST_HT_CHG: i32 = 0;
        let target_z = self.target_position.get().z;
        if LAST_HT >= target_z {
            self.allow_pass_by.set(true);
        }
        if !self.allow_pass_by.get() && (LAST_HT < target_z || passed) {
            let v_dist = WVec::new(-rel_hgt, -rel_hor, 0);
            let mut d = if v_dist.horizontal_length_squared() as i8 != 0 {
                v_dist.yaw().facing()
            } else {
                v_facing
            };
            if d == -1 {
                d = 0;
            }
            if passed {
                d = exts_clamp(d, -self.vrot(), self.vrot());
            } else {
                let tar_hgt = exts_clamp(
                    radius.wrapping_mul(WAngle::from_facing(v_facing).cos()) / 1024 - rel_hgt.abs(),
                    0,
                    radius,
                );
                let tar_dist = isqrt_i32(radius.wrapping_mul(radius).wrapping_sub(tar_hgt.wrapping_mul(tar_hgt)));
                let miss_dist = radius.wrapping_mul(WAngle::from_facing(v_facing).sin()) / 1024;
                if rel_hor <= tar_dist - rel_hgt.signum() * miss_dist {
                    self.change_speed(-1);
                } else {
                    self.change_speed(1);
                }
            }
            d
        } else if self.allow_pass_by.get() {
            let pos_z = self.pos.get().z;
            if !passed {
                if rel_hor > 2 * radius {
                    let d1 = (rel_hor - LAST_HT_CHG).max(0);
                    if d1 > 2 * radius {
                        return 0;
                    }
                    let h1 = radius - isqrt_i32(d1.wrapping_mul(2 * radius - d1)) - (pos_z - LAST_HT);
                    if h1 > radius.wrapping_mul(1024 - WAngle::from_facing(v_facing).cos()) / 1024 {
                        WAngle::arc_tan(isqrt_i32(h1.wrapping_mul(2 * radius - h1)), radius - h1).angle >> 2
                    } else {
                        0
                    }
                } else {
                    self.passing_facing(WVec::new(-rel_hgt, -rel_hor, 0))
                }
            } else {
                self.passing_facing(WVec::new(-rel_hgt, rel_hor, 0))
            }
        } else {
            self.cruise_facing(diff)
        }
    }

    fn passing_facing(&self, v_dist: WVec) -> i32 {
        let v_facing = self.v_facing.get();
        let d = if v_dist.horizontal_length_squared() as i8 != 0 {
            v_dist.yaw().facing()
        } else {
            v_facing
        };
        if d < 0 && self.vrot() < v_facing as i8 as i32 { 0 } else { d }
    }

    fn homing_tick(&self, world: &Rc<World>, tar_dist: WVec, rel_hor: i32) -> WVec {
        let info = &self.info;
        let diff = -self.pos.get().z;
        let rel_hgt = tar_dist.z;
        let vel_vec = tar_dist + self.pred_vel.get();
        let h_facing = self.h_facing.get();
        let mut desired_h = if vel_vec.horizontal_length_squared() != 0 {
            vel_vec.yaw().facing()
        } else {
            h_facing
        };
        let delta = normalize_facing(h_facing - desired_h);
        let mut passed = false;
        if self.allow_pass_by.get() && delta > 64 && delta < 192 {
            desired_h = (desired_h + 128) & 0xFF;
            passed = true;
        }
        let mut desired_v = self.homing_inner_tick(diff, rel_hor, rel_hgt, passed);
        if tar_dist.horizontal_length() < self.speed.get().wrapping_mul(WAngle::from_facing(self.v_facing.get()).cos()) / 1024 {
            self.target_passed_by.set(true);
        } else {
            self.target_passed_by.set(passed);
        }
        let jammed = info.jammable && world.actors_with_trait::<JamsMissiles>().iter().any(|(a, t)| self.jammed_by(a, t));
        if jammed {
            let range = info.jammed_diversion_range;
            desired_h = h_facing + world.shared_random.borrow_mut().next_range(-range, range + 1);
            desired_v = self.v_facing.get() + world.shared_random.borrow_mut().next_range(-range, range + 1);
        } else if !self.args.guided_target.is_valid_for(&self.args.source_actor) {
            desired_h = h_facing;
        }
        self.h_facing
            .set(tick_facing_int(h_facing, desired_h, info.horizontal_rate_of_turn.facing()));
        self.v_facing.set(tick_facing_int(self.v_facing.get(), desired_v, self.vrot()));
        facing_rotated(
            WVec::new(0, -1024i32.wrapping_mul(self.speed.get()), 0),
            self.v_facing.get(),
            self.h_facing.get(),
        ) / 1024
    }
}

impl Effect for Missile {
    fn tick(&self, world: &Rc<World>) {
        let info = &self.info;
        let args = &self.args;
        if let Some(a) = &self.anim {
            a.tick();
        }
        self.ticks.set(self.ticks.get() + 1);
        if self.ticks.get() == info.homing_activation_delay + 1 {
            self.state.set(MissileState::Homing);
            self.speed.set(self.velocity.get().length());
            self.loop_radius.set(loop_radius(self.speed.get(), self.vrot()));
        }
        if self.range_limit >= WDist::ZERO && self.distance_covered.get() > self.range_limit {
            self.state.set(MissileState::Freefall);
            self.velocity.set(facing_rotated(
                WVec::new(0, -self.speed.get(), 0),
                self.v_facing.get(),
                self.h_facing.get(),
            ));
        }
        let mut new_tar = self.target_position.get();
        if args.guided_target.is_valid_for(&args.source_actor) && self.lock_on {
            let p = if args.weapon.target_actor_center {
                args.guided_target.center_position()
            } else {
                util::closest_position(&args.guided_target.positions(), args.source).unwrap_or(WPos::ZERO)
            };
            new_tar = p + WVec::new(0, 0, info.airburst_altitude.length);
        }
        let facing_angle = WAngle::from_facing(self.h_facing.get());
        let old_vel = self.tar_vel.get();
        let yaw1 = if old_vel.horizontal_length_squared() != 0 {
            old_vel.yaw()
        } else {
            facing_angle
        };
        let tar_vel = new_tar - self.target_position.get();
        self.tar_vel.set(tar_vel);
        let yaw2 = if tar_vel.horizontal_length_squared() != 0 {
            tar_vel.yaw()
        } else {
            facing_angle
        };
        self.pred_vel.set(tar_vel.rotate(&WRot::from_yaw(yaw2 - yaw1)));
        self.target_position.set(new_tar);
        let tar_dist = new_tar + self.offset - self.pos.get();
        let rel_dist = tar_dist.length();
        let rel_hor = tar_dist.horizontal_length();
        let mv = if self.state.get() == MissileState::Freefall {
            self.freefall_tick()
        } else {
            self.homing_tick(world, tar_dist, rel_hor)
        };
        let last_pos = self.pos.get();
        self.render_facing.set(WVec::new(mv.x, mv.y - mv.z, 0).yaw());
        if info.allow_snapping && self.state.get() != MissileState::Freefall && rel_dist < mv.length() {
            self.pos.set(new_tar + self.offset);
        } else {
            self.pos.set(last_pos + mv);
        }
        let mut should_explode = false;
        if info.blockable
            && let Some(hit) = any_blocking_actors_between(world, &args.source_actor.owner(), last_pos, self.pos.get(), info.width)
        {
            self.pos.set(hit);
            should_explode = true;
        }
        if info.trail_image.as_deref().is_some_and(|i| !i.is_empty()) {
            self.ticks_to_next_smoke.set(self.ticks_to_next_smoke.get() - 1);
            if self.ticks_to_next_smoke.get() < 0 && (self.state.get() != MissileState::Freefall || info.trail_when_deactivated) {
                let info2 = info.clone();
                let at = self.pos.get() - mv * 3 / 2;
                let facing = self.render_facing.get();
                let palette = info.visuals.trail_palette(&args.source_actor.owner());
                world.add_frame_end_task(Box::new(move |w| {
                    let seq = random_sequence(w, &info2.trail_sequences);
                    let image = info2.trail_image.as_deref().unwrap_or_default();
                    let e: EffectRef = SpriteEffect::at(w, at, facing, image, &seq, palette);
                    w.add_effect(e);
                }));
                self.ticks_to_next_smoke.set(info.trail_interval);
            }
        }
        if let Some(c) = &self.contrail {
            c.borrow_mut().update(self.pos.get());
        }
        self.distance_covered
            .set(self.distance_covered.get() + WDist::new(self.speed.get()));
        let pos = self.pos.get();
        let cell = world.map().cell_containing(pos);
        let height = world.map().distance_above_terrain(pos);
        should_explode |= height.length < 0
            || rel_dist < info.close_enough.length
            || (info.explode_when_empty && self.range_limit >= WDist::ZERO && self.distance_covered.get() > self.range_limit)
            || !world.map().contains(cell)
            || (!info.bound_to_terrain_type.is_empty() && world.map().terrain_type_info(cell).ty != info.bound_to_terrain_type)
            || (height.length < info.airburst_altitude.length && rel_hor < info.close_enough.length);
        if should_explode {
            add_contrail_fader(world, pos, &self.contrail);
            let me: EffectRef = self.me.upgrade().expect("missile");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
            if self.ticks.get() <= info.arm {
                return;
            }
            let mut a = WarheadArgs::from_projectile(args);
            a.impact_orientation = WRot::new(
                WAngle::ZERO,
                WAngle::from_facing(self.v_facing.get()),
                WAngle::from_facing(self.h_facing.get()),
            );
            a.impact_position = pos;
            weapon_impact_args(&args.weapon, &Target::from_pos(pos), &a);
        }
    }

    fn is_sync(&self) -> bool {
        true
    }

    fn sync_hash(&self) -> i32 {
        self.pos.get().hash() ^ self.h_facing.get() ^ self.v_facing.get()
    }

    fn render(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if let Some(c) = &self.contrail {
            c.borrow().render(viewer, out);
        }
        if let Some(a) = &self.anim {
            self.info.visuals.render(a, self.pos.get(), &self.args, viewer, out);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
