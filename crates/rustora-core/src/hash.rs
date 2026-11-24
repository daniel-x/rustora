// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Hash collections with the Fx hasher instead of SipHash. Their iteration order is fixed by the hasher, but no
//! game result may depend on it, just as with the randomly seeded std hasher.

pub use rustc_hash::FxBuildHasher;

pub type HashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
pub type HashSet<K> = std::collections::HashSet<K, FxBuildHasher>;
pub type IndexMap<K, V> = indexmap::IndexMap<K, V, FxBuildHasher>;
pub type IndexSet<K> = indexmap::IndexSet<K, FxBuildHasher>;
