// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `LogicTickerWidget.cs`, `ProductionTypeButtonWidget.cs` and `ProductionPaletteWidget.cs`.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::order::Order;
use rustora_engine::target::Target;
use rustora_mods_common::player::PlayerResources;
use rustora_mods_common::previews::{ProductionIconOverlayManagerInfo, WithProductionIconOverlayInfo};
use rustora_mods_common::production::{BuildableInfo, ProductionItem, ProductionQueue};
use rustora_mods_common::render::RenderSpritesInfo;

use super::{ButtonWidget, TooltipContainerWidget, impl_as_any};
use crate::assets::Assets;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::input::{KeyEvent, KeyInput, Modifiers, MouseButton, MouseEvent, MouseInput};
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::widget::{Action, Ui, WidgetArgs, WidgetId, WidgetKind};

/// Port of `LogicTickerWidget`.
#[derive(Clone, Default)]
pub struct LogicTickerWidget {
    pub on_tick: Option<Action>,
}

impl WidgetKind for LogicTickerWidget {
    impl_as_any!();

    /// Runs `OnTick` deferred, and only if this ticker was not removed in between, since ids get reused.
    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        if let Some(f) = self.on_tick.clone() {
            ui.defer_for(id, move |ui| f(ui));
        }
    }

    fn ignores_mouse_over(&self) -> bool {
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}

/// Port of `LogicKeyListenerWidget`.
#[derive(Clone, Default)]
pub struct LogicKeyListenerWidget {
    pub handlers: Vec<Rc<dyn Fn(&mut Ui, &KeyInput) -> bool>>,
}

impl WidgetKind for LogicKeyListenerWidget {
    impl_as_any!();

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        self.handlers.iter().any(|h| h(ui, e))
    }

    fn ignores_mouse_over(&self) -> bool {
        true
    }

    fn cursor(&self, _default: &str) -> Option<String> {
        None
    }
}

/// Port of `ProductionTypeButtonWidget`: a button with a production group.
#[derive(Clone)]
pub struct ProductionTypeButtonWidget {
    pub button: ButtonWidget,
    pub production_group: String,
}

impl ProductionTypeButtonWidget {
    pub fn new(assets: &Assets) -> ProductionTypeButtonWidget {
        ProductionTypeButtonWidget {
            button: ButtonWidget::new(assets),
            production_group: String::new(),
        }
    }
}

impl WidgetKind for ProductionTypeButtonWidget {
    impl_as_any!();

    fn as_button(&mut self) -> Option<&mut ButtonWidget> {
        Some(&mut self.button)
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ProductionGroup" => self.production_group = value.to_string(),
            _ => return self.button.set_field_base(key, value),
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        self.button.draw(ui, id, dc);
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        self.button.handle_mouse(ui, id, mi)
    }

    fn handle_key(&mut self, ui: &mut Ui, id: WidgetId, e: &KeyInput) -> bool {
        self.button.handle_key(ui, id, e)
    }

    fn yield_mouse_focus(&mut self, ui: &mut Ui, id: WidgetId, mi: Option<&MouseInput>) -> bool {
        self.button.yield_mouse_focus(ui, id, mi)
    }

    fn mouse_entered(&mut self, ui: &mut Ui, id: WidgetId) {
        self.button.show_tooltip(ui, id);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.button.hide_tooltip(ui);
    }

    fn cursor(&self, default: &str) -> Option<String> {
        self.button.cursor(default)
    }
}

