// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The RA shellmap runs like behind the main menu. Its battle destroys barrels, whose frozen actors must not read
//! the traits of the destroyed actor.

use std::path::PathBuf;

use rustora_engine::world::WorldType;
use rustora_game::map::MapPackage;
use rustora_launch::skirmish::{SkirmishDriver, skirmish_session};
use rustora_launch::{Step, load_world_of_type, open_map};

#[test]
fn desert_shellmap_runs() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let engine = home.join("openra-ref");
    if !engine.join("mods/ra/mod.yaml").exists() {
        return;
    }
    let support = home.join("testdata/support");
    let package = open_map(&engine, "ra", "desert-shellmap").expect("shellmap");
    let map = MapPackage::load(package.clone()).expect("shellmap loads");
    let lobby = skirmish_session(&map, &[], 0).expect("lobby");
    let (world, _, _) = load_world_of_type(
        &engine,
        support.to_str().expect("support dir"),
        package,
        lobby,
        false,
        WorldType::Shellmap,
    );
    let mut driver = SkirmishDriver::new(&world);
    for _ in 0..3000 {
        assert_ne!(driver.step(), Step::Ended);
    }
}
