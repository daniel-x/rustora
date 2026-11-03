// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `GameRules/WeaponInfo.cs`. Projectile and warhead types live in the mod crates and register their
//! loaders in [`WeaponRegistry`].

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{WAngle, WDist, WPos, WRot, WVec};
use rustora_derive::FieldLoad;

use crate::actor::ActorRef;
use crate::actor_ext::enabled_target_types;
use crate::bitset::BitSet;
use crate::effect::{Effect, EffectRef};
use crate::frozen::FrozenActorRef;
use crate::target::{Target, TargetType};
use crate::terrain::TargetableType;
use crate::world::World;

pub struct ProjectileArgs {
    pub weapon: Rc<WeaponInfo>,
    pub damage_modifiers: Vec<i32>,
    pub inaccuracy_modifiers: Vec<i32>,
    pub range_modifiers: Vec<i32>,
    pub facing: WAngle,
    pub current_muzzle_facing: Rc<dyn Fn() -> WAngle>,
    pub source: WPos,
    pub current_source: Rc<dyn Fn() -> WPos>,
    pub source_actor: ActorRef,
    pub passive_target: WPos,
    pub guided_target: Target,
}

#[derive(Clone)]
pub struct WarheadArgs {
    pub weapon: Rc<WeaponInfo>,
    pub damage_modifiers: Vec<i32>,
    pub source: Option<WPos>,
    pub impact_orientation: WRot,
    pub impact_position: WPos,
    pub source_actor: ActorRef,
    pub weapon_target: Target,
}

impl WarheadArgs {
    pub fn from_projectile(args: &ProjectileArgs) -> Self {
        WarheadArgs {
            weapon: args.weapon.clone(),
            damage_modifiers: args.damage_modifiers.clone(),
            source: Some(args.source),
            impact_orientation: WRot::NONE,
            impact_position: args.passive_target,
            source_actor: args.source_actor.clone(),
            weapon_target: args.guided_target.clone(),
        }
    }
}

pub trait IProjectileInfo {
    fn create(&self, args: ProjectileArgs) -> Option<EffectRef>;
    fn as_any(&self) -> &dyn std::any::Any;
}

pub trait IWarhead {
    fn delay(&self) -> i32;
    fn do_impact(&self, target: &Target, args: &WarheadArgs);
    /// `Damage` if this is a `DamageWarhead`.
    fn damage_value(&self) -> Option<i32> {
        None
    }
}

pub type ProjectileLoader = fn(&MiniYaml) -> Result<Rc<dyn IProjectileInfo>, String>;
pub type WarheadLoader = fn(&MiniYaml) -> Result<Rc<dyn IWarhead>, String>;

/// Replaces `Game.CreateObject` for weapon parts.
#[derive(Default)]
pub struct WeaponRegistry {
    pub projectiles: HashMap<&'static str, ProjectileLoader>,
    pub warheads: HashMap<&'static str, WarheadLoader>,
}

#[derive(Clone, Debug, FieldLoad)]
struct WeaponFields {
    range: WDist,
    first_burst_target_offset: WVec,
    following_burst_target_offset: WVec,
    reload_delay: i32,
    burst: i32,
    can_target_self: bool,
    valid_targets: BitSet<TargetableType>,
    invalid_targets: BitSet<TargetableType>,
    air_threshold: WDist,
    burst_delays: Vec<i32>,
    min_range: WDist,
    target_actor_center: bool,
}

impl Default for WeaponFields {
    fn default() -> Self {
        WeaponFields {
            range: WDist::ZERO,
            first_burst_target_offset: WVec::ZERO,
            following_burst_target_offset: WVec::ZERO,
            reload_delay: 1,
            burst: 1,
            can_target_self: false,
            valid_targets: BitSet::new(&["Ground", "Water"]),
            invalid_targets: BitSet::default(),
            air_threshold: WDist::new(128),
            burst_delays: vec![5],
            min_range: WDist::ZERO,
            target_actor_center: false,
        }
    }
}

pub struct WeaponInfo {
    pub range: WDist,
    pub first_burst_target_offset: WVec,
    pub following_burst_target_offset: WVec,
    pub reload_delay: i32,
    pub burst: i32,
    pub can_target_self: bool,
    pub valid_targets: BitSet<TargetableType>,
    pub invalid_targets: BitSet<TargetableType>,
    pub air_threshold: WDist,
    pub burst_delays: Vec<i32>,
    pub min_range: WDist,
    pub target_actor_center: bool,
    pub projectile: Option<Rc<dyn IProjectileInfo>>,
    pub warheads: Vec<Rc<dyn IWarhead>>,
}

