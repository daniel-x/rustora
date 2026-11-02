// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Placeholder until `GainsExperience` and `ExternalCondition` are ported.

use rustora_engine::actor::ActorRef;

pub fn can_gain_level(_a: &ActorRef) -> bool {
    unimplemented!("GainsExperience port pending")
}

pub fn give_levels(_a: &ActorRef, _levels: i32) {
    unimplemented!("GainsExperience port pending")
}

pub fn accepts_external_condition(_a: &ActorRef, _condition: &str, _source: &ActorRef) -> bool {
    unimplemented!("ExternalCondition port pending")
}

pub fn grant_external_condition(_a: &ActorRef, _condition: &str, _levels: i32, _duration: i32, _source: &ActorRef) {
    unimplemented!("ExternalCondition port pending")
}
