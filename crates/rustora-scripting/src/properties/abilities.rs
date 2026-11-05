// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of the ability and support power property groups: `AmmoPoolProperties.cs`,
//! `AirstrikeProperties.cs`, `CaptureProperties.cs`, `CloakProperties.cs`, `DeliveryProperties.cs`
//! (`DeliversCash`), `DemolitionProperties.cs`, `GuardProperties.cs`, `HarvesterProperties.cs`,
//! `InstantlyRepairsProperties.cs`, `NukeProperties.cs`, `ParadropProperties.cs`,
//! `ParatroopersProperties.cs`, `RepairableBuildingProperties.cs`, `SellableProperties.cs`,
//! `TransformProperties.cs` and the Cnc `ChronosphereProperties.cs`, `DisguiseProperties.cs` and
//! `InfiltrateProperties.cs`.

use std::rc::Rc;

use mlua::{Table, Value};
use rustora_core::world_types::{CPos, WAngle, WPos};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_ext::enabled_target_types;
use rustora_engine::player::PlayerRef;
use rustora_engine::target::Target;
use rustora_engine::traits::IDisabledTrait;
use rustora_mods_common::aircraft::{Fly, FlyOffMap, RemoveSelf};
use rustora_mods_common::auto_target::{Guard, GuardableInfo};
use rustora_mods_common::base::RepairableBuilding;
use rustora_mods_common::capture::{CaptureManager, capture_activity};
use rustora_mods_common::cargo::ParaDrop;
use rustora_mods_common::cash_repair::{DeliversCashInfo, InstantlyRepairs, donate_cash_activity};
use rustora_mods_common::chrono::Chronoshiftable;
use rustora_mods_common::cloak::Cloak;
use rustora_mods_common::demolition::Demolition;
use rustora_mods_common::harvester::FindAndDeliverResources;
use rustora_mods_common::infiltrate::{Disguise, Infiltrates};
use rustora_mods_common::resupply::AmmoPool;
use rustora_mods_common::support_powers::{PowerKind, SupportPower};
use rustora_mods_common::transforms::{Sellable, Transforms};

use crate::members::Group;
use crate::values::{FromScript, LuaResult, actors_table, actor_string, error, type_name};

type G = Group<ActorRef>;

fn ammo_pool(a: &ActorRef, pool_name: &str) -> LuaResult<Rc<AmmoPool>> {
    match a.traits_implementing::<AmmoPool>().into_iter().find(|p| p.info.name == pool_name) {
        Some(p) => Ok(p),
        None => error(format!("Invalid ammopool name {pool_name} queried on actor {}.", actor_string(a))),
    }
}

fn ammo_pool_group() -> G {
    G::new("AmmoPool", &["AmmoPoolInfo"])
        .method("AmmoCount", |_, a, args| {
            let name: String = args.opt(0, "primary".into())?;
            Ok(ammo_pool(a, &name)?.current_ammo_count())
        })
        .method("MaximumAmmoCount", |_, a, args| {
            let name: String = args.opt(0, "primary".into())?;
            Ok(ammo_pool(a, &name)?.info.ammo)
        })
        .method("Reload", |_, a, args| {
            let name: String = args.opt(0, "primary".into())?;
            let amount: i32 = args.opt(1, 1)?;
            let pool = ammo_pool(a, &name)?;
            if amount > 0 {
                pool.give_ammo(a, amount);
            } else {
                pool.take_ammo(a, amount.wrapping_neg());
            }
            Ok(())
        })
}

/// `self.TraitsImplementing<XPower>().First()`.
fn support_power(a: &ActorRef, kind: PowerKind) -> LuaResult<Rc<SupportPower>> {
    match a.traits_implementing::<SupportPower>().into_iter().find(|p| p.info.kind == kind) {
        Some(p) => Ok(p),
        None => error("Sequence contains no elements"),
    }
}

