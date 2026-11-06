// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `SupportPowersWidget.cs` and `SelectGenericPowerTarget`. Every power targets through the generic
//! target selection for now, so the chronoshift second target and the directional targets of air powers are
//! missing; sounds, speech and icon overlay texts are not ported.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::CPos;
use rustora_engine::order::Order;
use rustora_engine::target::Target;
use rustora_engine::traits::subcell::FULL_CELL;
use rustora_mods_common::support_powers::{SupportPowerInstance, SupportPowerManager};

use super::TooltipContainerWidget;
use super::impl_as_any;
use super::production::format_time;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{KeyEvent, KeyInput, MouseButton, MouseEvent, MouseInput};
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::orders::{OrderContext, OrderGenerator};
use crate::widget::{Ui, WidgetArgs, WidgetId, WidgetKind};

/// `SelectGenericPowerTarget`.
pub struct SelectGenericPowerTarget {
    manager: Rc<SupportPowerManager>,
    order_key: String,
    cursor: String,
    blocked_cursor: String,
}

impl OrderGenerator for SelectGenericPowerTarget {
    fn order(&self, ctx: &OrderContext, cell: CPos, _world_px: (i32, i32), mi: &MouseInput) -> Vec<Order> {
        let left_down = mi.button == MouseButton::Left && mi.event == MouseEvent::Down;
        let right_up = mi.button == MouseButton::Right && mi.event == MouseEvent::Up;
        if !left_down && !right_up {
            return Vec::new();
        }
        ctx.cancel_input_mode();
        if mi.button == MouseButton::Left && ctx.world.map().contains(cell) {
            let target = Target::from_cell(ctx.world, cell, FULL_CELL);
            return vec![Order::new(&self.order_key, Some(self.manager.self_.clone()), target, false)];
        }
        Vec::new()
    }

    fn tick(&self, ctx: &OrderContext) {
        let usable = self
            .manager
            .powers
            .borrow()
            .get(&self.order_key)
            .is_some_and(|p| p.active() && p.ready());
        if !usable {
            ctx.cancel_input_mode();
        }
    }

    fn get_cursor(&self, ctx: &OrderContext, cell: CPos, _world_px: (i32, i32), _mi: &MouseInput) -> Option<String> {
        Some(if ctx.world.map().contains(cell) {
            self.cursor.clone()
        } else {
            self.blocked_cursor.clone()
        })
    }
}

/// `SupportPowersWidget.SupportPowerIcon`.
#[derive(Clone)]
pub struct SupportPowerIcon {
    pub power: Rc<SupportPowerInstance>,
    pub pos: (i32, i32),
    pub image: String,
    pub sequence: String,
    pub palette: String,
    pub hotkey: Option<String>,
}

/// Port of `SupportPowersWidget`.
#[derive(Clone)]
pub struct SupportPowersWidget {
    ready_text: String,
    hold_text: String,
    overlay_font: String,
    pub icon_size: (i32, i32),
    pub icon_margin: i32,
    icon_sprite_offset: (i32, i32),
    tooltip_container: Option<String>,
    tooltip_template: String,
    hotkey_prefix: String,
    hotkey_count: i32,
    clock_animation: String,
    clock_sequence: String,
    clock_palette: String,
    pub horizontal: bool,
    pub icon_count: i32,
    pub on_icon_count_changed: Option<Rc<dyn Fn(&mut Ui, i32, i32)>>,
    pub tooltip_icon: Option<SupportPowerIcon>,
    ingame: Option<Rc<Ingame>>,
    manager: Option<Rc<SupportPowerManager>>,
    icons: Vec<(Rect, SupportPowerIcon)>,
    icon_offset: (f32, f32),
    hold_offset: (f32, f32),
    ready_offset: (f32, f32),
    time_offset: (f32, f32),
}

impl Default for SupportPowersWidget {
    fn default() -> Self {
        SupportPowersWidget {
            ready_text: String::new(),
            hold_text: String::new(),
            overlay_font: "TinyBold".into(),
            icon_size: (64, 48),
            icon_margin: 10,
            icon_sprite_offset: (0, 0),
            tooltip_container: None,
            tooltip_template: "SUPPORT_POWER_TOOLTIP".into(),
            hotkey_prefix: String::new(),
            hotkey_count: 0,
            clock_animation: "clock".into(),
            clock_sequence: "idle".into(),
            clock_palette: "chrome".into(),
            horizontal: false,
            icon_count: 0,
            on_icon_count_changed: None,
            tooltip_icon: None,
            ingame: None,
            manager: None,
            icons: Vec::new(),
            icon_offset: (0.0, 0.0),
            hold_offset: (0.0, 0.0),
            ready_offset: (0.0, 0.0),
            time_offset: (0.0, 0.0),
        }
    }
}

