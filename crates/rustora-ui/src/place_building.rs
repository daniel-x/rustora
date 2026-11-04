// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `PlaceBuildingOrderGenerator.cs` for one variant and without plugs. The footprint is drawn as colored
//! cells until the placement previews of the render traits are ported, line build cells are not previewed, and
//! failed placements neither clear blockers nor play notifications.

use std::cell::Cell;
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::CPos;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::order::Order;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_mods_common::base::{LineBuildInfo, can_place_building, is_cell_buildable, is_close_enough_to_base};
use rustora_mods_common::building::BuildingInfo;
use rustora_mods_common::production::ProductionQueue;
use rustora_mods_common::resources::ResourceLayer;

use crate::draw::{DrawContext, Rect};
use crate::input::{MouseButton, MouseEvent, MouseInput};
use crate::orders::{OrderContext, OrderGenerator};
use crate::viewport::Viewport;

pub struct PlaceBuildingOrderGenerator {
    queue: Rc<ProductionQueue>,
    actor: Rc<ActorInfo>,
    building: Rc<BuildingInfo>,
    top_left: Cell<CPos>,
}

impl PlaceBuildingOrderGenerator {
    pub fn new(queue: Rc<ProductionQueue>, name: &str) -> PlaceBuildingOrderGenerator {
        let world = queue.actor.world();
        let actor = world.rules().actor(name).expect("building actor");
        let building = actor.trait_info::<BuildingInfo>().expect("Building");
        PlaceBuildingOrderGenerator {
            queue,
            actor,
            building,
            top_left: Cell::new(CPos::ZERO),
        }
    }

    /// `InnerOrder`.
    fn inner_order(&self, ctx: &OrderContext, mi: &MouseInput) -> Vec<Order> {
        let world = ctx.world;
        if world.paused() || mi.button != MouseButton::Left {
            return Vec::new();
        }
        let owner = self.queue.actor.owner();
        let top_left = self.top_left.get();
        if !can_place_building(world, top_left, &self.building, None)
            || !is_close_enough_to_base(&self.building, world, &owner, &self.actor, top_left)
        {
            return Vec::new();
        }
        let order_type = if self.actor.has_trait_info::<LineBuildInfo>() && !mi.modifiers.shift {
            "LineBuild"
        } else {
            "PlaceBuilding"
        };
        let mut o = Order::new(
            order_type,
            Some(owner.player_actor().clone()),
            Target::from_cell(world, top_left, FULL_CELL),
            false,
        );
        o.target_string = Some(self.actor.name.clone());
        o.extra_data = self.queue.actor.id;
        o.extra_location = CPos::new(0, 0);
        vec![o]
    }
}

impl OrderGenerator for PlaceBuildingOrderGenerator {
    fn order(&self, ctx: &OrderContext, cell: CPos, _world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        let left_down = mi.button == MouseButton::Left && mi.event == MouseEvent::Down;
        let right_up = mi.button == MouseButton::Right && mi.event == MouseEvent::Up;
        if !left_down && !right_up {
            return Vec::new();
        }
        self.top_left.set(cell);
        if right_up {
            ctx.cancel_input_mode();
        }
        let orders = self.inner_order(ctx, mi);
        if orders
            .iter()
            .any(|o| matches!(o.order_string.as_str(), "PlaceBuilding" | "LineBuild" | "PlacePlug"))
        {
            ctx.cancel_input_mode();
        }
        orders
    }

    fn tick(&self, ctx: &OrderContext) {
        if self.queue.all_queued().iter().all(|i| !i.done.get() || i.item != self.actor.name) {
            ctx.cancel_input_mode();
        }
    }

    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, _world_px: (i32, i32), _mi: &MouseInput) -> Option<String> {
        self.top_left.set(cell);
        Some(ctx.world_default_cursor.clone())
    }

    /// The footprint part of `RenderAboveShroud`: valid cells green, invalid cells red.
    fn draw(&self, ctx: &OrderContext, dc: &mut DrawContext, viewport: &Viewport) {
        let world = ctx.world;
        let top_left = self.top_left.get();
        let owner = self.queue.actor.owner();
        let local = ctx.local_player.cloned().unwrap_or_else(|| owner.clone());
        let resource_layer = world.world_actor().trait_or_default::<ResourceLayer>();
        let close_enough = is_close_enough_to_base(&self.building, world, &local, &self.actor, top_left);
        let cells: Vec<CPos> = if self.actor.has_trait_info::<LineBuildInfo>() {
            vec![top_left]
        } else {
            self.building.tiles(top_left)
        };
        for t in cells {
            let valid = close_enough
                && is_cell_buildable(world, t, &self.building, None)
                && resource_layer.as_ref().is_none_or(|r| r.get_resource(t).ty.is_none());
            let (x0, y0) = viewport.world_to_view_px((t.x() * 24, t.y() * 24));
            let (x1, y1) = viewport.world_to_view_px((t.x() * 24 + 24, t.y() * 24 + 24));
            let color = if valid {
                Color::from_argb(96, 0, 255, 0)
            } else {
                Color::from_argb(96, 255, 0, 0)
            };
            dc.fill_rect_with_color(Rect::new(x0, y0, x1 - x0, y1 - y0), color);
        }
    }
}
