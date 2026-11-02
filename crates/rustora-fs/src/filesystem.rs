// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::{Folder, FsError, Package, Platform, ZipPackage};

/// Parses package files of one format, like `IPackageLoader` in C#.
pub trait PackageLoader: Send + Sync {
    fn try_parse(&self, data: &[u8], filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>>;
}

struct ZipLoader;

impl PackageLoader for ZipLoader {
    fn try_parse(&self, data: &[u8], filename: &str, _fs: &FileSystem) -> Option<Arc<dyn Package>> {
        ZipPackage::from_bytes(data.to_vec(), filename).map(|z| Arc::new(z) as Arc<dyn Package>)
    }
}

/// Mounted packages with lookup priority (last mounted wins) and explicit mount names.
pub struct FileSystem {
    platform: Platform,
    mod_id: String,
    installed_mods: HashMap<String, Arc<dyn Package>>,
    loaders: Vec<Box<dyn PackageLoader>>,
    mounted: Vec<(Arc<dyn Package>, u32)>,
    explicit_mounts: Vec<(String, Arc<dyn Package>)>,
    file_index: HashMap<String, Vec<Arc<dyn Package>>>,
}

fn same(a: &Arc<dyn Package>, b: &Arc<dyn Package>) -> bool {
    Arc::ptr_eq(a, b)
}

impl FileSystem {
    pub fn new(
        platform: Platform,
        mod_id: &str,
        installed_mods: HashMap<String, Arc<dyn Package>>,
        mut loaders: Vec<Box<dyn PackageLoader>>,
    ) -> Self {
        loaders.push(Box::new(ZipLoader));
        FileSystem {
            platform,
            mod_id: mod_id.to_string(),
            installed_mods,
            loaders,
            mounted: Vec::new(),
            explicit_mounts: Vec::new(),
            file_index: HashMap::new(),
        }
    }

    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    pub fn mounted_packages(&self) -> impl Iterator<Item = &Arc<dyn Package>> {
        self.mounted.iter().map(|(p, _)| p)
    }

    pub fn try_parse_package(&self, data: Vec<u8>, filename: &str) -> Option<Arc<dyn Package>> {
        self.loaders.iter().find_map(|l| l.try_parse(&data, filename, self))
    }

    pub fn open_package(&self, filename: &str) -> Option<Arc<dyn Package>> {
        let resolved = self.platform.resolve_path(filename);
        if !resolved.contains('|') && Path::new(&resolved).is_dir() {
            return Some(Arc::new(Folder::new(resolved)));
        }
        if let Some((parent, sub)) = self.package_containing(filename) {
            return parent.open_package(&sub, self);
        }
        let data = self.try_open(filename)?;
        self.try_parse_package(data, filename)
    }

    /// Mounts a package given by name: `~` marks it optional, `$id` refers to an installed mod.
    pub fn mount(&mut self, name: &str, explicit_name: Option<&str>) -> Result<(), FsError> {
        let (optional, name) = match name.strip_prefix('~') {
            Some(n) => (true, n),
            None => (false, name),
        };
        let result = (|| {
            let package = if let Some(id) = name.strip_prefix('$') {
                self.installed_mods
                    .get(id)
                    .cloned()
                    .ok_or_else(|| FsError::Invalid(format!("Could not load mod '{id}'.")))?
            } else {
                self.open_package(name).ok_or_else(|| {
                    FsError::Invalid(format!(
                        "Could not open package '{name}', file not found or its format is not supported."
                    ))
                })?
            };
            self.mount_package(package, explicit_name);
            Ok(())
        })();
        if optional { Ok(()) } else { result }
    }

    pub fn mount_package(&mut self, package: Arc<dyn Package>, explicit_name: Option<&str>) {
        if let Some(entry) = self.mounted.iter_mut().find(|(p, _)| same(p, &package)) {
            entry.1 += 1;
            for f in package.contents() {
                let list = self.file_index.entry(f).or_default();
                list.retain(|p| !same(p, &package));
                list.push(package.clone());
            }
            return;
        }
        self.mounted.push((package.clone(), 1));
        if let Some(n) = explicit_name {
            if self.explicit_mounts.iter().any(|(k, _)| k == n) {
                log::warn!("explicit mount name {n} already in use");
            } else {
                self.explicit_mounts.push((n.to_string(), package.clone()));
            }
        }
        for f in package.contents() {
            self.file_index.entry(f).or_default().push(package.clone());
        }
    }

    pub fn unmount(&mut self, package: &Arc<dyn Package>) -> bool {
        let Some(idx) = self.mounted.iter().position(|(p, _)| same(p, package)) else {
            return false;
        };
        self.mounted[idx].1 -= 1;
        if self.mounted[idx].1 == 0 {
            for list in self.file_index.values_mut() {
                list.retain(|p| !same(p, package));
            }
            self.mounted.remove(idx);
            self.explicit_mounts.retain(|(_, p)| !same(p, package));
        }
        true
    }

    fn explicit(&self, name: &str) -> Option<&Arc<dyn Package>> {
        self.explicit_mounts.iter().find(|(k, _)| k == name).map(|(_, p)| p)
    }

    /// Port of `TryGetPackageContaining`: the package and the file name inside it.
    pub fn package_containing(&self, path: &str) -> Option<(Arc<dyn Package>, String)> {
        if let Some(split) = path.find('|').filter(|&s| s > 0) {
            if let Some(p) = self.explicit(&path[..split]) {
                return Some((p.clone(), path[split + 1..].to_string()));
            }
        }
        let p = self.file_index.get(path)?.iter().rev().find(|p| p.contains(path))?;
        Some((p.clone(), path.to_string()))
    }

    pub fn try_open(&self, filename: &str) -> Option<Vec<u8>> {
        let split = filename.find('|').filter(|&s| s > 0);
        if let Some(split) = split {
            if let Some(p) = self.explicit(&filename[..split]) {
                if let Some(d) = p.get(&filename[split + 1..]) {
                    return Some(d);
                }
            }
        }
        if let Some(p) = self
            .file_index
            .get(filename)
            .and_then(|l| l.iter().rev().find(|p| p.contains(filename)))
        {
            return p.get(filename);
        }
        if split.is_some() {
            return None;
        }
        let p = self.mounted.iter().rev().map(|(p, _)| p).find(|p| p.contains(filename))?;
        p.get(filename)
    }

    pub fn open(&self, filename: &str) -> Result<Vec<u8>, FsError> {
        self.try_open(filename).ok_or_else(|| FsError::NotFound(filename.to_string()))
    }

    pub fn exists(&self, filename: &str) -> bool {
        if let Some(split) = filename.find('|').filter(|&s| s > 0) {
            if self
                .explicit(&filename[..split])
                .is_some_and(|p| p.contains(&filename[split + 1..]))
            {
                return true;
            }
        }
        self.file_index.contains_key(filename)
    }

    pub fn is_external_file(&self, filename: &str) -> bool {
        !filename.starts_with(&format!("{}|", self.mod_id))
    }

    pub fn prefix_of(&self, package: &Arc<dyn Package>) -> Option<&str> {
        self.explicit_mounts.iter().find(|(_, p)| same(p, package)).map(|(k, _)| k.as_str())
    }
}
