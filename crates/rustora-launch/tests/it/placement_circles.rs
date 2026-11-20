// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Placing a defense shows the build radius of the construction yards and the weapon range, like the
//! `IPlaceBuildingDecorationInfo` traits in C#.

use std::path::PathBuf;

use rustora_core::world_types::WDist;
use rustora_launch::{ReplayDriver, Step, load_game};
use rustora_mods_common::decorations::place_building_circles;

#[test]
fn placing_a_defense_shows_build_radius_and_range() {
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
    let fact = loop {
        if let Some(fact) = world.actors().into_iter().find(|a| a.info.name == "fact" && a.is_in_world()) {
            break fact;
        }
        assert_ne!(driver.step(), Step::Ended, "a construction yard appears");
    };
    let viewer = fact.owner();
    let pbox = world.rules().actor("pbox").expect("pillbox");
    let center = fact.center_position();
    let mut circles = Vec::new();
    place_building_circles(&world, &pbox, center, Some(&viewer), &mut circles);
    assert!(
        circles
            .iter()
            .any(|c| c.center == fact.center_position() && c.radius == WDist::from_cells(16)),
        "the build radius of the construction yard"
    );
    assert!(
        circles
            .iter()
            .any(|c| c.center == center && c.radius > WDist::ZERO && c.radius < WDist::from_cells(16)),
        "the range of the pillbox"
    );

    let mut enemy_view = Vec::new();
    let enemy = world
        .players()
        .into_iter()
        .find(|p| p.playable && !p.is_allied_with(Some(&*viewer)))
        .expect("an enemy");
    place_building_circles(&world, &pbox, center, Some(&enemy), &mut enemy_view);
    assert!(
        enemy_view
            .iter()
            .all(|c| c.center != fact.center_position() || c.radius != WDist::from_cells(16)),
        "enemies do not see the build radius of others"
    );
}
