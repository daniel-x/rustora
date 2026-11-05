// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `AddFactionSuffixLogic.cs`, `ClassicProductionLogic.cs`, `IngameCashCounterLogic.cs` and
//! `IngamePowerCounterLogic.cs`. Cash tick sounds are not played.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_mods_common::base::{DeveloperMode, PowerManager};
use rustora_mods_common::player::PlayerResources;
use rustora_mods_common::production::ProductionQueue;

use super::{NoLogic, message, message_with, services};
use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::widget::{ChromeLogic, Ui, WidgetId};
use crate::widgets::production::{LogicTickerWidget, ProductionPaletteWidget, ProductionTypeButtonWidget};
use crate::widgets::{BackgroundWidget, ImageWidget, LabelWithTooltipWidget, ScrollPanelWidget, TextFieldWidget};

/// `Widget.Get`, which also finds the widget itself.
fn get_self_or_child(ui: &Ui, id: WidgetId, name: &str) -> WidgetId {
    if ui.node(id).id.as_deref() == Some(name) {
        id
    } else {
        ui.child(id, name)
    }
}

fn local_player(ui: &Ui) -> Option<rustora_engine::player::PlayerRef> {
    services(ui).ingame.borrow().as_ref().and_then(|i| i.local_player.clone())
}

/// `AddFactionSuffixLogic`.
pub fn add_faction_suffix(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(player) = local_player(ui).filter(|p| !p.spectating()) else {
        return cell(NoLogic);
    };
    let key = format!("FactionSuffix-{}", player.faction);
    let metric = ui.assets.borrow().metrics.string(&key);
    let suffix = format!("-{}", if metric.is_empty() { player.faction.clone() } else { metric });
    let kind = ui.kind(id);
    let mut kind = kind.borrow_mut();
    let any = kind.as_any();
    if let Some(image) = any.downcast_mut::<ImageWidget>() {
        image.image_collection += &suffix;
    } else if let Some(bg) = any.downcast_mut::<BackgroundWidget>() {
        bg.background += &suffix;
    } else if let Some(tf) = any.downcast_mut::<TextFieldWidget>() {
        tf.background += &suffix;
    } else if let Some(sp) = any.downcast_mut::<ScrollPanelWidget>() {
        sp.button += &suffix;
        sp.background += &suffix;
        sp.scrollbar_background += &suffix;
        sp.decorations += &suffix;
    } else if let Some(b) = kind.as_button() {
        b.background += &suffix;
    } else {
        log::warn!("AddFactionSuffixLogic does not support this widget type");
    }
    cell(NoLogic)
}

