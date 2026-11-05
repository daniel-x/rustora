// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `GradientColorBlockWidget.cs`, `LineGraphWidget.cs`, `ObserverProductionIconsWidget.cs`,
//! `ObserverSupportPowerIconsWidget.cs` and `ObserverArmyIconsWidget.cs`.

use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::player::PlayerRef;
use rustora_mods_common::player::statistics::PlayerStatistics;
use rustora_mods_common::production::{BuildableInfo, ProductionItem, ProductionQueue, ProductionQueueInfo};
use rustora_mods_common::render::RenderSpritesInfo;
use rustora_mods_common::support_powers::SupportPowerManager;

use super::TooltipContainerWidget;
use super::impl_as_any;
use super::production::{ProductionIcon, ProductionPaletteWidget, format_time};
use super::support_powers::SupportPowerIcon;
use super::tooltip::kind_getter;
use crate::draw::{DrawContext, Rect, TextStyle};
use crate::font::premultiplied;
use crate::logic::ingame::Ingame;
use crate::logic::services;
use crate::widget::{Getter, Ui, WidgetArgs, WidgetId, WidgetKind};
use crate::world_sprites::UiFrame;

const WHITE: Color = Color::from_argb(255, 255, 255, 255);
const BLACK: Color = Color::from_argb(255, 0, 0, 0);
const CONTRAST: TextStyle = TextStyle::Contrast {
    dark: BLACK,
    light: BLACK,
    radius: 1,
};

impl DrawContext<'_> {
    /// `WidgetUtils.DrawSpriteCentered` with a scale.
    pub fn draw_frame_centered_scaled(&mut self, f: UiFrame, pos: (f32, f32), scale: f32) {
        let s = f.sprite;
        let (w, h) = (s.w as f32 * scale, s.h as f32 * scale);
        self.draw_sprite_sized(s, pos.0 - w / 2.0 + f.offset.0 * scale, pos.1 - h / 2.0 + f.offset.1 * scale, w, h);
    }
}

fn parse_i32(value: &str, default: i32) -> i32 {
    value.trim().parse().unwrap_or(default)
}

/// Port of `GradientColorBlockWidget`.
#[derive(Clone, Default)]
pub struct GradientColorBlockWidget {
    pub top_left: Color,
    pub top_right: Color,
    pub bottom_right: Color,
    pub bottom_left: Color,
    pub get_top_left: Option<Getter<Color>>,
    pub get_top_right: Option<Getter<Color>>,
    pub get_bottom_right: Option<Getter<Color>>,
    pub get_bottom_left: Option<Getter<Color>>,
}

impl GradientColorBlockWidget {
    /// Sets all four color getters.
    pub fn set_colors(&mut self, tl: Getter<Color>, tr: Getter<Color>, br: Getter<Color>, bl: Getter<Color>) {
        self.get_top_left = Some(tl);
        self.get_top_right = Some(tr);
        self.get_bottom_right = Some(br);
        self.get_bottom_left = Some(bl);
    }
}

impl WidgetKind for GradientColorBlockWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        let target = match key {
            "TopLeftColor" => &mut self.top_left,
            "TopRightColor" => &mut self.top_right,
            "BottomRightColor" => &mut self.bottom_right,
            "BottomLeftColor" => &mut self.bottom_left,
            _ => return false,
        };
        *target = Color::parse(value).unwrap_or(*target);
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let get = |g: &Option<Getter<Color>>, c: Color| premultiplied(g.as_ref().map_or(c, |f| f()));
        let colors = [
            get(&self.get_top_left, self.top_left),
            get(&self.get_top_right, self.top_right),
            get(&self.get_bottom_right, self.bottom_right),
            get(&self.get_bottom_left, self.bottom_left),
        ];
        let r = ui.render_bounds(id);
        dc.list.gradient([r.x as f32, r.y as f32, r.w as f32, r.h as f32], colors);
    }
}

/// `LineGraphSeries`.
pub struct LineGraphSeries {
    pub key: String,
    pub color: Color,
    pub points: Vec<f32>,
}

