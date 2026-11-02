// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Map packages: `map.yaml` sections and the map UID. Port of parts of `OpenRA.Game/Map/Map.cs`.

use std::sync::Arc;

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::Package;
use sha1::{Digest, Sha1};

use crate::GameError;

/// The raw `map.yaml` of a map package plus its UID.
pub struct MapPackage {
    pub package: Arc<dyn Package>,
    pub yaml: MiniYaml,
    pub uid: String,
    pub format: i32,
}

impl MapPackage {
    pub fn load(package: Arc<dyn Package>) -> Result<Self, GameError> {
        let name = package.name().to_string();
        if !package.contains("map.yaml") || !package.contains("map.bin") {
            return Err(GameError::Data(format!("Not a valid map\n File: {name}")));
        }
        let data = package
            .get("map.yaml")
            .ok_or_else(|| GameError::Data(format!("{name}: map.yaml unreadable")))?;
        let yaml = MiniYaml::new(None, MiniYaml::from_bytes(&data, &format!("{name}:map.yaml"), true)?);
        let format = yaml.node_with_key("MapFormat")?.value.value_str().trim().parse().unwrap_or(0);
        let uid = compute_uid(package.as_ref(), format)?;
        Ok(MapPackage {
            package,
            yaml,
            uid,
            format,
        })
    }

    /// The node value of a custom definition section such as `Rules` or `Weapons`.
    pub fn section(&self, key: &str) -> Option<&MiniYaml> {
        self.yaml.nodes.iter().find(|n| n.key == key).map(|n| &n.value)
    }

    pub fn section_nodes(&self, key: &str) -> &[MiniYamlNode] {
        self.section(key).map_or(&[], |y| y.nodes.as_slice())
    }
}

/// SHA1 over all `.yaml`, `.bin`, `.lua` (and from format 12 on `map.png`) files in package order.
pub fn compute_uid(package: &dyn Package, format: i32) -> Result<String, GameError> {
    let contents = package.contents();
    for required in ["map.yaml", "map.bin"] {
        if !contents.iter().any(|c| c == required) {
            return Err(GameError::Data(format!("Required file {required} not present in this map")));
        }
    }
    let mut hasher = Sha1::new();
    for f in &contents {
        if f.ends_with(".yaml") || f.ends_with(".bin") || f.ends_with(".lua") || (format >= 12 && f == "map.png") {
            hasher.update(package.get(f).unwrap_or_default());
        }
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
