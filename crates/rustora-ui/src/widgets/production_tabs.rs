// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ProductionTabsWidget.cs`: the numbered tabs of the production queues of one group, used by the cnc
//! sidebar.

use std::cell::Cell;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::fieldloader::Color;
use rustora_engine::world::World;
use rustora_mods_common::production::{ProductionQueue, ProductionQueueInfo};

use super::button::{draw_background, stateful_image};
use super::impl_as_any;
use super::production::ProductionPaletteWidget;
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{KeyEvent, KeyInput, MouseButton, MouseEvent, MouseInput};
use crate::logic::services;
use crate::widget::{Ui, WidgetId, WidgetKind};

/// `ProductionTab`.
#[derive(Clone)]
pub struct ProductionTab {
    pub name: String,
    pub queue: Rc<ProductionQueue>,
}

/// `ProductionTabGroup`.
#[derive(Clone, Default)]
pub struct ProductionTabGroup {
    pub tabs: Vec<ProductionTab>,
    pub group: String,
}

impl ProductionTabGroup {
    /// `ProductionTabGroup.Alert`.
    pub fn alert(&self) -> bool {
        self.tabs.iter().any(|t| t.queue.all_queued().iter().any(|i| i.done.get()))
    }

    /// `ProductionTabGroup.Update`: keeps the names of the remaining queues and numbers the new ones after them.
    fn update(&mut self, all_queues: &[Rc<ProductionQueue>]) {
        let mut queues: Vec<Rc<ProductionQueue>> = all_queues
            .iter()
            .filter(|q| q.enabled() && queue_group(q) == self.group)
            .cloned()
            .collect();
        let mut tabs = Vec::new();
        let mut largest_used_name = 0;
        for t in &self.tabs {
            let Some(i) = queues.iter().position(|q| Rc::ptr_eq(q, &t.queue)) else {
                continue;
            };
            tabs.push(t.clone());
            queues.remove(i);
            largest_used_name = largest_used_name.max(t.name.parse().unwrap_or(0));
        }
        for (name, queue) in (largest_used_name + 1..).zip(queues) {
            tabs.push(ProductionTab {
                name: name.to_string(),
                queue,
            });
        }
        self.tabs = tabs;
    }
}

/// `ProductionQueueInfo.Group`, which falls back to the queue type.
fn queue_group(q: &ProductionQueue) -> String {
    q.info.group.clone().unwrap_or_else(|| q.info.ty.clone())
}

fn info_group(q: &ProductionQueueInfo) -> String {
    q.group.clone().unwrap_or_else(|| q.ty.clone())
}

#[derive(Clone)]
pub struct ProductionTabsWidget {
    world: Option<Rc<World>>,
    pub palette_widget: String,
    pub types_container: String,
    pub background_container: Option<String>,
    tab_width: i32,
    arrow_width: i32,
    click_sound: String,
    click_disabled_sound: String,
    previous_production_tab_key: String,
    next_production_tab_key: String,
    pub groups: IndexMap<String, ProductionTabGroup>,
    pub arrow_button: String,
    pub tab_button: String,
    pub background: String,
    pub decorations: String,
    /// `Decorations` at `Initialize`, where C# caches the arrow images, so later suffixes do not reach them.
    arrow_decorations: String,
    decoration_scroll_left: String,
    decoration_scroll_right: String,
    tab_color: Color,
    tab_color_done: Color,
    content_width: i32,
    list_offset: f32,
    left_pressed: bool,
    right_pressed: bool,
    left_button_rect: Rect,
    right_button_rect: Rect,
    queue_group: Option<String>,
    cached_enabled_states: Vec<(Rc<ProductionQueue>, bool)>,
    /// Set by the `ActorAdded` and `ActorRemoved` subscriptions of `ProductionTabsLogic`, which runs `ActorChanged`
    /// on its next tick, where it can reach the palette.
    pub actors_changed: Rc<Cell<bool>>,
    /// `IsVisible`: a queue group with tabs is selected. Updated on every tick.
    pub visible: Rc<Cell<bool>>,
    local_player: Option<rustora_engine::player::PlayerRef>,
    width: i32,
}

