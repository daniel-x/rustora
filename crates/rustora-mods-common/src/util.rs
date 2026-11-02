// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Util.cs` and small helpers of `ActorExts.cs`.

use rustora_core::world_types::{CPos, CVec, MersenneTwister, WAngle, WDist, WPos, WRot, WVec};
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRelationship;
use rustora_engine::target::Target;
use rustora_engine::world::World;

pub fn tick_facing(facing: WAngle, desired: WAngle, step: WAngle) -> WAngle {
    let left_turn = (facing - desired).angle;
    if left_turn < step.angle {
        return desired;
    }
    let right_turn = (desired - facing).angle;
    if right_turn < step.angle {
        return desired;
    }
    if right_turn < left_turn { facing + step } else { facing - step }
}

pub fn get_turn_direction(facing: WAngle, desired: WAngle) -> i32 {
    if (facing - desired).angle < 512 { -1 } else { 1 }
}

pub fn index_facing(facing: WAngle, num_frames: i32) -> i32 {
    let step = 1024 / num_frames;
    ((facing.angle + step / 2) & 1023) / step
}

pub fn quantize_facing(facing: WAngle, facings: i32) -> WAngle {
    WAngle::new(index_facing(facing, facings) * (1024 / facings))
}

pub fn facing_within_tolerance(facing: WAngle, desired: WAngle, tolerance: WAngle) -> bool {
    if tolerance.angle == 0 && facing == desired {
        return true;
    }
    let delta = (desired - facing).angle;
    delta <= tolerance.angle || delta >= 1024 - tolerance.angle
}

pub fn between_cells(w: &World, from: CPos, to: CPos) -> WPos {
    WPos::lerp(w.map().center_of_cell(from), w.map().center_of_cell(to), 1, 2)
}

pub fn get_vertical_angle(source: WPos, target: WPos) -> WAngle {
    let delta = target - source;
    WVec::new(-delta.z, -delta.horizontal_length(), 0).yaw()
}

/// Fisher-Yates shuffle consuming the random numbers like the C# iterator when fully enumerated.
pub fn shuffle<T: Clone>(ts: &[T], random: &mut MersenneTwister) -> Vec<T> {
    let mut items = ts.to_vec();
    if items.is_empty() {
        return items;
    }
    for i in 0..items.len() - 1 {
        let j = random.next_max((items.len() - i) as i32) as usize;
        items.swap(i, i + j);
    }
    items
}

fn neighbours(c: CPos, allow_diagonal: bool) -> Vec<CPos> {
    let mut v = vec![
        c,
        CPos::new(c.x() - 1, c.y()),
        CPos::new(c.x() + 1, c.y()),
        CPos::new(c.x(), c.y() - 1),
        CPos::new(c.x(), c.y() + 1),
    ];
    if allow_diagonal {
        v.extend([
            CPos::new(c.x() - 1, c.y() - 1),
            CPos::new(c.x() + 1, c.y() - 1),
            CPos::new(c.x() - 1, c.y() + 1),
            CPos::new(c.x() + 1, c.y() + 1),
        ]);
    }
    v
}

pub fn are_adjacent_cells(a: CPos, b: CPos) -> bool {
    let o = b - a;
    o.x.abs() < 2 && o.y.abs() < 2
}

pub fn expand_footprint(cells: &[CPos], allow_diagonal: bool) -> Vec<CPos> {
    let mut seen = indexmap::IndexSet::new();
    for c in cells {
        for n in neighbours(*c, allow_diagonal) {
            seen.insert(n);
        }
    }
    seen.into_iter().collect()
}

pub fn adjacent_cells(w: &World, target: &Target) -> Vec<CPos> {
    let mut cells = indexmap::IndexSet::new();
    for p in target.positions() {
        cells.insert(w.map().cell_containing(p));
    }
    expand_footprint(&cells.into_iter().collect::<Vec<_>>(), true)
}

