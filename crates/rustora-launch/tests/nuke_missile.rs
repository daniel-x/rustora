// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A launched nuke draws its missile, like `NukeLaunch.Render` in C#.

use std::path::PathBuf;

use rustora_launch::{ReplayDriver, Step, load_game};
use rustora_mods_common::support_powers::NukeLaunch;

#[test]
fn a_launched_nuke_draws_its_missile() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let engine = home.join("openra-ref");
    if !engine.join("mods/ra/mod.yaml").exists() {
        return;
    }
    let support = home.join("testdata/support");
    let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus/c020-singles-normal-england-rush-germany.orarep");
    let game = load_game(
        &engine,
        support.to_str().expect("support dir"),
        replay.to_str().expect("replay path"),
    );
    let world = game.world;
    let mut driver = ReplayDriver::new(&world, game.replay);
    loop {
        let drawn = world.effects().iter().any(|e| {
            e.as_any().downcast_ref::<NukeLaunch>().is_some_and(|_| {
                let mut out = Vec::new();
                e.render(None, &mut out);
                !out.is_empty()
            })
        });
        if drawn {
            return;
        }
        assert_ne!(driver.step(), Step::Ended, "a nuke missile is drawn");
    }
}