/// `string.Format` with one argument for the `{0}` and `{0:F<n>}` placeholders of the graph formats.
fn format_value(format: &str, value: f32) -> String {
    let Some(start) = format.find("{0") else {
        return format.to_string();
    };
    let Some(len) = format[start..].find('}') else {
        return format.to_string();
    };
    let spec = &format[start + 2..start + len];
    let text = match spec.strip_prefix(":F").and_then(|d| d.parse::<usize>().ok()) {
        Some(decimals) => format!("{value:.decimals$}"),
        None => format!("{value}"),
    };
    format!("{}{}{}", &format[..start], text, &format[start + len + 1..])
}

/// Port of `LineGraphWidget`.
#[derive(Clone)]
pub struct LineGraphWidget {
    pub get_series: Option<Rc<dyn Fn() -> Vec<LineGraphSeries>>>,
    value_format: String,
    x_axis_value_format: String,
    y_axis_value_format: String,
    x_axis_size: i32,
    y_axis_size: i32,
    x_axis_ticks_per_label: i32,
    x_axis_label: String,
    y_axis_label: String,
    display_first_y_axis_value: bool,
    label_font: Option<String>,
    axis_font: String,
    background_dark: Color,
    background_light: Color,
    padding: i32,
}

impl LineGraphWidget {
    pub fn new(assets: &crate::assets::Assets) -> LineGraphWidget {
        LineGraphWidget {
            get_series: None,
            value_format: "{0}".into(),
            x_axis_value_format: "{0}".into(),
            y_axis_value_format: "{0}".into(),
            x_axis_size: 10,
            y_axis_size: 10,
            x_axis_ticks_per_label: 1,
            x_axis_label: String::new(),
            y_axis_label: String::new(),
            display_first_y_axis_value: false,
            label_font: None,
            axis_font: String::new(),
            background_dark: assets.metrics.color("TextContrastColorDark"),
            background_light: assets.metrics.color("TextContrastColorLight"),
            padding: 5,
        }
    }
}

fn line(dc: &mut DrawContext, a: (f32, f32), b: (f32, f32), color: Color) {
    dc.list.line([a.0, a.1], [b.0, b.1], 1.0, premultiplied(color));
}

