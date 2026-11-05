// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/Radar/AppearsOnRadar.cs`. The trait keeps the sync shape of a `ConditionalTrait`; radar color
//! modifiers are not ported.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::CPos;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("AppearsOnRadar", |n| load_info!(AppearsOnRadarInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AppearsOnRadarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub use_location: bool,
    pub valid_relationships: PlayerRelationship,
}

impl Default for AppearsOnRadarInfo {
    fn default() -> Self {
        AppearsOnRadarInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            use_location: false,
            valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
        }
    }
}

pub struct AppearsOnRadar {
    pub info: Rc<AppearsOnRadarInfo>,
    state: ConditionalState,
}

register_traits!(AppearsOnRadar: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(AppearsOnRadarInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(AppearsOnRadar { info, state }))
});

conditional_simple!(AppearsOnRadar);

impl AppearsOnRadar {
    /// `PopulateRadarSignatureCells`, where `viewer` is the render player, or the local player.
    pub fn radar_signature_cells(&self, self_: &ActorRef, viewer: Option<&PlayerRef>, out: &mut Vec<(CPos, Color)>) {
        let owner = self_.owner();
        if self.state.is_trait_disabled()
            || viewer.is_some_and(|v| !self.info.valid_relationships.has_relationship(owner.relationship_with(Some(v))))
        {
            return;
        }
        let color = owner.color;
        if self.info.use_location {
            out.push((self_.location(), color));
            return;
        }
        if let Some(os) = self_.trait_or_default::<dyn IOccupySpace>() {
            out.extend(os.occupied_cells().into_iter().map(|(c, _)| (c, color)));
        }
    }
}