fn support_power_groups() -> Vec<G> {
    vec![
        G::new("Support Powers", &["AirstrikePowerInfo"]).method("TargetAirstrike", |lua, a, args| {
            let target: WPos = args.get(0)?;
            let facing: Option<WAngle> = args.opt(1, None)?;
            let aircraft = support_power(a, PowerKind::Airstrike)?.send_airstrike(a, target, facing);
            actors_table(lua, &aircraft)
        }),
        G::new("Support Powers", &["ParatroopersPowerInfo"]).method("TargetParatroopers", |lua, a, args| {
            let target: WPos = args.get(0)?;
            let facing: Option<WAngle> = args.opt(1, None)?;
            let aircraft = support_power(a, PowerKind::Paratroopers)?.send_paratroopers(a, target, facing);
            actors_table(lua, &aircraft)
        }),
        G::new("Support Powers", &["NukePowerInfo"]).method("ActivateNukePower", |_, a, args| {
            let target: CPos = args.get(0)?;
            let pos = a.world().map().center_of_cell(target);
            support_power(a, PowerKind::Nuke)?.launch_nuke(a, pos);
            Ok(())
        }),
        G::new("Support Powers", &["ChronoshiftPowerInfo"]).method("Chronoshift", |_, a, args| {
            let pairs: Table = args.get(0)?;
            let duration: i32 = args.opt(1, 0)?;
            let kill_cargo: bool = args.opt(2, false)?;
            for kv in pairs.pairs::<Value, Value>() {
                let (k, v) = kv?;
                let (Some(actor), Some(cell)) = (ActorRef::from_script(&k), CPos::from_script(&v)) else {
                    return error(format!(
                        "Chronoshift requires a table of actor,cpos pairs. Received {},{}",
                        type_name(&k),
                        type_name(&v)
                    ));
                };
                let cs = actor.traits_implementing::<Chronoshiftable>().into_iter().find(|c| !c.is_trait_disabled());
                if let Some(cs) = cs
                    && cs.can_chronoshift_to(cell)
                {
                    cs.teleport(cell, duration, kill_cargo, a);
                }
            }
            Ok(())
        }),
    ]
}

fn can_capture(a: &ActorRef, target: &ActorRef) -> bool {
    target
        .trait_or_default::<CaptureManager>()
        .is_some_and(|t| a.trait_::<CaptureManager>().can_target(&t))
}

