// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A power outage shows its remaining time as a yellow selection bar on the affected buildings, like
//! `AffectedByPowerOutage` in C#.

use std::path::PathBuf;

use rustora_engine::traits::ISelectionBar;
use rustora_launch::{ReplayDriver, Step, load_game};
use rustora_mods_common::base::{AffectedByPowerOutage, PowerManager};

#[test]
fn power_outage_bar_counts_down() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let engine = home.join("openra-ref");
    if !engine.join("mods/ra/mod.yaml").exists() {
        return;
    }
    let support = home.join("testdata/support");
    let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus/c000-ore-lord-normal-england-rush-germany.orarep");
    let game = load_game(
        &engine,
        support.to_str().expect("support dir"),
        replay.to_str().expect("replay path"),
    );
    let world = game.world;
    let mut driver = ReplayDriver::new(&world, game.replay);
    let building = loop {
        let found = world
            .actors()
            .into_iter()
            .find(|a| a.is_in_world() && !a.traits_implementing::<AffectedByPowerOutage>().is_empty());
        if let Some(building) = found {
            break building;
        }
        assert_ne!(driver.step(), Step::Ended, "a building affected by power outages appears");
    };
    let bar = |building: &rustora_engine::actor::ActorRef| {
        let bars = building.traits_implementing::<AffectedByPowerOutage>();
        bars[0].value(building, None)
    };
    assert_eq!(bar(&building), 0.0, "no outage, no bar");
    let power = building.owner().player_actor().trait_::<PowerManager>();
    power.trigger_power_outage(100);
    assert_eq!(bar(&building), 1.0);
    let start = world.world_tick();
    while world.world_tick() < start + 50 {
        assert_ne!(driver.step(), Step::Ended);
    }
    assert!(
        (bar(&building) - 0.5).abs() < 0.02,
        "half of the outage is left: {}",
        bar(&building)
    );
}
