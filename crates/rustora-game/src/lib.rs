// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Mod data: manifest, mounted file system, maps and rule sets.

pub mod manifest;
pub mod map;
pub mod moddata;

#[derive(Debug, thiserror::Error)]
pub enum GameError {
    #[error("{0}")]
    Data(String),
    #[error(transparent)]
    Yaml(#[from] rustora_core::miniyaml::YamlError),
}
