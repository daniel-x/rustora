// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `GameRules/ActorInfo.cs`: an actor type and its trait definitions.


use crate::generated::{TypeEntry, type_entry};

pub use rustora_game::actor_yaml::{ActorInfoError, TraitNode, parse_trait_nodes};

/// The type table entry of a trait node.
pub trait TraitNodeType {
    fn type_entry(&self) -> Option<&'static TypeEntry>;
}

impl TraitNodeType for TraitNode {
    fn type_entry(&self) -> Option<&'static TypeEntry> {
        type_entry(&self.info_type())
    }
}

/// Port of `ActorInfo.TraitsInConstructOrder`: indices into `traits` in construct order.
pub fn construct_order(actor: &str, traits: &[TraitNode]) -> Result<Vec<usize>, ActorInfoError> {
    struct Src {
        idx: usize,
        ty: &'static TypeEntry,
    }
    let mut source = Vec::with_capacity(traits.len());
    for (idx, t) in traits.iter().enumerate() {
        let ty = t.type_entry().ok_or_else(|| ActorInfoError {
            actor: actor.to_string(),
            message: format!("Unknown trait type `{}`", t.info_type()),
        })?;
        source.push(Src { idx, ty });
    }

    let resolvable = |d: &str, r: &TypeEntry| r.is_assignable_to(d);
    let mut resolved: Vec<usize> = Vec::new();
    let mut unresolved: Vec<usize> = Vec::new();
    for (i, s) in source.iter().enumerate() {
        if s.ty.requires.is_empty() && s.ty.not_before.is_empty() {
            resolved.push(i);
        } else {
            unresolved.push(i);
        }
    }

    let more = |resolved: &[usize], unresolved: &[usize]| -> Vec<usize> {
        unresolved
            .iter()
            .copied()
            .filter(|&u| {
                let ut = source[u].ty;
                ut.requires.iter().all(|d| {
                    resolved.iter().any(|&r| resolvable(d, source[r].ty)) && !unresolved.iter().any(|&u1| resolvable(d, source[u1].ty))
                }) && ut
                    .not_before
                    .iter()
                    .all(|d| !unresolved.iter().any(|&u1| resolvable(d, source[u1].ty)))
            })
            .collect()
    };

    let mut ready = more(&resolved, &unresolved);
    while !ready.is_empty() {
        resolved.extend(ready.iter().copied());
        unresolved.retain(|u| !ready.contains(u));
        ready = more(&resolved, &unresolved);
    }

    if !unresolved.is_empty() {
        let names: Vec<String> = unresolved.iter().map(|&u| traits[source[u].idx].display_name()).collect();
        return Err(ActorInfoError {
            actor: actor.to_string(),
            message: format!(
                "failed to initialize because of unresolved trait dependencies: {}",
                names.join(", ")
            ),
        });
    }

    Ok(resolved.into_iter().map(|i| source[i].idx).collect())
}

/// Static index of trait infos by type (`TypeDictionary` of an `ActorInfo`).
#[derive(Default)]
pub struct InfoIndex {
    map: std::cell::RefCell<std::collections::HashMap<std::any::TypeId, Vec<Box<dyn std::any::Any>>>>,
}

impl InfoIndex {
    pub fn add<T: ?Sized + 'static>(&self, info: std::rc::Rc<T>) {
        self.map
            .borrow_mut()
            .entry(std::any::TypeId::of::<T>())
            .or_default()
            .push(Box::new(info));
    }

    /// All infos registered under `T`, in definition order.
    pub fn all<T: ?Sized + 'static>(&self) -> Vec<std::rc::Rc<T>> {
        self.map
            .borrow()
            .get(&std::any::TypeId::of::<T>())
            .map(|v| {
                v.iter()
                    .map(|b| b.downcast_ref::<std::rc::Rc<T>>().expect("info type").clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get<T: ?Sized + 'static>(&self) -> Option<std::rc::Rc<T>> {
        let all = self.all::<T>();
        assert!(all.len() <= 1, "multiple trait infos of type {}", std::any::type_name::<T>());
        all.into_iter().next()
    }
}

/// A loaded actor type with typed trait infos.
pub struct ActorInfo {
    pub name: String,
    pub nodes: Vec<TraitNode>,
    pub traits: Vec<std::rc::Rc<dyn crate::traits::TraitInfo>>,
    construct_order: Vec<usize>,
    pub index: InfoIndex,
}

impl ActorInfo {
    pub fn new(name: &str, nodes: Vec<TraitNode>, traits: Vec<std::rc::Rc<dyn crate::traits::TraitInfo>>) -> Result<Self, ActorInfoError> {
        let construct_order = construct_order(name, &nodes)?;
        let index = InfoIndex::default();
        for t in &traits {
            t.clone().register_info(&index);
        }
        Ok(ActorInfo {
            name: name.to_string(),
            nodes,
            traits,
            construct_order,
            index,
        })
    }

    pub fn traits_in_construct_order(&self) -> impl Iterator<Item = &std::rc::Rc<dyn crate::traits::TraitInfo>> {
        self.construct_order.iter().map(|&i| &self.traits[i])
    }

    pub fn trait_info<T: ?Sized + 'static>(&self) -> Option<std::rc::Rc<T>> {
        self.index.get::<T>()
    }

    pub fn trait_infos<T: ?Sized + 'static>(&self) -> Vec<std::rc::Rc<T>> {
        self.index.all::<T>()
    }

    /// Whether a trait node with this exact name exists, also for traits not ported yet.
    pub fn has_trait_info_named(&self, name: &str) -> bool {
        self.nodes.iter().any(|n| n.name == name)
    }

    /// `HasTraitInfo<I>` for a C# info interface or base class `name`, using the generated type table.
    pub fn has_trait_info_assignable_to(&self, name: &str) -> bool {
        self.nodes
            .iter()
            .any(|n| n.type_entry().is_some_and(|e| e.assignable.contains(&name)))
    }

    pub fn has_trait_info<T: ?Sized + 'static>(&self) -> bool {
        !self.index.all::<T>().is_empty()
    }
}
