// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ProductionProperties.cs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use indexmap::{IndexMap, IndexSet};
use mlua::{Function, Lua};
use rustora_core::world_types::CPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInits, FactionInit, OwnerInit};
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::target::Target;
use rustora_engine::traits::IResolveOrder;
use rustora_mods_common::activities::WaitFor;
use rustora_mods_common::production::{
    BuildableInfo, PrimaryBuilding, Production, ProductionQueue, RallyPoint, get_initial_faction, nearest_exit_or_default,
};

use crate::context::{ScriptContext, ctx, world};
use crate::members::Group;
use crate::triggers::{ActorPairEvent, ScriptTriggers, Trigger};
use crate::values::{LuaCPos, LuaResult, actors_table, error};

fn buildable_info(lua: &Lua, actor_type: &str) -> LuaResult<Rc<BuildableInfo>> {
    let Some(ri) = world(lua)?.rules().actor(actor_type) else {
        return error(format!("The given key '{actor_type}' was not present in the dictionary."));
    };
    match ri.trait_info::<BuildableInfo>() {
        Some(bi) => Ok(bi),
        None => error(format!("Actor of type {actor_type} cannot be produced")),
    }
}

fn first_queue(bi: &BuildableInfo) -> LuaResult<String> {
    match bi.queue.first() {
        Some(q) => Ok(q.clone()),
        None => error("Sequence contains no elements"),
    }
}

/// `Order.StartProduction(subject, item, 1)`.
fn start_production(subject: &ActorRef, item: &str) -> Order {
    let mut o = Order::new("StartProduction", Some(subject.clone()), Target::Invalid, true);
    o.extra_data = 1;
    o.target_string = Some(item.to_string());
    o
}

/// The squad collector of `Build`: calls `f` with the living members once `size` units are produced.
struct Squad {
    context: Weak<ScriptContext>,
    lua: Lua,
    f: Function,
    size: usize,
    units: RefCell<Vec<ActorRef>>,
}

impl Squad {
    /// Adds `unit` and returns true once the squad is complete and `f` was called.
    fn add(&self, unit: &ActorRef) -> bool {
        self.units.borrow_mut().push(unit.clone());
        if self.units.borrow().len() < self.size {
            return false;
        }
        let alive: Vec<ActorRef> = self.units.borrow().iter().filter(|u| !u.is_dead()).cloned().collect();
        if let (Some(c), Ok(squad)) = (self.context.upgrade(), actors_table(&self.lua, &alive)) {
            c.call(&self.f, squad);
        }
        true
    }
}

fn remove_event(list: &RefCell<Vec<ActorPairEvent>>, event: &Weak<dyn Fn(&ActorRef, &ActorRef)>) {
    if let Some(e) = event.upgrade() {
        list.borrow_mut().retain(|h| !Rc::ptr_eq(h, &e));
    }
}