fn ability_groups() -> Vec<G> {
    vec![
        G::new("Ability", &["CaptureManagerInfo"])
            .method("Capture", |_, a, args| {
                let target: ActorRef = args.get(0)?;
                if can_capture(a, &target) {
                    a.queue_activity(capture_activity(a, &Target::from_actor(Some(&target))));
                }
                Ok(())
            })
            .method("CanCapture", |_, a, args| Ok(can_capture(a, &args.get(0)?))),
        G::new("Cloak", &["CloakInfo"]).get("IsCloaked", |_, a| {
            Ok(a.traits_implementing::<Cloak>().iter().any(|c| c.cloaked()))
        }),
        G::new("Ability", &["IMoveInfo", "DeliversCashInfo"]).method("DeliverCash", |_, a, args| {
            let target: ActorRef = args.get(0)?;
            let info = a.info.trait_info::<DeliversCashInfo>().expect("required");
            a.queue_activity(donate_cash_activity(a, &Target::from_actor(Some(&target)), info.payload, info.player_experience));
            Ok(())
        }),
        G::new("Combat", &["IMoveInfo", "DemolitionInfo"]).method("Demolish", |_, a, args| {
            let target: ActorRef = args.get(0)?;
            let d = a.traits_implementing::<Demolition>().into_iter().find(|d| !d.is_trait_disabled());
            if let Some(d) = d {
                a.queue_activity(d.demolish_activity(a, &Target::from_actor(Some(&target))));
            }
            Ok(())
        }),
        G::new("Combat", &["GuardInfo", "IMoveInfo"]).method("Guard", |_, a, args| {
            let target: ActorRef = args.get(0)?;
            if target.info.has_trait_info::<GuardableInfo>() {
                a.trait_::<Guard>().guard_target(a, &Target::from_actor(Some(&target)), false);
            }
            Ok(())
        }),
        G::new("Movement", &["HarvesterInfo"]).method("FindResources", |_, a, _| {
            a.queue_activity(FindAndDeliverResources::new(a, None));
            Ok(())
        }),
        G::new("Ability", &["IMoveInfo", "InstantlyRepairsInfo"]).method("InstantlyRepair", |_, a, args| {
            let target: ActorRef = args.get(0)?;
            let r = a.traits_implementing::<InstantlyRepairs>().into_iter().find(|r| !r.is_trait_disabled());
            if let Some(r) = r {
                a.queue_activity(r.instant_repair_activity(a, &Target::from_actor(Some(&target))));
            }
            Ok(())
        }),
        G::new("Transports", &["CargoInfo", "ParaDropInfo"]).method("Paradrop", |_, a, args| {
            let cell: CPos = args.get(0)?;
            a.trait_::<ParaDrop>().set_lz(cell, true);
            a.queue_activity(Fly::new(a, Target::from_cell(&a.world(), cell, 0)));
            a.queue_activity(FlyOffMap::new(a, None));
            a.queue_activity(RemoveSelf::new());
            Ok(())
        }),
        G::new("General", &["RepairableBuildingInfo"])
            .method("StartBuildingRepairs", |_, a, args| {
                let repairer = args.opt::<Option<PlayerRef>>(0, None)?.unwrap_or_else(|| a.owner());
                let rb = a.trait_::<RepairableBuilding>();
                if !rb.repairers.borrow().iter().any(|p| Rc::ptr_eq(p, &repairer)) {
                    rb.repair_building(a, &repairer);
                }
                Ok(())
            })
            .method("StopBuildingRepairs", |_, a, args| {
                let repairer = args.opt::<Option<PlayerRef>>(0, None)?.unwrap_or_else(|| a.owner());
                let rb = a.trait_::<RepairableBuilding>();
                if rb.repair_active() && rb.repairers.borrow().iter().any(|p| Rc::ptr_eq(p, &repairer)) {
                    rb.repair_building(a, &repairer);
                }
                Ok(())
            }),
        G::new("General", &["SellableInfo"]).method("Sell", |_, a, _| {
            a.trait_::<Sellable>().sell(a);
            Ok(())
        }),
        G::new("General", &["TransformsInfo"]).method("Deploy", |_, a, _| {
            a.trait_::<Transforms>().deploy_transform(true);
            Ok(())
        }),
        G::new("Ability", &["DisguiseInfo"])
            .method("DisguiseAs", |_, a, args| {
                let target: ActorRef = args.get(0)?;
                a.trait_::<Disguise>().disguise_as_actor(Some(&target));
                Ok(())
            })
            .method("DisguiseAsType", |_, a, args| {
                let actor_type: String = args.get(0)?;
                let new_owner: PlayerRef = args.get(1)?;
                let Some(info) = a.world().rules().actor(&actor_type) else {
                    return error(format!("The given key '{actor_type}' was not present in the dictionary."));
                };
                a.trait_::<Disguise>().disguise_as_info(info, Some(new_owner));
                Ok(())
            }),
        G::new("Ability", &["InfiltratesInfo"]).method("Infiltrate", |_, a, args| {
            let target: ActorRef = args.get(0)?;
            let types = enabled_target_types(&target);
            let infiltrates = a
                .traits_implementing::<Infiltrates>()
                .into_iter()
                .find(|i| !i.is_trait_disabled() && i.info().types.overlaps(&types));
            match infiltrates {
                Some(i) => {
                    a.queue_activity(i.infiltrate_activity(a, &Target::from_actor(Some(&target))));
                    Ok(())
                }
                None => error(format!(
                    "{} tried to infiltrate invalid target {}!",
                    actor_string(a),
                    actor_string(&target)
                )),
            }
        }),
    ]
}

pub fn actor_groups() -> Vec<G> {
    let mut groups = vec![ammo_pool_group()];
    groups.extend(support_power_groups());
    groups.extend(ability_groups());
    groups
}
