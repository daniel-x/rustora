// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ForceModifiersOrderGenerator.cs`, `AttackMoveOrderGenerator` and `GuardOrderGenerator.cs`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_core::world_types::CPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::order::Order;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_mods_common::attack_move::AttackMove;
use rustora_mods_common::auto_target::{AutoTargetInfo, GuardInfo, GuardableInfo};
use rustora_mods_common::util::appears_friendly_to;

use crate::input::{Modifiers, MouseInput};
use crate::orders::{OrderContext, OrderGenerator, UnitOrderGenerator};

/// Which command bar button made an order generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandMode {
    AttackMove,
    Guard,
    ForceModifiers(Modifiers),
}

/// `ForceModifiersOrderGenerator`.
pub struct ForceModifiersOrderGenerator {
    pub base: UnitOrderGenerator,
    pub modifiers: Modifiers,
    pub cancel_on_first_use: bool,
}

impl ForceModifiersOrderGenerator {
    fn modified(&self, mi: &MouseInput) -> MouseInput {
        MouseInput {
            modifiers: mi.modifiers.union(self.modifiers),
            ..*mi
        }
    }
}

impl OrderGenerator for ForceModifiersOrderGenerator {
    fn order(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        if self.cancel_on_first_use {
            ctx.cancel_input_mode();
        }
        self.base.order(ctx, cell, world_px, &self.modified(mi))
    }

    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Option<String> {
        self.base.get_cursor(ctx, cell, world_px, &self.modified(mi))
    }

    fn command_mode(&self) -> Option<CommandMode> {
        Some(CommandMode::ForceModifiers(self.modifiers))
    }

    fn is_unit_order_generator(&self) -> bool {
        true
    }

    fn input_overrides_selection(&self, ctx: &OrderContext, xy: (i32, i32), mi: &MouseInput) -> bool {
        self.base.input_overrides_selection(ctx, xy, mi)
    }
}

fn grouped_order(name: &str, target: Target, queued: bool, subjects: Vec<ActorRef>) -> Order {
    let mut order = Order::new(name, None, target, queued);
    order.grouped_actors = Some(subjects.into_iter().map(Some).collect());
    order
}

/// `AttackMoveOrderGenerator`. The expected button is read from the context on each use.
pub struct AttackMoveOrderGenerator {
    subjects: RefCell<Vec<(ActorRef, Rc<AttackMove>)>>,
}

fn attack_move_subjects(actors: impl IntoIterator<Item = ActorRef>) -> Vec<(ActorRef, Rc<AttackMove>)> {
    actors
        .into_iter()
        .filter(|a| !a.is_dead())
        .flat_map(|a| a.traits_implementing::<AttackMove>().into_iter().map(move |t| (a.clone(), t)))
        .collect()
}

impl AttackMoveOrderGenerator {
    pub fn new(subjects: impl IntoIterator<Item = ActorRef>) -> AttackMoveOrderGenerator {
        AttackMoveOrderGenerator {
            subjects: RefCell::new(attack_move_subjects(subjects)),
        }
    }
}

impl OrderGenerator for AttackMoveOrderGenerator {
    fn order(&self, ctx: &OrderContext, cell: CPos, _world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        if mi.button != ctx.action_button() {
            ctx.cancel_input_mode();
            return Vec::new();
        }
        let queued = mi.modifiers.shift;
        if !queued {
            ctx.cancel_input_mode();
        }
        let name = if mi.modifiers.ctrl { "AssaultMove" } else { "AttackMove" };
        let target = Target::from_cell(ctx.world, ctx.world.map().clamp(cell), FULL_CELL);
        let subjects = self.subjects.borrow().iter().map(|(a, _)| a.clone()).collect();
        vec![grouped_order(name, target, queued, subjects)]
    }

