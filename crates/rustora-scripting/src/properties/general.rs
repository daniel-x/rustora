// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `GeneralProperties.cs`.

use std::rc::Rc;

use mlua::Function;
use rustora_core::fieldloader::{Color, FieldValue};
use rustora_core::world_types::CPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;
use rustora_engine::traits::{IDisabledTrait, IFacing};
use rustora_mods_common::activities::{SimpleTeleport, Wait};
use rustora_mods_common::aircraft::RemoveSelf;
use rustora_mods_common::auto_target::{AutoTarget, UnitStance};
use rustora_mods_common::effects::FlashTarget;
use rustora_mods_common::tooltip::Tooltip;

use super::call_lua_func;
use crate::host::host;
use crate::members::Group;
use crate::script_traits::ScriptTags;
use crate::values::{LuaCPos, LuaPlayer, LuaWAngle, LuaWPos, actor_string, error};

fn effective_owner(a: &ActorRef) -> PlayerRef {
    a.cache()
        .effective_owner
        .as_ref()
        .and_then(|e| e.owner())
        .unwrap_or_else(|| a.owner())
}

/// `Enum<UnitStance>.TryParse(value, true, ...)`, which also accepts the numeric values.
fn parse_stance(value: &str) -> Option<UnitStance> {
    if let Ok(n) = value.trim().parse::<i32>() {
        return [
            UnitStance::HoldFire,
            UnitStance::ReturnFire,
            UnitStance::Defend,
            UnitStance::AttackAnything,
        ]
        .get(usize::try_from(n).ok()?)
        .copied();
    }
    UnitStance::parse("Stance", Some(value), None).ok()
}

fn tag<R>(a: &ActorRef, f: impl FnOnce(&ScriptTags) -> R, default: R) -> R {
    a.trait_or_default::<ScriptTags>().map_or(default, |t| f(&t))
}

type G = Group<ActorRef>;

pub fn groups() -> Vec<G> {
    vec![
        G::new("General", &[])
            .exposed_for_destroyed()
            .get_set(
                "IsInWorld",
                |_, a| Ok(a.is_in_world()),
                |_, a, value: bool| {
                    let actor = a.clone();
                    a.world()
                        .add_frame_end_task(Box::new(move |w| if value { w.add(&actor) } else { w.remove(&actor) }));
                    Ok(())
                },
            )
            .get("IsDead", |_, a| Ok(a.is_dead()))
            .get("IsIdle", |_, a| Ok(a.is_idle()))
            .get_set(
                "Owner",
                |_, a| Ok(LuaPlayer(a.owner())),
                |_, a, value: Option<PlayerRef>| match value {
                    None => error(format!(
                        "Attempted to change the owner of actor '{}' to nil value.",
                        actor_string(a)
                    )),
                    Some(p) => {
                        if !Rc::ptr_eq(&a.owner(), &p) {
                            a.change_owner(p);
                        }
                        Ok(())
                    }
                },
            )
            .get("Type", |_, a| Ok(a.info.name.clone()))
            .method("HasProperty", |lua, a, args| {
                crate::members::actor_has_property(lua, a, &args.get::<String>(0)?)
            })
            .method("Flash", |_, a, args| {
                let color: Color = args.get(0)?;
                let count: i32 = args.opt(1, 2)?;
                let interval: i32 = args.opt(2, 2)?;
                let delay: i32 = args.opt(3, 0)?;
                let w = a.world();
                w.add_effect(FlashTarget::new(&w, a.clone(), color, 0.5, count, interval, delay));
                Ok(())
            })
            .get("EffectiveOwner", |_, a| Ok(LuaPlayer(effective_owner(a)))),
        G::new("General", &[])
            .method("Teleport", |_, a, args| {
                let cell: CPos = args.get(0)?;
                a.queue_activity(SimpleTeleport::new(cell));
                Ok(())
            })
            .method("CallFunc", |lua, a, args| {
                let f: Function = args.get(0)?;
                a.queue_activity(call_lua_func(lua, f)?);
                Ok(())
            })
            .method("Wait", |_, a, args| {
                a.queue_activity(Wait::new(args.get(0)?));
                Ok(())
            })
            .method("Destroy", |_, a, _| {
                a.queue_activity(RemoveSelf::new());
                Ok(())
            })
            .method("Stop", |_, a, _| {
                a.cancel_activity();
                Ok(())
            })
            .get_set(
                "Stance",
                |_, a| Ok(a.trait_or_default::<AutoTarget>().map(|t| format!("{:?}", t.stance()))),
                |_, a, value: Option<String>| {
                    let Some(auto_target) = a.trait_or_default::<AutoTarget>() else {
                        return Ok(());
                    };
                    let value = value.unwrap_or_default();
                    match parse_stance(&value) {
                        Some(stance) => {
                            auto_target.set_stance(a, stance);
                            Ok(())
                        }
                        None => error(format!("Unknown stance type '{value}'")),
                    }
                },
            )
            .get("TooltipName", |_, a| {
                let tooltip = a.traits_implementing::<Tooltip>().into_iter().find(|t| !t.is_trait_disabled());
                Ok(tooltip.and_then(|t| t.info.name.as_deref().map(|n: &str| host().fluent_message(n, &[]))))
            })
            .get("IsTaggable", |_, a| Ok(a.trait_or_default::<ScriptTags>().is_some()))
            .method("AddTag", |_, a, args| {
                let t: Option<String> = args.get(0)?;
                Ok(tag(a, |s| s.add_tag(t.as_deref()), false))
            })
            .method("RemoveTag", |_, a, args| {
                let t: Option<String> = args.get(0)?;
                Ok(tag(a, |s| s.remove_tag(t.as_deref()), false))
            })
            .method("HasTag", |_, a, args| {
                let t: Option<String> = args.get(0)?;
                Ok(tag(a, |s| s.has_tag(t.as_deref()), false))
            }),
        G::new("General", &["IOccupySpaceInfo"])
            .get("Location", |_, a| Ok(LuaCPos(a.location())))
            .get("CenterPosition", |_, a| Ok(LuaWPos(a.center_position()))),
        G::new("General", &["IFacingInfo"]).get("Facing", |_, a| Ok(LuaWAngle(a.trait_::<dyn IFacing>().facing()))),
    ]
}
