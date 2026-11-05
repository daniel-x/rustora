// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `MapGlobal.cs`, `ActorGlobal.cs` and `PlayerGlobal.cs`.

use std::rc::Rc;

use mlua::{Function, IntoLua, Lua, Table, Value};
use rustora_core::world_types::{CPos, WAngle, WDist, WPos};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::*;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::{World, WorldType};
use rustora_mods_common::actor_map::WorldExt;
use rustora_mods_common::actor_reference::{HealthInit, TurretFacingInit};
use rustora_mods_common::aircraft::AircraftInfo;
use rustora_mods_common::auto_target::{StanceInit, UnitStance};
use rustora_mods_common::production::{BuildableInfo, ClassicProductionQueueInfo, ProductionQueueInfo, ValuedInfo};
use rustora_mods_common::world::SpawnMapActors;

use crate::context::world;
use crate::members::Group;
use crate::script_traits::{ScriptLobbyDropdown, ScriptTags};
use crate::values::{FromScript, LuaActor, LuaCPos, LuaPlayer, LuaResult, LuaWPos, actors_table, error, key_string, type_name};

/// `ScriptGlobal.FilteredObjects`: the objects for which `filter` returns a true first value.
pub fn filtered<T: Clone, V: IntoLua>(objects: Vec<T>, filter: &Option<Function>, wrap: impl Fn(&T) -> V) -> LuaResult<Vec<T>> {
    let Some(f) = filter else { return Ok(objects) };
    let mut out = Vec::new();
    for o in objects {
        let ret: mlua::MultiValue = f.call(wrap(&o))?;
        match ret.front() {
            Some(v) => {
                if !matches!(v, Value::Nil | Value::Boolean(false)) {
                    out.push(o);
                }
            }
            None => return error("Sequence contains no elements"),
        }
    }
    Ok(out)
}

fn filtered_actors(lua: &Lua, actors: Vec<ActorRef>, filter: &Option<Function>) -> LuaResult<Value> {
    let actors = filtered(actors, filter, |a| LuaActor(a.clone()))?;
    actors_table(lua, &actors)
}

/// Whether the world has the custom movement layer `index`. No custom movement layers are ported yet.
pub fn has_custom_layer(_lua: &Lua, _index: u8) -> LuaResult<bool> {
    Ok(false)
}

fn spawn_map_actors(w: &World) -> LuaResult<Rc<SpawnMapActors>> {
    w.world_actor()
        .trait_or_default::<SpawnMapActors>()
        .map_or_else(|| error("SpawnMapActors is missing"), Ok)
}

fn lobby_option(w: &World, id: &str) -> Option<String> {
    w.world_actor()
        .traits_implementing::<ScriptLobbyDropdown>()
        .into_iter()
        .find(|s| s.info.id == id)
        .map(|s| s.value())
}