/// `ClassicProductionLogic`.
pub fn classic_production(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(player) = local_player(ui) else {
        return cell(NoLogic);
    };
    let palette = ui.child(id, "PRODUCTION_PALETTE");
    let palette_kind = ui.kind(palette);
    let background = ui.find(id, "PALETTE_BACKGROUND");
    let foreground = ui.find(id, "PALETTE_FOREGROUND");
    if background.is_some() || foreground.is_some() {
        let bg_template = background.map(|b| ui.child(b, "ROW_TEMPLATE"));
        let bg_bottom = background.and_then(|b| ui.find(b, "BOTTOM_CAP"));
        let fg_template = foreground.map(|f| ui.child(f, "ROW_TEMPLATE"));
        if let Some(t) = bg_template {
            ui.hide_child(background.expect("background"), t);
        }
        if let Some(t) = bg_bottom {
            ui.hide_child(background.expect("background"), t);
        }
        if let Some(t) = fg_template {
            ui.hide_child(foreground.expect("foreground"), t);
        }
        let update = Rc::new(move |ui: &mut Ui, _old: i32, icons: i32| {
            let (min, max, columns) = {
                let p = ui.get::<ProductionPaletteWidget>(palette);
                (p.minimum_rows, p.maximum_rows, p.columns)
            };
            let rows = min.max((icons + columns - 1) / columns).min(max);
            if let (Some(bg), Some(template)) = (background, bg_template) {
                ui.remove_children(bg);
                let row_height = ui.node(template).bounds.h;
                for i in 0..rows {
                    let row = ui.clone_widget(template);
                    ui.node_mut(row).bounds.y = i * row_height;
                    ui.add_child(bg, row);
                }
                if let Some(bottom) = bg_bottom {
                    let bottom = ui.clone_widget(bottom);
                    ui.node_mut(bottom).bounds.y = rows * row_height;
                    ui.add_child(bg, bottom);
                }
            }
            if let (Some(fg), Some(template)) = (foreground, fg_template) {
                ui.remove_children(fg);
                let row_height = ui.node(template).bounds.h;
                for i in 0..rows {
                    let row = ui.clone_widget(template);
                    ui.node_mut(row).bounds.y = i * row_height;
                    ui.add_child(fg, row);
                }
            }
        });
        ui.get::<ProductionPaletteWidget>(palette).on_icon_count_changed = Some(update.clone());
        update(ui, 0, 0);
    }

    let types = ui.child(id, "PRODUCTION_TYPES");
    let buttons: Vec<WidgetId> = ui
        .node(types)
        .children
        .clone()
        .into_iter()
        .filter(|&c| ui.kind(c).borrow_mut().as_any().is::<ProductionTypeButtonWidget>())
        .collect();
    let all_queues: Vec<Rc<ProductionQueue>> = player.player_actor().traits_implementing::<ProductionQueue>();
    let disabled_fns: Rc<RefCell<Vec<(WidgetId, Rc<dyn Fn() -> bool>)>>> = Rc::default();
    let current_queue = {
        let k = palette_kind.clone();
        move || {
            k.borrow_mut()
                .as_any()
                .downcast_mut::<ProductionPaletteWidget>()
                .and_then(|p| p.current_queue())
        }
    };
    for &button in &buttons {
        let group = ui.get::<ProductionTypeButtonWidget>(button).production_group.clone();
        let queues: Rc<Vec<Rc<ProductionQueue>>> = Rc::new(
            all_queues
                .iter()
                .filter(|q| q.info.group.as_ref().unwrap_or(&q.info.ty) == &group)
                .cloned()
                .collect(),
        );
        let select_tab = {
            let queues = queues.clone();
            Rc::new(move |ui: &mut Ui| {
                let queue = queues.iter().find(|q| q.enabled()).cloned();
                let kind = ui.kind(palette);
                let mut kind = kind.borrow_mut();
                let p = kind.as_any().downcast_mut::<ProductionPaletteWidget>().expect("palette");
                p.set_current_queue(ui, palette, queue);
                p.scroll_to_top();
                p.pick_up_completed_building();
            })
        };
        let is_disabled: Rc<dyn Fn() -> bool> = {
            let queues = queues.clone();
            Rc::new(move || !queues.iter().any(|q| q.any_items_to_build()))
        };
        {
            let mut b = ui.button(button);
            b.is_disabled = Some(is_disabled.clone());
            b.on_click = Some(select_tab);
            let (queues, current) = (queues.clone(), current_queue.clone());
            b.is_highlighted = Some(Rc::new(move || current().is_some_and(|c| queues.iter().any(|q| Rc::ptr_eq(q, &c)))));
        }
        let chrome_name = group.to_lowercase();
        let icon = ui.child(button, "ICON");
        {
            let (queues, d) = (queues.clone(), is_disabled.clone());
            ui.get::<ImageWidget>(icon).get_image_name = Some(Rc::new(move || {
                if d() {
                    format!("{chrome_name}-disabled")
                } else if queues.iter().any(|q| q.all_queued().iter().any(|i| i.done.get())) {
                    format!("{chrome_name}-alert")
                } else {
                    chrome_name.clone()
                }
            }));
        }
        disabled_fns.borrow_mut().push((button, is_disabled));
    }

    let ticker = ui.child(id, "PRODUCTION_TICKER");
    ui.get::<LogicTickerWidget>(ticker).on_tick = Some(Rc::new(move |ui: &mut Ui| {
        let (current, displayed) = {
            let p = ui.get::<ProductionPaletteWidget>(palette);
            (p.current_queue(), p.displayed_icon_count)
        };
        if current.is_none() || displayed == 0 {
            let first = disabled_fns.borrow().iter().find(|(_, d)| !d()).map(|(b, _)| *b);
            if let Some(button) = first {
                let click = ui.button(button).on_click.clone();
                if let Some(click) = click {
                    click(ui);
                }
            }
        }
    }));

    for (name, down) in [("SCROLL_DOWN_BUTTON", true), ("SCROLL_UP_BUTTON", false)] {
        let Some(b) = ui.find(id, name) else { continue };
        let state = palette_kind.clone();
        let mut button = ui.button(b);
        button.on_click = Some(Rc::new(move |ui: &mut Ui| {
            let mut p = ui.get::<ProductionPaletteWidget>(palette);
            if down { p.scroll_down() } else { p.scroll_up() }
        }));
        let s = state.clone();
        button.is_disabled = Some(Rc::new(move || {
            let mut k = s.borrow_mut();
            let p = k.as_any().downcast_mut::<ProductionPaletteWidget>().expect("palette");
            if down { !p.can_scroll_down() } else { !p.can_scroll_up() }
        }));
        drop(button);
        ui.set_visible_fn(b, move || {
            let mut k = state.borrow_mut();
            let p = k.as_any().downcast_mut::<ProductionPaletteWidget>().expect("palette");
            p.total_icon_count > p.max_icon_row_offset.saturating_mul(p.columns)
        });
    }

    let sidebar_y = ui.find(ui.root, "SIDEBAR_PRODUCTION").map(|c| ui.node(c).bounds.y);
    if let Some(y) = sidebar_y {
        let mut p = ui.get::<ProductionPaletteWidget>(palette);
        let max_offset = (ui.resolution.1 - y) / p.icon_size.1 - 1;
        p.max_icon_row_offset = max_offset.min(p.maximum_rows);
    }
    cell(NoLogic)
}

