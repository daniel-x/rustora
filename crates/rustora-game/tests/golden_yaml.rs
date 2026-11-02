// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Compares merged rule trees and map UIDs against golden output of the C# reference.
//! Needs the outer project directory with `openra-ref` and `testdata`; skipped otherwise.

use std::path::PathBuf;
use std::sync::Arc;

use rustora_core::miniyaml::nodes_to_string;
use rustora_fs::{Folder, MapFs, Package, Platform, ZipPackage};
use rustora_game::map::MapPackage;
use rustora_game::moddata::ModData;

fn project_home() -> Option<PathBuf> {
    let p = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    p.join("testdata/golden").is_dir().then_some(p)
}

fn load_ra(home: &std::path::Path) -> ModData {
    ModData::load(Platform::new(home.join("openra-ref"), home.join("testdata/support")), "ra").expect("load ra")
}

fn map_packages(home: &std::path::Path) -> Vec<(String, Arc<dyn Package>)> {
    let dir = home.join("openra-ref/mods/ra/maps");
    let mut out: Vec<(String, Arc<dyn Package>)> = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        let pkg: Arc<dyn Package> = if p.is_dir() {
            Arc::new(Folder::new(&p))
        } else {
            Arc::new(ZipPackage::open_file(&p).unwrap())
        };
        out.push((name, pkg));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

type Kind = (&'static str, fn(&rustora_game::manifest::Manifest) -> &Vec<String>, &'static str);

const KINDS: [Kind; 5] = [
    ("rules", |m| &m.rules, "Rules"),
    ("weapons", |m| &m.weapons, "Weapons"),
    ("voices", |m| &m.voices, "Voices"),
    ("notifications", |m| &m.notifications, "Notifications"),
    ("music", |m| &m.music, "Music"),
];

#[test]
fn content_is_mounted() {
    let Some(home) = project_home() else { return };
    let md = load_ra(&home);
    assert!(md.content_available, "RA content should be found in testdata/support");
    assert!(md.file_system.exists("conquer.mix"));
    assert!(md.file_system.try_open("e1.shp").is_some(), "e1.shp from conquer.mix");
}

#[test]
fn mod_yaml_matches_reference() {
    let Some(home) = project_home() else { return };
    let md = load_ra(&home);
    for (kind, files, _) in KINDS {
        let nodes = ModData::load_yaml(&md.file_system, files(&md.manifest), None).unwrap();
        let expected = std::fs::read_to_string(home.join(format!("testdata/golden/yaml/mod.{kind}.yaml"))).unwrap();
        assert!(nodes_to_string(&nodes) == expected, "mod {kind} differs");
    }
}

#[test]
fn map_yaml_and_uids_match_reference() {
    let Some(home) = project_home() else { return };
    let md = load_ra(&home);
    let hashes = std::fs::read_to_string(home.join("testdata/golden/maphash.txt")).unwrap();
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, pkg) in map_packages(&home) {
        let map = MapPackage::load(pkg.clone()).unwrap();
        let expected_uid = hashes.lines().find_map(|l| l.strip_prefix(&format!("{name} "))).unwrap();
        if map.uid != expected_uid {
            failures.push(format!("{name}: uid {} != {expected_uid}", map.uid));
        }
        let fs = MapFs {
            package: pkg.as_ref(),
            fs: &md.file_system,
        };
        for (kind, files, section) in KINDS {
            let Some(map_yaml) = map.section(section) else { continue };
            let golden = home.join(format!("testdata/golden/yaml/map.{name}.{kind}.yaml"));
            let expected = std::fs::read_to_string(&golden).unwrap();
            let actual = match ModData::load_yaml(&fs, files(&md.manifest), Some(map_yaml)) {
                Ok(n) => nodes_to_string(&n),
                Err(e) => format!("ERROR {e}"),
            };
            checked += 1;
            if expected.starts_with("ERROR") != actual.starts_with("ERROR") || (!expected.starts_with("ERROR") && actual != expected) {
                failures.push(format!("{name} {kind}"));
            }
        }
    }
    eprintln!("checked {checked} map yaml sections");
    assert!(checked > 50);
    assert!(failures.is_empty(), "{} of {checked} differ: {failures:?}", failures.len());
}
