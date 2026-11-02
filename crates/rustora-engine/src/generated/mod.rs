// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Tables generated from the C# reference.

mod type_table;

use std::collections::HashMap;
use std::sync::OnceLock;

/// C# type metadata of one TraitInfo class.
#[derive(Debug)]
pub struct TypeEntry {
    pub name: &'static str,
    pub is_abstract: bool,
    /// The type itself, its base classes and its interfaces.
    pub assignable: &'static [&'static str],
    pub requires: &'static [&'static str],
    pub not_before: &'static [&'static str],
    pub trait_name: Option<&'static str>,
    /// Interfaces and base classes of the trait class created by this info.
    pub trait_interfaces: &'static [&'static str],
    /// `[Sync]` members of the trait class as `name:type` (properties end in `:prop`).
    pub sync: &'static [&'static str],
}

impl TypeEntry {
    pub fn is_assignable_to(&self, ty: &str) -> bool {
        self.assignable.contains(&ty)
    }

    pub fn trait_implements(&self, interface: &str) -> bool {
        self.trait_interfaces.contains(&interface)
    }
}

/// Looks up the metadata of a TraitInfo class by name (e.g. `HealthInfo`).
pub fn type_entry(name: &str) -> Option<&'static TypeEntry> {
    static INDEX: OnceLock<HashMap<&'static str, &'static TypeEntry>> = OnceLock::new();
    INDEX
        .get_or_init(|| type_table::TYPES.iter().map(|t| (t.name, t)).collect())
        .get(name)
        .copied()
}

pub fn all_types() -> &'static [TypeEntry] {
    type_table::TYPES
}