impl WidgetKind for LineGraphWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "ValueFormat" => self.value_format = value.to_string(),
            "XAxisValueFormat" => self.x_axis_value_format = value.to_string(),
            "YAxisValueFormat" => self.y_axis_value_format = value.to_string(),
            "XAxisSize" => self.x_axis_size = parse_i32(value, 10),
            "YAxisSize" => self.y_axis_size = parse_i32(value, 10),
            "XAxisTicksPerLabel" => self.x_axis_ticks_per_label = parse_i32(value, 1),
            "XAxisLabel" => self.x_axis_label = value.to_string(),
            "YAxisLabel" => self.y_axis_label = value.to_string(),
            "DisplayFirstYAxisValue" => self.display_first_y_axis_value = super::parse_bool(value),
            "LabelFont" => self.label_font = Some(value.to_string()),
            "AxisFont" => self.axis_font = value.to_string(),
            "Padding" => self.padding = parse_i32(value, 5),
            _ => return false,
        }
        true
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let (Some(get_series), Some(font)) = (self.get_series.clone(), self.label_font.clone()) else {
            return;
        };
        let mut series = get_series();
        if series.is_empty() {
            return;
        }
        let axis_font = self.axis_font.clone();
        let rect = ui.render_bounds(id);
        let padding = self.padding;
        let (x_axis_size, y_axis_size) = (self.x_axis_size.max(1), self.y_axis_size.max(1));
        let x_axis_label_size = dc.measure(&axis_font, &self.x_axis_label);
        let x_axis_point_label_height = dc.measure(&font, "0").1;
        let graph_bottom_offset = padding * 2 + x_axis_label_size.1 + x_axis_point_label_height;
        let height = rect.h - (graph_bottom_offset + padding);
        let max_value = series.iter().flat_map(|s| s.points.iter().copied()).fold(0f32, f32::max);
        let longest_name = series
            .iter()
            .map(|s| s.key.as_str())
            .fold("", |a, b| if b.chars().count() > a.chars().count() { b } else { a })
            .to_string();
        let scale = 200.0 / 5000f32.max((max_value / 1000.0).ceil() * 1000.0);
        let width_max_value = dc.measure(&font, &format_value(&self.y_axis_value_format, height as f32 / scale)).0;
        let width_longest_name = dc.measure(&font, &longest_name).0;
        let y_axis_label_size = dc.measure(&axis_font, &self.y_axis_label);
        let width = rect.w - (padding * 4 + width_max_value + width_longest_name + y_axis_label_size.1);
        let x_step = width / x_axis_size;
        let y_step = height / y_axis_size;
        let point_count = series.iter().map(|s| s.points.len() as i32).max().unwrap_or(0);
        let point_start = (point_count - x_axis_size).max(0);
        let point_end = point_count.max(x_axis_size);
        let origin_x = (rect.x + padding * 2 + width_max_value + y_axis_label_size.1) as f32;
        let origin_y = (rect.bottom() - graph_bottom_offset) as f32;
        let shadow = TextStyle::Shadow {
            dark: self.background_dark,
            light: self.background_light,
            offset: 1,
        };

        series.sort_by(|a, b| {
            let (a, b) = (a.points.last().copied().unwrap_or(0.0), b.points.last().copied().unwrap_or(0.0));
            b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
        });
        for (key_offset, s) in series.iter().enumerate() {
            if !s.points.is_empty() {
                let skip = s.points.len().saturating_sub(x_axis_size as usize);
                let points = &s.points[skip..];
                let at = |x: usize, p: f32| (origin_x + (x as i32 * x_step) as f32, origin_y - p * scale);
                for (x, w) in points.windows(2).enumerate() {
                    line(dc, at(x, w[0]), at(x + 1, w[1]), s.color);
                }
                let last_x = points.len() - 1;
                let last_point = points[last_x];
                if last_point != 0.0 {
                    let (x, y) = at(last_x, last_point);
                    let text = format_value(&self.value_format, last_point);
                    dc.draw_text(&font, &text, x as i32, (y - 2.0) as i32, s.color, shadow);
                }
            }
            let (x, y) = (rect.right() - (width_longest_name + padding), rect.y + 10 * key_offset as i32 + 3);
            dc.draw_text(&font, &s.key, x, y, s.color, shadow);
        }

        let x = origin_x as i32 + width / 2 - x_axis_label_size.0 / 2;
        let y = rect.bottom() - (x_axis_label_size.1 + padding);
        dc.draw_text(&axis_font, &self.x_axis_label, x, y, WHITE, shadow);
        let mut x = 0;
        for n in point_start..=point_end {
            line(dc, (origin_x + x as f32, origin_y), (origin_x + x as f32, origin_y - 5.0), WHITE);
            if n % self.x_axis_ticks_per_label.max(1) == 0 {
                let text = format_value(&self.x_axis_value_format, (n / self.x_axis_ticks_per_label.max(1)) as f32);
                let text_width = dc.measure(&font, &text).0;
                dc.draw_text(
                    &font,
                    &text,
                    origin_x as i32 + x - text_width / 2,
                    origin_y as i32 + 2,
                    WHITE,
                    shadow,
                );
            }
            x += x_step;
        }

        let top_offset = dc.font_top_offset(&axis_font);
        let x = rect.x + 5 - top_offset;
        let y = origin_y as i32 - (height / 2 - y_axis_label_size.0 / 2);
        dc.draw_text_rotated(&axis_font, &self.y_axis_label, x, y, WHITE, shadow);
        let label_top_offset = dc.font_top_offset(&font);
        let mut y = if self.display_first_y_axis_value { 0 } else { y_step };
        while y <= height && y_step > 0 {
            let y_value = y as f32 / scale;
            line(dc, (origin_x, origin_y - y as f32), (origin_x + 5.0, origin_y - y as f32), WHITE);
            let text = format_value(&self.y_axis_value_format, y_value);
            let size = dc.measure(&font, &text);
            let y_location = y + (size.1 + label_top_offset) / 2;
            dc.draw_text(
                &font,
                &text,
                origin_x as i32 - (size.0 + 3),
                origin_y as i32 - y_location,
                WHITE,
                shadow,
            );
            y += y_step;
        }
        line(dc, (origin_x, origin_y), (origin_x + width as f32, origin_y), WHITE);
        line(dc, (origin_x, origin_y), (origin_x, origin_y - height as f32), WHITE);
    }
}

