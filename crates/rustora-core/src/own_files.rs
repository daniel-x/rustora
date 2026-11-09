// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The directory of the files that rustora writes for itself: logs, exception reports, replays and saves. It lies in
//! the support directory, which rustora shares with OpenRA for settings, content and maps, so the files of the two
//! programs do not mix.

use std::path::{Path, PathBuf};

/// `<support_dir>/rustora`.
pub fn dir(support_dir: &Path) -> PathBuf {
    support_dir.join("rustora")
}

/// `<support_dir>/rustora/Logs`: the log channels and exception reports.
pub fn logs_dir(support_dir: &Path) -> PathBuf {
    dir(support_dir).join("Logs")
}

/// `<support_dir>/rustora/Replays/<mod>/<version>`: the replays of a mod version.
pub fn replays_dir(support_dir: &Path, mod_id: &str, version: &str) -> PathBuf {
    dir(support_dir).join("Replays").join(mod_id).join(version)
}

/// `<support_dir>/rustora/Saves/<mod>/<version>`: the game saves of a mod version and their state snapshots.
pub fn saves_dir(support_dir: &Path, mod_id: &str, version: &str) -> PathBuf {
    dir(support_dir).join("Saves").join(mod_id).join(version)
}