/// `ApplyPercentageModifiers`: exact product truncated toward zero, like C# `decimal`.
pub fn apply_percentage_modifiers(number: i32, percentages: impl IntoIterator<Item = i32>) -> i32 {
    let mut num = number as i128;
    let mut den = 1i128;
    for p in percentages {
        num *= p as i128;
        den *= 100;
        let g = gcd(num.unsigned_abs(), den as u128) as i128;
        if g > 1 {
            num /= g;
            den /= g;
        }
    }
    (num / den) as i32
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// `RandomWalk`, stateful: returns the next position.
pub fn random_walk_step(p: CPos, r: &mut MersenneTwister) -> CPos {
    loop {
        let dx = r.next_range(-1, 2);
        let dy = r.next_range(-1, 2);
        if dx == 0 && dy == 0 {
            continue;
        }
        return p + CVec::new(dx, dy);
    }
}

pub fn random_in_range(random: &mut MersenneTwister, range: &[i32]) -> i32 {
    match range.len() {
        0 => 0,
        1 => range[0],
        _ => random.next_range(range[0], range[1]),
    }
}

pub fn random_distance(random: &mut MersenneTwister, distance: &[WDist]) -> WDist {
    match distance.len() {
        0 => WDist::ZERO,
        1 => distance[0],
        _ => WDist::new(random.next_range(distance[0].length, distance[1].length)),
    }
}

pub fn random_vector(random: &mut MersenneTwister, vector: &[WVec]) -> WVec {
    match vector.len() {
        0 => WVec::ZERO,
        1 => vector[0],
        _ => {
            let x = random.next_range(vector[0].x, vector[1].x);
            let y = random.next_range(vector[0].y, vector[1].y);
            let z = random.next_range(vector[0].z, vector[1].z);
            WVec::new(x, y, z)
        }
    }
}

pub fn is_at_ground_level(a: &ActorRef) -> bool {
    if a.cache().occupies_space.is_none() || !a.is_in_world() {
        return false;
    }
    let w = a.world();
    w.map().contains(a.location()) && w.map().distance_above_terrain(a.center_position()).length == 0
}

pub fn appears_friendly_to(a: &ActorRef, to: &ActorRef) -> bool {
    let stance = to.owner().relationship_with(Some(&a.owner()));
    if stance == PlayerRelationship::ALLY {
        return true;
    }
    if let Some(eo) = &a.cache().effective_owner {
        if eo.disguised() && !to.info.has_trait_info_named("IgnoresDisguise") {
            return to.owner().relationship_with(eo.owner().as_deref()) == PlayerRelationship::ALLY;
        }
    }
    false
}

pub fn appears_hostile_to(a: &ActorRef, to: &ActorRef) -> bool {
    let stance = to.owner().relationship_with(Some(&a.owner()));
    if stance == PlayerRelationship::ALLY {
        return false;
    }
    if let Some(eo) = &a.cache().effective_owner {
        if eo.disguised() && !to.info.has_trait_info_named("IgnoresDisguise") {
            return to.owner().relationship_with(eo.owner().as_deref()) == PlayerRelationship::ENEMY;
        }
    }
    stance == PlayerRelationship::ENEMY
}

/// `MinByOrDefault` of the distance to `position`: the first closest position.
pub fn closest_position(positions: &[WPos], position: WPos) -> Option<WPos> {
    let mut best: Option<(WPos, i64)> = None;
    for p in positions {
        let d = (*p - position).length_squared();
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((*p, d));
        }
    }
    best.map(|b| b.0)
}

/// `WRot.FromYaw` shortcut used by offset math.
pub fn rot_from_yaw(a: WAngle) -> WRot {
    WRot::from_yaw(a)
}

/// `ClosestToWithPathFrom`: the closest of `actors` reachable by `source`.
pub fn closest_to_with_path_from(actors: impl IntoIterator<Item = ActorRef>, source: &ActorRef) -> Option<ActorRef> {
    let w = source.world();
    let is_air = source.info.has_trait_info::<crate::aircraft::AircraftInfo>();
    let mobile = source.trait_or_default::<crate::mobile::Mobile>();
    if !is_air && mobile.is_none() {
        return None;
    }
    let path_finder = w.world_actor().trait_::<crate::path_finder_trait::PathFinder>();
    let src_pos = source.center_position();
    let mut best: Option<(ActorRef, i64)> = None;
    for a in actors {
        if !is_air {
            let from = w.map().cell_containing(src_pos);
            let to = w.map().cell_containing(a.center_position());
            if !path_finder.path_exists_for_locomotor(&mobile.as_ref().unwrap().locomotor(), from, to) {
                continue;
            }
        }
        let d = (a.center_position() - src_pos).length_squared();
        if best.as_ref().is_none_or(|(_, b)| d < *b) {
            best = Some((a, d));
        }
    }
    best.map(|b| b.0)
}