/// The fields shared by the three observer icon widgets.
#[derive(Clone)]
struct IconRow {
    icon_width: i32,
    icon_height: i32,
    icon_spacing: i32,
    clock_animation: String,
    clock_sequence: String,
    clock_palette: String,
    min_width: i32,
    tooltip_template: String,
    tooltip_container: Option<String>,
    tooltip_token: i32,
    last_icon_idx: usize,
    ingame: Option<Rc<Ingame>>,
}

impl Default for IconRow {
    fn default() -> Self {
        IconRow {
            icon_width: 32,
            icon_height: 24,
            icon_spacing: 1,
            clock_animation: "clock".into(),
            clock_sequence: "idle".into(),
            clock_palette: "chrome".into(),
            min_width: 240,
            tooltip_template: String::new(),
            tooltip_container: None,
            tooltip_token: 0,
            last_icon_idx: 0,
            ingame: None,
        }
    }
}

impl IconRow {
    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "IconWidth" => self.icon_width = parse_i32(value, 32),
            "IconHeight" => self.icon_height = parse_i32(value, 24),
            "IconSpacing" => self.icon_spacing = parse_i32(value, 1),
            "ClockAnimation" => self.clock_animation = value.to_string(),
            "ClockSequence" => self.clock_sequence = value.to_string(),
            "ClockPalette" => self.clock_palette = value.to_string(),
            "MinWidth" => self.min_width = parse_i32(value, 240),
            "TooltipTemplate" => self.tooltip_template = value.to_string(),
            "TooltipContainer" => self.tooltip_container = Some(value.to_string()),
            _ => return false,
        }
        true
    }

    fn with_template(template: &str) -> IconRow {
        IconRow {
            tooltip_template: template.to_string(),
            ..IconRow::default()
        }
    }

    fn bounds(&self, pos: (i32, i32)) -> Rect {
        Rect::new(pos.0, pos.1, self.icon_width, self.icon_height)
    }

    /// The `Tick` of the icon widgets: shows the tooltip of the icon under the mouse. `same` tells whether two icons
    /// are of the same item, and `args` makes the tooltip arguments.
    fn tick_tooltip<T: Clone>(
        &mut self,
        ui: &mut Ui,
        id: WidgetId,
        icons: &[(Rect, T)],
        tooltip: &mut Option<T>,
        same: impl Fn(&T, &T) -> bool,
        args: impl FnOnce(&mut Ui) -> WidgetArgs,
    ) {
        let Some(container) = self.tooltip_container.as_deref().and_then(|c| ui.find(ui.root, c)) else {
            return;
        };
        if ui.mouse_over != Some(id) {
            if tooltip.is_some() {
                TooltipContainerWidget::remove_tooltip_token(ui, container, self.tooltip_token);
                self.last_icon_idx = 0;
                *tooltip = None;
            }
            return;
        }
        let (mx, my) = ui.last_mouse;
        if let Some(current) = tooltip.as_ref()
            && icons
                .get(self.last_icon_idx)
                .is_some_and(|(r, icon)| same(icon, current) && r.contains(mx, my))
        {
            return;
        }
        if let Some(i) = icons.iter().position(|(r, _)| r.contains(mx, my)) {
            self.last_icon_idx = i;
            *tooltip = Some(icons[i].1.clone());
            let args = args(ui);
            self.tooltip_token = TooltipContainerWidget::set_tooltip(ui, container, &self.tooltip_template, args);
            return;
        }
        *tooltip = None;
    }

    fn step(&self) -> i32 {
        self.icon_width + self.icon_spacing
    }

    fn size(&self) -> (f32, f32) {
        (self.icon_width as f32, self.icon_height as f32)
    }

    /// The column origin of icon `col`.
    fn icon_top_left(&self, ui: &Ui, id: WidgetId, col: i32) -> (i32, i32) {
        let (x, y) = ui.render_origin(id);
        (x + col * self.step(), y)
    }

    fn draw_icon(&self, dc: &mut DrawContext, image: &str, sequence: &str, palette: &str, center: (f32, f32)) {
        let Some(ingame) = &self.ingame else { return };
        if let Some(f) = ingame.world_sprites.frame(dc.renderer, image, sequence, 0, palette) {
            dc.draw_frame_centered_scaled(f, center, 0.5);
        }
    }

    fn draw_clock(&self, dc: &mut DrawContext, center: (f32, f32), progress: impl Fn(i32) -> i32) {
        let Some(ingame) = &self.ingame else { return };
        let sprites = ingame.world_sprites.clone();
        let length = sprites.length(dc.renderer, &self.clock_animation, &self.clock_sequence);
        let frame = progress(length);
        if let Some(f) = sprites.frame(dc.renderer, &self.clock_animation, &self.clock_sequence, frame, &self.clock_palette) {
            dc.draw_frame_centered_scaled(f, center, 0.5);
        }
    }

    /// Sets the width of the widget to the icon count, and resets the tooltips when the mouse crosses the border.
    fn resize(&self, ui: &mut Ui, id: WidgetId, count: i32) {
        let new_width = (count * self.step()).max(self.min_width);
        if new_width == ui.node(id).bounds.w {
            return;
        }
        let (mx, my) = ui.last_mouse;
        let was_in = ui.event_bounds_contains(id, mx, my);
        ui.node_mut(id).bounds.w = new_width;
        if was_in != ui.event_bounds_contains(id, mx, my) {
            ui.reset_tooltips();
        }
    }

    /// Widens the scroll item, its `PLAYER_GRADIENT` and the stats panel to fit `count` icons.
    fn fit_parents(&self, ui: &mut Ui, id: WidgetId, count: i32) {
        let bounds = ui.node(id).bounds;
        let Some(parent) = ui.node(id).parent else { return };
        ui.node_mut(parent).bounds.w = bounds.x + bounds.w;
        if let Some(gradient) = ui.find(parent, "PLAYER_GRADIENT") {
            let offset = ui.node(gradient).bounds.x - bounds.x;
            ui.node_mut(gradient).bounds.w = (self.min_width - offset).max(count * self.step());
        }
        let Some(panel) = ui.node(parent).parent else { return };
        let widest = ui.node(panel).children.iter().map(|&c| ui.node(c).bounds.w).max().unwrap_or(0);
        ui.node_mut(panel).bounds.w = (25 + widest).max(bounds.x + self.min_width);
    }
}