fn parse_int2(value: &str) -> (i32, i32) {
    let mut parts = value.split(',').map(|p| p.trim().parse::<i32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

impl SupportPowersWidget {
    /// `RefreshIcons`.
    fn refresh_icons(&mut self, ui: &mut Ui, id: WidgetId) {
        self.icons.clear();
        let Some(manager) = self.manager.clone() else {
            return;
        };
        let mut powers: Vec<(Rc<SupportPowerInstance>, i32)> = manager
            .powers
            .borrow()
            .values()
            .filter(|p| !p.disabled())
            .filter_map(|p| p.info().map(|i| (p.clone(), i.support_power_palette_order)))
            .collect();
        powers.sort_by_key(|(_, order)| *order);
        let old = self.icon_count;
        self.icon_count = 0;
        let rb = ui.render_bounds(id);
        let mut union: Option<Rect> = None;
        for (p, _) in powers {
            let Some(info) = p.info() else { continue };
            let rect = if self.horizontal {
                Rect::new(
                    rb.x + self.icon_count * (self.icon_size.0 + self.icon_margin),
                    rb.y,
                    self.icon_size.0,
                    self.icon_size.1,
                )
            } else {
                Rect::new(
                    rb.x,
                    rb.y + self.icon_count * (self.icon_size.1 + self.icon_margin),
                    self.icon_size.0,
                    self.icon_size.1,
                )
            };
            let hotkey = (self.icon_count < self.hotkey_count).then(|| format!("{}{:02}", self.hotkey_prefix, self.icon_count + 1));
            self.icons.push((
                rect,
                SupportPowerIcon {
                    power: p,
                    pos: (rect.x, rect.y),
                    image: info.icon_image.clone(),
                    sequence: info.icon.clone().unwrap_or_default(),
                    palette: info.icon_palette.clone(),
                    hotkey,
                },
            ));
            union = Some(match union {
                None => rect,
                Some(u) => {
                    let (l, t) = (u.x.min(rect.x), u.y.min(rect.y));
                    Rect::new(l, t, u.right().max(rect.right()) - l, u.bottom().max(rect.bottom()) - t)
                }
            });
            self.icon_count += 1;
        }
        ui.node_mut(id).event_rect = Some(union.unwrap_or_default());
        if old != self.icon_count
            && let Some(f) = self.on_icon_count_changed.clone()
        {
            let new = self.icon_count;
            ui.defer_for(id, move |ui| f(ui, old, new));
        }
    }

    /// `ClickIcon` and `SupportPowerInstance.Target`.
    fn click_icon(&self, icon: &SupportPowerIcon) {
        let (Some(ingame), Some(manager)) = (&self.ingame, &self.manager) else {
            return;
        };
        if !icon.power.active() {
            return;
        }
        let Some(instance) = icon.power.target_instance() else {
            return;
        };
        instance.notify_power_event(
            &instance.info.select_target_sound,
            &instance.info.select_target_speech_notification,
            &instance.info.select_target_text_notification,
        );
        if instance.info.kind.targets_immediately() {
            ingame
                .world
                .issue_order(Order::new(&icon.power.key, Some(manager.self_.clone()), Target::Invalid, false));
            return;
        }
        ingame.set_order_generator(Rc::new(SelectGenericPowerTarget {
            manager: manager.clone(),
            order_key: icon.power.key.clone(),
            cursor: instance.info.cursor.clone(),
            blocked_cursor: instance.info.blocked_cursor.clone(),
        }));
    }
}

impl WidgetKind for SupportPowersWidget {
    impl_as_any!();

    fn event_bounds(&self) -> crate::widget::EventBounds {
        crate::widget::EventBounds::Own
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ReadyText" => self.ready_text = value.to_string(),
            "HoldText" => self.hold_text = value.to_string(),
            "OverlayFont" => self.overlay_font = value.to_string(),
            "IconSize" => self.icon_size = parse_int2(value),
            "IconMargin" => self.icon_margin = value.trim().parse().unwrap_or(10),
            "IconSpriteOffset" => self.icon_sprite_offset = parse_int2(value),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            "HotkeyPrefix" => self.hotkey_prefix = value.to_string(),
            "HotkeyCount" => self.hotkey_count = value.trim().parse().unwrap_or(0),
            "ClockAnimation" => self.clock_animation = value.to_string(),
            "ClockSequence" => self.clock_sequence = value.to_string(),
            "ClockPalette" => self.clock_palette = value.to_string(),
            "Horizontal" => self.horizontal = super::parse_bool(value),
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.ingame = services(ui).ingame.borrow().clone();
        self.manager = self
            .ingame
            .as_ref()
            .and_then(|i| i.local_player.as_ref())
            .and_then(|p| p.player_actor().trait_or_default::<SupportPowerManager>());
        let assets = ui.assets.clone();
        let mut assets = assets.borrow_mut();
        self.icon_offset = (
            0.5 * self.icon_size.0 as f32 + self.icon_sprite_offset.0 as f32,
            0.5 * self.icon_size.1 as f32 + self.icon_sprite_offset.1 as f32,
        );
        let centered = |size: (i32, i32), o: (f32, f32)| (o.0 - size.0 as f32 / 2.0, o.1 - size.1 as f32 / 2.0);
        self.hold_text = assets.messages.get(&self.hold_text);
        self.ready_text = assets.messages.get(&self.ready_text);
        self.hold_offset = centered(assets.measure(&self.overlay_font, &self.hold_text), self.icon_offset);
        self.ready_offset = centered(assets.measure(&self.overlay_font, &self.ready_text), self.icon_offset);
        let timestep = self.ingame.as_ref().map_or(40, |i| i.world.timestep);
        self.time_offset = centered(assets.measure(&self.overlay_font, &format_time(0, timestep)), self.icon_offset);
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        self.refresh_icons(ui, id);
    }

    fn mouse_entered(&mut self, ui: &mut Ui, id: WidgetId) {
        let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) else {
            return;
        };
        let get_icon = super::tooltip::kind_getter(ui, id, |w: &SupportPowersWidget| w.tooltip_icon.clone());
        let mut args = WidgetArgs::default().with_value("getTooltipIcon", get_icon);
        if let Some(manager) = &self.manager {
            args = args.with_value("player", manager.self_.owner());
        }
        TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }

    fn handle_mouse(&mut self, _ui: &mut Ui, _id: WidgetId, mi: &MouseInput) -> bool {
        let icon_at = |icons: &[(Rect, SupportPowerIcon)]| icons.iter().find(|(r, _)| r.contains(mi.x, mi.y)).map(|(_, i)| i.clone());
        if mi.event == MouseEvent::Move {
            self.tooltip_icon = icon_at(&self.icons);
            return false;
        }
        if mi.event != MouseEvent::Down {
            return false;
        }
        let clicked = icon_at(&self.icons);
        if let Some(icon) = clicked {
            self.click_icon(&icon);
        }
        true
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        if e.event != KeyEvent::Down {
            return false;
        }
        let icon = {
            let assets = ui.assets.borrow();
            self.icons
                .iter()
                .map(|(_, i)| i)
                .find(|i| i.hotkey.as_deref().is_some_and(|h| assets.hotkeys.is_activated(h, e)))
                .cloned()
        };
        match icon {
            Some(icon) => {
                self.click_icon(&icon);
                true
            }
            None => false,
        }
    }

    fn draw(&mut self, _ui: &mut Ui, _id: WidgetId, dc: &mut DrawContext) {
        let Some(ingame) = self.ingame.clone() else {
            return;
        };
        let sprites = ingame.world_sprites.clone();
        let clock_length = sprites.length(dc.renderer, &self.clock_animation, &self.clock_sequence);
        for (_, p) in &self.icons {
            let pos = (p.pos.0 as f32 + self.icon_offset.0, p.pos.1 as f32 + self.icon_offset.1);
            if let Some(f) = sprites.frame(dc.renderer, &p.image, &p.sequence, 0, &p.palette) {
                dc.draw_frame_centered(f, pos);
            }
            let sp = &p.power;
            let frame = if sp.total_ticks == 0 {
                clock_length - 1
            } else {
                (sp.total_ticks - sp.remaining_ticks()) * (clock_length - 1) / sp.total_ticks
            };
            if let Some(f) = sprites.frame(dc.renderer, &self.clock_animation, &self.clock_sequence, frame, &self.clock_palette) {
                dc.draw_frame_centered(f, pos);
            }
        }
        let white = Color::from_argb(255, 255, 255, 255);
        let black = Color::from_argb(255, 0, 0, 0);
        let style = TextStyle::Contrast {
            dark: black,
            light: black,
            radius: 1,
        };
        for (_, p) in &self.icons {
            let at = |o: (f32, f32)| (p.pos.0 + o.0 as i32, p.pos.1 + o.1 as i32);
            if p.power.ready() {
                let (x, y) = at(self.ready_offset);
                dc.draw_text(&self.overlay_font, &self.ready_text, x, y, white, style);
            } else if !p.power.active() {
                let (x, y) = at(self.hold_offset);
                dc.draw_text(&self.overlay_font, &self.hold_text, x, y, white, style);
            } else {
                let (x, y) = at(self.time_offset);
                let text = format_time(p.power.remaining_ticks(), ingame.world.timestep);
                dc.draw_text(&self.overlay_font, &text, x, y, white, style);
            }
        }
    }
}
