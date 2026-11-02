// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

use std::path::{Path, PathBuf};

/// Engine, support and binary directories used to resolve `^EngineDir`, `^SupportDir` and `^BinDir`.
#[derive(Clone, Debug)]
pub struct Platform {
    pub engine_dir: PathBuf,
    pub support_dir: PathBuf,
    pub bin_dir: PathBuf,
}

impl Platform {
    pub fn new(engine_dir: impl Into<PathBuf>, support_dir: impl Into<PathBuf>) -> Self {
        let engine_dir = engine_dir.into();
        Platform {
            bin_dir: engine_dir.clone(),
            engine_dir,
            support_dir: support_dir.into(),
        }
    }

    /// The default support directory of OpenRA on this OS, so content installed by OpenRA is found.
    pub fn default_support_dir() -> PathBuf {
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("OpenRA")
        } else if cfg!(target_os = "macos") {
            home_dir().join("Library/Application Support/OpenRA")
        } else {
            let xdg = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join(".config"));
            let legacy = home_dir().join(".openra");
            if legacy.is_dir() { legacy } else { xdg.join("openra") }
        }
    }

    /// Port of `Platform.ResolvePath`: expands the directory tokens, keeps everything else.
    pub fn resolve_path(&self, path: &str) -> String {
        let path = path.trim_end_matches([' ', '\t']);
        let dir = |p: &Path| p.to_string_lossy().trim_end_matches(['/', '\\']).to_string() + std::path::MAIN_SEPARATOR_STR;
        match path {
            "^SupportDir" => return dir(&self.support_dir),
            "^EngineDir" => return dir(&self.engine_dir),
            "^BinDir" => return dir(&self.bin_dir),
            _ => {}
        }
        for (token, base) in [
            ("^SupportDir|", &self.support_dir),
            ("^EngineDir|", &self.engine_dir),
            ("^BinDir|", &self.bin_dir),
        ] {
            if let Some(rest) = path.strip_prefix(token) {
                return dir(base) + rest;
            }
        }
        path.to_string()
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}