type GetPlayer = Option<Getter<Option<PlayerRef>>>;

fn initialize_row(row: &mut IconRow, ui: &Ui) {
    row.ingame = services(ui).ingame.borrow().clone();
}

/// The player and the tooltip arguments of an icon widget.
fn tooltip_args<W: WidgetKind + 'static, T: 'static>(
    ui: &Ui,
    id: WidgetId,
    player: Option<PlayerRef>,
    name: &str,
    f: fn(&W) -> Option<T>,
) -> WidgetArgs {
    let args = WidgetArgs::default().with_value(name, kind_getter(ui, id, f));
    match player {
        Some(player) => args.with_value("player", player),
        None => args,
    }
}

/// Port of `ObserverProductionIconsWidget`.
#[derive(Clone)]
pub struct ObserverProductionIconsWidget {
    row: IconRow,
    pub get_player: GetPlayer,
    icons: Vec<(Rect, ProductionIcon)>,
    pub tooltip_icon: Option<ProductionIcon>,
}

impl Default for ObserverProductionIconsWidget {
    fn default() -> Self {
        ObserverProductionIconsWidget {
            row: IconRow::with_template("PRODUCTION_TOOLTIP"),
            get_player: None,
            icons: Vec::new(),
            tooltip_icon: None,
        }
    }
}

struct QueuedIcon {
    pos: (i32, i32),
    queued: Vec<Rc<ProductionItem>>,
}

/// `GetOverlayForItem` of the production icons.
fn production_overlay(item: &ProductionItem, timestep: i32) -> String {
    if item.paused.get() {
        "ON HOLD".into()
    } else if item.done.get() {
        "READY".into()
    } else {
        format_time(item.remaining_time_actual(), timestep)
    }
}

