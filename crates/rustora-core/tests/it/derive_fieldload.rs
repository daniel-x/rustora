// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use rustora_core::fieldloader::{FieldLoad, LoadError, load};
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::WDist;
use rustora_derive::FieldLoad;

#[derive(Default, FieldLoad)]
struct Base {
    requires_condition: Option<String>,
}

#[derive(Default, FieldLoad)]
struct HealthInfo {
    #[field(require, name = "HP")]
    hp: i32,
    #[field(flatten)]
    base: Base,
    range: WDist,
    types: Vec<String>,
    #[field(ignore)]
    #[allow(dead_code)]
    cached: i32,
    #[field(loader = "load_special")]
    special: i32,
}

fn load_special(my: &MiniYaml) -> Result<i32, LoadError> {
    Ok(my.nodes.len() as i32)
}

#[test]
fn derive_loads_fields() {
    let n = MiniYaml::from_str("H:\n\tHP: 50\n\tRange: 2c0\n\tTypes: a, b\n\tRequiresCondition: x\n", "t", true).unwrap();
    let h: HealthInfo = load(&n[0].value).unwrap();
    assert_eq!(h.hp, 50);
    assert_eq!(h.range, WDist::new(2048));
    assert_eq!(h.types, vec!["a", "b"]);
    assert_eq!(h.base.requires_condition.as_deref(), Some("x"));
    assert_eq!(h.special, 4);
    assert!(HealthInfo::field_names().contains(&"RequiresCondition"));
}

#[test]
fn derive_reports_missing_required() {
    let n = MiniYaml::from_str("H:\n\tRange: 2c0\n", "t", true).unwrap();
    assert!(load::<HealthInfo>(&n[0].value).is_err());
}
