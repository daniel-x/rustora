// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Loads all RA actor rules into typed infos (stubs for unported traits).

use std::path::PathBuf;

use rustora_engine::rules::{Ruleset, TraitRegistry};
use rustora_fs::Platform;
use rustora_game::moddata::ModData;

#[test]
fn ra_rules_load() {
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let support = std::env::temp_dir().join(format!("rustora-ruleset-test-{}", std::process::id()));
    let md = ModData::load(Platform::new(engine, support), "ra").unwrap();
    let rules = ModData::load_yaml(&md.file_system, &md.manifest.rules, None).unwrap();
    let registry = TraitRegistry::default();
    let ruleset = Ruleset::load(&registry, &Default::default(), &rules, &[]).unwrap();
    assert!(ruleset.actor("mcv").is_some());
    assert!(ruleset.actor("world").is_some());
    assert_eq!(ruleset.actors().count(), 307);
}
