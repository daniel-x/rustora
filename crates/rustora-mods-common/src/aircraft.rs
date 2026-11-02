// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Placeholder until `Traits/Air/Aircraft.cs` is ported.

use rustora_engine::activity::ActivityRef;
use rustora_engine::actor::ActorRef;
use rustora_engine::target::Target;

use rustora_core::world_types::WAngle;
use rustora_derive::FieldLoad;

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct AircraftInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub initial_facing: WAngle,
}

pub fn is_reserved(_a: &ActorRef) -> bool {
    false
}

pub struct Aircraft;

impl Aircraft {
    pub fn is_trait_disabled(&self) -> bool {
        true
    }
    pub fn is_trait_paused(&self) -> bool {
        false
    }
    pub fn require_force_move(&self) -> bool {
        false
    }
    pub fn un_reserve(&self) {}
}

pub struct Land;

impl Land {
    pub fn new(_self_: &ActorRef) -> ActivityRef {
        unimplemented!("aircraft port pending")
    }
}

pub struct Fly;

impl Fly {
    pub fn new(_self_: &ActorRef, _target: Target) -> ActivityRef {
        unimplemented!("aircraft port pending")
    }
}
