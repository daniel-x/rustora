// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The OpenRA assets that rustora does not ship. The engine directory holds only the mod files that rustora may
//! redistribute; the artwork, sounds and other files of the OpenRA mod folders that derive from the original
//! Westwood games, or whose license is unclear, are downloaded by the content installer from the OpenRA source
//! archive into `<support dir>/Content/openra-assets/<OpenRA version>`, laid out like the engine directory.
//! A [`crate::Folder`] inside the engine directory looks up its files there first.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The OpenRA release whose source archive provides the assets.
pub const OPENRA_ASSETS_VERSION: &str = "release-20250330";

/// The assets directory below the support directory, as a `^SupportDir|` path suffix.
pub fn assets_subdir() -> String {
    format!("Content/openra-assets/{OPENRA_ASSETS_VERSION}")
}

/// Engine directories (canonical) and the asset directories that overlay them.
static OVERLAYS: RwLock<Vec<(PathBuf, PathBuf)>> = RwLock::new(Vec::new());

/// Lets the folders inside `engine_dir` find the downloaded assets of `support_dir`. The latest registration of an
/// engine directory wins.
pub fn register_overlay(engine_dir: &Path, support_dir: &Path) {
    let Ok(engine) = engine_dir.canonicalize() else {
        return;
    };
    let overlay = support_dir.join(assets_subdir());
    let mut list = OVERLAYS.write().unwrap_or_else(|e| e.into_inner());
    list.retain(|(e, _)| *e != engine);
    list.push((engine, overlay));
}

/// The directory that overlays the folder `path`, if `path` lies inside a registered engine directory.
pub(crate) fn overlay_of(path: &Path) -> Option<PathBuf> {
    let list = OVERLAYS.read().unwrap_or_else(|e| e.into_inner());
    if list.is_empty() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    list.iter()
        .rev()
        .find_map(|(engine, overlay)| canonical.strip_prefix(engine).ok().map(|rel| overlay.join(rel)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Folder, Package};

    #[test]
    fn engine_folders_read_downloaded_assets_first() {
        let base = std::env::temp_dir().join(format!("rustora-assets-test-{}", std::process::id()));
        let (engine, support) = (base.join("engine"), base.join("support"));
        let overlay = support.join(assets_subdir());
        std::fs::create_dir_all(engine.join("mods/x/bits")).unwrap();
        std::fs::create_dir_all(overlay.join("mods/x/bits")).unwrap();
        std::fs::write(engine.join("mods/x/bits/shipped.txt"), "engine").unwrap();
        std::fs::write(engine.join("mods/x/bits/both.txt"), "engine").unwrap();
        std::fs::write(overlay.join("mods/x/bits/both.txt"), "overlay").unwrap();
        std::fs::write(overlay.join("mods/x/bits/asset.txt"), "overlay").unwrap();
        register_overlay(&engine, &support);

        let folder = Folder::new(engine.join("mods/x/bits"));
        assert_eq!(folder.get("shipped.txt").unwrap(), b"engine");
        assert_eq!(folder.get("both.txt").unwrap(), b"overlay");
        assert_eq!(folder.get("asset.txt").unwrap(), b"overlay");
        assert_eq!(folder.contents(), vec!["asset.txt", "both.txt", "shipped.txt"]);
        let elsewhere = Folder::new(&support);
        assert!(!elsewhere.contains("asset.txt"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
