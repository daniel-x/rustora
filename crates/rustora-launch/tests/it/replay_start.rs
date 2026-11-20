// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A replay whose server dropped two clients that were not ready at the start: their disconnects come before the
//! `StartGame` order and must not reach the order manager of the game.

use std::path::PathBuf;

use rustora_engine::order_manager::ReplayConnection;
use rustora_launch::skirmish::SkirmishDriver;
use rustora_launch::{Step, load_game};

#[test]
fn a_replay_with_disconnects_before_the_start_plays() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let engine = home.join("openra-ref");
    let replay = home.join("testdata/replays/prestart-disconnect.orarep");
    if !engine.join("mods/ra/mod.yaml").exists() || !replay.exists() {
        return;
    }
    let support = home.join("testdata/support");
    let game = load_game(
        &engine,
        support.to_str().expect("support dir"),
        replay.to_str().expect("replay path"),
    );
    let connection = ReplayConnection::new(game.replay.chunks, game.world.order_latency);
    let mut driver = SkirmishDriver::replay(&game.world, Box::new(connection));
    let mut ticks = 0;
    while ticks < 200 {
        match driver.step() {
            Step::Ticked => ticks += 1,
            Step::Waiting => {}
            Step::Ended => panic!("the replay ended after {ticks} ticks"),
        }
    }
}
