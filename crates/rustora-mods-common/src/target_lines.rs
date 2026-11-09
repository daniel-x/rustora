// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `DrawLineToTarget`: remembers until when an actor shows its target lines. The client draws the lines.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use rustora_core::world_types::WPos;
use rustora_derive::FieldLoad;
use rustora_engine::activity::{TargetLineNode, chain_target_line_nodes};
use rustora_engine::actor::ActorRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::TargetType;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("DrawLineToTarget", |n| load_info!(DrawLineToTargetInfo, n));
}

/// `DrawLineToTargetInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct DrawLineToTargetInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub delay: i32,
    pub line_width: i32,
    pub queued_line_width: i32,
    pub marker_width: i32,
    pub queued_marker_width: i32,
}

impl Default for DrawLineToTargetInfo {
    fn default() -> Self {
        DrawLineToTargetInfo {
            instance_name: None,
            delay: 2400,
            line_width: 1,
            queued_line_width: 1,
            marker_width: 2,
            queued_marker_width: 2,
        }
    }
}

/// `DrawLineToTarget`.
#[derive(rustora_engine::persist::Persist)]
pub struct DrawLineToTarget {
    pub info: Rc<DrawLineToTargetInfo>,
    lifetime: Cell<u64>,
}

register_traits!(DrawLineToTarget:);
info_impl!(DrawLineToTargetInfo => |info, _init| Some(Rc::new(DrawLineToTarget {
    info,
    lifetime: Cell::new(0),
})));

/// `Game.RunTime` for cosmetic timers of the simulation crates: milliseconds since the first call.
fn run_time() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// One segment of `TargetLineRenderable`.
pub struct TargetLine {
    pub from: WPos,
    pub to: WPos,
    pub color: rustora_core::fieldloader::Color,
    pub line_width: i32,
    pub marker_width: i32,
}

impl DrawLineToTarget {
    /// `ShowTargetLines` without the setting check of `TargetLines`, which the client applies when drawing.
    pub fn show(&self, self_: &ActorRef) {
        if !self_.is_idle() {
            self.lifetime.set(run_time() + self.info.delay as u64);
        }
    }

    /// The time part of `ShouldRender`.
    pub fn within_lifetime(&self) -> bool {
        run_time() <= self.lifetime.get()
    }

    /// `IRenderAnnotationsWhenSelected.RenderAnnotations`, in the order of drawing.
    pub fn lines(&self, self_: &ActorRef) -> Vec<TargetLine> {
        let mut out: Vec<TargetLine> = Vec::new();
        let mut prev = self_.center_position();
        let nodes: Vec<TargetLineNode> = chain_target_line_nodes(self_.current_activity(), self_);
        for n in nodes.into_iter().filter(|n| n.target.target_type() != TargetType::Invalid) {
            let to = n.target.center_position();
            let (line_width, marker_width) = if out.is_empty() {
                (self.info.line_width, self.info.marker_width)
            } else {
                (self.info.queued_line_width, self.info.queued_marker_width)
            };
            out.push(TargetLine {
                from: prev,
                to,
                color: n.color,
                line_width,
                marker_width,
            });
            prev = to;
        }
        out.reverse();
        out
    }
}

/// `LineTargetExts.ShowTargetLines`.
pub fn show_target_lines(self_: &ActorRef) {
    let world = self_.world();
    if !world.sound.local_player().is_some_and(|p| Rc::ptr_eq(&p, &self_.owner())) {
        return;
    }
    if let Some(line) = self_.trait_or_default::<DrawLineToTarget>() {
        let actor = self_.clone();
        world.add_frame_end_task(Box::new(move |_| line.show(&actor)));
    }
}
