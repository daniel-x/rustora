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
