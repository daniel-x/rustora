// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ResourceProperties.cs`, `ScaredCatProperties.cs`, `PowerProperties.cs`,
//! `PlayerExperienceProperties.cs`, `PlayerConditionProperties.cs`, `ConditionProperties.cs` and
//! `GainsExperienceProperties.cs`.

use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;
use rustora_mods_common::base::{Power, PowerManager, PowerState, PlayerExperience};
use rustora_mods_common::conditions::ScaredyCat;
use rustora_mods_common::experience::{ExternalCondition, GainsExperience};
use rustora_mods_common::player::PlayerResources;

use crate::members::Group;
use crate::values::{Args, LuaResult, error};

/// The `ExternalCondition` source keys of the condition properties. C# uses the properties object, which is
/// unique per actor and script context; one world has one script context.
const ACTOR_CONDITION_SOURCE: usize = usize::MAX - 0x5c41;
const PLAYER_CONDITION_SOURCE: usize = usize::MAX - 0x5c42;

fn resources() -> Group<PlayerRef> {
    let pr = |p: &PlayerRef| p.player_actor().trait_::<PlayerResources>();
    Group::new("Resources", &["PlayerResourcesInfo"])
        .get_set(
            "Resources",
            move |_, p| Ok(pr(p).resources.get()),
            move |_, p, value: i32| {
                let pr = pr(p);
                pr.resources.set(value.clamp(0, pr.resource_capacity.get().max(0)));
                Ok(())
            },
        )
        .get("ResourceCapacity", move |_, p| Ok(pr(p).resource_capacity.get()))
        .get_set(
            "Cash",
            move |_, p| Ok(pr(p).cash.get()),
            move |_, p, value: i32| {
                pr(p).cash.set(value.max(0));
                Ok(())
            },
        )
}

fn power_state_name(s: PowerState) -> String {
    match s {
        PowerState::Normal => "Normal".into(),
        PowerState::Low => "Low".into(),
        PowerState::Critical => "Critical".into(),
        PowerState(n) => n.to_string(),
    }
}

fn player_power() -> Group<PlayerRef> {
    let pm = |p: &PlayerRef| p.player_actor().trait_::<PowerManager>();
    Group::new("Power", &["PowerManagerInfo"])
        .get("PowerProvided", move |_, p| Ok(pm(p).power_provided()))
        .get("PowerDrained", move |_, p| Ok(pm(p).power_drained()))
        .get("PowerState", move |_, p| Ok(power_state_name(pm(p).power_state())))
        .method("TriggerPowerOutage", move |_, p, a| {
            pm(p).trigger_power_outage(a.get(0)?);
            Ok(())
        })
        .get_set(
            "PlayLowPowerNotification",
            move |_, p| Ok(pm(p).play_low_power_notification.get()),
            move |_, p, value: bool| {
                pm(p).play_low_power_notification.set(value);
                Ok(())
            },
        )
}

fn player_experience() -> Group<PlayerRef> {
    let exp = |p: &PlayerRef| p.player_actor().trait_::<PlayerExperience>();
    Group::new("Player", &["PlayerExperienceInfo"]).get_set(
        "Experience",
        move |_, p| Ok(exp(p).experience()),
        move |_, p, value: i32| {
            let exp = exp(p);
            exp.give_experience(value.wrapping_sub(exp.experience()));
            Ok(())
        },
    )
}

fn grant_condition(a: &ActorRef, source: usize, args: &Args) -> LuaResult<i32> {
    let condition: String = args.get(0)?;
    let duration: i32 = args.opt(1, 0)?;
    let external = a
        .traits_implementing::<ExternalCondition>()
        .into_iter()
        .find(|t| t.info.condition == condition && t.can_grant_condition(source));
    match external {
        Some(e) => Ok(e.grant_condition(a, source, duration, 0)),
        None => error(format!(
            "Condition `{condition}` has not been listed on an enabled ExternalCondition trait"
        )),
    }
}

fn revoke_condition(a: &ActorRef, source: usize, args: &Args) -> LuaResult<()> {
    let token: i32 = args.get(0)?;
    for e in a.traits_implementing::<ExternalCondition>() {
        if e.try_revoke_condition(a, source, token) {
            break;
        }
    }
    Ok(())
}

fn accepts_condition(a: &ActorRef, source: usize, args: &Args) -> LuaResult<bool> {
    let condition: String = args.get(0)?;
    Ok(a
        .traits_implementing::<ExternalCondition>()
        .iter()
        .any(|t| t.info.condition == condition && t.can_grant_condition(source)))
}

fn player_conditions() -> Group<PlayerRef> {
    const S: usize = PLAYER_CONDITION_SOURCE;
    Group::<PlayerRef>::new("Player", &[])
        .method("GrantCondition", |_, p, a| grant_condition(p.player_actor(), S, a))
        .method("RevokeCondition", |_, p, a| revoke_condition(p.player_actor(), S, a))
        .method("AcceptsCondition", |_, p, a| accepts_condition(p.player_actor(), S, a))
}

pub fn player_groups() -> Vec<Group<PlayerRef>> {
    vec![resources(), player_power(), player_experience(), player_conditions()]
}

pub fn actor_groups() -> Vec<Group<ActorRef>> {
    const S: usize = ACTOR_CONDITION_SOURCE;
    let gains = |a: &ActorRef| a.trait_::<GainsExperience>();
    vec![
        Group::<ActorRef>::new("Movement", &["ScaredyCatInfo"]).method("Panic", |_, a, _| {
            a.trait_::<ScaredyCat>().panic();
            Ok(())
        }),
        Group::<ActorRef>::new("Power", &["PowerInfo"]).get("Power", |_, a| {
            Ok(a.traits_implementing::<Power>().iter().fold(0i32, |s, p| s.wrapping_add(p.get_enabled_power())))
        }),
        Group::<ActorRef>::new("General", &["ExternalConditionInfo"])
            .method("GrantCondition", |_, a, args| grant_condition(a, S, args))
            .method("RevokeCondition", |_, a, args| revoke_condition(a, S, args))
            .method("AcceptsCondition", |_, a, args| accepts_condition(a, S, args)),
        Group::<ActorRef>::new("Experience", &["GainsExperienceInfo"])
            .get("Experience", move |_, a| Ok(gains(a).experience()))
            .get("Level", move |_, a| Ok(gains(a).level()))
            .get("MaxLevel", move |_, a| Ok(gains(a).max_level()))
            .get("CanGainLevel", move |_, a| Ok(gains(a).can_gain_level()))
            .method("GiveExperience", move |_, a, args| {
                let amount: i32 = args.get(0)?;
                if amount < 0 {
                    return error("Revoking experience is not implemented.");
                }
                gains(a).give_experience_silently(amount, args.opt(1, false)?);
                Ok(())
            })
            .method("GiveLevels", move |_, a, args| {
                gains(a).give_levels_silently(args.get(0)?, args.opt(1, false)?);
                Ok(())
            }),
    ]
}
