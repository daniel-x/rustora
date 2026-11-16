// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Saves a replayed game, restores it into a new world, and checks that both worlds stay equal.

use std::path::PathBuf;

use rustora_client::replay::Replay;
use rustora_launch::{ReplayDriver, Step, load_game, restore_game, save_game};

#[test]
fn restored_game_continues_like_the_original() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let engine = home.join("openra-ref");
    if !engine.join("mods/ra/mod.yaml").exists() {
        return;
    }
    let support = home.join("testdata/support");
    let support = support.to_str().expect("support dir");
    let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus/c000-ore-lord-normal-england-rush-germany.orarep");
    let replay = replay.to_str().expect("replay path");

    let game = load_game(&engine, support, replay);
    let world = game.world;
    let mut original = ReplayDriver::new(&world, game.replay);
    while world.world_tick() < 1500 {
        assert_ne!(original.step(), Step::Ended, "the replay is long enough");
    }

    let data = save_game(&world).expect("save");
    let (restored, _, _) = restore_game(&engine, support, &data, true, false).expect("restore");
    assert_eq!(save_game(&restored).expect("save the restored world"), data);

    let second = Replay::parse(&std::fs::read(replay).expect("read replay")).expect("parse replay");
    let mut resumed = ReplayDriver::resumed(&restored, second);
    while world.world_tick() < 1800 {
        let step = original.step();
        assert_eq!(resumed.step(), step);
        if step == Step::Ticked {
            assert_eq!(restored.world_tick(), world.world_tick());
            assert_eq!(restored.sync_hash(), world.sync_hash(), "sync hash at tick {}", world.world_tick());
        }
    }
    let blocks = |w| rustora_engine::savegame::debug_blocks(w).expect("save blocks");
    assert!(blocks(&world) == blocks(&restored), "the saved states differ at tick 1800");
}

/// A save game of a newer minor version restores; one of another major version does not.
#[test]
fn a_newer_minor_version_restores_and_another_major_version_does_not() {
    use rustora_engine::savegame::VERSION;
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let engine = home.join("openra-ref");
    if !engine.join("mods/ra/mod.yaml").exists() {
        return;
    }
    let support = home.join("testdata/support");
    let support = support.to_str().expect("support dir");
    let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/corpus/c000-ore-lord-normal-england-rush-germany.orarep");
    let game = load_game(&engine, support, replay.to_str().expect("replay path"));
    let mut driver = ReplayDriver::new(&game.world, game.replay);
    while game.world.world_tick() < 100 {
        assert_ne!(driver.step(), Step::Ended, "the replay is long enough");
    }
    let data = save_game(&game.world).expect("save");
    let version_at = b"RUSTORA-SAVE".len();
    let with_version = |major: u16, minor: u16| {
        let mut d = data.clone();
        d[version_at..version_at + 2].copy_from_slice(&major.to_le_bytes());
        d[version_at + 2..version_at + 4].copy_from_slice(&minor.to_le_bytes());
        d
    };
    let newer = with_version(VERSION.major, VERSION.minor + 1);
    let (restored, _, _) = restore_game(&engine, support, &newer, true, false).expect("restore a newer minor version");
    assert_eq!(restored.sync_hash(), game.world.sync_hash());
    let other = with_version(VERSION.major + 1, 0);
    assert!(restore_game(&engine, support, &other, true, false).is_err());
}
