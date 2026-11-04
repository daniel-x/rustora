// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `IOrderGenerator.cs` and `UnitOrderGenerator.cs`. Frozen actors are not targetable yet.

use std::rc::Rc;

use rustora_core::world_types::CPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_engine::world::World;
use rustora_mods_common::orders::{IIssueOrder, IOrderTargeter, TargetModifiers};
use rustora_mods_common::selection::{SelectableInfo, Selection};
use rustora_mods_common::targetable::TargetableInfo;

use crate::input::{KeyInput, Modifiers, MouseButton, MouseInput};
use crate::screen_map::{ScreenMap, SelectionContext, with_highest_selection_priority};

/// What order generators read of the world and the client.
pub struct OrderContext<'a> {
    pub world: &'a Rc<World>,
    pub screen_map: &'a ScreenMap,
    pub selection: &'a Selection,
    pub local_player: Option<&'a PlayerRef>,
    pub render_player: Option<&'a PlayerRef>,
    pub use_classic_mouse_style: bool,
    pub selection_deadzone: i32,
}

impl<'a> OrderContext<'a> {
    pub fn selection_context(&self) -> SelectionContext<'a> {
        SelectionContext {
            world: self.world,
            screen_map: self.screen_map,
            local_player: self.local_player,
            render_player: self.render_player,
            deadzone: self.selection_deadzone,
        }
    }

    /// `MouseButtonPreference.Action`.
    pub fn action_button(&self) -> MouseButton {
        if self.use_classic_mouse_style {
            MouseButton::Left
        } else {
            MouseButton::Right
        }
    }
}

