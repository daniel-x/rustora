// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/Tooltip.cs`: the names the user interface shows for actors. The trait keeps the sync shape of a
//! `ConditionalTrait`.

use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::conditional::*;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("Tooltip", |n| load_info!(TooltipInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TooltipInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub name: Option<String>,
    pub generic_name: Option<String>,
    pub generic_stance_prefix: bool,
    pub ally_prefix: Option<String>,
    pub neutral_prefix: Option<String>,
    pub enemy_prefix: Option<String>,
    pub generic_visibility: PlayerRelationship,
    pub show_owner_row: bool,
}

impl Default for TooltipInfo {
    fn default() -> Self {
        TooltipInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            name: None,
            generic_name: None,
            generic_stance_prefix: true,
            ally_prefix: Some("label-tooltip-prefix.ally".into()),
            neutral_prefix: None,
            enemy_prefix: Some("label-tooltip-prefix.enemy".into()),
            generic_visibility: PlayerRelationship::NONE,
            show_owner_row: true,
        }
    }
}

pub struct Tooltip {
    pub info: Rc<TooltipInfo>,
    state: ConditionalState,
}

register_traits!(Tooltip: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TooltipInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Tooltip { info, state }))
});

conditional_simple!(Tooltip);

/// The fluent key of the first tooltip that is enabled by default, as `TraitInfos<TooltipInfo>().FirstOrDefault`
/// with `EnabledByDefault` reads it.
pub fn default_tooltip_name(actor: &ActorInfo) -> Option<String> {
    actor
        .trait_infos::<TooltipInfo>()
        .into_iter()
        .find(|t| t.base.enabled_by_default())
        .and_then(|t| t.name.clone())
}
