// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `MobileProperties.cs`, `AircraftProperties.cs` and `TransportProperties.cs`.

use rustora_core::world_types::{CPos, WDist};
use rustora_engine::actor::ActorRef;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::ANY;
use rustora_mods_common::activities::{Move, Nudge};
use rustora_mods_common::aircraft::{Aircraft, Fly, Land, LandArgs, ReturnToBase};
use rustora_mods_common::cargo::{Cargo, RideTransport, UnloadCargo};
use rustora_mods_common::interfaces::IPositionable;
use rustora_mods_common::mobile::Mobile;
use rustora_mods_common::resupply::Resupply;

use crate::members::Group;
use crate::values::{LuaActor, actors_table, error};

type G = Group<ActorRef>;

fn mobile() -> G {
    G::new("Movement", &["MobileInfo"])
        .method("Move", |_, a, args| {
            let cell: CPos = args.get(0)?;
            let close_enough: i32 = args.opt(1, 0)?;
            a.queue_activity(Move::new_near(a, cell, WDist::from_cells(close_enough), None, false));
            Ok(())
        })
        .method("ScriptedMove", |_, a, args| {
            a.queue_activity(Move::new_script(a, args.get(0)?));
            Ok(())
        })
        .method("MoveIntoWorld", |_, a, args| {
            let cell: CPos = args.get(0)?;
            let mobile = a.trait_::<Mobile>();
            let pos = a.center_position();
            mobile.set_position_cell(a, cell, ANY);
            mobile.set_center_position(a, pos);
            if let Some(activity) = rustora_mods_common::interfaces::IMove::return_to_cell(&*mobile, a) {
                a.queue_activity(activity);
            }
            Ok(())
        })
        .method("Scatter", |_, a, _| {
            a.queue_activity_queued(false, Nudge::new(a));
            Ok(())
        })
        .method("EnterTransport", |_, a, args| {
            let transport: ActorRef = args.get(0)?;
            a.queue_activity(RideTransport::new(a, &Target::from_actor(Some(&transport))));
            Ok(())
        })
        .get("IsMobile", |_, a| {
            let mobile = a.trait_::<Mobile>();
            Ok(!mobile.is_trait_disabled() && !mobile.is_trait_paused())
        })
}

fn aircraft() -> G {
    G::new("Movement", &["AircraftInfo"])
        .method("Move", |_, a, args| {
            let cell: CPos = args.get(0)?;
            a.queue_activity(Fly::new(a, Target::from_cell(&a.world(), cell, ANY)));
            Ok(())
        })
        .method("ReturnToBase", |_, a, args| {
            let destination: Option<ActorRef> = args.opt(0, None)?;
            a.queue_activity(ReturnToBase::new(a, destination, true));
            Ok(())
        })
        .method("Land", |_, a, args| {
            let land_on: ActorRef = args.get(0)?;
            a.queue_activity(Land::full(a, Target::from_actor(Some(&land_on)), LandArgs::default()));
            Ok(())
        })
        .method("Resupply", |_, a, _| {
            let aircraft = a.trait_::<Aircraft>();
            let at_land_altitude = a.world().map().distance_above_terrain(a.center_position()) == aircraft.info.land_altitude;
            if let (true, Some(host)) = (at_land_altitude, aircraft.get_actor_below()) {
                a.queue_activity(Resupply::new(a, &host, WDist::ZERO, false));
            }
            Ok(())
        })
}

fn transport() -> G {
    G::new("Transports", &["CargoInfo"])
        .get("Passengers", |lua, a| actors_table(lua, &a.trait_::<Cargo>().passengers()))
        .get("HasPassengers", |_, a| Ok(a.trait_::<Cargo>().passenger_count() > 0))
        .get("PassengerCount", |_, a| Ok(a.trait_::<Cargo>().passenger_count() as i32))
        .method("LoadPassenger", |_, a, args| {
            let passenger: ActorRef = args.get(0)?;
            if !passenger.is_idle() {
                return error("LoadPassenger requires the passenger to be idle.");
            }
            a.trait_::<Cargo>().load(a, &passenger);
            Ok(())
        })
        .method("UnloadPassenger", |_, a, args| {
            let passenger: Option<ActorRef> = args.opt(0, None)?;
            Ok(LuaActor(a.trait_::<Cargo>().unload(a, passenger.as_ref())))
        })
        .method("UnloadPassengers", |_, a, args| {
            let cell: Option<CPos> = args.opt(0, None)?;
            let range = WDist::from_cells(args.opt(1, 5)?);
            let activity = match cell {
                Some(cell) => UnloadCargo::with_destination(a, Target::from_cell(&a.world(), cell, ANY), range, true),
                None => UnloadCargo::new(a, range),
            };
            a.queue_activity(activity);
            Ok(())
        })
}

pub fn groups() -> Vec<G> {
    vec![mobile(), aircraft(), transport()]
}
