// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `PlayerProperties.cs`, `MissionObjectiveProperties.cs`, `DiplomacyProperties.cs` and
//! `PlayerStatsProperties.cs`.

use std::rc::Rc;

use mlua::Lua;
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_mods_common::attack::AttackBase;
use rustora_mods_common::mobile::MobileInfo;
use rustora_mods_common::player::objectives::{MissionObjective, MissionObjectives, ObjectiveState, has_no_required_units};
use rustora_mods_common::player::statistics::PlayerStatistics;
use rustora_mods_common::production::TechTree;
use rustora_mods_common::base::MapOptions;

use crate::context::world;
use crate::host::{host, resolved_player_name};
use crate::members::Group;
use crate::values::{LuaCPos, LuaColor, LuaResult, actors_table, error};

type G = Group<PlayerRef>;

fn owned_actors(p: &PlayerRef, filter: impl Fn(&ActorRef) -> bool) -> Vec<ActorRef> {
    pw(p)
        .actors()
        .into_iter()
        .filter(|a| Rc::ptr_eq(&a.owner(), p) && !a.is_dead() && a.is_in_world() && filter(a))
        .collect()
}

/// The team and handicap of the lobby client of `p`, or zeros.
fn client_values(p: &PlayerRef) -> (i32, i32) {
    pw(p).lobby_info
        .clients
        .iter()
        .find(|c| c.index == p.client_index)
        .map_or((0, 0), |c| (c.team, c.handicap))
}

fn pw(p: &PlayerRef) -> Rc<World> {
    p.player_actor().world()
}

fn unknown_type(lua: &Lua, ty: &str) -> LuaResult<()> {
    if world(lua)?.rules().actor(ty).is_none() {
        return error(format!("Unknown actor type '{ty}'"));
    }
    Ok(())
}

fn player() -> G {
    G::new("Player", &[])
        .get("InternalName", |_, p| Ok(p.internal_name.clone()))
        .get("Name", |lua, p| Ok(resolved_player_name(lua, p)))
        .get("Color", |_, p| Ok(LuaColor(p.color)))
        .get("Faction", |_, p| Ok(p.faction.clone()))
        .get("Spawn", |_, p| Ok(p.spawn_point))
        .get("HomeLocation", |_, p| Ok(LuaCPos(p.home_location)))
        .get("Team", |_, p| Ok(client_values(p).0))
        .get("Handicap", |_, p| Ok(client_values(p).1))
        .get("IsBot", |_, p| Ok(p.is_bot))
        .get("IsNonCombatant", |_, p| Ok(p.non_combatant))
        .get("IsLocalPlayer", |_, p| {
            Ok(host().render_or_local_player(&pw(p)).is_some_and(|l| Rc::ptr_eq(&l, p)))
        })
        .method("GetActors", |lua, p, _| actors_table(lua, &owned_actors(p, |_| true)))
        .method("GetGroundAttackers", |lua, p, _| {
            let actors: Vec<ActorRef> = pw(p)
                .actors_having_trait::<AttackBase>()
                .into_iter()
                .filter(|a| {
                    Rc::ptr_eq(&a.owner(), p) && !a.is_dead() && a.is_in_world() && a.info.has_trait_info::<MobileInfo>()
                })
                .collect();
            actors_table(lua, &actors)
        })
        .method("GetActorsByType", |lua, p, args| {
            let ty: String = args.get(0)?;
            let Some(ai) = world(lua)?.rules().actor(&ty) else {
                return error(format!("Unknown actor type '{ty}'"));
            };
            actors_table(lua, &owned_actors(p, |a| a.info.name == ai.name))
        })
        .method("GetActorsByTypes", |lua, p, args| {
            let types: Vec<String> = args.get(0)?;
            for ty in &types {
                unknown_type(lua, ty)?;
            }
            actors_table(lua, &owned_actors(p, |a| types.contains(&a.info.name)))
        })
        .method("HasPrerequisites", |_, p, args| {
            let types: Vec<String> = args.get(0)?;
            match p.player_actor().trait_or_default::<TechTree>() {
                Some(tt) => Ok(tt.has_prerequisites(&types)),
                None => error(format!("Missing TechTree trait on player {}!", p.player_name)),
            }
        })
}