pub fn map() -> Group<()> {
    Group::new("Map", &[])
        .method("ActorsInCircle", |lua, _, a| {
            let location: WPos = a.get(0)?;
            let radius: WDist = a.get(1)?;
            let filter: Option<Function> = a.opt(2, None)?;
            let actors = world(lua)?.find_actors_in_circle(location, radius);
            filtered_actors(lua, actors, &filter)
        })
        .method("ActorsInBox", |lua, _, a| {
            let top_left: WPos = a.get(0)?;
            let bottom_right: WPos = a.get(1)?;
            let filter: Option<Function> = a.opt(2, None)?;
            let actors = world(lua)?.actor_map().actors_in_box(top_left, bottom_right);
            filtered_actors(lua, actors, &filter)
        })
        .get("TopLeft", |lua, _| Ok(LuaWPos(world(lua)?.map().projected_top_left())))
        .get("BottomRight", |lua, _| Ok(LuaWPos(world(lua)?.map().projected_bottom_right())))
        .method("RandomCell", |lua, _, _| {
            let w = world(lua)?;
            let cell = w.map().choose_random_cell(&mut w.shared_random.borrow_mut());
            Ok(LuaCPos(cell))
        })
        .method("RandomEdgeCell", |lua, _, _| {
            let w = world(lua)?;
            let cell = w.map().choose_random_edge_cell(&mut w.shared_random.borrow_mut());
            Ok(LuaCPos(cell))
        })
        .method("ClosestEdgeCell", |lua, _, a| {
            Ok(LuaCPos(world(lua)?.map().choose_closest_edge_cell(a.get(0)?)))
        })
        .method("ClosestMatchingEdgeCell", |lua, _, a| {
            let given: CPos = a.get(0)?;
            let filter: Function = a.get(1)?;
            let w = world(lua)?;
            let mut cells: Vec<CPos> = w.map().all_edge_cells().to_vec();
            cells.sort_by_key(|c| (given - *c).length());
            let matching = filtered(cells, &Some(filter), |c| LuaCPos(*c))?;
            Ok(LuaCPos(matching.first().copied().unwrap_or(CPos::ZERO)))
        })
        .method("CenterOfCell", |lua, _, a| Ok(LuaWPos(world(lua)?.map().center_of_cell(a.get(0)?))))
        .method("TerrainType", |lua, _, a| {
            let cell: CPos = a.get(0)?;
            Ok(world(lua)?.map().terrain_type_info(cell).ty.clone())
        })
        .get("IsSinglePlayer", |lua, _| Ok(world(lua)?.lobby_info.non_bot_players().count() == 1))
        .get("IsPausedShellmap", |lua, _| {
            let w = world(lua)?;
            Ok(w.world_type == WorldType::Shellmap && w.pause_shellmap.get())
        })
        .method("LobbyOption", |lua, _, a| {
            let id: String = a.get(0)?;
            let value = lobby_option(&*world(lua)?, &id);
            if value.is_none() {
                log::info!(target: "lua", "A ScriptLobbyDropdown with ID `{id}` was not found.");
            }
            Ok(value)
        })
        .method("LobbyOptionOrDefault", |lua, _, a| {
            let id: String = a.get(0)?;
            let fallback: Option<String> = a.get(1)?;
            Ok(lobby_option(&*world(lua)?, &id).or(fallback))
        })
        .get("NamedActors", |lua, _| {
            let sma = spawn_map_actors(&*world(lua)?)?;
            let actors: Vec<ActorRef> = sma.actors.borrow().values().cloned().collect();
            actors_table(lua, &actors)
        })
        .method("NamedActor", |lua, _, a| {
            let name: String = a.get(0)?;
            let sma = spawn_map_actors(&*world(lua)?)?;
            let actor = sma.actors.borrow().get(&name).filter(|a| !a.disposed()).cloned();
            Ok(actor.map(LuaActor))
        })
        .method("IsNamedActor", |lua, _, a| {
            let actor: ActorRef = a.get(0)?;
            let sma = spawn_map_actors(&*world(lua)?)?;
            let last = sma.last_map_actor_id.get();
            let count = sma.actors.borrow().len() as u32;
            Ok(actor.id <= last && actor.id > last.wrapping_sub(count))
        })
        .method("ActorsWithTag", |lua, _, a| {
            let tag: String = a.get(0)?;
            let actors: Vec<ActorRef> = world(lua)?
                .actors_with_trait::<ScriptTags>()
                .into_iter()
                .filter(|(_, t)| t.has_tag(&tag))
                .map(|(a, _)| a)
                .collect();
            actors_table(lua, &actors)
        })
        .get("ActorsInWorld", |lua, _| actors_table(lua, &world(lua)?.actors()))
}

fn init_value<T: FromScript>(name: &str, value: &Value) -> LuaResult<T> {
    T::from_script(value).map_or_else(
        || {
            error(format!(
                "Invalid data type for '{name}' (expected one of {}, got {})",
                T::NAME,
                type_name(value)
            ))
        },
        Ok,
    )
}

/// `ActorGlobal.CreateInit` for the init types that scripts use.
fn create_init(name: &str, value: &Value) -> LuaResult<Rc<dyn ActorInit>> {
    let (init, instance) = match name.split_once('@') {
        Some((a, b)) => (a, Some(b.to_string())),
        None => (name, None),
    };
    let init: Rc<dyn ActorInit> = match init {
        "Owner" => Rc::new(OwnerInit::from_player(&init_value::<PlayerRef>(init, value)?)),
        "Location" => Rc::new(LocationInit::new(init_value(init, value)?)),
        "Facing" => Rc::new(FacingInit {
            value: init_value::<WAngle>(init, value)?,
            instance_name: instance,
        }),
        "CenterPosition" => Rc::new(CenterPositionInit::new(init_value(init, value)?)),
        "SubCell" => Rc::new(SubCellInit::new(init_value::<i32>(init, value)? as u8)),
        "Faction" => Rc::new(FactionInit::new(init_value(init, value)?)),
        "Health" => Rc::new(HealthInit::new(init_value(init, value)?)),
        "TurretFacing" => Rc::new(TurretFacingInit {
            value: init_value::<WAngle>(init, value)?,
            instance_name: instance,
        }),
        "Stance" => {
            let stance = match init_value::<i32>(init, value)? {
                0 => UnitStance::HoldFire,
                1 => UnitStance::ReturnFire,
                2 => UnitStance::Defend,
                _ => UnitStance::AttackAnything,
            };
            Rc::new(StanceInit::new(stance))
        }
        "ScriptTags" => Rc::new(crate::script_traits::ScriptTagsInit::new(init_value(init, value)?)),
        _ => return error(format!("Unknown initializer type '{init}'")),
    };
    Ok(init)
}

