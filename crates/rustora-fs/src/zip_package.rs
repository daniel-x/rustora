// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{FileSystem, Package};

const ZIP_SIGNATURE: u32 = 0x04034b50;

/// A zip archive held in memory (`.oramap` files and zipped content).
pub struct ZipPackage {
    name: String,
    archive: Mutex<zip::ZipArchive<Cursor<Arc<[u8]>>>>,
    entries: Vec<(String, bool)>,
}

impl ZipPackage {
    pub fn from_bytes(data: Vec<u8>, name: &str) -> Option<Self> {
        if data.len() < 4 || u32::from_le_bytes([data[0], data[1], data[2], data[3]]) != ZIP_SIGNATURE {
            return None;
        }
        let data: Arc<[u8]> = data.into();
        let archive = zip::ZipArchive::new(Cursor::new(data)).ok()?;
        let entries = (0..archive.len())
            .filter_map(|i| {
                archive
                    .name_for_index(i)
                    .and_then(|r| r.ok())
                    .map(|n| (n.to_string(), n.ends_with('/')))
            })
            .collect();
        Some(ZipPackage {
            name: name.to_string(),
            archive: Mutex::new(archive),
            entries,
        })
    }

    pub fn open_file(path: &Path) -> Option<Self> {
        if !path.is_file() {
            return None;
        }
        let data = std::fs::read(path).ok()?;
        Self::from_bytes(data, &path.to_string_lossy())
    }

    /// SharpZipLib looks up entries case-insensitively.
    fn find(&self, filename: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|(n, _)| n == filename)
            .or_else(|| self.entries.iter().position(|(n, _)| n.eq_ignore_ascii_case(filename)))
    }
}

impl Package for ZipPackage {
    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> Vec<String> {
        self.entries.iter().filter(|(_, dir)| !dir).map(|(n, _)| n.clone()).collect()
    }

    fn get(&self, filename: &str) -> Option<Vec<u8>> {
        let idx = self.find(filename)?;
        if self.entries[idx].1 {
            return None;
        }
        let mut archive = self.archive.lock().ok()?;
        let mut f = archive.by_index(idx).ok()?;
        let mut out = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut out).ok()?;
        Some(out)
    }

    fn contains(&self, filename: &str) -> bool {
        self.find(filename).is_some()
    }

    fn open_package(&self, filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        let dir = format!("{filename}/");
        {
            let idx = self.find(filename).or_else(|| self.find(&dir))?;
            if self.entries[idx].1 {
                return Some(Arc::new(ZipFolder {
                    parent: self.snapshot(),
                    name: filename.trim_end_matches('/').to_string(),
                }));
            }
        }
        let data = self.get(filename)?;
        fs.try_parse_package(data, filename)
    }
}

impl ZipPackage {
    fn snapshot(&self) -> Arc<ZipPackage> {
        let archive = self.archive.lock().unwrap().clone();
        Arc::new(ZipPackage {
            name: self.name.clone(),
            archive: Mutex::new(archive),
            entries: self.entries.clone(),
        })
    }
}

/// A directory inside a zip archive.
struct ZipFolder {
    parent: Arc<ZipPackage>,
    name: String,
}

impl Package for ZipFolder {
    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> Vec<String> {
        self.parent
            .contents()
            .into_iter()
            .filter(|e| e.starts_with(&self.name) && *e != self.name)
            .filter_map(|e| {
                let f = e[self.name.len() + 1..].to_string();
                (f.split('/').filter(|c| !c.is_empty()).count() == 1).then_some(f)
            })
            .collect()
    }

    fn get(&self, filename: &str) -> Option<Vec<u8>> {
        self.parent.get(&format!("{}/{}", self.name, filename))
    }

    fn contains(&self, filename: &str) -> bool {
        self.parent.contains(&format!("{}/{}", self.name, filename))
    }

    fn open_package(&self, filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        self.parent.open_package(&format!("{}/{}", self.name, filename), fs)
    }
}
