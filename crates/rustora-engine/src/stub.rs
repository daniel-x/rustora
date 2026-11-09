// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Placeholder for traits that are not ported yet. It reproduces the sync shape of the C#
//! trait (whether it is `ISync`, and the condition driven `IsTraitDisabled`/`IsTraitPaused`),
//! so actors hash like in C# as long as the trait has no further synced state.

use crate::actor_info::TraitNodeType;
use std::any::Any;
use std::rc::Rc;

use rustora_core::expression::BooleanExpression;
use rustora_core::fieldloader::get_value_yaml;

use crate::actor::ActorRef;
use crate::actor_info::{InfoIndex, TraitNode};
use crate::actor_init::ActorInitializer;
use crate::conditional::*;
use crate::generated::{TypeEntry, type_entry};
use crate::trait_dict::TraitDict;
use crate::traits::*;

pub struct StubInfo {
    pub name: String,
    pub instance_name: Option<String>,
    pub entry: &'static TypeEntry,
    pub conditional: bool,
    pub pausable: bool,
    requires_condition: Option<BooleanExpression>,
    pause_on_condition: Option<BooleanExpression>,
}

impl StubInfo {
    pub fn load(node: &TraitNode) -> Result<Rc<dyn TraitInfo>, String> {
        let entry = node.type_entry().ok_or_else(|| format!("Unknown trait `{}`", node.name))?;
        let conditional = entry.is_assignable_to("ConditionalTraitInfo");
        let pausable = entry.is_assignable_to("PausableConditionalTraitInfo");
        let field = |k: &str| -> Result<Option<BooleanExpression>, String> {
            match node.yaml.nodes.iter().find(|n| n.key == k) {
                Some(n) => get_value_yaml::<Option<BooleanExpression>>(k, &n.value, false).map_err(|e| e.to_string()),
                None => Ok(None),
            }
        };
        Ok(Rc::new(StubInfo {
            name: node.name.clone(),
            instance_name: node.instance_name.clone(),
            entry,
            conditional,
            pausable,
            requires_condition: if conditional { field("RequiresCondition")? } else { None },
            pause_on_condition: if pausable { field("PauseOnCondition")? } else { None },
        }))
    }

    fn creates_trait(&self) -> bool {
        self.entry.trait_name.is_some()
    }
}

impl TraitInfo for StubInfo {
    fn create(self: Rc<Self>, _init: &ActorInitializer) -> Option<Rc<dyn TraitObject>> {
        if !self.creates_trait() {
            return None;
        }
        let state = if self.pausable {
            ConditionalState::new_pausable(&PausableConditionalTraitInfo {
                base: ConditionalTraitInfo {
                    requires_condition: self.requires_condition.clone(),
                },
                pause_on_condition: self.pause_on_condition.clone(),
            })
        } else {
            ConditionalState::new(&ConditionalTraitInfo {
                requires_condition: self.requires_condition.clone(),
            })
        };
        Some(Rc::new(StubTrait { info: self, state }))
    }

    fn register_info(self: Rc<Self>, index: &InfoIndex) {
        index.add::<StubInfo>(self);
    }

    fn instance_name(&self) -> Option<&str> {
        self.instance_name.as_deref()
    }

    fn info_type_name(&self) -> &'static str {
        self.entry.name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_rc(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

#[derive(crate::persist::Persist)]
pub struct StubTrait {
    pub info: Rc<StubInfo>,
    state: ConditionalState,
}

impl ConditionalHooks for StubTrait {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for StubTrait {
    fn sync_hash(&self) -> i32 {
        let mut h = 0;
        for m in self.info.entry.sync {
            let mut parts = m.split(':');
            let (name, ty) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            h ^= match (name, ty) {
                ("IsTraitDisabled", _) => sync_bool(self.state.is_trait_disabled()),
                ("IsTraitPaused", _) => sync_bool(self.state.is_trait_paused()),
                (_, "Boolean") => sync_bool(false),
                _ => 0,
            };
        }
        h
    }
}

impl IObservesVariables for StubTrait {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for StubTrait {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl TraitObject for StubTrait {
    fn register(self: Rc<Self>, dict: &TraitDict, actor: &ActorRef) {
        dict.add::<StubTrait>(actor, self.clone());
        if self.info.entry.trait_implements("ISync") {
            dict.add::<dyn ISync>(actor, self.clone());
        }
        if self.info.conditional {
            dict.add::<dyn IObservesVariables>(actor, self.clone());
            dict.add::<dyn IDisabledTrait>(actor, self.clone());
        }
    }

    fn type_name(&self) -> &'static str {
        self.info.entry.trait_name.unwrap_or("Stub")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn cast_rc(self: Rc<Self>, t: std::any::TypeId) -> Option<Box<dyn Any>> {
        if t == std::any::TypeId::of::<StubTrait>() {
            return Some(Box::new(self));
        }
        if t == std::any::TypeId::of::<dyn ISync>() {
            let rc: Rc<dyn ISync> = self;
            return Some(Box::new(rc));
        }
        None
    }

    fn save_state(&self, w: &mut crate::persist::SaveWriter) {
        crate::persist::PersistState::save_state(self, w)
    }

    fn load_state(&self, r: &mut crate::persist::SaveReader) {
        crate::persist::PersistState::load_state(self, r)
    }
}

/// Whether a trait type is known to the type table.
pub fn is_known_trait(name: &str) -> bool {
    type_entry(&format!("{name}Info")).is_some()
}