impl WidgetKind for ObserverProductionIconsWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        self.row.set_field(key, value)
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        initialize_row(&mut self.row, ui);
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        let player = self.get_player.as_ref().and_then(|f| f());
        let mut tooltip = self.tooltip_icon.take();
        self.row.tick_tooltip(
            ui,
            id,
            &self.icons,
            &mut tooltip,
            |a, b| Rc::ptr_eq(&a.actor, &b.actor),
            |ui| tooltip_args(ui, id, player, "getTooltipIcon", |w: &Self| w.tooltip_icon.clone()),
        );
        self.tooltip_icon = tooltip;
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        self.icons.clear();
        let (Some(ingame), Some(player)) = (self.row.ingame.clone(), self.get_player.as_ref().and_then(|f| f())) else {
            return;
        };
        let world = &ingame.world;
        let mut groups: Vec<(String, Vec<(Rc<ProductionQueue>, Rc<ProductionItem>)>)> = Vec::new();
        for (actor, queue) in world.actors_with_trait::<ProductionQueue>() {
            if !Rc::ptr_eq(&actor.owner(), &player) {
                continue;
            }
            let Some(item) = queue.current_item() else { continue };
            match groups.iter_mut().find(|(name, _)| *name == item.item) {
                Some((_, list)) => list.push((queue, item)),
                None => groups.push((item.item.clone(), vec![(queue, item)])),
            }
        }
        groups.sort_by_key(|(_, list)| (list[0].0.info.display_order, list[0].1.build_palette_order));

        let size = self.row.size();
        let mut icons = Vec::new();
        for (name, mut queued) in groups.iter().cloned() {
            queued.sort_by_key(|(_, pi)| {
                let state = if pi.done.get() {
                    0
                } else if pi.paused.get() {
                    2
                } else {
                    1
                };
                (state, pi.remaining_time_actual())
            });
            let (queue, current) = queued[0].clone();
            let faction = queue.actor.owner().faction.clone();
            if !queue.all_items().contains(&name) {
                continue;
            }
            let Some(actor) = world.rules().actor(&name) else { continue };
            let Some(bi) = actor.trait_info::<BuildableInfo>() else { continue };
            let image = actor
                .trait_info::<RenderSpritesInfo>()
                .map_or_else(|| actor.name.clone(), |rsi| rsi.get_image(&actor, &faction));
            let pos = self.row.icon_top_left(ui, id, icons.len() as i32);
            let center = (pos.0 as f32 + 0.5 * size.0, pos.1 as f32 + 0.5 * size.1);
            let palette = if bi.icon_palette_is_player_palette {
                format!("{}{}", bi.icon_palette, player.internal_name)
            } else {
                bi.icon_palette.clone()
            };
            self.row.draw_icon(dc, &image, &bi.icon, &palette, center);
            for overlay in ProductionPaletteWidget::active_overlays(&queue, &name) {
                let Some(sequence) = overlay.sequence.as_deref() else { continue };
                if let Some(f) = ingame
                    .world_sprites
                    .frame(dc.renderer, &overlay.image, sequence, 0, &overlay.palette)
                {
                    let offset = ((f.sprite.w as f32 - size.0) / 2.0, (f.sprite.h as f32 - size.1) / 2.0);
                    dc.draw_frame_centered_scaled(f, (center.0 + offset.0, center.1 + offset.1), 0.5);
                }
            }
            let (total, remaining) = (current.total_time.get(), current.remaining_time.get());
            self.row.draw_clock(dc, center, |length| {
                if total == 0 {
                    0
                } else {
                    (total - remaining) * (length - 1) / total
                }
            });
            let queued: Vec<Rc<ProductionItem>> = queued.into_iter().map(|(_, pi)| pi).collect();
            self.icons.push((
                self.row.bounds(pos),
                ProductionIcon {
                    actor: actor.clone(),
                    name: name.clone(),
                    hotkey: None,
                    image: image.clone(),
                    sequence: bi.icon.clone(),
                    palette: palette.clone(),
                    pos,
                    queued: queued.clone(),
                    queue: queue.clone(),
                },
            ));
            icons.push(QueuedIcon { pos, queued });
        }
        self.row.resize(ui, id, icons.len() as i32);

        for icon in &icons {
            let text = production_overlay(&icon.queued[0], world.timestep);
            let w = dc.measure("Tiny", &text).0;
            dc.draw_text("Tiny", &text, icon.pos.0 + 16 - w / 2, icon.pos.1 + 12, WHITE, CONTRAST);
            if icon.queued.len() > 1 {
                let text = icon.queued.len().to_string();
                let w = dc.measure("Small", &text).0;
                dc.draw_text("Small", &text, icon.pos.0 + 16 - w / 2, icon.pos.1, WHITE, CONTRAST);
            }
        }
        self.row.fit_parents(ui, id, groups.len() as i32);
    }
}

