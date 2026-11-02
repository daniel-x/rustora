// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Collections that iterate like .NET 6 `HashSet` and `Dictionary`: entries sit in an array in
//! insertion order, removed slots go on a LIFO free list and are reused first. The iteration
//! order therefore depends only on the add/remove sequence, not on hash codes.

use std::collections::HashMap;
use std::hash::Hash;

/// `Dictionary<K, V>` iteration semantics.
#[derive(Clone, Debug)]
pub struct NetMap<K, V> {
    entries: Vec<Option<(K, V)>>,
    free: Vec<usize>,
    index: HashMap<K, usize>,
}

impl<K, V> Default for NetMap<K, V> {
    fn default() -> Self {
        NetMap {
            entries: Vec::new(),
            free: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<K: Hash + Eq + Clone, V> NetMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.index.contains_key(k)
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.index.get(k).map(|&i| &self.entries[i].as_ref().expect("live entry").1)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        let i = *self.index.get(k)?;
        Some(&mut self.entries[i].as_mut().expect("live entry").1)
    }

    /// Inserts or overwrites in place; returns true when the key is new.
    pub fn insert(&mut self, k: K, v: V) -> bool {
        if let Some(&i) = self.index.get(&k) {
            self.entries[i].as_mut().expect("live entry").1 = v;
            return false;
        }
        let i = match self.free.pop() {
            Some(i) => {
                self.entries[i] = Some((k.clone(), v));
                i
            }
            None => {
                self.entries.push(Some((k.clone(), v)));
                self.entries.len() - 1
            }
        };
        self.index.insert(k, i);
        true
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        let i = self.index.remove(k)?;
        self.free.push(i);
        self.entries[i].take().map(|e| e.1)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.free.clear();
        self.index.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().filter_map(|e| e.as_ref().map(|(k, v)| (k, v)))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|e| e.0)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|e| e.1)
    }
}

/// `HashSet<T>` iteration semantics.
#[derive(Clone, Debug)]
pub struct NetSet<T> {
    map: NetMap<T, ()>,
}

impl<T> Default for NetSet<T> {
    fn default() -> Self {
        NetSet { map: NetMap::default() }
    }
}

impl<T: Hash + Eq + Clone> NetSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn contains(&self, v: &T) -> bool {
        self.map.contains_key(v)
    }

    pub fn insert(&mut self, v: T) -> bool {
        if self.map.contains_key(&v) {
            return false;
        }
        self.map.insert(v, ())
    }

    pub fn remove(&mut self, v: &T) -> bool {
        self.map.remove(v).is_some()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.map.keys()
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }
}