const DISPLAY_FRAC_PER_FRAME: f32 = 0.07;
const DISPLAY_DELTA_PER_FRAME: i32 = 37;

/// `IngameCashCounterLogic`.
struct CashCounter {
    resources: Rc<PlayerResources>,
    display: i32,
    text: Rc<RefCell<String>>,
    tooltip: Rc<RefCell<String>>,
    last_silo: Cell<(i32, i32)>,
}

impl ChromeLogic for CashCounter {
    fn tick(&mut self, ui: &mut Ui) {
        let actual = self.resources.get_cash_and_resources();
        let diff = (actual - self.display).abs();
        let step = ((diff as f32 * DISPLAY_FRAC_PER_FRAME) as i32)
            .max(DISPLAY_DELTA_PER_FRAME)
            .min(diff);
        if self.display < actual {
            self.display += step;
        } else if self.display > actual {
            self.display -= step;
        }
        let silo = (self.resources.resources.get(), self.resources.resource_capacity.get());
        if silo != self.last_silo.get() || self.tooltip.borrow().is_empty() {
            self.last_silo.set(silo);
            *self.tooltip.borrow_mut() = message_with(
                ui,
                "label-silo-usage",
                &[
                    ("usage", MessageArg::Number(silo.0 as i64)),
                    ("capacity", MessageArg::Number(silo.1 as i64)),
                ],
            );
        }
        *self.text.borrow_mut() = self.display.to_string();
    }
}

pub fn cash_counter(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(player) = local_player(ui) else {
        return cell(NoLogic);
    };
    let resources = player.player_actor().trait_::<PlayerResources>();
    let display = resources.get_cash_and_resources();
    let text = Rc::new(RefCell::new(display.to_string()));
    let tooltip: Rc<RefCell<String>> = Rc::default();
    let cash = get_self_or_child(ui, id, "CASH");
    {
        let mut w = ui.get::<LabelWithTooltipWidget>(cash);
        let t = text.clone();
        w.label.get_text = Some(Rc::new(move || t.borrow().clone()));
        let t = tooltip.clone();
        w.get_tooltip_text = Some(Rc::new(move || t.borrow().clone()));
    }
    cell(CashCounter {
        resources,
        display,
        text,
        tooltip,
        last_silo: Cell::new((-1, -1)),
    })
}

/// `IngamePowerCounterLogic`; the tooltip text is updated each tick instead of when it is read.
struct PowerCounter {
    power: Rc<PowerManager>,
    developer: Rc<DeveloperMode>,
    tooltip: Rc<RefCell<String>>,
    infinite: String,
}

impl ChromeLogic for PowerCounter {
    fn tick(&mut self, ui: &mut Ui) {
        let capacity = if self.developer.unlimited_power() {
            self.infinite.clone()
        } else {
            self.power.power_provided().to_string()
        };
        let usage = self.power.power_drained().to_string();
        *self.tooltip.borrow_mut() = message_with(
            ui,
            "label-power-usage",
            &[("usage", MessageArg::Text(&usage)), ("capacity", MessageArg::Text(&capacity))],
        );
    }
}

