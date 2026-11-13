// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A firing armament with a `MuzzleSequence` shows its muzzle flash, like `WithMuzzleOverlay` in C#.

use std::path::PathBuf;

use rustora_launch::{ReplayDriver, Step, load_game};
use rustora_mods_common::render_traits::WithMuzzleOverlay;

#[test]
fn firing_shows_the_muzzle_flash() {
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
    loop {
        if world
            .actors_with_trait::<WithMuzzleOverlay>()
            .iter()
            .any(|(_, m)| m.visible_flashes() > 0)
        {
            return;
        }
        assert_ne!(driver.step(), Step::Ended, "a muzzle flash appears");
    }
}
