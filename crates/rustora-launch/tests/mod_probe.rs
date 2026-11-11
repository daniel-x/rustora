// SPDX-License-Identifier: GPL-3.0-or-later
//! Temporary probe: unported traits of the other mods.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rustora_engine::rules::{Ruleset, TraitRegistry};
use rustora_fs::Platform;
use rustora_game::moddata::ModData;

#[test]
#[ignore]
fn probe_mods() {
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../openra-ref");
    let support = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/support");
    let mut registry = TraitRegistry::default();
    rustora_mods_common::register(&mut registry);
    rustora_scripting::register(&mut registry);
    for m in ["ra", "cnc", "d2k", "ts"] {
        let md = match ModData::load(Platform::new(engine.clone(), support.clone()), m) {
            Ok(md) => md,
            Err(e) => {
                println!("== {m}: ModData error {e:?}");
                continue;
            }
        };
        let rules = ModData::load_yaml(&md.file_system, &md.manifest.rules, None).unwrap();
        let mut missing: BTreeMap<String, usize> = BTreeMap::new();
        for a in &rules {
            for t in &a.value.nodes {
                let name = t.key.split('@').next().unwrap().trim_start_matches('-');
                if name.is_empty() || name.chars().next().unwrap().is_lowercase() || registry.is_ported(name) {
                    continue;
                }
                *missing.entry(name.to_string()).or_default() += 1;
            }
        }
        println!("== {m}: {} unported trait names", missing.len());
        for (k, v) in &missing {
            println!("  {k} {v}");
        }
        let weapons = ModData::load_yaml(&md.file_system, &md.manifest.weapons, None).unwrap();
        let mut wr = rustora_engine::weapon::WeaponRegistry::default();
        rustora_mods_common::weapons::register_weapon_parts(&mut wr);
        match Ruleset::load(&registry, &wr, &rules, &weapons) {
            Ok(r) => println!("== {m}: ruleset ok, {} actors", r.actors().count()),
            Err(e) => println!("== {m}: ruleset error {e}"),
        }
    }
}
