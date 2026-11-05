// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `CombatProperties.cs` and `HealthProperties.cs`.

use std::rc::{Rc, Weak};

use mlua::{Function, Value};
use rustora_core::world_types::CPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::damage::{Damage, DamageTypes};
use rustora_engine::target::Target;
use rustora_engine::traits::IHealth;
use rustora_mods_common::activities::{CallFunc, Wait};
use rustora_mods_common::attack::{AttackBase, AttackSource, Hunt};
use rustora_mods_common::attack_move::AttackMoveActivity;
use rustora_mods_common::interfaces::IMove;
use rustora_mods_common::visibility::FrozenUnderFogInfo;

use crate::context::{ScriptContext, ctx};
use crate::members::Group;
use crate::values::{FromScript, LuaActor, actor_string};

type G = Group<ActorRef>;

fn attack_move(a: &ActorRef, cell: CPos, close_enough: i32) {
    let mv = a.trait_::<dyn IMove>();
    a.queue_activity(AttackMoveActivity::new(
        a,
        Rc::new(move || mv.move_to(cell, close_enough, None, false)),
        false,
    ));
}

fn patrol(a: &ActorRef, waypoints: Rc<Vec<CPos>>, looped: bool, wait: i32) {
    for &wpt in waypoints.iter() {
        attack_move(a, wpt, 2);
        a.queue_activity(Wait::new(wait));
    }
    if looped {
        let actor = a.clone();
        a.queue_activity(CallFunc::new(Box::new(move || patrol(&actor, waypoints, looped, wait))));
    }
}

/// `PatrolUntil`. A Lua error of `f` inside the queued repetition is reported as fatal.
fn patrol_until(context: &Weak<ScriptContext>, a: &ActorRef, waypoints: Rc<Vec<CPos>>, f: Function, wait: i32) -> mlua::Result<()> {
    patrol(a, waypoints.clone(), false, wait);
    let ret: mlua::MultiValue = f.call(LuaActor(a.clone()))?;
    let repeat = ret.front().is_some_and(|v| !matches!(v, Value::Nil | Value::Boolean(false)));
    if repeat {
        let (context, actor) = (context.clone(), a.clone());
        a.queue_activity(CallFunc::new(Box::new(move || {
            if let Err(e) = patrol_until(&context, &actor, waypoints, f, wait)
                && let Some(c) = context.upgrade()
            {
                c.fatal_error(&e);
            }
        })));
    }
    Ok(())
}

fn combat() -> G {
    G::new("Combat", &["AttackBaseInfo", "IMoveInfo"])
        .method("Hunt", |_, a, _| {
            a.queue_activity(Hunt::new(a));
            Ok(())
        })
        .method("AttackMove", |_, a, args| {
            attack_move(a, args.get(0)?, args.opt(1, 0)?);
            Ok(())
        })
        .method("Patrol", |_, a, args| {
            let waypoints: Vec<CPos> = args.get(0)?;
            patrol(a, Rc::new(waypoints), args.opt(1, true)?, args.opt(2, 0)?);
            Ok(())
        })
        .method("PatrolUntil", |lua, a, args| {
            let waypoints: Vec<CPos> = args.get(0)?;
            let f: Function = args.get(1)?;
            let wait: i32 = args.opt(2, 0)?;
            patrol_until(&Rc::downgrade(&ctx(lua)?), a, Rc::new(waypoints), f, wait)
        })
}

fn general_combat() -> G {
    G::new("Combat", &["AttackBaseInfo"])
        .method("Attack", |_, a, args| {
            let target_actor: ActorRef = args.get(0)?;
            let allow_move: bool = args.opt(1, true)?;
            let force_attack: bool = args.opt(2, false)?;
            let target = Target::from_actor(Some(&target_actor));
            if !target.is_valid_for(a) {
                log::info!(target: "lua", "{} is an invalid target for {}!", actor_string(&target_actor), actor_string(a));
            }
            let owner = a.owner();
            if !target_actor.info.has_trait_info::<FrozenUnderFogInfo>() && !target_actor.can_be_viewed_by_player(Some(&owner)) {
                log::info!(target: "lua", "{} is not revealed for player {}!", actor_string(&target_actor), owner.player_name);
            }
            for attack in a.traits_implementing::<AttackBase>() {
                attack.attack_target(&target, AttackSource::Default, true, allow_move, force_attack, None);
            }
            Ok(())
        })
        .method("CanTarget", |_, a, args| {
            let target_actor: ActorRef = args.get(0)?;
            Ok(Target::from_actor(Some(&target_actor)).is_valid_for(a))
        })
}

fn health() -> G {
    G::new("General", &["IHealthInfo"])
        .get_set(
            "Health",
            |_, a| Ok(a.trait_::<dyn IHealth>().hp()),
            |_, a, value: i32| {
                let health = a.trait_::<dyn IHealth>();
                let damage = Damage::new(health.hp().wrapping_sub(value), DamageTypes::default());
                health.inflict_damage(a, Some(a), &damage, true);
                Ok(())
            },
        )
        .get("MaxHealth", |_, a| Ok(a.trait_::<dyn IHealth>().max_hp()))
        .method("Kill", |_, a, args| {
            let health = a.trait_::<dyn IHealth>();
            let types = match args.raw(0) {
                Some(Value::String(s)) => DamageTypes::new(&[s.to_string_lossy()]),
                Some(v @ Value::Table(_)) => Vec::<String>::from_script(v).map(|ds| DamageTypes::new(&ds)).unwrap_or_default(),
                _ => DamageTypes::default(),
            };
            health.inflict_damage(a, Some(a), &Damage::new(health.max_hp(), types), true);
            Ok(())
        })
}

pub fn groups() -> Vec<G> {
    vec![combat(), general_combat(), health()]
}
