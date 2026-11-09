// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The mouse part of `ScreenMap.cs`, `SelectionUtils.cs` and the bounds helpers of `SelectableExts.cs`.
//! The client refills the map with the mouse bounds of all actors in world pixels before input is handled.
//! Frozen actors are not tracked yet.

use std::rc::Rc;

use rustora_engine::actor::ActorRef;
use rustora_engine::frozen::FrozenActorRef;
use rustora_engine::player::PlayerRef;
use rustora_engine::world::World;
use rustora_mods_common::selection::{SelectionModifiers, info_selection_priority, selection_class, selection_priority};

use crate::draw::Rect;
use crate::input::Modifiers;

/// `ActorBoundsPair` with a rectangle in place of the polygon.
#[derive(Clone)]
pub struct ActorBounds {
    pub actor: ActorRef,
    pub bounds: Rect,
    /// `Interactable.DecorationBounds`.
    pub decoration: Rect,
}

#[derive(Default)]
pub struct ScreenMap {
    entries: Vec<ActorBounds>,
    /// The frozen actors of the render player with their mouse bounds.
    frozen: Vec<FrozenActorBounds>,
}

/// A frozen actor and its `MouseBounds`.
#[derive(Clone)]
pub struct FrozenActorBounds {
    pub frozen: FrozenActorRef,
    pub bounds: Rect,
}

fn rect_with_corners(a: (i32, i32), b: (i32, i32)) -> Rect {
    let (l, t) = (a.0.min(b.0), a.1.min(b.1));
    Rect::new(l, t, a.0.max(b.0) - l, a.1.max(b.1) - t)
}

fn selection_modifiers(m: Modifiers) -> SelectionModifiers {
    SelectionModifiers { ctrl: m.ctrl, alt: m.alt }
}

impl ScreenMap {
    pub fn set(&mut self, entries: Vec<ActorBounds>) {
        self.entries = entries;
    }

    pub fn entries(&self) -> &[ActorBounds] {
        &self.entries
    }

    pub fn set_frozen(&mut self, frozen: Vec<FrozenActorBounds>) {
        self.frozen = frozen;
    }

    /// `FrozenActorsAtMouse` of the render player.
    pub fn frozen_actors_at_mouse(&self, px: (i32, i32)) -> Vec<FrozenActorBounds> {
        self.frozen.iter().filter(|e| e.bounds.contains(px.0, px.1)).cloned().collect()
    }

    /// `ActorsAtMouse`.
    pub fn actors_at_mouse(&self, px: (i32, i32)) -> Vec<ActorBounds> {
        self.entries
            .iter()
            .filter(|e| e.actor.is_in_world() && e.bounds.contains(px.0, px.1))
            .cloned()
            .collect()
    }

    /// `ActorsInMouseBox`.
    pub fn actors_in_box(&self, a: (i32, i32), b: (i32, i32)) -> Vec<ActorBounds> {
        let r = rect_with_corners(a, b);
        self.entries
            .iter()
            .filter(|e| e.actor.is_in_world() && e.bounds.intersects(r))
            .cloned()
            .collect()
    }
}

/// The viewer of selection priorities: the local player, or the render player when spectating.
pub fn priority_viewer(local_player: Option<&PlayerRef>, render_player: Option<&PlayerRef>) -> Option<PlayerRef> {
    match local_player {
        Some(p) if !p.spectating() => Some(p.clone()),
        _ => render_player.cloned(),
    }
}

/// `WithHighestSelectionPriority` of actor and bounds pairs; the first of equal priorities wins like `MaxByOrDefault`.
pub fn with_highest_selection_priority(actors: &[ActorBounds], px: (i32, i32), modifiers: Modifiers) -> Option<ActorRef> {
    let m = selection_modifiers(modifiers);
    let mut best: Option<(i64, &ActorRef)> = None;
    for e in actors {
        let base = info_selection_priority(&e.actor.info, m) as i64;
        let b = e.bounds;
        let priority = if b.w == 0 && b.h == 0 {
            base
        } else {
            let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
            let (dx, dy) = ((cx - px.0) as f64, (cy - px.1) as f64);
            let distance = (dx * dx + dy * dy).sqrt() as i32 as i64;
            (base - distance) << 16
        };
        if best.is_none_or(|(p, _)| priority > p) {
            best = Some((priority, &e.actor));
        }
    }
    best.map(|(_, a)| a.clone())
}

