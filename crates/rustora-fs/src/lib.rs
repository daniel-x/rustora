// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Virtual file system: packages (folders, zip, mix) mounted with priorities and explicit names.
//!
//! Port of `OpenRA.Game/FileSystem` and the mix loader of `OpenRA.Mods.Cnc`.

mod filesystem;
mod folder;
pub mod mix;
mod platform;
mod zip_package;

pub use filesystem::{FileSystem, PackageLoader};
pub use folder::Folder;
pub use platform::Platform;
pub use zip_package::ZipPackage;

use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("io error on {path}: {source}")]
    Io { path: String, source: std::io::Error },
}

/// A read-only collection of files, like `IReadOnlyPackage` in C#.
pub trait Package: Send + Sync {
    fn name(&self) -> &str;

    /// File (and for folders also directory) names in the package, in the C# enumeration order.
    fn contents(&self) -> Vec<String>;

    fn get(&self, filename: &str) -> Option<Vec<u8>>;

    fn contains(&self, filename: &str) -> bool;

    /// Opens a nested package (subfolder or archive file) inside this package.
    fn open_package(&self, filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>>;

    /// Filesystem path for packages that live on disk, used to detect identical folders.
    fn disk_path(&self) -> Option<&std::path::Path> {
        None
    }
}