fn actor_info(w: &World, ty: &str) -> LuaResult<Rc<rustora_engine::actor_info::ActorInfo>> {
    w.rules().actor(ty).map_or_else(|| error(format!("Unknown actor type '{ty}'")), Ok)
}

pub fn actor() -> Group<()> {
    Group::new("Actor", &[])
        .method("Create", |lua, _, a| {
            let ty: String = a.get(0)?;
            let add_to_world: bool = a.get(1)?;
            let table: Table = a.get(2)?;
            let mut inits = ActorInits::new();
            for pair in table.pairs::<Value, Value>() {
                let (k, v) = pair?;
                inits.push(create_init(&key_string(&k), &v)?);
            }
            if !inits.contains::<OwnerInit>() {
                return error(format!("Tried to create actor '{ty}' with an invalid or no owner init!"));
            }
            let w = world(lua)?;
            if w.rules().actor(&ty.to_lowercase()).is_none() {
                return error(format!("No rules definition for unit {}", ty.to_lowercase()));
            }
            let actor = w.create_actor_add(false, &ty, inits);
            if add_to_world {
                let added = actor.clone();
                w.add_frame_end_task(Box::new(move |w: &Rc<World>| w.add(&added)));
            }
            Ok(LuaActor(actor))
        })
        .method("BuildTime", |lua, _, a| {
            let ty: String = a.get(0)?;
            let queue: Option<String> = a.opt(1, None)?;
            let w = world(lua)?;
            let ai = actor_info(&w, &ty)?;
            let Some(bi) = ai.trait_info::<BuildableInfo>() else { return Ok(0) };
            let mut time = bi.build_duration;
            if time == -1 {
                match ai.trait_info::<ValuedInfo>() {
                    Some(v) => time = v.cost,
                    None => return Ok(0),
                }
            }
            let queues = w
                .rules()
                .actors()
                .flat_map(|(_, a)| {
                    let plain = a
                        .trait_infos::<ProductionQueueInfo>()
                        .into_iter()
                        .map(|q| (q.ty.clone(), q.build_duration_modifier));
                    let classic = a
                        .trait_infos::<ClassicProductionQueueInfo>()
                        .into_iter()
                        .map(|q| (q.base.ty.clone(), q.base.build_duration_modifier));
                    plain.chain(classic).collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let pbi = match &queue {
                Some(q) => match queues.iter().find(|x| &x.0 == q) {
                    Some(p) => p.1,
                    None => return error(format!("The specified queue '{q}' does not exist!")),
                },
                None => match queues.iter().find(|x| bi.queue.contains(&x.0)) {
                    Some(p) => p.1,
                    None => return error(format!("No actors can produce actor '{ty}'!")),
                },
            };
            Ok(time.wrapping_mul(bi.build_duration_modifier).wrapping_mul(pbi) / 10000)
        })
        .method("CruiseAltitude", |lua, _, a| {
            let ty: String = a.get(0)?;
            let ai = actor_info(&*world(lua)?, &ty)?;
            Ok(ai.trait_info::<AircraftInfo>().map_or(0, |i| i.cruise_altitude.length))
        })
        .method("Cost", |lua, _, a| {
            let ty: String = a.get(0)?;
            let ai = actor_info(&*world(lua)?, &ty)?;
            match ai.trait_info::<ValuedInfo>() {
                Some(v) => Ok(v.cost),
                None => error(format!(
                    "Actor type '{ty}' does not have the Valued trait required to get the Cost."
                )),
            }
        })
}

pub fn player() -> Group<()> {
    Group::new("Player", &[])
        .method("GetPlayer", |lua, _, a| {
            let name: String = a.get(0)?;
            let p = world(lua)?.players().into_iter().find(|p| p.internal_name == name);
            Ok(p.map(LuaPlayer))
        })
        .method("GetPlayers", |lua, _, a| {
            let filter: Option<Function> = a.get(0)?;
            let players = filtered(world(lua)?.players(), &filter, |p| LuaPlayer(p.clone()))?;
            crate::values::players_table(lua, &players)
        })
}
