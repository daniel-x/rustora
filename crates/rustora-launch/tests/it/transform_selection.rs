// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A deploying MCV hands its selection and control group to the construction yard, like `Transform` in C#.

use std::path::PathBuf;
use std::rc::Rc;

use rustora_launch::{ReplayDriver, Step, load_game};
use rustora_mods_common::selection::{ControlGroups, Selection};

#[test]
fn deployed_mcv_stays_selected_and_grouped() {
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
    let count = |name: &str| world.actors().iter().filter(|a| a.info.name == name && !a.disposed()).count();
    while count("mcv") == 0 {
        assert_ne!(driver.step(), Step::Ended, "the game has MCVs");
    }
    let selection = Selection::of(&world).expect("selection");
    let groups = ControlGroups::of(&world).expect("control groups");
    let mcv = world.actors().into_iter().find(|a| a.info.name == "mcv").expect("an MCV");
    groups.set_local_player(Some(mcv.owner()));
    selection.add(&mcv);
    groups.create_control_group(&selection, 3);
    while !mcv.disposed() {
        assert_ne!(driver.step(), Step::Ended, "the MCV deploys");
    }
    driver.step();
    let fact = mcv.replaced_by_actor.borrow().clone().expect("the MCV became a construction yard");
    assert_eq!(fact.info.name, "fact");
    assert!(selection.contains(&fact), "the construction yard is selected");
    assert!(
        groups.actors_in_control_group(3).iter().any(|a| Rc::ptr_eq(a, &fact)),
        "the construction yard is in group 3"
    );
}