pub fn power_counter(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(player) = local_player(ui) else {
        return cell(NoLogic);
    };
    let pa = player.player_actor();
    let power = pa.trait_::<PowerManager>();
    let developer = pa.trait_::<DeveloperMode>();
    let infinite = message(ui, "label-infinite-power");
    let label = get_self_or_child(ui, id, "POWER");
    let icon = ui.child(label, "POWER_ICON");
    let p = power.clone();
    ui.get::<ImageWidget>(icon).get_image_name = Some(Rc::new(move || {
        if p.excess_power() < 0 { "power-critical" } else { "power-normal" }.to_string()
    }));
    let tooltip: Rc<RefCell<String>> = Rc::default();
    {
        let mut w = ui.get::<LabelWithTooltipWidget>(label);
        let p = power.clone();
        w.label.get_color = Some(Rc::new(move || {
            if p.excess_power() < 0 {
                Color::from_argb(255, 255, 0, 0)
            } else {
                Color::from_argb(255, 255, 255, 255)
            }
        }));
        let (p, d, inf) = (power.clone(), developer.clone(), infinite.clone());
        w.label.get_text = Some(Rc::new(move || {
            if d.unlimited_power() {
                inf.clone()
            } else {
                p.excess_power().to_string()
            }
        }));
        let t = tooltip.clone();
        w.get_tooltip_text = Some(Rc::new(move || t.borrow().clone()));
    }
    cell(PowerCounter {
        power,
        developer,
        tooltip,
        infinite,
    })
}

/// `SupportPowerBinLogic`.
pub fn support_power_bin(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    use crate::widgets::support_powers::SupportPowersWidget;
    let palette = ui.child(id, "SUPPORT_PALETTE");
    let background = ui.find(id, "PALETTE_BACKGROUND");
    let foreground = ui.find(id, "PALETTE_FOREGROUND");
    if background.is_none() && foreground.is_none() {
        return cell(NoLogic);
    }
    let bg_template = background.map(|b| ui.child(b, "ICON_TEMPLATE"));
    let fg_template = foreground.map(|f| ui.child(f, "ICON_TEMPLATE"));
    if let (Some(b), Some(t)) = (background, bg_template) {
        ui.hide_child(b, t);
    }
    if let (Some(f), Some(t)) = (foreground, fg_template) {
        ui.hide_child(f, t);
    }
    let update = Rc::new(move |ui: &mut Ui, _old: i32, icons: i32| {
        let (size, margin, horizontal) = {
            let p = ui.get::<SupportPowersWidget>(palette);
            (p.icon_size, p.icon_margin, p.horizontal)
        };
        for (parent, template) in [(background, bg_template), (foreground, fg_template)] {
            let (Some(parent), Some(template)) = (parent, template) else {
                continue;
            };
            ui.remove_children(parent);
            for i in 0..icons {
                let row = ui.clone_widget(template);
                if horizontal {
                    ui.node_mut(row).bounds.x += i * (size.0 + margin);
                } else {
                    ui.node_mut(row).bounds.y += i * (size.1 + margin);
                }
                ui.add_child(parent, row);
            }
        }
    });
    ui.get::<SupportPowersWidget>(palette).on_icon_count_changed = Some(update.clone());
    update(ui, 0, 0);
    cell(NoLogic)
}

/// A label text that logic updates in `tick`, read by the label's getter while drawing.
fn shared_text(ui: &mut Ui, label: WidgetId) -> Rc<RefCell<String>> {
    let text: Rc<RefCell<String>> = Rc::default();
    let t = text.clone();
    ui.label(label).get_text = Some(Rc::new(move || t.borrow().clone()));
    text
}

/// `ProductionTooltipLogic`; `BeforeRender` runs in `tick`.
struct ProductionTooltip {
    widget: WidgetId,
    palette: WidgetId,
    queue_power: Option<Rc<PowerManager>>,
    labels: TooltipLabels,
    texts: TooltipTexts,
    cost: Rc<Cell<i32>>,
    power: Rc<Cell<i32>>,
    last: Option<(String, Option<String>, Option<rustora_mods_common::base::PowerState>)>,
    desc_y: i32,
    desc_padding: i32,
    icon_margin: i32,
}