impl WeaponInfo {
    pub fn load(registry: &WeaponRegistry, content: &MiniYaml) -> Result<WeaponInfo, String> {
        let mut f = WeaponFields::default();
        rustora_core::fieldloader::FieldLoad::load(&mut f, content).map_err(|e| e.to_string())?;
        let mut projectile = None;
        if let Some(n) = content.nodes.iter().find(|n| n.key == "Projectile") {
            if let Some(l) = n.value.value.as_deref().and_then(|v| registry.projectiles.get(v)) {
                projectile = Some(l(&n.value)?);
            }
        }
        let mut warheads = Vec::new();
        for n in content.nodes.iter().filter(|n| n.key.starts_with("Warhead")) {
            if let Some(l) = n.value.value.as_deref().and_then(|v| registry.warheads.get(v)) {
                warheads.push(l(&n.value)?);
            }
        }
        Ok(WeaponInfo {
            range: f.range,
            first_burst_target_offset: f.first_burst_target_offset,
            following_burst_target_offset: f.following_burst_target_offset,
            reload_delay: f.reload_delay,
            burst: f.burst,
            can_target_self: f.can_target_self,
            valid_targets: f.valid_targets,
            invalid_targets: f.invalid_targets,
            air_threshold: f.air_threshold,
            burst_delays: f.burst_delays,
            min_range: f.min_range,
            target_actor_center: f.target_actor_center,
            projectile,
            warheads,
        })
    }

    pub fn is_valid_target(&self, target_types: &BitSet<TargetableType>) -> bool {
        self.valid_targets.overlaps(target_types) && !self.invalid_targets.overlaps(target_types)
    }

    pub fn is_valid_against(&self, target: &Target, world: &World, fired_by: &ActorRef) -> bool {
        match target.target_type() {
            TargetType::Actor => self.is_valid_against_actor(target.actor().unwrap(), fired_by),
            TargetType::FrozenActor => self.is_valid_against_frozen(target.frozen_actor().unwrap(), fired_by),
            TargetType::Terrain => {
                let pos = target.center_position();
                if world.map().distance_above_terrain(pos) > self.air_threshold {
                    return self.is_valid_target(&BitSet::new(&["Air"]));
                }
                let cell = world.map().cell_containing(pos);
                world.map().contains(cell) && self.is_valid_target(&world.map().terrain_type_info(cell).target_types)
            }
            TargetType::Invalid => false,
        }
    }

    pub fn is_valid_against_actor(&self, victim: &ActorRef, fired_by: &ActorRef) -> bool {
        if !self.can_target_self && victim.id == fired_by.id {
            return false;
        }
        self.is_valid_target(&enabled_target_types(victim))
    }

    pub fn is_valid_against_frozen(&self, victim: &FrozenActorRef, fired_by: &ActorRef) -> bool {
        if !victim.is_valid() {
            return false;
        }
        if !self.can_target_self && victim.actor().is_some_and(|a| a.id == fired_by.id) {
            return false;
        }
        self.is_valid_target(&victim.target_types())
    }
}

/// `Impact(in Target, WarheadArgs)`.
pub fn weapon_impact_args(weapon: &WeaponInfo, target: &Target, args: &WarheadArgs) {
    let world = args.source_actor.world();
    for warhead in &weapon.warheads {
        if warhead.delay() > 0 {
            let (w, t, a) = (warhead.clone(), target.clone(), args.clone());
            world.add_frame_end_task(Box::new(move |world| {
                world.add_effect(Rc::new_cyclic(|me: &std::rc::Weak<DelayedImpact>| DelayedImpact { me: me.clone(), delay: std::cell::Cell::new(w.delay()), warhead: w, target: t, args: a }));
            }));
        } else {
            warhead.do_impact(target, args);
        }
    }
}

/// `Impact(in Target, Actor)`.
pub fn weapon_impact(weapon: &Rc<WeaponInfo>, target: &Target, fired_by: &ActorRef) {
    let args = WarheadArgs {
        weapon: weapon.clone(),
        damage_modifiers: Vec::new(),
        source: fired_by.cache().occupies_space.is_some().then(|| fired_by.center_position()),
        impact_orientation: WRot::NONE,
        impact_position: WPos::ZERO,
        source_actor: fired_by.clone(),
        weapon_target: target.clone(),
    };
    weapon_impact_args(weapon, target, &args);
}

/// `Effects/DelayedImpact.cs`.
struct DelayedImpact {
    delay: std::cell::Cell<i32>,
    warhead: Rc<dyn IWarhead>,
    target: Target,
    args: WarheadArgs,
    me: std::rc::Weak<DelayedImpact>,
}

impl Effect for DelayedImpact {
    fn tick(&self, world: &Rc<World>) {
        let d = self.delay.get() - 1;
        self.delay.set(d);
        if d <= 0 {
            let me: EffectRef = self.me.upgrade().expect("effect self reference");
            let (warhead, target, args) = (self.warhead.clone(), self.target.clone(), self.args.clone());
            world.add_frame_end_task(Box::new(move |w| {
                w.remove_effect(&me);
                warhead.do_impact(&target, &args);
            }));
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
