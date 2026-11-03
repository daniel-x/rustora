// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The value formats of the `IGameSaveTraitData` nodes (`FieldSaver.FormatValue`) of the bot modules.

use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{CPos, WVec};

pub fn node_value<'a>(data: &'a MiniYaml, key: &str) -> Option<&'a str> {
    data.nodes.iter().find(|n| n.key == key).map(|n| n.value.value_str())
}

pub fn format_cpos(c: CPos) -> String {
    format!("{},{}", c.x(), c.y())
}

pub fn parse_cpos(text: &str) -> Option<CPos> {
    let (x, y) = text.split_once(',')?;
    Some(CPos::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

pub fn format_wvec(v: WVec) -> String {
    format!("{},{},{}", v.x, v.y, v.z)
}

pub fn parse_wvec(text: &str) -> Option<WVec> {
    let mut parts = text.split(',').map(|p| p.trim().parse().ok());
    Some(WVec::new(parts.next()??, parts.next()??, parts.next()??))
}

pub fn format_ids(ids: impl Iterator<Item = u32>) -> String {
    ids.map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
}

pub fn parse_ids(text: &str) -> impl Iterator<Item = u32> + '_ {
    text.split(',').filter_map(|p| p.trim().parse().ok())
}

pub fn parse_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let c = CPos::new(-3, 7);
        assert_eq!(format_cpos(c), "-3,7");
        assert_eq!(parse_cpos("-3,7"), Some(c));
        assert_eq!(parse_wvec(&format_wvec(WVec::new(1, -2, 3))), Some(WVec::new(1, -2, 3)));
        assert_eq!(format_ids([1u32, 2].into_iter()), "1, 2");
        assert_eq!(parse_ids("1, 2").collect::<Vec<_>>(), [1, 2]);
        assert_eq!(parse_ids("").count(), 0);
        assert_eq!(parse_list("e1, e3"), ["e1", "e3"]);
        assert!(parse_list("").is_empty());
    }
}
