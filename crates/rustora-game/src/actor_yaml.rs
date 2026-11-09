// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The trait nodes of an actor definition before typed loading. Client and server read them.

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};

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
