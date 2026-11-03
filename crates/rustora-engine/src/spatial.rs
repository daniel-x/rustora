// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Primitives/SpatiallyPartitioned.cs`.

use std::collections::HashSet;
use std::hash::Hash;

use rustora_core::fieldloader::Rectangle;
use rustora_core::world_types::Int2;

use crate::net_collections::NetMap;

fn div_away(a: i32, b: i32) -> i32 {
    let (q, r) = (a / b, a % b);
    if r != 0 && ((a < 0) == (b < 0)) {
        q + 1
    } else if r != 0 {
        q - 1
    } else {
        q
    }
}

fn intersects(a: &Rectangle, b: &Rectangle) -> bool {
    b.x < a.x + a.width && a.x < b.x + b.width && b.y < a.y + a.height && a.y < b.y + b.height
}

fn contains_rect(a: &Rectangle, b: &Rectangle) -> bool {
    a.x <= b.x && b.x + b.width <= a.x + a.width && a.y <= b.y && b.y + b.height <= a.y + a.height
}

/// `K` identifies the item (C# hashes the object); `V` is the item returned by queries.
pub struct SpatiallyPartitioned<K, V> {
    rows: i32,
    cols: i32,
    bin_size: i32,
    bins: Vec<NetMap<K, (V, Rectangle)>>,
    item_bounds: NetMap<K, Rectangle>,
}

impl<K: Hash + Eq + Clone, V: Clone> SpatiallyPartitioned<K, V> {
    pub fn new(width: i32, height: i32, bin_size: i32) -> Self {
        let rows = div_away(height, bin_size);
        let cols = div_away(width, bin_size);
        SpatiallyPartitioned {
            rows,
            cols,
            bin_size,
            bins: (0..rows * cols).map(|_| NetMap::new()).collect(),
            item_bounds: NetMap::new(),
        }
    }

    fn rows_cols(&self, b: &Rectangle) -> (i32, i32, i32, i32) {
        let (top, bottom) = (b.top().min(b.bottom()), b.top().max(b.bottom()));
        let (left, right) = (b.left().min(b.right()), b.left().max(b.right()));
        (
            (top / self.bin_size).max(0),
            div_away(bottom, self.bin_size).min(self.rows),
            (left / self.bin_size).max(0),
            div_away(right, self.bin_size).min(self.cols),
        )
    }

    pub fn add(&mut self, key: K, item: V, bounds: Rectangle) {
        assert!(bounds.width != 0 && bounds.height != 0, "Bounds are empty.");
        assert!(self.item_bounds.insert(key.clone(), bounds), "duplicate item");
        let (r0, r1, c0, c1) = self.rows_cols(&bounds);
        for r in r0..r1 {
            for c in c0..c1 {
                let i = (r * self.cols + c) as usize;
                assert!(self.bins[i].insert(key.clone(), (item.clone(), bounds)), "duplicate item in bin");
            }
        }
    }

    pub fn update(&mut self, key: K, item: V, bounds: Rectangle) {
        self.remove(&key);
        self.add(key, item, bounds);
    }

    pub fn remove(&mut self, key: &K) -> bool {
        let Some(bounds) = self.item_bounds.get(key).copied() else {
            return false;
        };
        let (r0, r1, c0, c1) = self.rows_cols(&bounds);
        for r in r0..r1 {
            for c in c0..c1 {
                self.bins[(r * self.cols + c) as usize].remove(key);
            }
        }
        self.item_bounds.remove(key);
        true
    }

    pub fn at(&self, l: Int2) -> Vec<V> {
        let col = (l.x / self.bin_size).clamp(0, self.cols - 1);
        let row = (l.y / self.bin_size).clamp(0, self.rows - 1);
        self.bins[(row * self.cols + col) as usize]
            .values()
            .filter(|(_, b)| b.contains(l.x, l.y))
            .map(|(v, _)| v.clone())
            .collect()
    }

    pub fn in_box(&self, b: Rectangle) -> Vec<V> {
        let (r0, r1, c0, c1) = self.rows_cols(&b);
        let mut seen: HashSet<K> = HashSet::new();
        let mut out = Vec::new();
        for r in r0..r1 {
            for c in c0..c1 {
                let bin_bounds = Rectangle::new(c * self.bin_size, r * self.bin_size, self.bin_size, self.bin_size);
                for (k, (v, bounds)) in self.bins[(r * self.cols + c) as usize].iter() {
                    if intersects(bounds, &b) && (contains_rect(&bin_bounds, bounds) || seen.insert(k.clone())) {
                        out.push(v.clone());
                    }
                }
            }
        }
        out
    }
}
