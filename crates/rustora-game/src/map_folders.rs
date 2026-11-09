// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The map folders of a mod, the `MapFolders` of its manifest, as `MapCache.LoadMaps` walks them. Client and
//! server find their maps here.

use std::sync::Arc;

use rustora_fs::{FileSystem, Package};

use crate::moddata::ModData;

/// An opened map folder.
pub struct MapFolder {
    /// The folder as the manifest names it, without the `~` of optional folders.
    pub name: String,
    /// The `MapClassification` text of the folder, like `System` or `User`.
    pub classification: String,
    pub package: Arc<dyn Package>,
}

impl MapFolder {
    /// The packages of the maps in this folder with their entry names, in the order the folder lists them.
    pub fn map_packages(&self, fs: &FileSystem) -> Vec<(String, Arc<dyn Package>)> {
        self.package
            .contents()
            .into_iter()
            .filter_map(|entry| Some((entry.clone(), self.package.open_package(&entry, fs)?)))
            .collect()
    }
}

/// The map folders of the manifest of `data` that exist, in manifest order.
pub fn map_folders(data: &ModData) -> Vec<MapFolder> {
    data.manifest
        .map_folders
        .iter()
        .filter_map(|(name, classification)| {
            let name = name.strip_prefix('~').unwrap_or(name).to_string();
            let package = data.file_system.open_package(&name)?;
            Some(MapFolder {
                name,
                classification: classification.clone(),
                package,
            })
        })
        .collect()
}
