// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of the loading parts of `ModData.cs`, `InstalledMods.cs` and the mod file system loaders.

use std::collections::HashMap;
use std::sync::Arc;

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_fs::mix::MixLoader;
use rustora_fs::{FileSystem, Folder, Package, PackageLoader, Platform, ReadFs};

use crate::GameError;
use crate::manifest::{Manifest, split_list};

/// Finds the mods below `<engine dir>/mods`, keyed by id (the folder name).
pub fn installed_mods(platform: &Platform) -> HashMap<String, Arc<dyn Package>> {
    let mut mods = HashMap::new();
    let dir = platform.engine_dir.join("mods");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.join("mod.yaml").is_file() {
                let id = e.file_name().to_string_lossy().into_owned();
                mods.insert(id, Arc::new(Folder::new(path)) as Arc<dyn Package>);
            }
        }
    }
    mods
}

/// The loaded manifest and mounted file system of the active mod.
pub struct ModData {
    pub manifest: Manifest,
    pub file_system: FileSystem,
    pub platform: Platform,
    pub installed: HashMap<String, Arc<dyn Package>>,
    /// False when content packages are missing and the content installer would be needed.
    pub content_available: bool,
}

impl ModData {
    pub fn load(platform: Platform, mod_id: &str) -> Result<Self, GameError> {
        let installed = installed_mods(&platform);
        let package = installed
            .get(mod_id)
            .cloned()
            .ok_or_else(|| GameError::Data(format!("Unknown mod `{mod_id}`")))?;
        let manifest = Manifest::load(mod_id, package)?;

        let mut loaders: Vec<Box<dyn PackageLoader>> = Vec::new();
        if manifest.package_formats.iter().any(|f| f == "Mix") {
            loaders.push(Box::new(MixLoader::default()));
        }
        let mut file_system = FileSystem::new(platform.clone(), mod_id, installed.clone(), loaders);
        let content_available = mount_file_system(&mut file_system, &manifest.file_system)?;

        Ok(ModData {
            manifest,
            file_system,
            platform,
            installed,
            content_available,
        })
    }

    /// `ContentInstallerFileSystemLoader.ContentInstallerMod`.
    pub fn content_installer_mod(&self) -> Option<String> {
        self.manifest
            .file_system
            .nodes
            .iter()
            .find(|n| n.key == "ContentInstallerMod")
            .and_then(|n| n.value.value.clone())
    }

    /// Port of `MiniYaml.Load`: parses `files` (plus files listed in the map rule value), appends
    /// inline map rules and merges everything.
    pub fn load_yaml(fs: &dyn ReadFs, files: &[String], map_rules: Option<&MiniYaml>) -> Result<Vec<MiniYamlNode>, GameError> {
        let mut all: Vec<String> = files.to_vec();
        if let Some(v) = map_rules.and_then(|m| m.value.as_deref()) {
            all.extend(split_list(Some(v)));
        }
        let mut sources = Vec::with_capacity(all.len() + 1);
        for f in &all {
            let data = fs.open(f).map_err(|e| GameError::Data(e.to_string()))?;
            sources.push(MiniYaml::from_bytes(&data, f, true)?);
        }
        if let Some(m) = map_rules.filter(|m| !m.nodes.is_empty()) {
            sources.push(m.nodes.clone());
        }
        Ok(MiniYaml::merge(sources)?)
    }
}

/// Mounts packages as `ContentInstallerFileSystem` or `DefaultFileSystem` does.
/// Returns whether all content packages and required files are available.
fn mount_file_system(fs: &mut FileSystem, spec: &MiniYaml) -> Result<bool, GameError> {
    let section = |key: &str| {
        spec.nodes
            .iter()
            .find(|n| n.key == key)
            .map(|n| n.value.nodes.clone())
            .unwrap_or_default()
    };
    let mount_all = |fs: &mut FileSystem, nodes: &[MiniYamlNode], must: bool| -> Result<bool, GameError> {
        let mut ok = true;
        for n in nodes {
            if let Err(e) = fs.mount(&n.key, n.value.value.as_deref()) {
                if must {
                    return Err(GameError::Data(e.to_string()));
                }
                ok = false;
            }
        }
        Ok(ok)
    };

    match spec.value_str() {
        "ContentInstallerFileSystem" => {
            mount_all(fs, &section("SystemPackages"), true)?;
            let mut available = mount_all(fs, &section("ContentPackages"), false)?;
            for n in section("RequiredContentFiles") {
                if !fs.exists(&n.key) {
                    available = false;
                }
            }
            Ok(available)
        }
        "DefaultFileSystem" => {
            mount_all(fs, &section("Packages"), true)?;
            Ok(true)
        }
        other => Err(GameError::Data(format!("Unknown file system loader `{other}`"))),
    }
}