    fn selection_changed(&self, ctx: &OrderContext) {
        let subjects = attack_move_subjects(ctx.selection.actors());
        let any_auto_target = subjects.iter().any(|(a, _)| a.info.has_trait_info::<AutoTargetInfo>());
        *self.subjects.borrow_mut() = subjects;
        if !any_auto_target {
            ctx.cancel_input_mode();
        }
    }

    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, _world_px: (i32, i32), mi: &MouseInput) -> Option<String> {
        let subjects = self.subjects.borrow();
        let (actor, attack_move) = subjects.first()?;
        let info = &attack_move.info;
        let assault = mi.modifiers.ctrl;
        if !ctx.world.map().contains(cell) {
            return Some(if assault {
                info.assault_move_blocked_cursor.clone()
            } else {
                info.attack_move_blocked_cursor.clone()
            });
        }
        let cannot_move = subjects.iter().find(|(_, t)| !t.info.move_into_shroud);
        let blocked = cannot_move.filter(|_| !actor.owner().shroud().is_explored(cell));
        Some(match (blocked, assault) {
            (Some((_, t)), true) => t.info.assault_move_blocked_cursor.clone(),
            (Some((_, t)), false) => t.info.attack_move_blocked_cursor.clone(),
            (None, true) => info.assault_move_cursor.clone(),
            (None, false) => info.attack_move_cursor.clone(),
        })
    }

    fn command_mode(&self) -> Option<CommandMode> {
        Some(CommandMode::AttackMove)
    }

    fn is_unit_order_generator(&self) -> bool {
        true
    }

    fn clear_selection_on_left_click(&self) -> bool {
        false
    }
}

/// `GuardOrderGenerator` with the order `Guard` and the cursor `guard`.
pub struct GuardOrderGenerator {
    subjects: RefCell<Vec<ActorRef>>,
}

impl GuardOrderGenerator {
    pub fn new(subjects: Vec<ActorRef>) -> GuardOrderGenerator {
        GuardOrderGenerator {
            subjects: RefCell::new(subjects),
        }
    }

    /// `FriendlyGuardableUnits`.
    fn friendly_guardable_units(ctx: &OrderContext, world_px: (i32, i32)) -> Vec<ActorRef> {
        let Some(player) = ctx.local_player else {
            return Vec::new();
        };
        let player_actor = player.player_actor().clone();
        let sel = ctx.selection_context();
        ctx.screen_map
            .actors_at_mouse(world_px)
            .into_iter()
            .map(|e| e.actor)
            .filter(|a| {
                !a.is_dead() && appears_friendly_to(a, &player_actor) && a.info.has_trait_info::<GuardableInfo>() && !sel.fog_obscures(a)
            })
            .collect()
    }
}

impl OrderGenerator for GuardOrderGenerator {
    fn order(&self, ctx: &OrderContext, _cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        if mi.button != ctx.action_button() {
            ctx.cancel_input_mode();
        }
        let Some(target) = Self::friendly_guardable_units(ctx, world_px).into_iter().next() else {
            return Vec::new();
        };
        let queued = mi.modifiers.shift;
        if !queued {
            ctx.cancel_input_mode();
        }
        let subjects = self.subjects.borrow().iter().filter(|s| !Rc::ptr_eq(s, &target)).cloned().collect();
        vec![grouped_order("Guard", Target::from_actor(Some(&target)), queued, subjects)]
    }

    fn selection_changed(&self, ctx: &OrderContext) {
        let subjects: Vec<ActorRef> = ctx
            .selection
            .actors()
            .into_iter()
            .filter(|s| !s.is_dead() && s.info.has_trait_info::<GuardInfo>())
            .collect();
        let any_auto_target = subjects.iter().any(|s| s.info.has_trait_info::<AutoTargetInfo>());
        *self.subjects.borrow_mut() = subjects;
        if !any_auto_target {
            ctx.cancel_input_mode();
        }
    }

    fn get_cursor(&self, ctx: &OrderContext, _cell: CPos, world_px: (i32, i32), _mi: &MouseInput) -> Option<String> {
        let subjects = self.subjects.borrow();
        let first = subjects.first()?;
        let can_guard = Self::friendly_guardable_units(ctx, world_px)
            .iter()
            .any(|a| subjects.len() > 1 || !Rc::ptr_eq(a, first));
        Some(if can_guard { "guard" } else { "move-blocked" }.to_string())
    }

    fn command_mode(&self) -> Option<CommandMode> {
        Some(CommandMode::Guard)
    }

    fn is_unit_order_generator(&self) -> bool {
        true
    }

    fn clear_selection_on_left_click(&self) -> bool {
        false
    }
}
