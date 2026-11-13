// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{FileSystem, Package, ZipPackage};

/// A directory on disk. Inside an engine directory, the downloaded assets (see [`crate::assets`]) overlay it.
pub struct Folder {
    name: String,
    path: PathBuf,
    overlay: Option<PathBuf>,
}

impl Folder {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Folder {
            name: path.to_string_lossy().into_owned(),
            overlay: crate::assets::overlay_of(&path),
            path,
        }
    }

    /// The file `filename` of the overlay if it exists there, else of the folder itself.
    fn join(&self, filename: &str) -> PathBuf {
        if let Some(o) = &self.overlay {
            let p = o.join(filename);
            if p.exists() {
                return p;
            }
        }
        self.path.join(filename)
    }
}

/// Orders strings like .NET's culture invariant comparison does for typical file names:
/// case-insensitive first, then lowercase before uppercase.
pub(crate) fn invariant_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect::<String>();
    key(a).cmp(&key(b)).then_with(|| {
        for (ca, cb) in a.chars().zip(b.chars()) {
            if ca != cb {
                return if ca.is_lowercase() {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
        }
        a.len().cmp(&b.len())
    })
}

impl Package for Folder {
    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> Vec<String> {
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for dir in std::iter::once(&self.path).chain(self.overlay.as_ref()) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let list = if e.path().is_dir() { &mut dirs } else { &mut files };
                    if !list.contains(&n) {
                        list.push(n);
                    }
                }
            }
        }
        files.extend(dirs);
        files.sort_by(|a, b| invariant_cmp(a, b));
        files
    }

    fn get(&self, filename: &str) -> Option<Vec<u8>> {
        let p = self.join(filename);
        if !p.is_file() {
            return None;
        }
        std::fs::read(p).ok()
    }

    fn contains(&self, filename: &str) -> bool {
        self.join(filename).is_file()
    }

    fn open_package(&self, filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        let resolved = fs.platform().resolve_path(&self.join(filename).to_string_lossy());
        let resolved_path = Path::new(&resolved);
        if resolved_path.is_dir() {
            return Some(Arc::new(Folder::new(resolved_path)));
        }
        if let Some(z) = ZipPackage::open_file(resolved_path) {
            return Some(Arc::new(z));
        }
        let data = self.get(filename)?;
        fs.try_parse_package(data, filename)
    }

    fn disk_path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}
