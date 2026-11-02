// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Compares trait construct order of all RA actors with the C# reference.

use std::path::PathBuf;

use rustora_engine::actor_info::{construct_order, parse_trait_nodes};
use rustora_fs::Platform;
use rustora_game::moddata::ModData;

#[test]
fn construct_order_matches_reference() {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    let Ok(expected) = std::fs::read_to_string(home.join("testdata/golden/construct-order.txt")) else {
        return;
    };
    let md = ModData::load(Platform::new(home.join("openra-ref"), home.join("testdata/support")), "ra").unwrap();
    let rules = ModData::load_yaml(&md.file_system, &md.manifest.rules, None).unwrap();
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in expected.lines() {
        let (name, order) = line.split_once(": ").unwrap_or((line.trim_end_matches(':'), ""));
        let node = rules
            .iter()
            .find(|n| n.key.to_lowercase() == name)
            .unwrap_or_else(|| panic!("actor {name}"));
        let traits = parse_trait_nodes(name, &node.value.nodes).unwrap();
        let idx = construct_order(name, &traits).unwrap();
        let actual: Vec<String> = idx.iter().map(|&i| traits[i].display_name()).collect();
        if actual.join(" ") != order {
            failures.push(name.to_string());
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{} of {checked} differ: {failures:?}", failures.len());
}