struct TooltipLabels {
    name: WidgetId,
    hotkey: WidgetId,
    requires: WidgetId,
    power: WidgetId,
    power_icon: WidgetId,
    time: WidgetId,
    time_icon: WidgetId,
    cost: WidgetId,
    cost_icon: WidgetId,
    desc: WidgetId,
}

struct TooltipTexts {
    name: Rc<RefCell<String>>,
    hotkey: Rc<RefCell<String>>,
    requires: Rc<RefCell<String>>,
    power: Rc<RefCell<String>>,
    time: Rc<RefCell<String>>,
    cost: Rc<RefCell<String>>,
    desc: Rc<RefCell<String>>,
}

fn actor_name(world: &rustora_engine::world::World, ui: &Ui, a: &str) -> String {
    world
        .rules()
        .actor(&a.to_lowercase())
        .and_then(|ai| rustora_mods_common::tooltip::default_tooltip_name(&ai))
        .map_or_else(|| a.to_string(), |key| message(ui, &key))
}

impl ChromeLogic for ProductionTooltip {
    fn tick(&mut self, ui: &mut Ui) {
        let icon = ui.get::<ProductionPaletteWidget>(self.palette).tooltip_icon.clone();
        let Some(icon) = icon else { return };
        let actor = icon.actor.clone();
        let hotkey = icon
            .hotkey
            .as_deref()
            .and_then(|h| ui.assets.borrow().hotkeys.get(h))
            .map(|h| h.display_string(&ui.assets.borrow().messages));
        let power_state = self.queue_power.as_ref().map(|pm| pm.power_state());
        let key = (actor.name.clone(), hotkey.clone(), power_state);
        if self.last.as_ref() == Some(&key) {
            return;
        }
        let world = icon.queue.actor.world();
        let l = &self.labels;
        let name = rustora_mods_common::tooltip::default_tooltip_name(&actor).map_or_else(|| actor.name.clone(), |k| message(ui, &k));
        let bi = actor
            .trait_info::<rustora_mods_common::production::BuildableInfo>()
            .expect("buildable");
        self.cost.set(icon.queue.get_production_cost(&actor));
        let font = ui.label(l.name).font.clone();
        let name_x = ui.node(l.name).bounds.x;
        let name_size = ui.measure(&font, &name);
        *self.texts.name.borrow_mut() = name;
        let mut hotkey_width = 0;
        ui.node_mut(l.hotkey).visible = hotkey.is_some();
        if let Some(h) = &hotkey {
            let text = format!("({h})");
            hotkey_width = ui.measure(&font, &text).0 + 2 * name_x;
            *self.texts.hotkey.borrow_mut() = text;
            ui.node_mut(l.hotkey).bounds.x = name_size.0 + 2 * name_x;
        }
        let prereqs: Vec<String> = bi
            .prerequisites
            .iter()
            .map(|a| actor_name(&world, ui, a))
            .filter(|s| !s.starts_with('~') && !s.starts_with('!'))
            .collect();
        let mut requires_size = (0, 0);
        if prereqs.is_empty() {
            ui.node_mut(l.requires).visible = false;
            ui.node_mut(l.desc).bounds.y = self.desc_y;
        } else {
            let text = message_with(ui, "label-requires", &[("prerequisites", MessageArg::Text(&prereqs.join(", ")))]);
            let requires_font = ui.label(l.requires).font.clone();
            requires_size = ui.measure(&requires_font, &text);
            *self.texts.requires.borrow_mut() = text;
            ui.node_mut(l.requires).visible = true;
            let h = ui.node(l.requires).bounds.h;
            ui.node_mut(l.desc).bounds.y = self.desc_y + h;
        }
        let mut power_size = (0, 0);
        if self.queue_power.is_some() {
            let power: i32 = actor
                .trait_infos::<rustora_mods_common::base::PowerInfo>()
                .iter()
                .filter(|p| p.base.enabled_by_default())
                .map(|p| p.amount)
                .sum();
            self.power.set(power);
            let text = power.to_string();
            power_size = ui.measure(&font, &text);
            *self.texts.power.borrow_mut() = text;
            ui.node_mut(l.power).visible = power != 0;
            ui.node_mut(l.power_icon).visible = power != 0;
        }
        let build_time = icon.queue.get_build_time(&actor, &bi);
        let low_power = power_state.is_some_and(|s| s != rustora_mods_common::base::PowerState::Normal);
        let modifier = if low_power { icon.queue.info.low_power_modifier } else { 100 };
        let time_text = crate::widgets::production::format_time(build_time * modifier / 100, world.timestep);
        let time_size = ui.measure(&font, &time_text);
        *self.texts.time.borrow_mut() = time_text;
        ui.label(l.time).text_color = if low_power && icon.queue.info.low_power_modifier > 100 {
            Color::from_argb(255, 255, 0, 0)
        } else {
            Color::from_argb(255, 255, 255, 255)
        };
        let cost_text = self.cost.get().to_string();
        let cost_size = ui.measure(&font, &cost_text);
        *self.texts.cost.borrow_mut() = cost_text;
        let desc = bi
            .description
            .as_deref()
            .filter(|d| !d.is_empty())
            .map(|d| message(ui, d))
            .unwrap_or_default();
        let desc_font = ui.label(l.desc).font.clone();
        let desc_size = ui.measure(&desc_font, &desc);
        *self.texts.desc.borrow_mut() = desc;
        ui.node_mut(l.desc).bounds.w = desc_size.0;
        ui.node_mut(l.desc).bounds.h = desc_size.1 + self.desc_padding;
        let left = (name_size.0 + hotkey_width).max(requires_size.0).max(desc_size.0);
        let right = power_size.0.max(time_size.0).max(cost_size.0);
        let icon_x = left + 2 * name_x;
        for i in [l.time_icon, l.power_icon, l.cost_icon] {
            ui.node_mut(i).bounds.x = icon_x;
        }
        let label_x = icon_x + ui.node(l.time_icon).bounds.w + self.icon_margin;
        for i in [l.time, l.power, l.cost] {
            ui.node_mut(i).bounds.x = label_x;
        }
        let time_icon_w = ui.node(l.time_icon).bounds.w;
        ui.node_mut(self.widget).bounds.w = left + right + 3 * name_x + time_icon_w + self.icon_margin;
        let desc_b = ui.node(l.desc).bounds;
        let left_height = desc_b.bottom() + desc_b.x;
        let right_bottom = if ui.node(l.power).visible {
            ui.node(l.power_icon).bounds.bottom()
        } else {
            ui.node(l.time_icon).bounds.bottom()
        };
        let right_height = right_bottom + ui.node(l.cost_icon).bounds.y;
        ui.node_mut(self.widget).bounds.h = left_height.max(right_height);
        self.last = Some(key);
    }
}

