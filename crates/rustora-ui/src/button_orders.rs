// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `GlobalButtonOrderGenerator.cs` (sell and power down), `RepairOrderGenerator.cs`,
//! `BeaconOrderGenerator.cs` and `OrderButtonsChromeLogic.cs`. Power down orders are issued, but
//! `ToggleConditionOnOrder` is still a sim stub that ignores them.

use std::rc::Rc;

use rustora_core::world_types::CPos;
use rustora_engine::actor::ActorRef;
use rustora_engine::damage::DamageState;
use rustora_engine::order::Order;
use rustora_engine::player::WinState;
use rustora_engine::target::Target;
use rustora_engine::traits::IHealth;
use rustora_mods_common::base::RepairableBuildingInfo;
use rustora_mods_common::resupply::{Repairable, RepairableNear};
use rustora_mods_common::transforms::Sellable;

use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::orders::{OrderContext, OrderGenerator};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonOrder {
    Sell,
    Repair,
    PowerDown,
    Beacon,
}

/// The order generators of the sell, repair and power down buttons.
pub struct ButtonOrderGenerator {
    pub kind: ButtonOrder,
}

fn is_local(ctx: &OrderContext, a: &ActorRef) -> bool {
    ctx.local_player.is_some_and(|p| Rc::ptr_eq(p, &a.owner()))
}

fn damage_state(a: &ActorRef) -> Option<DamageState> {
    a.trait_or_default::<dyn IHealth>().map(|h| h.damage_state())
}

impl ButtonOrderGenerator {
    /// `OrderInner(world, mi)` with the mouse at `world_px`.
    fn orders_at(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        if mi.button != MouseButton::Left {
            return Vec::new();
        }
        if self.kind == ButtonOrder::Beacon {
            return self.beacon_orders(ctx, cell);
        }
        let under: Vec<ActorRef> = ctx.screen_map.actors_at_mouse(world_px).into_iter().map(|e| e.actor).collect();
        match self.kind {
            ButtonOrder::Sell => under
                .into_iter()
                .find(|a| is_local(ctx, a) && a.traits_implementing::<Sellable>().iter().any(|s| !s.is_trait_disabled()))
                .map(|a| vec![Order::new("Sell", Some(a), Target::Invalid, false)])
                .unwrap_or_default(),
            ButtonOrder::PowerDown => under
                .into_iter()
                .find(|a| is_local(ctx, a) && a.info.has_trait_info_named("ToggleConditionOnOrder"))
                .map(|a| vec![Order::new("PowerDown", Some(a), Target::Invalid, false)])
                .unwrap_or_default(),
            ButtonOrder::Repair => self.repair_orders(ctx, under, mi),
            ButtonOrder::Beacon => Vec::new(),
        }
    }

    /// `BeaconOrderGenerator.OrderInner` of a left click.
    fn beacon_orders(&self, ctx: &OrderContext, cell: CPos) -> Vec<Order> {
        ctx.cancel_input_mode();
        let Some(local) = ctx.local_player else {
            return Vec::new();
        };
        let target = Target::from_cell(ctx.world, cell, rustora_engine::traits::subcell::FULL_CELL);
        let mut order = Order::new("PlaceBeacon", Some(local.player_actor().clone()), target, false);
        order.suppress_visual_feedback = true;
        vec![order]
    }

    /// `RepairOrderGenerator.OrderInner`.
    fn repair_orders(&self, ctx: &OrderContext, under: Vec<ActorRef>, mi: &MouseInput) -> Vec<Order> {
        let Some(local) = ctx.local_player else {
            return Vec::new();
        };
        let player_actor = local.player_actor().clone();
        let sel = ctx.selection_context();
        let Some(a) = under
            .into_iter()
            .find(|a| rustora_mods_common::util::appears_friendly_to(a, &player_actor) && !sel.fog_obscures(a))
        else {
            return Vec::new();
        };
        if damage_state(&a).is_none_or(|d| d == DamageState::UNDAMAGED) {
            return Vec::new();
        }
        let mut out = Vec::new();
        if a.info.has_trait_info::<RepairableBuildingInfo>() {
            out.push(Order::new(
                "RepairBuilding",
                Some(player_actor),
                Target::from_actor(Some(&a)),
                false,
            ));
        }
        if !is_local(ctx, &a) {
            return out;
        }
        let (order_id, building) = if let Some(r) = a.trait_or_default::<Repairable>() {
            ("Repair", r.find_repair_building(&a))
        } else if let Some(r) = a.trait_or_default::<RepairableNear>() {
            ("RepairNear", r.find_repair_building(&a))
        } else {
            ("Repair", None)
        };
        if let Some(b) = building {
            out.push(Order::new(
                order_id,
                Some(a.clone()),
                Target::from_actor(Some(&b)),
                mi.modifiers.shift,
            ));
        }
        out
    }
}

impl OrderGenerator for ButtonOrderGenerator {
    fn order(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        let left_down = mi.button == MouseButton::Left && mi.event == MouseEvent::Down;
        let right_up = mi.button == MouseButton::Right && mi.event == MouseEvent::Up;
        if !left_down && !right_up {
            return Vec::new();
        }
        if mi.button == MouseButton::Right {
            ctx.cancel_input_mode();
        }
        self.orders_at(ctx, cell, world_px, mi)
    }

    fn tick(&self, ctx: &OrderContext) {
        if self.kind != ButtonOrder::Beacon
            && ctx.local_player.is_some_and(|p| p.win_state.get() != WinState::Undefined) {
            ctx.cancel_input_mode();
        }
    }

    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, world_px: (i32, i32), mi: &MouseInput) -> Option<String> {
        if self.kind == ButtonOrder::Beacon {
            return Some("ability".to_string());
        }
        let mi = MouseInput {
            button: MouseButton::Left,
            ..*mi
        };
        let orders = self.orders_at(ctx, cell, world_px, &mi);
        Some(match self.kind {
            ButtonOrder::Sell => orders
                .iter()
                .filter_map(|o| o.subject.as_ref())
                .flat_map(|s| s.traits_implementing::<Sellable>())
                .find(|s| !s.is_trait_disabled())
                .map_or_else(|| "sell-blocked".to_string(), |s| s.info.cursor.clone()),
            ButtonOrder::Repair => if orders.is_empty() { "repair-blocked" } else { "repair" }.to_string(),
            ButtonOrder::PowerDown => if orders.is_empty() { "powerdown-blocked" } else { "powerdown" }.to_string(),
            ButtonOrder::Beacon => "ability".to_string(),
        })
    }

    fn button_order(&self) -> Option<ButtonOrder> {
        Some(self.kind)
    }
}