fn actor_production() -> Vec<Group<ActorRef>> {
    vec![
        Group::<ActorRef>::new("Production", &["ProductionInfo"]).method("Produce", |lua, a, args| {
            let actor_type: String = args.get(0)?;
            let faction_variant: Option<String> = args.opt(1, None)?;
            let production_type: Option<String> = args.opt(2, None)?;
            let Some(actor_info) = world(lua)?.rules().actor(&actor_type) else {
                return error(format!("Unknown actor type '{actor_type}'"));
            };
            let Some(bi) = actor_info.trait_info::<BuildableInfo>() else {
                return error(format!("Actor of type {actor_type} cannot be produced"));
            };
            let productions = a.traits_implementing::<Production>();
            let actor = a.clone();
            a.queue_activity(WaitFor::new(
                Box::new(move || {
                    for p in &productions {
                        let ty = production_type
                            .clone()
                            .or_else(|| bi.build_at_production_type.clone())
                            .unwrap_or_default();
                        if !ty.is_empty() && !p.info.produces.contains(&ty) {
                            continue;
                        }
                        let faction = faction_variant
                            .clone()
                            .unwrap_or_else(|| get_initial_faction(&actor_info, &p.faction()));
                        let mut inits = ActorInits::new();
                        inits.push(Rc::new(OwnerInit::from_player(&actor.owner())));
                        inits.push(Rc::new(FactionInit::new(faction)));
                        if p.produce(&actor, &actor_info, &ty, inits, 0) {
                            return true;
                        }
                    }
                    false
                }),
                true,
            ));
            Ok(())
        }),
        Group::<ActorRef>::new("Production", &["RallyPointInfo"]).get_set(
            "RallyPoint",
            |_, a| {
                let rp = a.trait_::<RallyPoint>();
                if let Some(&last) = rp.path.borrow().last() {
                    return Ok(LuaCPos(last));
                }
                Ok(LuaCPos(match nearest_exit_or_default(a, a.center_position(), None, None) {
                    Some(exit) => a.location() + exit.info.exit_cell,
                    None => a.location(),
                }))
            },
            |_, a, value: CPos| {
                *a.trait_::<RallyPoint>().path.borrow_mut() = vec![value];
                Ok(())
            },
        ),
        Group::<ActorRef>::new("Production", &["PrimaryBuildingInfo"]).get_set(
            "IsPrimaryBuilding",
            |_, a| Ok(a.trait_::<PrimaryBuilding>().is_primary()),
            |_, a, value: bool| {
                a.trait_::<PrimaryBuilding>().set_primary_producer(a, value);
                Ok(())
            },
        ),
        Group::<ActorRef>::new("Production", &["ProductionQueueInfo", "ScriptTriggersInfo"])
            .method("Build", |lua, a, args| {
                let actor_types: Vec<String> = args.get(0)?;
                let action: Option<Function> = args.opt(1, None)?;
                let queues = actor_queues(lua, a)?;
                let triggers = a.trait_::<ScriptTriggers>();
                if triggers.has_any_callbacks_for(Trigger::OnProduction) {
                    return Ok(false);
                }
                let mut queue = None;
                for q in queues.iter() {
                    let mut all = true;
                    for t in &actor_types {
                        if !buildable_info(lua, t)?.queue.contains(&q.info.ty) {
                            all = false;
                            break;
                        }
                    }
                    if all && q.all_queued().is_empty() {
                        queue = Some(q.clone());
                        break;
                    }
                }
                let Some(queue) = queue else {
                    return Ok(false);
                };
                if let Some(f) = action {
                    let player = a.owner();
                    let squad = Squad {
                        context: Rc::downgrade(&ctx(lua)?),
                        lua: lua.clone(),
                        f,
                        size: actor_types.len(),
                        units: RefCell::new(Vec::new()),
                    };
                    let weak_triggers = Rc::downgrade(&triggers);
                    let handler: ActorPairEvent = Rc::new_cyclic(|me: &Weak<_>| {
                        let me: Weak<dyn Fn(&ActorRef, &ActorRef)> = me.clone();
                        move |factory: &ActorRef, unit: &ActorRef| {
                            let done = !Rc::ptr_eq(&player, &factory.owner()) || squad.add(unit);
                            if let (true, Some(t)) = (done, weak_triggers.upgrade()) {
                                remove_event(&t.on_produced_internal, &me);
                            }
                        }
                    });
                    triggers.on_produced_internal.borrow_mut().push(handler);
                }
                for actor_type in &actor_types {
                    queue.resolve_order(a, &start_production(a, actor_type));
                }
                Ok(true)
            })
            .method("IsProducing", |lua, a, args| {
                let actor_type: String = args.get(0)?;
                let queues = actor_queues(lua, a)?;
                if a.trait_::<ScriptTriggers>().has_any_callbacks_for(Trigger::OnProduction) {
                    return Ok(true);
                }
                for q in queues.iter() {
                    if buildable_info(lua, &actor_type)?.queue.contains(&q.info.ty) && !q.all_queued().is_empty() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }),
    ]
}

/// The enabled queues of `a` when its properties were first used.
fn actor_queues(lua: &Lua, a: &ActorRef) -> LuaResult<Rc<Vec<Rc<ProductionQueue>>>> {
    Ok(ctx(lua)?.group_state(a.id, "ProductionQueueProperties", || {
        a.traits_implementing::<ProductionQueue>()
            .into_iter()
            .filter(|q| q.enabled())
            .collect()
    }))
}

type Handler = Rc<dyn Fn(&ActorRef, &ActorRef)>;

/// The fields of `ClassicProductionQueueProperties`.
struct ClassicState {
    handlers: RefCell<HashMap<String, Handler>>,
    queues: IndexMap<String, Rc<ProductionQueue>>,
}

fn classic_state(lua: &Lua, p: &PlayerRef) -> LuaResult<Rc<ClassicState>> {
    let context = ctx(lua)?;
    let pa = p.player_actor().clone();
    let mut created = false;
    let state = context.group_state(pa.id, "ClassicProductionQueueProperties", || {
        created = true;
        let queues = pa
            .traits_implementing::<ProductionQueue>()
            .into_iter()
            .filter(|q| q.is_classic() && q.enabled())
            .map(|q| (q.info.ty.clone(), q))
            .collect();
        ClassicState {
            handlers: RefCell::new(HashMap::new()),
            queues,
        }
    });
    if created {
        let (weak_state, player, lua) = (Rc::downgrade(&state), p.clone(), lua.clone());
        let global: ActorPairEvent = Rc::new(move |factory: &ActorRef, unit: &ActorRef| {
            if !factory.is_owned_by(&player) {
                return;
            }
            let Some(state) = weak_state.upgrade() else { return };
            let Ok(queue) = buildable_info(&lua, &unit.info.name).and_then(|bi| first_queue(&bi)) else {
                return;
            };
            let handler = state.handlers.borrow().get(&queue).cloned();
            if let Some(h) = handler {
                h(factory, unit);
            }
        });
        pa.trait_::<ScriptTriggers>().on_other_produced_internal.borrow_mut().push(global);
    }
    Ok(state)
}

fn player_production() -> Group<PlayerRef> {
    Group::<PlayerRef>::new("Production", &["ClassicProductionQueueInfo", "ScriptTriggersInfo"])
        .method("Build", |lua, p, args| {
            let actor_types: Vec<String> = args.get(0)?;
            let action: Option<Function> = args.opt(1, None)?;
            let state = classic_state(lua, p)?;
            let mut type_to_queue = IndexMap::new();
            for t in actor_types.iter().collect::<IndexSet<_>>() {
                let bi = buildable_info(lua, t)?;
                type_to_queue.insert(t.clone(), first_queue(&bi)?);
            }
            let queue_types: Vec<String> = type_to_queue.values().cloned().collect::<IndexSet<_>>().into_iter().collect();
            if queue_types
                .iter()
                .any(|t| !state.queues.contains_key(t) || state.handlers.borrow().contains_key(t))
            {
                return Ok(false);
            }
            if queue_types.iter().any(|t| !state.queues[t].all_queued().is_empty()) {
                return Ok(false);
            }
            if let Some(f) = action {
                let squad = Squad {
                    context: Rc::downgrade(&ctx(lua)?),
                    lua: lua.clone(),
                    f,
                    size: actor_types.len(),
                    units: RefCell::new(Vec::new()),
                };
                let (weak_state, types) = (Rc::downgrade(&state), queue_types.clone());
                let handler: Handler = Rc::new(move |_factory: &ActorRef, unit: &ActorRef| {
                    if squad.add(unit)
                        && let Some(state) = weak_state.upgrade()
                    {
                        for q in &types {
                            state.handlers.borrow_mut().remove(q);
                        }
                    }
                });
                for q in &queue_types {
                    state.handlers.borrow_mut().insert(q.clone(), handler.clone());
                }
            }
            for actor_type in &actor_types {
                let queue = &state.queues[&type_to_queue[actor_type]];
                queue.resolve_order(&queue.actor, &start_production(&queue.actor, actor_type));
            }
            Ok(true)
        })
        .method("IsProducing", |lua, p, args| {
            let actor_type: String = args.get(0)?;
            let state = classic_state(lua, p)?;
            let bi = buildable_info(lua, &actor_type)?;
            let queue = first_queue(&bi)?;
            Ok(match state.queues.get(&queue) {
                None => true,
                Some(q) => state.handlers.borrow().contains_key(&queue) || !q.all_queued().is_empty(),
            })
        })
}

pub fn actor_groups() -> Vec<Group<ActorRef>> {
    actor_production()
}

pub fn player_groups() -> Vec<Group<PlayerRef>> {
    vec![player_production()]
}