pub fn production_tooltip(ui: &mut Ui, id: WidgetId, args: &crate::widget::WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (Some(palette), Some(player)) = (args.value::<WidgetId>("palette"), local_player(ui)) else {
        return cell(NoLogic);
    };

    let pa = player.player_actor();
    let queue_power = pa.trait_or_default::<PowerManager>();
    let resources = pa.trait_::<PlayerResources>();
    let get = |ui: &Ui, n: &str| ui.child(id, n);
    let labels = TooltipLabels {
        name: get(ui, "NAME"),
        hotkey: get(ui, "HOTKEY"),
        requires: get(ui, "REQUIRES"),
        power: get(ui, "POWER"),
        power_icon: get(ui, "POWER_ICON"),
        time: get(ui, "TIME"),
        time_icon: get(ui, "TIME_ICON"),
        cost: get(ui, "COST"),
        cost_icon: get(ui, "COST_ICON"),
        desc: get(ui, "DESC"),
    };
    let texts = TooltipTexts {
        name: shared_text(ui, labels.name),
        hotkey: shared_text(ui, labels.hotkey),
        requires: shared_text(ui, labels.requires),
        power: shared_text(ui, labels.power),
        time: shared_text(ui, labels.time),
        cost: shared_text(ui, labels.cost),
        desc: shared_text(ui, labels.desc),
    };
    let palette_kind = ui.kind(palette);
    ui.set_visible_fn(id, move || {
        palette_kind
            .try_borrow_mut()
            .ok()
            .and_then(|mut k| {
                k.as_any()
                    .downcast_mut::<ProductionPaletteWidget>()
                    .map(|p| p.tooltip_icon.is_some())
            })
            .unwrap_or(false)
    });
    let cost = Rc::new(Cell::new(0));
    let c = cost.clone();
    ui.label(labels.cost).get_color = Some(Rc::new(move || {
        if resources.get_cash_and_resources() >= c.get() {
            Color::from_argb(255, 255, 255, 255)
        } else {
            Color::from_argb(255, 255, 0, 0)
        }
    }));
    let power = Rc::new(Cell::new(0));
    if let Some(pm) = queue_power.clone() {
        let p = power.clone();
        ui.label(labels.power).get_color = Some(Rc::new(move || {
            if pm.power_provided() - pm.power_drained() >= -p.get() || p.get() > 0 {
                Color::from_argb(255, 255, 255, 255)
            } else {
                Color::from_argb(255, 255, 0, 0)
            }
        }));
    }
    let desc_b = ui.node(labels.desc).bounds;
    let icon_margin = ui.node(labels.time_icon).bounds.x;
    cell(ProductionTooltip {
        widget: id,
        palette,
        queue_power,
        labels,
        texts,
        cost,
        power,
        last: None,
        desc_y: desc_b.y,
        desc_padding: desc_b.h,
        icon_margin,
    })
}