/// `ProductionIcon`.
#[derive(Clone)]
pub struct ProductionIcon {
    pub actor: Rc<ActorInfo>,
    pub name: String,
    pub hotkey: Option<String>,
    pub image: String,
    pub sequence: String,
    pub palette: String,
    pub pos: (i32, i32),
    pub queued: Vec<Rc<ProductionItem>>,
    pub queue: Rc<ProductionQueue>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadyTextStyle {
    Solid,
    AlternatingColor,
    Blinking,
}

/// Port of `ProductionPaletteWidget`.
#[derive(Clone)]
pub struct ProductionPaletteWidget {
    ready_text_style: ReadyTextStyle,
    text_color: Color,
    ready_text_alt_color: Color,
    pub columns: i32,
    pub icon_size: (i32, i32),
    icon_margin: (i32, i32),
    icon_sprite_offset: (i32, i32),
    queued_offset: (i32, i32),
    tooltip_container: Option<String>,
    tooltip_template: String,
    hotkey_prefix: String,
    hotkey_count: i32,
    select_production_building_hotkey: String,
    clock_animation: String,
    clock_sequence: String,
    clock_palette: String,
    not_buildable_animation: String,
    not_buildable_sequence: String,
    not_buildable_palette: String,
    overlay_font: String,
    symbols_font: String,
    draw_time: bool,
    ready_text: String,
    hold_text: String,
    infinite_symbol: String,
    click_sound: String,
    click_disabled_sound: String,
    pub displayed_icon_count: i32,
    pub total_icon_count: i32,
    pub on_icon_count_changed: Option<Rc<dyn Fn(&mut Ui, i32, i32)>>,
    pub tooltip_icon: Option<ProductionIcon>,
    pub minimum_rows: i32,
    pub maximum_rows: i32,
    pub icon_row_offset: i32,
    pub max_icon_row_offset: i32,
    ingame: Option<Rc<Ingame>>,
    current_queue: Option<Rc<ProductionQueue>>,
    icons: Vec<(Rect, ProductionIcon)>,
    icon_offset: (f32, f32),
    hold_offset: (f32, f32),
    ready_offset: (f32, f32),
    time_offset: (f32, f32),
    infinite_offset: (f32, f32),
    time_text_size: (i32, i32),
}

impl ProductionPaletteWidget {
    /// `IProductionIconOverlay` of the queue owner that are active for the actor `name`.
    pub(crate) fn active_overlays(queue: &ProductionQueue, name: &str) -> Vec<Rc<ProductionIconOverlayManagerInfo>> {
        let owner = queue.actor.owner();
        let player = owner.player_actor();
        let world = queue.actor.world();
        let Some(ai) = world.rules().actor(name) else {
            return Vec::new();
        };
        let overlays = ai.trait_infos::<WithProductionIconOverlayInfo>();
        if overlays.is_empty() {
            return Vec::new();
        }
        let Some(tech_tree) = player.trait_or_default::<rustora_mods_common::production::TechTree>() else {
            return Vec::new();
        };
        player
            .info
            .trait_infos::<ProductionIconOverlayManagerInfo>()
            .into_iter()
            .filter(|m| {
                overlays
                    .iter()
                    .any(|o| o.types.contains(&m.ty) && tech_tree.has_prerequisites(&o.prerequisites))
            })
            .collect()
    }

    pub fn new(assets: &Assets) -> ProductionPaletteWidget {
        let _ = assets;
        ProductionPaletteWidget {
            click_sound: assets.metrics.string("ClickSound"),
            click_disabled_sound: assets.metrics.string("ClickDisabledSound"),
            ready_text_style: ReadyTextStyle::AlternatingColor,
            text_color: Color::from_argb(255, 255, 255, 255),
            ready_text_alt_color: Color::from_argb(255, 255, 215, 0),
            columns: 3,
            icon_size: (64, 48),
            icon_margin: (0, 0),
            icon_sprite_offset: (0, 0),
            queued_offset: (4, 2),
            tooltip_container: None,
            tooltip_template: "PRODUCTION_TOOLTIP".into(),
            hotkey_prefix: String::new(),
            hotkey_count: 0,
            select_production_building_hotkey: String::new(),
            clock_animation: "clock".into(),
            clock_sequence: "idle".into(),
            clock_palette: "chrome".into(),
            not_buildable_animation: "clock".into(),
            not_buildable_sequence: "idle".into(),
            not_buildable_palette: "chrome".into(),
            overlay_font: "TinyBold".into(),
            symbols_font: "Symbols".into(),
            draw_time: true,
            ready_text: String::new(),
            hold_text: String::new(),
            infinite_symbol: "\u{221E}".into(),
            displayed_icon_count: 0,
            total_icon_count: 0,
            on_icon_count_changed: None,
            tooltip_icon: None,
            minimum_rows: 4,
            maximum_rows: i32::MAX,
            icon_row_offset: 0,
            max_icon_row_offset: i32::MAX,
            ingame: None,
            current_queue: None,
            icons: Vec::new(),
            icon_offset: (0.0, 0.0),
            hold_offset: (0.0, 0.0),
            ready_offset: (0.0, 0.0),
            time_offset: (0.0, 0.0),
            infinite_offset: (0.0, 0.0),
            time_text_size: (0, 0),
        }
    }