/// `IOrderGenerator`.
pub trait OrderGenerator {
    fn order(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Vec<Order>;
    fn tick(&self, _ctx: &OrderContext) {}
    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Option<String>;
    fn deactivate(&self) {}
    fn handle_key(&self, _e: &KeyInput) -> bool {
        false
    }
    fn selection_changed(&self, _ctx: &OrderContext) {}
    /// `this is UnitOrderGenerator`.
    fn as_unit_order_generator(&self) -> Option<&UnitOrderGenerator> {
        None
    }
}

/// `UnitOrderGenerator`.
pub struct UnitOrderGenerator {
    pub world_select_cursor: String,
    pub world_default_cursor: String,
}

/// `UnitOrderResult`.
struct UnitOrderResult {
    actor: ActorRef,
    order: Box<dyn IOrderTargeter>,
    trait_: Rc<dyn IIssueOrder>,
    cursor: Option<String>,
    target: Target,
}

pub fn target_modifiers(m: Modifiers) -> TargetModifiers {
    TargetModifiers::NONE
        .with(TargetModifiers::FORCE_ATTACK, m.ctrl)
        .with(TargetModifiers::FORCE_QUEUE, m.shift)
        .with(TargetModifiers::FORCE_MOVE, m.alt)
}

impl UnitOrderGenerator {
    /// `TargetForInput`.
    fn target_for_input(ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Target {
        let sel = ctx.selection_context();
        let candidates: Vec<_> = ctx
            .screen_map
            .actors_at_mouse(world_px)
            .into_iter()
            .filter(|e| !e.actor.is_dead() && e.actor.info.has_trait_info::<TargetableInfo>() && !sel.fog_obscures(&e.actor))
            .collect();
        match with_highest_selection_priority(&candidates, world_px, mi.modifiers) {
            Some(a) => Target::from_actor(Some(&a)),
            None => Target::from_cell(ctx.world, cell, FULL_CELL),
        }
    }

    /// `OrderForUnit`: the most appropriate order of an actor for a target, preferring orders against the target
    /// over orders against the cell.
    fn order_for_unit(ctx: &OrderContext, self_: &ActorRef, target: &Target, xy: CPos, mi: &MouseInput) -> Option<UnitOrderResult> {
        if mi.button != ctx.action_button() {
            return None;
        }
        if !ctx.local_player.is_some_and(|p| Rc::ptr_eq(p, &self_.owner())) {
            return None;
        }
        if ctx.world.is_game_over() || self_.disposed() || !target.is_valid_for(self_) {
            return None;
        }
        let modifiers = target_modifiers(mi.modifiers);
        let mut orders: Vec<(Rc<dyn IIssueOrder>, Box<dyn IOrderTargeter>)> = self_
            .traits_implementing::<dyn IIssueOrder>()
            .into_iter()
            .flat_map(|t| t.orders(self_).into_iter().map(move |o| (t.clone(), o)))
            .collect();
        orders.sort_by_key(|(_, o)| std::cmp::Reverse(o.order_priority()));
        let mut target = target.clone();
        for _ in 0..2 {
            let found = orders.iter().enumerate().find_map(|(i, (_, order))| {
                let mut local_modifiers = modifiers;
                let mut cursor = None;
                order
                    .can_target(self_, &target, &mut local_modifiers, &mut cursor)
                    .then_some((i, cursor))
            });
            if let Some((i, cursor)) = found {
                let (trait_, order) = orders.swap_remove(i);
                return Some(UnitOrderResult {
                    actor: self_.clone(),
                    order,
                    trait_,
                    cursor,
                    target,
                });
            }
            target = Target::from_cell(ctx.world, xy, FULL_CELL);
        }
        None
    }

    /// Used for classic mouse orders, determines whether or not action at `xy` is move or select.
    pub fn input_overrides_selection(&self, ctx: &OrderContext, xy: (i32, i32), mi: &MouseInput) -> bool {
        let sel = ctx.selection_context();
        let candidates: Vec<_> = ctx
            .screen_map
            .actors_at_mouse(xy)
            .into_iter()
            .filter(|e| {
                let a = &e.actor;
                !a.is_dead()
                    && a.info.has_trait_info::<SelectableInfo>()
                    && (a.owner().is_allied_with(ctx.render_player.map(|p| &**p)) || !sel.fog_obscures(a))
            })
            .collect();
        let Some(actor) = with_highest_selection_priority(&candidates, xy, mi.modifiers) else {
            return true;
        };
        let target = Target::from_actor(Some(&actor));
        let cell = ctx.world.map().cell_containing(target.center_position());
        let actors_at = rustora_mods_common::actor_map::WorldExt::actor_map(&**ctx.world).get_actors_at(cell);
        let modifiers = target_modifiers(mi.modifiers);
        ctx.selection.actors().iter().any(|a| {
            Self::order_for_unit(ctx, a, &target, cell, mi)
                .is_some_and(|o| o.order.target_overrides_selection(a, &o.target, &actors_at, cell, modifiers))
        })
    }

    pub fn clear_selection_on_left_click(&self) -> bool {
        true
    }
}

impl OrderGenerator for UnitOrderGenerator {
    fn order(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        let target = Self::target_for_input(ctx, cell, world_px, mi);
        let results: Vec<UnitOrderResult> = ctx
            .selection
            .actors()
            .iter()
            .filter_map(|a| Self::order_for_unit(ctx, a, &target, cell, mi))
            .collect();
        let mut involved: Vec<ActorRef> = Vec::new();
        for r in &results {
            if !involved.iter().any(|a| Rc::ptr_eq(a, &r.actor)) {
                involved.push(r.actor.clone());
            }
        }
        let Some(first) = involved.first() else {
            return Vec::new();
        };
        let mut group = Order::new("CreateGroup", Some(first.owner().player_actor().clone()), Target::Invalid, false);
        group.extra_actors = Some(involved.iter().cloned().map(Some).collect());
        let mut out = vec![group];
        for r in results {
            let order = r.trait_.issue_order(&r.actor, &*r.order, &r.target, mi.modifiers.shift);
            match (&order, r.order.order_id()) {
                (None, id) => log::debug!("BUG: in order targeter - decided on {id} but then didn't order"),
                (Some(o), id) if o.order_string != id => {
                    log::debug!("BUG: in order targeter - decided on {id} but ordered {}", o.order_string)
                }
                _ => {}
            }
            out.extend(order);
        }
        out
    }

    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Option<String> {
        let target = Self::target_for_input(ctx, cell, world_px, mi);
        let target_selectable =
            target.target_type() == TargetType::Actor && target.actor().is_some_and(|a| a.info.has_trait_info::<SelectableInfo>());
        let use_select = if ctx.use_classic_mouse_style && !self.input_overrides_selection(ctx, world_px, mi) {
            target_selectable
        } else {
            let mut best: Option<(i32, String)> = None;
            for a in ctx.selection.actors() {
                let Some(r) = Self::order_for_unit(ctx, &a, &target, cell, mi) else {
                    continue;
                };
                let Some(cursor) = r.cursor else { continue };
                let priority = r.order.order_priority();
                if best.as_ref().is_none_or(|(p, _)| priority > *p) {
                    best = Some((priority, cursor));
                }
            }
            if let Some((_, cursor)) = best {
                return Some(cursor);
            }
            target_selectable && (mi.modifiers.shift || ctx.selection.actors().is_empty())
        };
        Some(if use_select {
            self.world_select_cursor.clone()
        } else {
            self.world_default_cursor.clone()
        })
    }

    fn as_unit_order_generator(&self) -> Option<&UnitOrderGenerator> {
        Some(self)
    }
}