fn objective<R>(p: &PlayerRef, id: i32, f: impl FnOnce(&MissionObjectives, &MissionObjective) -> R) -> LuaResult<R> {
    let mo = p.player_actor().trait_::<MissionObjectives>();
    let objectives = mo.objectives();
    match usize::try_from(id).ok().and_then(|i| objectives.get(i)) {
        Some(o) => Ok(f(&mo, o)),
        None => error("Objective ID is out of range."),
    }
}

fn add_objective(p: &PlayerRef, description: &str, ty: &str, required: bool) -> i32 {
    p.player_actor().trait_::<MissionObjectives>().add(p, description, ty, required) as i32
}

fn mission_objectives() -> G {
    G::new("MissionObjectives", &["MissionObjectivesInfo"])
        .method("AddObjective", |_, p, args| {
            let description: String = args.get(0)?;
            let ty: String = args.opt(1, "Primary".to_string())?;
            let required: bool = args.opt(2, true)?;
            Ok(add_objective(p, &description, &ty, required))
        })
        .method("AddPrimaryObjective", |_, p, args| {
            Ok(add_objective(p, &args.get::<String>(0)?, "Primary", true))
        })
        .method("AddSecondaryObjective", |_, p, args| {
            Ok(add_objective(p, &args.get::<String>(0)?, "Secondary", false))
        })
        .method("MarkCompletedObjective", |_, p, args| {
            let id: i32 = args.get(0)?;
            objective(p, id, |_, _| ())?;
            p.player_actor().trait_::<MissionObjectives>().mark_completed(p, id as usize);
            Ok(())
        })
        .method("MarkFailedObjective", |_, p, args| {
            let id: i32 = args.get(0)?;
            objective(p, id, |_, _| ())?;
            p.player_actor().trait_::<MissionObjectives>().mark_failed(p, id as usize);
            Ok(())
        })
        .method("IsObjectiveCompleted", |_, p, args| {
            objective(p, args.get(0)?, |_, o| o.state == ObjectiveState::Completed)
        })
        .method("IsObjectiveFailed", |_, p, args| {
            objective(p, args.get(0)?, |_, o| o.state == ObjectiveState::Failed)
        })
        .method("GetObjectiveDescription", |_, p, args| {
            objective(p, args.get(0)?, |_, o| o.description.clone())
        })
        .method("GetObjectiveType", |_, p, args| objective(p, args.get(0)?, |_, o| o.ty.clone()))
        .method("HasNoRequiredUnits", |_, p, _| {
            let w = pw(p);
            let short_game = w.world_actor().trait_::<MapOptions>().short_game();
            Ok(has_no_required_units(&w, p, short_game))
        })
}

fn diplomacy() -> G {
    G::new("Diplomacy", &[]).method("IsAlliedWith", |_, p, args| {
        let target: Option<PlayerRef> = args.get(0)?;
        Ok(p.is_allied_with(target.as_deref()))
    })
}

fn stats() -> G {
    fn stat(f: fn(&PlayerStatistics) -> i32) -> impl Fn(&Lua, &PlayerRef) -> LuaResult<i32> {
        move |_, p| Ok(f(&p.player_actor().trait_::<PlayerStatistics>()))
    }
    G::new("Player", &["PlayerStatisticsInfo"])
        .get("KillsCost", stat(|s| s.kills_cost.get()))
        .get("DeathsCost", stat(|s| s.deaths_cost.get()))
        .get("UnitsKilled", stat(|s| s.units_killed.get()))
        .get("UnitsLost", stat(|s| s.units_dead.get()))
        .get("BuildingsKilled", stat(|s| s.buildings_killed.get()))
        .get("BuildingsLost", stat(|s| s.buildings_dead.get()))
}

pub fn groups() -> Vec<G> {
    vec![player(), mission_objectives(), diplomacy(), stats()]
}