impl ProductionTabsWidget {
    pub fn new(assets: &Assets) -> ProductionTabsWidget {
        ProductionTabsWidget {
            world: None,
            palette_widget: String::new(),
            types_container: String::new(),
            background_container: None,
            tab_width: 30,
            arrow_width: 20,
            click_sound: assets.metrics.string("ClickSound"),
            click_disabled_sound: assets.metrics.string("ClickDisabledSound"),
            previous_production_tab_key: String::new(),
            next_production_tab_key: String::new(),
            groups: IndexMap::new(),
            arrow_button: "button".into(),
            tab_button: "button".into(),
            background: "panel-black".into(),
            decorations: "scrollpanel-decorations".into(),
            arrow_decorations: String::new(),
            decoration_scroll_left: "left".into(),
            decoration_scroll_right: "right".into(),
            tab_color: Color::from_argb(255, 255, 255, 255),
            tab_color_done: Color::from_argb(255, 255, 215, 0),
            content_width: 0,
            list_offset: 0.0,
            left_pressed: false,
            right_pressed: false,
            left_button_rect: Rect::new(0, 0, 0, 0),
            right_button_rect: Rect::new(0, 0, 0, 0),
            queue_group: None,
            cached_enabled_states: Vec::new(),
            actors_changed: Rc::new(Cell::new(true)),
            visible: Rc::new(Cell::new(false)),
            local_player: None,
            width: 0,
        }
    }

    fn update_visible(&self) {
        self.visible.set(
            self.queue_group
                .as_ref()
                .is_some_and(|g| self.groups.get(g).is_some_and(|g| !g.tabs.is_empty())),
        );
    }

    pub fn queue_group(&self) -> Option<&str> {
        self.queue_group.as_deref()
    }

    fn palette(&self, ui: &Ui) -> Option<WidgetId> {
        ui.find(ui.root, &self.palette_widget)
    }

    /// `CurrentQueue`, the queue of the palette.
    pub fn current_queue(&self, ui: &Ui) -> Option<Rc<ProductionQueue>> {
        let palette = self.palette(ui)?;
        ui.get::<ProductionPaletteWidget>(palette).current_queue()
    }

    fn set_current_queue(&mut self, ui: &mut Ui, queue: Option<Rc<ProductionQueue>>) {
        if let Some(palette) = self.palette(ui) {
            let kind = ui.kind(palette);
            let mut kind = kind.borrow_mut();
            if let Some(p) = kind.as_any().downcast_mut::<ProductionPaletteWidget>() {
                p.set_current_queue(ui, palette, queue.clone());
            }
        }
        self.queue_group = queue.map(|q| queue_group(&q));
        self.update_visible();
    }

    /// The `QueueGroup` setter.
    pub fn set_queue_group(&mut self, ui: &mut Ui, group: Option<String>) {
        self.list_offset = 0.0;
        self.queue_group = group;
        self.select_next_tab(ui, false);
        self.update_visible();
    }

    /// `SelectNextTab`: the next queue of the group, queues with finished items first.
    pub fn select_next_tab(&mut self, ui: &mut Ui, reverse: bool) -> bool {
        let Some(group) = self.queue_group.clone() else {
            return true;
        };
        let mut queues: Vec<Rc<ProductionQueue>> = self.groups[&group].tabs.iter().map(|t| t.queue.clone()).collect();
        queues.sort_by_key(|q| std::cmp::Reverse(q.all_queued().iter().any(|i| i.done.get()) as i32));
        if reverse {
            queues.reverse();
        }
        let current = self.current_queue(ui);
        let next = queues
            .iter()
            .skip_while(|q| current.as_ref().is_none_or(|c| !Rc::ptr_eq(q, c)))
            .nth(1)
            .or(queues.first())
            .cloned();
        self.set_current_queue(ui, next);
        true
    }

    /// `PickUpCompletedBuilding` of the palette.
    pub fn pick_up_completed_building(&self, ui: &Ui) {
        if let Some(palette) = self.palette(ui) {
            ui.get::<ProductionPaletteWidget>(palette).pick_up_completed_building();
        }
    }

    /// `ActorChanged`: the queues of the local player that are in the world.
    pub fn actor_changed(&mut self, ui: &mut Ui) {
        let Some(world) = self.world.clone() else { return };
        let local = self.local_player.clone();
        let queues: Vec<Rc<ProductionQueue>> = world
            .actors_with_trait::<ProductionQueue>()
            .into_iter()
            .filter(|(a, _)| local.as_ref().is_some_and(|p| Rc::ptr_eq(&a.owner(), p)) && a.is_in_world())
            .map(|(_, q)| q)
            .collect();
        self.cached_enabled_states = queues.iter().map(|q| (q.clone(), q.enabled())).collect();
        for g in self.groups.values_mut() {
            g.update(&queues);
        }
        self.update_visible();
        let Some(group) = self.queue_group.clone() else { return };
        if self.groups[&group].tabs.is_empty() {
            let first = self.groups.iter().find(|(_, g)| !g.tabs.is_empty()).map(|(k, _)| k.clone());
            self.set_queue_group(ui, first);
        } else {
            let current = self.current_queue(ui);
            if !self.groups[&group]
                .tabs
                .iter()
                .any(|t| current.as_ref().is_some_and(|c| Rc::ptr_eq(&t.queue, c)))
            {
                self.select_next_tab(ui, false);
            }
        }
    }

