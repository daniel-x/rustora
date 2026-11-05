// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Map/ActorReference.cs`: map actor definitions turned into actor inits.

use std::rc::Rc;

use rustora_core::fieldloader::get_value_yaml;
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{CPos, WAngle};
use rustora_engine::actor_init::*;
use rustora_engine::value_init;

value_init!(HealthInit, i32);

/// The `allowZero` argument of `HealthInit`: a value of 0 creates a dead actor instead of one with 1%.
pub struct AllowZeroHealthInit;

impl ActorInit for AllowZeroHealthInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
value_init!(TurretFacingInit, WAngle);

/// `SkipMakeAnimsInit`.
pub struct SkipMakeAnimsInit;
impl ActorInit for SkipMakeAnimsInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `SpawnedByMapInit`.
pub struct SpawnedByMapInit;
impl ActorInit for SpawnedByMapInit {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// An init type not ported yet; kept so lookups by name still work.
pub struct UnknownInit {
    pub name: String,
    pub instance_name: Option<String>,
    pub yaml: MiniYaml,
}

impl ActorInit for UnknownInit {
    fn instance_name(&self) -> Option<&str> {
        self.instance_name.as_deref()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct ActorReference {
    pub actor_type: String,
    pub inits: ActorInits,
}

impl ActorReference {
    /// Parses `Type` plus init nodes (`Owner`, `Location`, `Facing`, ...).
    pub fn from_yaml(actor_type: &str, value: &MiniYaml) -> Result<ActorReference, String> {
        let mut inits = ActorInits::new();
        for n in &value.nodes {
            let (name, instance) = match n.key.split_once('@') {
                Some((a, b)) => (a, Some(b.to_string())),
                None => (n.key.as_str(), None),
            };
            let err = |e: rustora_core::fieldloader::LoadError| format!("{actor_type}: init {name}: {e}");
            let init: Rc<dyn ActorInit> = match name {
                "Owner" => Rc::new(OwnerInit::from_name(n.value.value_str())),
                "Location" => Rc::new(LocationInit::new(get_value_yaml::<CPos>(name, &n.value, false).map_err(err)?)),
                "Facing" => Rc::new(FacingInit {
                    value: get_value_yaml::<WAngle>(name, &n.value, false).map_err(err)?,
                    instance_name: instance,
                }),
                "SubCell" => Rc::new(SubCellInit::new(get_value_yaml::<u8>(name, &n.value, false).map_err(err)?)),
                "Faction" => Rc::new(FactionInit::new(n.value.value_str().to_string())),
                "Health" => Rc::new(HealthInit::new(get_value_yaml::<i32>(name, &n.value, false).map_err(err)?)),
                "TurretFacing" => Rc::new(TurretFacingInit {
                    value: get_value_yaml::<WAngle>(name, &n.value, false).map_err(err)?,
                    instance_name: instance,
                }),
                _ => Rc::new(UnknownInit {
                    name: name.to_string(),
                    instance_name: instance,
                    yaml: n.value.clone(),
                }),
            };
            inits.push(init);
        }
        Ok(ActorReference {
            actor_type: actor_type.to_string(),
            inits,
        })
    }

    pub fn owner_name(&self) -> Option<String> {
        self.inits.get::<OwnerInit>().map(|o| o.internal_name.clone())
    }
}
