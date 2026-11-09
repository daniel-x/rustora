// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/Tooltip.cs`: the names the user interface shows for actors. The trait keeps the sync shape of a
//! `ConditionalTrait`.

use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::conditional::*;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

use crate::base::{DeveloperMode, PowerManager};

pub fn register(r: &mut TraitRegistry) {
    r.register("Tooltip", |n| load_info!(TooltipInfo, n));
    r.register("TooltipDescription", |n| load_info!(TooltipDescriptionInfo, n));
    r.register("PowerTooltip", |n| load_info!(PowerTooltipInfo, n));
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

#[derive(rustora_engine::persist::Persist)]
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

impl TooltipInfo {
    /// `TooltipForPlayerStance`, with `message` resolving fluent keys.
    pub fn tooltip_for_player_stance(&self, relationship: PlayerRelationship, message: &dyn Fn(&str) -> String) -> String {
        if relationship == PlayerRelationship::NONE || !self.generic_visibility.has_relationship(relationship) {
            return self.name.as_deref().map(message).unwrap_or_default();
        }
        let generic_name = self
            .generic_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .map(message)
            .unwrap_or_default();
        if self.generic_stance_prefix {
            let prefix = if relationship == PlayerRelationship::ALLY {
                &self.ally_prefix
            } else if relationship == PlayerRelationship::NEUTRAL {
                &self.neutral_prefix
            } else if relationship == PlayerRelationship::ENEMY {
                &self.enemy_prefix
            } else {
                &None
            };
            if let Some(prefix) = prefix.as_deref().filter(|p| !p.is_empty()) {
                return format!("{} {generic_name}", message(prefix));
            }
        }
        generic_name
    }
}

/// `IProvideTooltipInfo`. `sell_mode` is `World.OrderGenerator is SellOrderGenerator`, which only the client knows.
pub trait IProvideTooltipInfo {
    fn is_tooltip_visible(&self, self_: &ActorRef, for_player: Option<&PlayerRef>, sell_mode: bool) -> bool;
    fn tooltip_text(&self, self_: &ActorRef, message: &dyn Fn(&str) -> String) -> String;
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TooltipDescriptionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub description: String,
    pub valid_relationships: PlayerRelationship,
}

impl Default for TooltipDescriptionInfo {
    fn default() -> Self {
        TooltipDescriptionInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            description: String::new(),
            valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct TooltipDescription {
    pub info: Rc<TooltipDescriptionInfo>,
    state: ConditionalState,
}

register_traits!(TooltipDescription: INotifyCreated, IObservesVariables, IDisabledTrait, ISync, IProvideTooltipInfo);
info_impl!(TooltipDescriptionInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(TooltipDescription { info, state }))
});

conditional_simple!(TooltipDescription);

impl IProvideTooltipInfo for TooltipDescription {
    fn is_tooltip_visible(&self, self_: &ActorRef, for_player: Option<&PlayerRef>, _sell_mode: bool) -> bool {
        let Some(for_player) = for_player else {
            return false;
        };
        !self.state.is_trait_disabled()
            && self
                .info
                .valid_relationships
                .has_relationship(self_.owner().relationship_with(Some(for_player)))
    }

    fn tooltip_text(&self, _actor: &ActorRef, message: &dyn Fn(&str) -> String) -> String {
        message(&self.info.description)
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PowerTooltipInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

#[derive(rustora_engine::persist::Persist)]
pub struct PowerTooltip;

register_traits!(PowerTooltip: IProvideTooltipInfo);
info_impl!(PowerTooltipInfo => |_info, _init| Some(Rc::new(PowerTooltip)));

impl IProvideTooltipInfo for PowerTooltip {
    fn is_tooltip_visible(&self, self_: &ActorRef, for_player: Option<&PlayerRef>, _sell_mode: bool) -> bool {
        for_player.is_some_and(|p| Rc::ptr_eq(p, &self_.owner()))
    }

    fn tooltip_text(&self, self_: &ActorRef, _message: &dyn Fn(&str) -> String) -> String {
        let owner = self_.owner();
        let actor = owner.player_actor();
        let power = actor.trait_::<PowerManager>();
        let provided = if actor.trait_::<DeveloperMode>().unlimited_power() {
            String::new()
        } else {
            format!("/{}", power.power_provided())
        };
        format!("Power Usage: {}{provided}", power.power_drained())
    }
}

/// The fluent key of the first tooltip that is enabled by default, as `TraitInfos<TooltipInfo>().FirstOrDefault`
/// with `EnabledByDefault` reads it.
pub fn default_tooltip_name(actor: &ActorInfo) -> Option<String> {
    actor
        .trait_infos::<TooltipInfo>()
        .into_iter()
        .find(|t| t.base.enabled_by_default())
        .and_then(|t| t.name.clone())
}
