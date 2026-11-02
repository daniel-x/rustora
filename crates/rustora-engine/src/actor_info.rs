// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `GameRules/ActorInfo.cs`: an actor type and its trait definitions.

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};

use crate::generated::{TypeEntry, type_entry};

/// One trait node of an actor definition before typed loading.
#[derive(Clone, Debug)]
pub struct TraitNode {
    /// Trait class name without `Info`, e.g. `Health`.
    pub name: String,
    /// The part after `@`, if any.
    pub instance_name: Option<String>,
    pub yaml: MiniYaml,
}

impl TraitNode {
    pub fn info_type(&self) -> String {
        format!("{}Info", self.name)
    }

    pub fn type_entry(&self) -> Option<&'static TypeEntry> {
        type_entry(&self.info_type())
    }

    pub fn display_name(&self) -> String {
        match &self.instance_name {
            Some(i) => format!("{}@{}", self.name, i),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Actor type {actor}: {message}")]
pub struct ActorInfoError {
    pub actor: String,
    pub message: String,
}

/// Parses the trait nodes of an actor definition.
pub fn parse_trait_nodes(actor: &str, nodes: &[MiniYamlNode]) -> Result<Vec<TraitNode>, ActorInfoError> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        if n.value.value.as_deref().is_some_and(|v| !v.is_empty()) {
            return Err(ActorInfoError {
                actor: actor.to_string(),
                message: format!("Junk value `{}` on trait node {}", n.value.value_str(), n.key),
            });
        }
        let mut parts = n.key.splitn(2, '@');
        let name = parts.next().unwrap_or("").to_string();
        let instance_name = parts.next().map(str::to_string);
        out.push(TraitNode {
            name,
            instance_name,
            yaml: n.value.clone(),
        });
    }
    Ok(out)
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