    fn scroll(&mut self, ui: &Ui, amount: i32) {
        self.list_offset += amount as f32 * ui.ui_scroll_speed;
        let min = (self.width - self.right_button_rect.w - self.left_button_rect.w - self.content_width) as f32;
        self.list_offset = self.list_offset.max(min).min(0.0);
    }

    fn sound(&self, name: &str) {
        if let Some(world) = &self.world {
            rustora_engine::sound::play_notification(world, None, "Sounds", name, None);
        }
    }
}

impl WidgetKind for ProductionTabsWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "PaletteWidget" => self.palette_widget = value.to_string(),
            "TypesContainer" => self.types_container = value.to_string(),
            "BackgroundContainer" => self.background_container = Some(value.to_string()),
            "TabWidth" => self.tab_width = value.trim().parse().unwrap_or(30),
            "ArrowWidth" => self.arrow_width = value.trim().parse().unwrap_or(20),
            "ClickSound" => self.click_sound = value.to_string(),
            "ClickDisabledSound" => self.click_disabled_sound = value.to_string(),
            "PreviousProductionTabKey" => self.previous_production_tab_key = value.to_string(),
            "NextProductionTabKey" => self.next_production_tab_key = value.to_string(),
            "ArrowButton" => self.arrow_button = value.to_string(),
            "TabButton" => self.tab_button = value.to_string(),
            "Background" => self.background = value.to_string(),
            "Decorations" => self.decorations = value.to_string(),
            "DecorationScrollLeft" => self.decoration_scroll_left = value.to_string(),
            "DecorationScrollRight" => self.decoration_scroll_right = value.to_string(),
            "TabColor" => self.tab_color = Color::parse(value).unwrap_or(self.tab_color),
            "TabColorDone" => self.tab_color_done = Color::parse(value).unwrap_or(self.tab_color_done),
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, id: WidgetId) {
        let ingame = services(ui).ingame.borrow().clone();
        self.world = ingame.as_ref().map(|i| i.world.clone());
        self.local_player = ingame.and_then(|i| i.local_player.clone());
        if let Some(world) = &self.world {
            for (_, actor) in world.rules().actors() {
                for q in actor.trait_infos::<ProductionQueueInfo>() {
                    let group = info_group(&q);
                    self.groups
                        .entry(group.clone())
                        .or_insert_with(|| ProductionTabGroup { tabs: Vec::new(), group });
                }
            }
        }
        self.arrow_decorations = self.decorations.clone();
        let rb = ui.render_bounds(id);
        self.width = rb.w;
        self.left_button_rect = Rect::new(rb.x, rb.y, self.arrow_width, rb.h);
        self.right_button_rect = Rect::new(rb.right() - self.arrow_width, rb.y, self.arrow_width, rb.h);
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let Some(group) = self.queue_group.clone() else { return };
        let tabs: Vec<ProductionTab> = self.groups[&group]
            .tabs
            .iter()
            .filter(|t| !t.queue.buildable_items().is_empty())
            .cloned()
            .collect();
        if tabs.is_empty() {
            return;
        }
        let rb = ui.render_bounds(id);
        let (mx, my) = ui.last_mouse;
        let over = ui.mouse_over == Some(id);
        let left_disabled = self.list_offset >= 0.0;
        let left_hover = over && self.left_button_rect.contains(mx, my);
        let right_disabled = self.list_offset <= (rb.w - self.right_button_rect.w - self.left_button_rect.w - self.content_width) as f32;
        let right_hover = over && self.right_button_rect.contains(mx, my);

        dc.draw_panel(&self.background, rb);
        draw_background(
            dc,
            &self.arrow_button,
            self.left_button_rect,
            left_disabled,
            self.left_pressed,
            left_hover,
            false,
        );
        draw_background(
            dc,
            &self.arrow_button,
            self.right_button_rect,
            right_disabled,
            self.right_pressed,
            right_hover,
            false,
        );
        for (rect, image, state) in [
            (
                self.left_button_rect,
                &self.decoration_scroll_left,
                (left_disabled, self.left_pressed, left_hover, false, false),
            ),
            (
                self.right_button_rect,
                &self.decoration_scroll_right,
                (right_disabled, self.right_pressed, right_hover, false, false),
            ),
        ] {
            if let Some(s) = stateful_image(dc, &self.arrow_decorations, image, state) {
                let (w, h) = (s.w, s.h);
                dc.draw_sprite(s, (rect.x + (rect.w - w) / 2) as f32, (rect.y + (rect.h - h) / 2) as f32);
            }
        }

        dc.push_clip(Rect::new(
            self.left_button_rect.right(),
            rb.y + 1,
            self.right_button_rect.x - self.left_button_rect.right() - 1,
            rb.h,
        ));
        let origin = (self.left_button_rect.right() - 1 + self.list_offset as i32, self.left_button_rect.y);
        self.content_width = 0;
        let current = self.current_queue(ui);
        for tab in &tabs {
            let rect = Rect::new(origin.0 + self.content_width, origin.1, self.tab_width, rb.h);
            let hover = !left_hover && !right_hover && over && rect.contains(mx, my);
            let highlighted = current.as_ref().is_some_and(|c| Rc::ptr_eq(c, &tab.queue));
            draw_background(dc, &self.tab_button, rect, false, false, hover, highlighted);
            self.content_width += self.tab_width - 1;
            let (tw, th) = dc.measure("TinyBold", &tab.name);
            let color = if tab.queue.all_queued().iter().any(|i| i.done.get()) {
                self.tab_color_done
            } else {
                self.tab_color
            };
            dc.draw_text(
                "TinyBold",
                &tab.name,
                rect.x + (rect.w - tw) / 2,
                rect.y + (rect.h - th) / 2,
                color,
                TextStyle::Contrast {
                    dark: Color::from_argb(255, 0, 0, 0),
                    light: Color::from_argb(255, 0, 0, 0),
                    radius: 1,
                },
            );
        }
        dc.pop_clip();
    }

    fn tick(&mut self, ui: &mut Ui, _id: WidgetId) {
        if self.left_pressed {
            self.scroll(ui, 1);
        }
        if self.right_pressed {
            self.scroll(ui, -1);
        }
        if self.cached_enabled_states.iter().any(|(q, enabled)| q.enabled() != *enabled) {
            let queues: Vec<Rc<ProductionQueue>> = self.cached_enabled_states.iter().map(|(q, _)| q.clone()).collect();
            for g in self.groups.values_mut() {
                g.update(&queues);
            }
        }
        self.update_visible();
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, _mi: Option<&MouseInput>) -> bool {
        self.left_pressed = false;
        self.right_pressed = false;
        if ui.mouse_focus == Some(id) {
            ui.mouse_focus = None;
        }
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        if mi.event == MouseEvent::Scroll {
            self.scroll(ui, mi.scroll_delta);
            return true;
        }
        if mi.button != MouseButton::Left {
            return true;
        }
        if mi.event == MouseEvent::Down && !ui.take_mouse_focus(id, mi) {
            return true;
        }
        if !ui.has_mouse_focus(id) {
            return true;
        }
        if mi.event == MouseEvent::Up {
            return self.yield_mouse_focus(ui, id, Some(mi));
        }
        self.left_pressed = self.left_button_rect.contains(mi.x, mi.y);
        self.right_pressed = self.right_button_rect.contains(mi.x, mi.y);
        let left_disabled = self.list_offset >= 0.0;
        let right_disabled =
            self.list_offset <= (self.width - self.right_button_rect.w - self.left_button_rect.w - self.content_width) as f32;
        if self.left_pressed || self.right_pressed {
            if (self.left_pressed && !left_disabled) || (self.right_pressed && !right_disabled) {
                self.sound(&self.click_sound);
            } else {
                self.sound(&self.click_disabled_sound);
            }
        }
        let offset = mi.x - (self.left_button_rect.right() - 1 + self.list_offset as i32);
        if offset > 0
            && offset < self.content_width
            && let Some(group) = self.queue_group.clone()
        {
            let queue = self.groups[&group]
                .tabs
                .get((offset / (self.tab_width - 1)) as usize)
                .map(|t| t.queue.clone());
            if queue.is_some() {
                self.set_current_queue(ui, queue);
                self.sound(&self.click_sound);
            }
        }
        true
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        if e.event != KeyEvent::Down {
            return false;
        }
        let (previous, next) = {
            let hotkeys = &ui.assets.borrow().hotkeys;
            (
                !self.previous_production_tab_key.is_empty() && hotkeys.is_activated(&self.previous_production_tab_key, e),
                !self.next_production_tab_key.is_empty() && hotkeys.is_activated(&self.next_production_tab_key, e),
            )
        };
        if previous || next {
            self.sound(&self.click_sound);
            return self.select_next_tab(ui, previous);
        }
        false
    }
}