/// `WithHighestSelectionPriority` of frozen actors; the first of equal priorities wins like `MaxByOrDefault`.
pub fn frozen_with_highest_selection_priority(
    frozen: &[FrozenActorBounds],
    px: (i32, i32),
    modifiers: Modifiers,
) -> Option<FrozenActorRef> {
    let m = selection_modifiers(modifiers);
    let mut best: Option<(i64, &FrozenActorRef)> = None;
    for e in frozen {
        let b = e.bounds;
        let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
        let (dx, dy) = ((cx - px.0) as f64, (cy - px.1) as f64);
        let distance = (dx * dx + dy * dy).sqrt() as i32 as i64;
        let priority = (info_selection_priority(e.frozen.info(), m) as i64 - distance) << 16;
        if best.is_none_or(|(p, _)| priority > p) {
            best = Some((priority, &e.frozen));
        }
    }
    best.map(|(_, f)| f.clone())
}

/// `SubsetWithHighestSelectionPriority`.
pub fn subset_with_highest_selection_priority(actors: Vec<ActorRef>, viewer: Option<&PlayerRef>, modifiers: Modifiers) -> Vec<ActorRef> {
    let m = selection_modifiers(modifiers);
    let priorities: Vec<i32> = actors.iter().map(|a| selection_priority(a, viewer, m)).collect();
    let Some(&max) = priorities.iter().max() else {
        return actors;
    };
    actors
        .into_iter()
        .zip(priorities)
        .filter(|(_, p)| *p == max)
        .map(|(a, _)| a)
        .collect()
}

/// The world state the selection helpers read.
pub struct SelectionContext<'a> {
    pub world: &'a World,
    pub screen_map: &'a ScreenMap,
    pub local_player: Option<&'a PlayerRef>,
    pub render_player: Option<&'a PlayerRef>,
    pub deadzone: i32,
}

impl SelectionContext<'_> {
    /// `World.FogObscures`.
    pub fn fog_obscures(&self, a: &ActorRef) -> bool {
        self.render_player.is_some() && !a.can_be_viewed_by_player(self.render_player)
    }

    fn selectable_by_viewer(&self, a: &ActorRef) -> bool {
        a.info.has_trait_info::<rustora_mods_common::selection::SelectableInfo>()
            && (a.owner().is_allied_with(self.render_player.map(|p| &**p)) || !self.fog_obscures(a))
    }

    /// `SelectHighestPriorityActorAtPoint`.
    pub fn select_highest_priority_actor_at_point(&self, px: (i32, i32), modifiers: Modifiers) -> Vec<ActorRef> {
        let candidates: Vec<ActorBounds> = self
            .screen_map
            .actors_at_mouse(px)
            .into_iter()
            .filter(|e| self.selectable_by_viewer(&e.actor))
            .collect();
        with_highest_selection_priority(&candidates, px, modifiers).into_iter().collect()
    }

    /// `SelectActorsInBoxWithDeadzone`.
    pub fn select_actors_in_box_with_deadzone(&self, a: (i32, i32), b: (i32, i32), modifiers: Modifiers) -> Vec<ActorRef> {
        let (dx, dy) = ((a.0 - b.0) as f64, (a.1 - b.1) as f64);
        let a = if (dx * dx + dy * dy).sqrt() as i32 <= self.deadzone { b } else { a };
        if a == b {
            return self.select_highest_priority_actor_at_point(a, modifiers);
        }
        let actors = self
            .screen_map
            .actors_in_box(a, b)
            .into_iter()
            .map(|e| e.actor)
            .filter(|x| self.selectable_by_viewer(x))
            .collect();
        let viewer = priority_viewer(self.local_player, self.render_player);
        subset_with_highest_selection_priority(actors, viewer.as_ref(), modifiers)
    }

    /// `GetPlayersToIncludeInSelection`.
    pub fn players_to_include_in_selection(&self) -> Vec<PlayerRef> {
        let viewer = self.render_player.or(self.local_player);
        let shroud_disabled = viewer.is_none() || (self.render_player.is_none() && self.local_player.is_some_and(|p| p.spectating()));
        let everyone = viewer.is_some_and(|v| v.non_combatant && v.spectating());
        if shroud_disabled || everyone {
            self.world.players()
        } else {
            viewer.into_iter().cloned().collect()
        }
    }

    /// `SelectActorsOnScreen`.
    pub fn select_actors_on_screen(
        &self,
        top_left: (i32, i32),
        bottom_right: (i32, i32),
        classes: &[String],
        players: &[PlayerRef],
    ) -> Vec<ActorRef> {
        self.screen_map
            .actors_in_box(top_left, bottom_right)
            .into_iter()
            .map(|e| e.actor)
            .filter(|a| {
                let owner = a.owner();
                players.iter().any(|p| Rc::ptr_eq(p, &owner))
                    && selection_class(a).is_some_and(|c| classes.is_empty() || classes.contains(&c))
            })
            .collect()
    }
}