/// Port of `ObserverSupportPowerIconsWidget`.
#[derive(Clone)]
pub struct ObserverSupportPowerIconsWidget {
    row: IconRow,
    pub get_player: GetPlayer,
    icons: Vec<(Rect, SupportPowerIcon)>,
    pub tooltip_icon: Option<SupportPowerIcon>,
}

impl Default for ObserverSupportPowerIconsWidget {
    fn default() -> Self {
        ObserverSupportPowerIconsWidget {
            row: IconRow::with_template("SUPPORT_POWER_TOOLTIP"),
            get_player: None,
            icons: Vec::new(),
            tooltip_icon: None,
        }
    }
}

impl WidgetKind for ObserverSupportPowerIconsWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        self.row.set_field(key, value)
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        initialize_row(&mut self.row, ui);
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        let player = self.get_player.as_ref().and_then(|f| f());
        let mut tooltip = self.tooltip_icon.take();
        self.row.tick_tooltip(
            ui,
            id,
            &self.icons,
            &mut tooltip,
            |a, b| Rc::ptr_eq(&a.power, &b.power),
            |ui| tooltip_args(ui, id, player, "getTooltipIcon", |w: &Self| w.tooltip_icon.clone()),
        );
        self.tooltip_icon = tooltip;
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        self.icons.clear();
        let (Some(ingame), Some(player)) = (self.row.ingame.clone(), self.get_player.as_ref().and_then(|f| f())) else {
            return;
        };
        let Some(manager) = player.player_actor().trait_or_default::<SupportPowerManager>() else {
            return;
        };
        let mut powers: Vec<_> = manager.powers.borrow().values().filter(|p| !p.disabled()).cloned().collect();
        powers.sort_by_key(|p| p.info().map_or(0, |i| i.support_power_palette_order));
        ui.node_mut(id).bounds.w = powers.len() as i32 * self.row.step();
        let size = self.row.size();
        let mut icons = Vec::new();
        for (i, power) in powers.iter().enumerate() {
            let Some(info) = power.info() else { continue };
            let Some(sequence) = info.icon.as_deref() else { continue };
            let rb = ui.render_bounds(id);
            let pos = (rb.x + i as i32 * self.row.step(), rb.y);
            let center = (pos.0 as f32 + 0.5 * size.0, pos.1 as f32 + 0.5 * size.1);
            self.row.draw_icon(dc, &info.icon_image, sequence, &info.icon_palette, center);
            let (total, remaining) = (power.total_ticks, power.remaining_ticks());
            self.row.draw_clock(dc, center, |length| {
                if total == 0 {
                    0
                } else {
                    (total - remaining) * (length - 1) / total
                }
            });
            self.icons.push((
                self.row.bounds(pos),
                SupportPowerIcon {
                    power: power.clone(),
                    pos,
                    image: info.icon_image.clone(),
                    sequence: sequence.to_string(),
                    palette: info.icon_palette.clone(),
                    hotkey: None,
                },
            ));
            icons.push((pos, power.clone()));
        }
        for (pos, power) in icons {
            let text = if power.disabled() {
                "ON HOLD".into()
            } else if power.ready() {
                "READY".into()
            } else {
                format_time(power.remaining_ticks(), ingame.world.timestep)
            };
            let w = dc.measure("Tiny", &text).0;
            dc.draw_text("Tiny", &text, pos.0 + 16 - w / 2, pos.1 + 12, WHITE, CONTRAST);
        }
    }
}