    pub fn current_queue(&self) -> Option<Rc<ProductionQueue>> {
        self.current_queue.clone()
    }

    /// The `CurrentQueue` setter.
    pub fn set_current_queue(&mut self, ui: &mut Ui, id: WidgetId, queue: Option<Rc<ProductionQueue>>) {
        self.current_queue = queue;
        self.refresh_icons(ui, id);
    }

    pub fn can_scroll_down(&self) -> bool {
        let total_rows = (self.total_icon_count + self.columns - 1) / self.columns;
        self.icon_row_offset < total_rows - self.max_icon_row_offset
    }

    pub fn can_scroll_up(&self) -> bool {
        self.icon_row_offset > 0
    }

    pub fn scroll_down(&mut self) {
        if self.can_scroll_down() {
            self.icon_row_offset += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        if self.can_scroll_up() {
            self.icon_row_offset -= 1;
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.icon_row_offset = 0;
    }

    /// `AllBuildables`, ordered by `BuildPaletteOrder`.
    fn all_buildables(&self) -> Vec<Rc<ActorInfo>> {
        let Some(queue) = &self.current_queue else {
            return Vec::new();
        };
        let world = queue.actor.world();
        let mut items: Vec<Rc<ActorInfo>> = queue.all_items().iter().filter_map(|n| world.rules().actor(n)).collect();
        items.sort_by_key(|a| a.trait_info::<BuildableInfo>().map_or(0, |b| b.build_palette_order));
        items
    }

    /// `PickUpCompletedBuildingIcon`.
    fn pick_up_completed_building_icon(&self, item: Option<&Rc<ProductionItem>>) -> bool {
        let (Some(item), Some(queue), Some(ingame)) = (item, &self.current_queue, &self.ingame) else {
            return false;
        };
        let world = queue.actor.world();
        let is_building = world
            .rules()
            .actor(&item.item)
            .is_some_and(|a| a.has_trait_info::<rustora_mods_common::building::BuildingInfo>());
        if item.done.get() && is_building {
            let generator = crate::place_building::PlaceBuildingOrderGenerator::new(queue.clone(), &item.item);
            ingame.set_order_generator(Rc::new(generator));
            return true;
        }
        false
    }

    /// `PickUpCompletedBuilding`.
    pub fn pick_up_completed_building(&self) {
        if let Some(queue) = &self.current_queue {
            self.pick_up_completed_building_icon(queue.current_item().as_ref());
        }
    }

    fn issue(&self, order: Order) {
        if let Some(ingame) = &self.ingame {
            ingame.world.issue_order(order);
        }
    }

    fn production_order(&self, order: &str, item: &str, extra_data: u32, queued: bool) -> Option<Order> {
        let queue = self.current_queue.as_ref()?;
        let mut o = Order::new(order, Some(queue.actor.clone()), Target::Invalid, queued);
        o.target_string = Some(item.to_string());
        o.extra_data = extra_data;
        Some(o)
    }

    fn handle_left_click(&self, item: Option<&Rc<ProductionItem>>, icon: &ProductionIcon, count: u32, m: Modifiers) -> bool {
        if self.pick_up_completed_building_icon(item) {
            self.click_sound(&self.click_sound);
            return true;
        }
        let Some(queue) = &self.current_queue else {
            return false;
        };
        if item.is_some_and(|i| i.paused.get()) {
            self.click_sound(&self.click_sound);
            self.speech(&queue.info.queued_audio, &queue.info.queued_text_notification);
            self.issue_opt(self.production_order("PauseProduction", &icon.name, 0, false));
            return true;
        }
        if !queue.buildable_items().contains(&icon.name) {
            return false;
        }
        if queue.info.pay_up_front {
            let cash = queue
                .actor
                .owner()
                .player_actor()
                .trait_::<PlayerResources>()
                .get_cash_and_resources();
            if queue.get_production_cost(&icon.actor) > cash {
                return false;
            }
        }
        self.click_sound(&self.click_sound);
        let can_queue = queue.can_queue(&icon.actor);
        if queue.all_queued().is_empty() {
            if can_queue {
                self.speech(&queue.info.queued_audio, &queue.info.queued_text_notification);
            } else {
                self.speech(&queue.info.limited_audio, &queue.info.limited_text_notification);
            }
        }
        if can_queue {
            self.issue_opt(self.production_order("StartProduction", &icon.name, count, !m.ctrl));
            return true;
        }
        false
    }

    /// `Game.Sound.PlayNotification(World.Map.Rules, World.LocalPlayer, "Sounds", name, null)`.
    fn click_sound(&self, name: &str) {
        if let Some(ingame) = &self.ingame {
            rustora_engine::sound::play_notification(&ingame.world, ingame.local_player.as_ref(), "Sounds", name, None);
        }
    }

    /// The `PlayNotification` with the faction of the local player and the `AddTransientLine` that follow an action.
    fn speech(&self, notification: &Option<String>, text: &Option<String>) {
        let Some(ingame) = &self.ingame else { return };
        let Some(player) = ingame.local_player.as_ref() else { return };
        if let Some(n) = notification {
            rustora_engine::sound::play_notification(&ingame.world, Some(player), "Speech", n, Some(&player.faction));
        }
        ingame.world.sound.add_transient_line(Some(player), text.as_deref());
    }

    fn issue_opt(&self, order: Option<Order>) {
        if let Some(o) = order {
            self.issue(o);
        }
    }

    fn handle_right_click(&self, item: Option<&Rc<ProductionItem>>, icon: &ProductionIcon, count: u32) -> bool {
        let (Some(item), Some(queue)) = (item, &self.current_queue) else {
            return false;
        };
        self.click_sound(&self.click_sound);
        let cancel = queue.info.disallow_paused
            || item.paused.get()
            || item.done.get()
            || item.total_cost == item.remaining_cost.get()
            || !item.started.get();
        if cancel {
            self.speech(&queue.info.cancelled_audio, &queue.info.cancelled_text_notification);
            self.issue_opt(self.production_order("CancelProduction", &icon.name, count, false));
        } else {
            self.speech(&queue.info.on_hold_audio, &queue.info.on_hold_text_notification);
            self.issue_opt(self.production_order("PauseProduction", &icon.name, 1, false));
        }
        true
    }

    fn handle_middle_click(&self, item: Option<&Rc<ProductionItem>>, icon: &ProductionIcon, count: u32) -> bool {
        let (Some(_), Some(queue)) = (item, &self.current_queue) else {
            return false;
        };
        self.click_sound(&self.click_sound);
        self.speech(&queue.info.cancelled_audio, &queue.info.cancelled_text_notification);
        self.issue_opt(self.production_order("CancelProduction", &icon.name, count, false));
        true
    }

    /// `HandleEvent`.
    fn handle_event(&self, icon: &ProductionIcon, button: MouseButton, m: Modifiers) -> bool {
        let start_count = if m.shift { 5 } else { 1 };
        let cancel_count = match &self.current_queue {
            Some(q) if m.ctrl => q.all_queued().len() as u32,
            _ => start_count,
        };
        let item = icon.queued.first();
        let handled = match button {
            MouseButton::Left => self.handle_left_click(item, icon, start_count, m),
            MouseButton::Right => self.handle_right_click(item, icon, cancel_count),
            MouseButton::Middle => self.handle_middle_click(item, icon, cancel_count),
            MouseButton::None => false,
        };
        if !handled {
            self.click_sound(&self.click_disabled_sound);
        }
        true
    }

    /// `SelectProductionBuilding`.
    fn select_production_building(&self) -> bool {
        let (Some(queue), Some(ingame)) = (&self.current_queue, &self.ingame) else {
            return true;
        };
        let Some((facility, _)) = queue.most_likely_producer() else {
            return true;
        };
        let selection = &ingame.selection;
        let actors = selection.actors();
        if actors.len() == 1 && selection.contains(&facility) {
            let p = facility.center_position();
            let px = (p.x * 24 / 1024, (p.y - p.z) * 24 / 1024);
            ingame.viewport.borrow_mut().center_on(px);
        } else {
            selection.combine(&[facility], false, true);
        }
        self.click_sound(&self.click_sound);
        true
    }

    /// `RefreshIcons`.
    pub fn refresh_icons(&mut self, ui: &mut Ui, id: WidgetId) {
        self.icons.clear();
        let producer = self.current_queue.as_ref().and_then(|q| q.most_likely_producer());
        let (Some(queue), Some((producer_actor, producer))) = (self.current_queue.clone(), producer) else {
            if self.displayed_icon_count != 0 {
                self.icon_count_changed(ui, id, self.displayed_icon_count, 0);
                self.displayed_icon_count = 0;
            }
            ui.node_mut(id).event_rect = Some(Rect::default());
            return;
        };
        let old_count = self.displayed_icon_count;
        self.displayed_icon_count = 0;
        let rb = ui.render_bounds(id);
        let faction = producer.faction();
        let queued = queue.all_queued();
        let skip = (self.icon_row_offset * self.columns).max(0) as usize;
        let take = (self.max_icon_row_offset as i64 * self.columns as i64).clamp(0, i32::MAX as i64) as usize;
        let mut union: Option<Rect> = None;
        for item in self.all_buildables().into_iter().skip(skip).take(take) {
            let x = self.displayed_icon_count % self.columns;
            let y = self.displayed_icon_count / self.columns;
            let rect = Rect::new(
                rb.x + x * (self.icon_size.0 + self.icon_margin.0),
                rb.y + y * (self.icon_size.1 + self.icon_margin.1),
                self.icon_size.0,
                self.icon_size.1,
            );
            let image = item
                .trait_info::<RenderSpritesInfo>()
                .map_or_else(|| item.name.clone(), |rsi| rsi.get_image(&item, &faction));
            let bi = item.trait_info::<BuildableInfo>().expect("buildable");
            let palette = if bi.icon_palette_is_player_palette {
                format!("{}{}", bi.icon_palette, producer_actor.owner().internal_name)
            } else {
                bi.icon_palette.clone()
            };
            let hotkey = (self.displayed_icon_count < self.hotkey_count)
                .then(|| format!("{}{:02}", self.hotkey_prefix, self.displayed_icon_count + 1));
            let icon = ProductionIcon {
                name: item.name.clone(),
                hotkey,
                image,
                sequence: bi.icon.clone(),
                palette,
                pos: (rect.x, rect.y),
                queued: queued.iter().filter(|q| q.item == item.name).cloned().collect(),
                queue: queue.clone(),
                actor: item,
            };
            union = Some(match union {
                None => rect,
                Some(u) => {
                    let (l, t) = (u.x.min(rect.x), u.y.min(rect.y));
                    Rect::new(l, t, u.right().max(rect.right()) - l, u.bottom().max(rect.bottom()) - t)
                }
            });
            self.icons.push((rect, icon));
            self.displayed_icon_count += 1;
        }
        ui.node_mut(id).event_rect = Some(union.unwrap_or_default());
        if old_count != self.displayed_icon_count {
            self.icon_count_changed(ui, id, old_count, self.displayed_icon_count);
        }
    }

    fn icon_count_changed(&self, ui: &mut Ui, id: WidgetId, old: i32, new: i32) {
        if let Some(f) = self.on_icon_count_changed.clone() {
            ui.defer_for(id, move |ui| f(ui, old, new));
        }
    }

    fn icon_at(&self, x: i32, y: i32) -> Option<&ProductionIcon> {
        self.icons.iter().find(|(r, _)| r.contains(x, y)).map(|(_, i)| i)
    }
}

/// `WidgetUtils.FormatTime`.
pub fn format_time(ticks: i32, timestep: i32) -> String {
    let seconds = (ticks as f32 * timestep as f32 / 1000.0).ceil() as i32;
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds % 60)
    } else {
        format!("{:02}:{:02}", minutes, seconds % 60)
    }
}

fn parse_int2(value: &str) -> (i32, i32) {
    let mut parts = value.split(',').map(|p| p.trim().parse::<i32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

impl WidgetKind for ProductionPaletteWidget {
    impl_as_any!();

    fn event_bounds(&self) -> crate::widget::EventBounds {
        crate::widget::EventBounds::Own
    }

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ReadyTextStyle" => {
                self.ready_text_style = match value.trim() {
                    "Solid" => ReadyTextStyle::Solid,
                    "Blinking" => ReadyTextStyle::Blinking,
                    _ => ReadyTextStyle::AlternatingColor,
                }
            }
            "TextColor" => self.text_color = Color::parse(value).unwrap_or(self.text_color),
            "ReadyTextAltColor" => self.ready_text_alt_color = Color::parse(value).unwrap_or(self.ready_text_alt_color),
            "Columns" => self.columns = value.trim().parse().unwrap_or(3),
            "IconSize" => self.icon_size = parse_int2(value),
            "IconMargin" => self.icon_margin = parse_int2(value),
            "IconSpriteOffset" => self.icon_sprite_offset = parse_int2(value),
            "QueuedOffset" => self.queued_offset = parse_int2(value),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            "HotkeyPrefix" => self.hotkey_prefix = value.to_string(),
            "HotkeyCount" => self.hotkey_count = value.trim().parse().unwrap_or(0),
            "SelectProductionBuildingHotkey" => self.select_production_building_hotkey = value.to_string(),
            "ClockAnimation" => self.clock_animation = value.to_string(),
            "ClockSequence" => self.clock_sequence = value.to_string(),
            "ClockPalette" => self.clock_palette = value.to_string(),
            "NotBuildableAnimation" => self.not_buildable_animation = value.to_string(),
            "NotBuildableSequence" => self.not_buildable_sequence = value.to_string(),
            "NotBuildablePalette" => self.not_buildable_palette = value.to_string(),
            "OverlayFont" => self.overlay_font = value.to_string(),
            "SymbolsFont" => self.symbols_font = value.to_string(),
            "DrawTime" => self.draw_time = super::parse_bool(value),
            "ReadyText" => self.ready_text = value.to_string(),
            "HoldText" => self.hold_text = value.to_string(),
            "InfiniteSymbol" => self.infinite_symbol = value.to_string(),
            _ => return false,
        }
        true
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        self.ingame = services(ui).ingame.borrow().clone();
        let assets = ui.assets.clone();
        let mut assets = assets.borrow_mut();
        self.icon_offset = (
            0.5 * self.icon_size.0 as f32 + self.icon_sprite_offset.0 as f32,
            0.5 * self.icon_size.1 as f32 + self.icon_sprite_offset.1 as f32,
        );
        self.hold_text = assets.messages.get(&self.hold_text);
        self.ready_text = assets.messages.get(&self.ready_text);
        let centered = |size: (i32, i32), o: (f32, f32)| (o.0 - size.0 as f32 / 2.0, o.1 - size.1 as f32 / 2.0);
        self.hold_offset = centered(assets.measure(&self.overlay_font, &self.hold_text), self.icon_offset);
        self.ready_offset = centered(assets.measure(&self.overlay_font, &self.ready_text), self.icon_offset);
        let timestep = self.ingame.as_ref().map_or(40, |i| i.world.timestep);
        self.time_text_size = assets.measure(&self.overlay_font, &format_time(0, timestep));
        self.time_offset = centered(self.time_text_size, self.icon_offset);
        let infinite = assets.metrics.int2("InfiniteOffset");
        self.infinite_offset = (
            (infinite.0 + self.queued_offset.0) as f32,
            (infinite.1 + self.queued_offset.1) as f32,
        );
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        self.total_icon_count = self.all_buildables().len() as i32;
        if self.current_queue.as_ref().is_some_and(|q| !q.actor.is_in_world()) {
            self.current_queue = None;
            self.refresh_icons(ui, id);
        }
        if self.current_queue.is_some() {
            self.refresh_icons(ui, id);
        }
    }

