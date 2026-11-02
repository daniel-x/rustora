// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Loads all RA actor rules into typed infos (stubs for unported traits).

use std::path::PathBuf;

use rustora_engine::rules::{Ruleset, TraitRegistry};
use rustora_fs::Platform;
use rustora_game::moddata::ModData;

#[test]
fn ra_rules_load() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    if !home.join("testdata/golden").is_dir() {
        return;
    }
    let md = ModData::load(Platform::new(home.join("openra-ref"), home.join("testdata/support")), "ra").unwrap();
    let rules = ModData::load_yaml(&md.file_system, &md.manifest.rules, None).unwrap();
    let registry = TraitRegistry::default();
    let ruleset = Ruleset::load(&registry, &rules).unwrap();
    assert!(ruleset.actor("mcv").is_some());
    assert!(ruleset.actor("world").is_some());
    assert_eq!(ruleset.actors().count(), 307);
}
