// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Primitives/BitSet.cs`: sets of strings with bits allocated per category in first-seen order.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;

use rustora_core::fieldloader::{ArrayAware, FieldValue, LoadError};
use rustora_core::miniyaml::MiniYaml;

thread_local! {
    static ALLOCATORS: RefCell<HashMap<TypeId, Vec<String>>> = RefCell::new(HashMap::new());
}

fn bit_of<T: 'static>(s: &str) -> u128 {
    ALLOCATORS.with(|a| {
        let mut a = a.borrow_mut();
        let names = a.entry(TypeId::of::<T>()).or_default();
        let idx = match names.iter().position(|n| n == s) {
            Some(i) => i,
            None => {
                names.push(s.to_string());
                names.len() - 1
            }
        };
        assert!(idx < 128, "too many BitSet values");
        1u128 << idx
    })
}

/// A set of names of category `T`.
pub struct BitSet<T> {
    bits: u128,
    _t: PhantomData<T>,
}

impl<T> Clone for BitSet<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for BitSet<T> {}
impl<T> Default for BitSet<T> {
    fn default() -> Self {
        BitSet { bits: 0, _t: PhantomData }
    }
}
impl<T> PartialEq for BitSet<T> {
    fn eq(&self, o: &Self) -> bool {
        self.bits == o.bits
    }
}
impl<T> Eq for BitSet<T> {}
impl<T> std::fmt::Debug for BitSet<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BitSet({:#x})", self.bits)
    }
}

impl<T: 'static> BitSet<T> {
    pub fn new<S: AsRef<str>>(values: &[S]) -> Self {
        BitSet {
            bits: values.iter().fold(0, |b, v| b | bit_of::<T>(v.as_ref())),
            _t: PhantomData,
        }
    }

    pub fn contains(&self, v: &str) -> bool {
        self.bits & bit_of::<T>(v) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    pub fn overlaps(&self, o: &Self) -> bool {
        self.bits & o.bits != 0
    }

    pub fn is_subset_of(&self, o: &Self) -> bool {
        self.bits & o.bits == self.bits
    }

    pub fn union(&self, o: &Self) -> Self {
        BitSet {
            bits: self.bits | o.bits,
            _t: PhantomData,
        }
    }

    pub fn except(&self, o: &Self) -> Self {
        BitSet {
            bits: self.bits & !o.bits,
            _t: PhantomData,
        }
    }

    /// Names in bit order.
    pub fn names(&self) -> Vec<String> {
        ALLOCATORS.with(|a| {
            let a = a.borrow();
            let names = a.get(&TypeId::of::<T>());
            (0..128)
                .filter(|i| self.bits & (1u128 << i) != 0)
                .filter_map(|i| names.and_then(|n| n.get(i)).cloned())
                .collect()
        })
    }
}

impl<T: 'static> FieldValue for BitSet<T> {
    const TYPE_NAME: &'static str = "BitSet";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        match value {
            Some(v) => Ok(BitSet::new(
                &v.split(',').filter(|s| !s.is_empty()).map(str::trim).collect::<Vec<_>>(),
            )),
            None => rustora_core::fieldloader::invalid(value, "BitSet", field),
        }
    }
}
impl<T: 'static> ArrayAware for BitSet<T> {}