    fn mouse_entered(&mut self, ui: &mut Ui, _id: WidgetId) {
        let Some(container) = self.tooltip_container.clone() else {
            return;
        };
        let Some(ingame) = self.ingame.clone() else {
            return;
        };
        let get_icon = super::tooltip::kind_getter(ui, _id, |w: &ProductionPaletteWidget| w.tooltip_icon.clone());
        let mut args = WidgetArgs::default().with_value("getTooltipIcon", get_icon);
        if let Some(player) = ingame.local_player.clone() {
            args = args.with_value("player", player);
        }
        let container = ui.child(ui.root, &container);
        TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
    }

    fn mouse_exited(&mut self, ui: &mut Ui, _id: WidgetId) {
        if let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) {
            TooltipContainerWidget::remove_tooltip(ui, container);
        }
    }

    fn handle_mouse(&mut self, ui: &mut Ui, _id: WidgetId, mi: &MouseInput) -> bool {
        let icon = self.icon_at(mi.x, mi.y).cloned();
        if mi.event == MouseEvent::Move {
            self.tooltip_icon = icon.clone();
        }
        if mi.event == MouseEvent::Scroll {
            if mi.scroll_delta < 0 && self.can_scroll_down() {
                self.scroll_down();
                self.click_sound(&self.click_sound);
                ui.reset_tooltips();
            } else if mi.scroll_delta > 0 && self.can_scroll_up() {
                self.scroll_up();
                self.click_sound(&self.click_sound);
                ui.reset_tooltips();
            }
        }
        let Some(icon) = icon else {
            return false;
        };
        if mi.event != MouseEvent::Down {
            return true;
        }
        self.handle_event(&icon, mi.button, mi.modifiers)
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        if e.event == KeyEvent::Up || self.current_queue.is_none() {
            return false;
        }
        let hotkeys = &ui.assets.borrow().hotkeys;
        if !self.select_production_building_hotkey.is_empty() && hotkeys.is_activated(&self.select_production_building_hotkey, e) {
            return self.select_production_building();
        }
        let batch = Modifiers {
            shift: e.modifiers.shift,
            ..Default::default()
        };
        let mut unshifted = e.clone();
        unshifted.modifiers.shift = false;
        let to_build = self
            .icons
            .iter()
            .map(|(_, i)| i)
            .find(|i| i.hotkey.as_deref().is_some_and(|h| hotkeys.is_activated(h, &unshifted)))
            .cloned();
        to_build.is_some_and(|icon| self.handle_event(&icon, MouseButton::Left, batch))
    }

    fn draw(&mut self, _ui: &mut Ui, _id: WidgetId, dc: &mut DrawContext) {
        let (Some(queue), Some(ingame)) = (self.current_queue.clone(), self.ingame.clone()) else {
            return;
        };
        let sprites = ingame.world_sprites.clone();
        let buildable = queue.buildable_items();
        let clock_length = sprites.length(dc.renderer, &self.clock_animation, &self.clock_sequence);
        for (_, icon) in &self.icons {
            let pos = (icon.pos.0 as f32 + self.icon_offset.0, icon.pos.1 as f32 + self.icon_offset.1);
            if let Some(f) = sprites.frame(dc.renderer, &icon.image, &icon.sequence, 0, &icon.palette) {
                dc.draw_frame_centered(f, pos);
            }
            for overlay in Self::active_overlays(&queue, &icon.name) {
                let Some(sequence) = overlay.sequence.as_deref() else {
                    continue;
                };
                if let Some(f) = sprites.frame(dc.renderer, &overlay.image, sequence, 0, &overlay.palette) {
                    let (w, h) = (f.sprite.w as f32, f.sprite.h as f32);
                    let offset = ((w - self.icon_size.0 as f32) / 2.0, (h - self.icon_size.1 as f32) / 2.0);
                    dc.draw_frame_centered(f, (pos.0 + offset.0, pos.1 + offset.1));
                }
            }
            if let Some(first) = icon.queued.first() {
                let total = first.total_time.get().max(1);
                let frame = (total - first.remaining_time.get()) * (clock_length - 1).max(0) / total;
                if let Some(f) = sprites.frame(dc.renderer, &self.clock_animation, &self.clock_sequence, frame, &self.clock_palette) {
                    dc.draw_frame_centered(f, pos);
                }
            } else if !buildable.contains(&icon.name) {
                let frame = sprites.frame(
                    dc.renderer,
                    &self.not_buildable_animation,
                    &self.not_buildable_sequence,
                    0,
                    &self.not_buildable_palette,
                );
                if let Some(f) = frame {
                    dc.draw_frame_centered(f, pos);
                }
            }
        }
        let black = Color::from_argb(255, 0, 0, 0);
        let style = TextStyle::Contrast {
            dark: black,
            light: black,
            radius: 1,
        };
        let world = &ingame.world;
        let blink_on = world.world_tick() * world.timestep / 360 % 2 == 0;
        for (_, icon) in &self.icons {
            let total = icon.queued.len();
            let Some(first) = icon.queued.first() else {
                continue;
            };
            let at = |o: (f32, f32)| (icon.pos.0 + o.0 as i32, icon.pos.1 + o.1 as i32);
            let waiting = !queue.is_producing(first) && !first.done.get();
            if first.done.get() {
                let (x, y) = at(self.ready_offset);
                if self.ready_text_style == ReadyTextStyle::Solid || blink_on {
                    dc.draw_text(&self.overlay_font, &self.ready_text, x, y, self.text_color, style);
                } else if self.ready_text_style == ReadyTextStyle::AlternatingColor {
                    dc.draw_text(&self.overlay_font, &self.ready_text, x, y, self.ready_text_alt_color, style);
                }
            } else if first.paused.get() {
                let (x, y) = at(self.hold_offset);
                dc.draw_text(&self.overlay_font, &self.hold_text, x, y, self.text_color, style);
            } else if !waiting && self.draw_time {
                let (x, y) = at(self.time_offset);
                let text = format_time(first.remaining_time_actual(), world.timestep);
                dc.draw_text(&self.overlay_font, &text, x, y, self.text_color, style);
            }
            if first.infinite.get() && dc.assets.fonts.contains_key(&self.symbols_font) {
                let (x, y) = at(self.infinite_offset);
                dc.draw_text(&self.symbols_font, &self.infinite_symbol, x, y, self.text_color, style);
            } else if total > 1 || waiting {
                let (x, y) = at((self.queued_offset.0 as f32, self.queued_offset.1 as f32));
                dc.draw_text(&self.overlay_font, &total.to_string(), x, y, self.text_color, style);
            }
        }
    }

    fn cursor(&self, default: &str) -> Option<String> {
        self.tooltip_icon.as_ref().map(|_| default.to_string())
    }
}
