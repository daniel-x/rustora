// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `GameRules/Ruleset.cs` (actors part) and the trait info registry.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::miniyaml::MiniYamlNode;

use crate::actor_info::{ActorInfo, TraitNode, parse_trait_nodes};
use crate::stub::StubInfo;
use crate::traits::TraitInfo;

pub type InfoLoader = fn(&TraitNode) -> Result<Rc<dyn TraitInfo>, String>;

/// Maps trait names (without `Info`) to loaders. Mod crates register their traits here.
#[derive(Default)]
pub struct TraitRegistry {
    loaders: HashMap<&'static str, InfoLoader>,
}

impl TraitRegistry {
    pub fn register(&mut self, name: &'static str, loader: InfoLoader) {
        self.loaders.insert(name, loader);
    }

    pub fn is_ported(&self, name: &str) -> bool {
        self.loaders.contains_key(name)
    }

    /// Loads a trait info; unported traits get a [`StubInfo`] that reproduces their sync shape.
    pub fn load(&self, node: &TraitNode) -> Result<Rc<dyn TraitInfo>, String> {
        match self.loaders.get(node.name.as_str()) {
            Some(l) => l(node),
            None => StubInfo::load(node),
        }
    }
}

/// Loads a typed info with `FieldLoad`, sets the instance name and wraps it.
#[macro_export]
macro_rules! load_info {
    ($ty:ty, $node:expr) => {{
        let mut info = <$ty>::default();
        rustora_core::fieldloader::FieldLoad::load(&mut info, &$node.yaml)
            .map_err(|e| format!("Trait name {}: {}", $node.display_name(), e))?;
        info.instance_name = $node.instance_name.clone();
        Ok(::std::rc::Rc::new(info) as ::std::rc::Rc<dyn $crate::traits::TraitInfo>)
    }};
}

pub struct Ruleset {
    actors: HashMap<String, Rc<ActorInfo>>,
}

impl Ruleset {
    /// Builds actor infos from merged rule nodes; abstract (`^`) definitions are skipped.
    pub fn load(registry: &TraitRegistry, rules: &[MiniYamlNode]) -> Result<Ruleset, String> {
        let mut actors = HashMap::new();
        for n in rules {
            if n.key.starts_with('^') {
                continue;
            }
            let name = n.key.to_lowercase();
            let nodes = parse_trait_nodes(&name, &n.value.nodes).map_err(|e| e.to_string())?;
            let mut traits = Vec::with_capacity(nodes.len());
            for t in &nodes {
                traits.push(registry.load(t).map_err(|e| format!("Actor type {name}: {e}"))?);
            }
            let info = ActorInfo::new(&name, nodes, traits).map_err(|e| e.to_string())?;
            actors.insert(name, Rc::new(info));
        }
        Ok(Ruleset { actors })
    }

    pub fn actor(&self, name: &str) -> Option<Rc<ActorInfo>> {
        self.actors.get(name).cloned()
    }

    pub fn actors(&self) -> impl Iterator<Item = (&String, &Rc<ActorInfo>)> {
        self.actors.iter()
    }
}
