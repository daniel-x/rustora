// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Mod data: manifest, mounted file system, maps and rule sets.

pub mod actor_yaml;
pub mod cab;
pub mod color_picker;
pub mod installer;
pub mod lobby_rules;
pub mod manifest;
pub mod map;
pub mod map_cache;
pub mod moddata;
pub mod players;
pub mod settings;

#[derive(Debug, thiserror::Error)]
pub enum GameError {
    #[error("{0}")]
    Data(String),
    #[error(transparent)]
    Yaml(#[from] rustora_core::miniyaml::YamlError),
}