/// `OrderButtonsChromeUtils.BindOrderButton`.
pub fn bind_order_button(ui: &mut Ui, id: WidgetId, kind: crate::button_orders::ButtonOrder, icon: &str) -> Rc<RefCell<dyn ChromeLogic>> {
    use crate::button_orders::ButtonOrderGenerator;
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let active = {
        let ingame = ingame.clone();
        move || ingame.order_generator().button_order() == Some(kind)
    };
    {
        let mut b = ui.button(id);
        let ingame = ingame.clone();
        let a = active.clone();
        b.on_click = Some(Rc::new(move |_ui: &mut Ui| {
            if a() {
                ingame.cancel_input_mode();
            } else {
                ingame.set_order_generator(Rc::new(ButtonOrderGenerator { kind }));
            }
        }));
        b.is_highlighted = Some(Rc::new(active.clone()));
    }
    let image = ui.child(id, "ICON");
    let icon = icon.to_string();
    ui.get::<ImageWidget>(image).get_image_name = Some(Rc::new(move || if active() { format!("{icon}-active") } else { icon.clone() }));
    cell(NoLogic)
}

/// `IngameRadarDisplayLogic`, without the radar sounds.
pub fn radar_display(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    use crate::widgets::radar::RadarWidget;
    let Some(player) = local_player(ui) else {
        return cell(NoLogic);
    };
    let Some(world) = services(ui).ingame.borrow().as_ref().map(|i| i.world.clone()) else {
        return cell(NoLogic);
    };
    let enabled = Rc::new(Cell::new(false));
    let radar = ui.child(id, "RADAR_MINIMAP");
    let e = enabled.clone();
    ui.get::<RadarWidget>(radar).is_enabled = Some(Rc::new(move || e.get()));
    let developer = player.player_actor().trait_::<DeveloperMode>();
    let ticker = ui.child(id, "RADAR_TICKER");
    let e = enabled.clone();
    ui.get::<LogicTickerWidget>(ticker).on_tick = Some(Rc::new(move |_ui: &mut Ui| {
        let any_radar = world
            .actors_with_trait::<rustora_mods_common::gps::ProvidesRadar>()
            .iter()
            .any(|(a, r)| !rustora_engine::traits::IDisabledTrait::is_trait_disabled(&**r) && Rc::ptr_eq(&a.owner(), &player));
        e.set(developer.disable_shroud() || any_radar);
    }));
    if let Some(block) = ui.find(id, "RADAR_FADETOBLACK") {
        let alpha = Rc::new(Cell::new(0u8));
        let a = alpha.clone();
        ui.get::<RadarWidget>(radar).animating = Some(Rc::new(move |x: f32| a.set((255.0 * x) as u8)));
        let a = alpha.clone();
        ui.set_visible_fn(block, move || a.get() != 0);
        let a = alpha.clone();
        ui.get::<crate::widgets::ColorBlockWidget>(block).get_color = Some(Rc::new(move || Color::from_argb(a.get(), 0, 0, 0)));
    }
    cell(NoLogic)
}