/// `ArmyUnit`: the icon and the sort keys of an actor type in the army display.
struct ArmyUnit {
    actor: Rc<ActorInfo>,
    image: String,
    icon: String,
    palette: String,
    production_queue_order: i32,
    build_palette_order: i32,
    count: i32,
}

fn army_unit(ingame: &Ingame, player: &PlayerRef, name: &str, count: i32) -> Option<ArmyUnit> {
    let rules = ingame.world.rules();
    let actor = rules.actor(name)?;
    let bi = actor.trait_info::<BuildableInfo>()?;
    let rsi = actor.trait_info::<RenderSpritesInfo>()?;
    let production_queue_order = rules
        .actors()
        .flat_map(|(_, a)| a.trait_infos::<ProductionQueueInfo>())
        .filter(|q| bi.queue.contains(&q.ty))
        .map(|q| q.display_order)
        .min()
        .unwrap_or(0);
    let palette = if bi.icon_palette_is_player_palette {
        format!("{}{}", bi.icon_palette, player.internal_name)
    } else {
        bi.icon_palette.clone()
    };
    Some(ArmyUnit {
        actor: actor.clone(),
        image: rsi.get_image(&actor, &player.faction),
        icon: bi.icon.clone(),
        palette,
        production_queue_order,
        build_palette_order: bi.build_palette_order,
        count,
    })
}

/// Port of `ObserverArmyIconsWidget`.
#[derive(Clone)]
pub struct ObserverArmyIconsWidget {
    row: IconRow,
    pub get_player: GetPlayer,
    icons: Vec<(Rect, Rc<ActorInfo>)>,
    pub tooltip_unit: Option<Rc<ActorInfo>>,
}

impl Default for ObserverArmyIconsWidget {
    fn default() -> Self {
        ObserverArmyIconsWidget {
            row: IconRow::with_template("ARMY_TOOLTIP"),
            get_player: None,
            icons: Vec::new(),
            tooltip_unit: None,
        }
    }
}

impl WidgetKind for ObserverArmyIconsWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        self.row.set_field(key, value)
    }

    fn initialize(&mut self, ui: &mut Ui, _id: WidgetId) {
        initialize_row(&mut self.row, ui);
    }

    fn tick(&mut self, ui: &mut Ui, id: WidgetId) {
        let mut tooltip = self.tooltip_unit.take();
        self.row.tick_tooltip(ui, id, &self.icons, &mut tooltip, Rc::ptr_eq, |ui| {
            tooltip_args(ui, id, None, "getTooltipUnit", |w: &Self| w.tooltip_unit.clone())
        });
        self.tooltip_unit = tooltip;
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        self.icons.clear();
        let (Some(ingame), Some(player)) = (self.row.ingame.clone(), self.get_player.as_ref().and_then(|f| f())) else {
            return;
        };
        let Some(stats) = player.player_actor().trait_or_default::<PlayerStatistics>() else {
            return;
        };
        let mut units: Vec<ArmyUnit> = stats
            .units
            .borrow()
            .iter()
            .filter(|(_, c)| **c > 0)
            .filter_map(|(name, &count)| army_unit(&ingame, &player, name, count))
            .collect();
        units.sort_by_key(|u| (u.production_queue_order, u.build_palette_order));
        let size = self.row.size();
        let mut positions = Vec::new();
        for (col, unit) in units.iter().enumerate() {
            let pos = self.row.icon_top_left(ui, id, col as i32);
            let center = (pos.0 as f32 + 0.5 * size.0, pos.1 as f32 + 0.5 * size.1);
            self.row.draw_icon(dc, &unit.image, &unit.icon, &unit.palette, center);
            self.icons.push((self.row.bounds(pos), unit.actor.clone()));
            positions.push(pos);
        }
        self.row.resize(ui, id, units.len() as i32);
        let top_offset = dc.font_top_offset("TinyBold");
        for (pos, unit) in positions.iter().zip(&units) {
            let text = unit.count.to_string();
            let w = dc.measure("TinyBold", &text).0;
            dc.draw_text(
                "TinyBold",
                &text,
                pos.0 + self.row.icon_width - w,
                pos.1 - top_offset,
                WHITE,
                CONTRAST,
            );
        }
        self.row.fit_parents(ui, id, units.len() as i32);
    }
}
