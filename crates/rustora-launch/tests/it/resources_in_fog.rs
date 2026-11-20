// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The fog hides resource changes: the client draws the resources as the render player last saw them, like
//! `ResourceRenderer` in C#.

use std::path::PathBuf;

use rustora_launch::{ReplayDriver, load_game};
use rustora_mods_common::resources::{ResourceLayer, ResourceRenderer};

#[test]
fn a_change_under_fog_shows_once_the_cell_is_visible() {
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
    for _ in 0..100 {
        driver.step();
    }
    let layer = world.world_actor().trait_or_default::<ResourceLayer>().expect("resource layer");
    let renderer = world
        .world_actor()
        .trait_or_default::<ResourceRenderer>()
        .expect("resource renderer");
    let player = world.players().into_iter().find(|p| p.playable).expect("a player");
    let cell = world
        .map()
        .all_cells()
        .find(|&c| layer.get_resource(c).density > 1 && !player.shroud().is_visible(c))
        .expect("a resource cell under fog");
    renderer.tick_render(&world, Some(&player));
    let before = renderer.contents(cell).density;
    assert_eq!(
        before,
        layer.get_resource(cell).density,
        "the resources at the start show through the fog"
    );

    let ty = layer.get_resource(cell).ty.expect("resource type");
    layer.remove_resource(&ty, cell, 1);
    renderer.tick_render(&world, Some(&player));
    assert_eq!(renderer.contents(cell).density, before, "the fog hides the change");

    renderer.tick_render(&world, None);
    assert_eq!(
        renderer.contents(cell).density,
        before - 1,
        "the change shows once the cell is visible"
    );
}
