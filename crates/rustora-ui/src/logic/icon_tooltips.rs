// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `SupportPowerTooltipLogic.cs` and `ArmyTooltipLogic.cs`. `BeforeRender` runs in `tick`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_engine::actor_info::ActorInfo;
use rustora_mods_common::production::BuildableInfo;
use rustora_mods_common::tooltip::default_tooltip_name;

use super::{NoLogic, message, services};
use crate::loader::cell;
use crate::widget::{ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::production::format_time;
use crate::widgets::support_powers::SupportPowerIcon;

/// A label text that the logic updates in `tick`, read by the label's getter while drawing.
fn shared_text(ui: &mut Ui, label: WidgetId) -> Rc<RefCell<String>> {
    let text: Rc<RefCell<String>> = Rc::default();
    let t = text.clone();
    ui.label(label).get_text = Some(Rc::new(move || t.borrow().clone()));
    text
}

fn measure(ui: &Ui, label: WidgetId, text: &str) -> (i32, i32) {
    let font = ui.label(label).font.clone();
    ui.measure(&font, text)
}

/// `SupportPowerTooltipLogic`.
struct SupportPowerTooltip {
    widget: WidgetId,
    get_icon: Getter<Option<SupportPowerIcon>>,
    name: WidgetId,
    hotkey: WidgetId,
    time: WidgetId,
    desc: WidgetId,
    texts: [Rc<RefCell<String>>; 4],
    base_height: i32,
    time_offset: i32,
    timestep: i32,
    last: Option<(*const (), Option<String>, i32)>,
}

impl ChromeLogic for SupportPowerTooltip {
    fn tick(&mut self, ui: &mut Ui) {
        let Some(icon) = (self.get_icon)() else { return };
        let sp = &icon.power;
        let Some(info) = sp.info() else { return };
        let remaining_seconds = (sp.remaining_ticks() as f32 * self.timestep as f32 / 1000.0).ceil() as i32;
        let hotkey = {
            let assets = ui.assets.borrow();
            icon.hotkey
                .as_deref()
                .and_then(|h| assets.hotkeys.get(h))
                .map(|h| h.display_string(&assets.messages))
        };
        let key = (Rc::as_ptr(sp) as *const (), hotkey.clone(), remaining_seconds);
        if self.last.as_ref() == Some(&key) {
            return;
        }
        let resolve = |ui: &Ui, key: &Option<String>| key.as_deref().map(|k| message(ui, k)).unwrap_or_default();
        let name = resolve(ui, &info.name);
        let desc = resolve(ui, &info.description);
        let name_size = measure(ui, self.name, &name);
        let desc_size = measure(ui, self.desc, &desc);
        let time_text = format!(
            "{} / {}",
            format_time(sp.remaining_ticks(), self.timestep),
            format_time(info.charge_interval, self.timestep)
        );
        let time_width = measure(ui, self.time, &time_text).0;
        let name_x = ui.node(self.name).bounds.x;
        let mut hotkey_width = 0;
        ui.node_mut(self.hotkey).visible = hotkey.is_some();
        if let Some(h) = &hotkey {
            let text = format!("({h})");
            hotkey_width = measure(ui, self.hotkey, &text).0 + 2 * name_x;
            *self.texts[1].borrow_mut() = text;
            ui.node_mut(self.hotkey).bounds.x = name_size.0 + 2 * name_x;
        }
        *self.texts[0].borrow_mut() = name;
        *self.texts[2].borrow_mut() = time_text;
        *self.texts[3].borrow_mut() = desc;
        let top_width = name_size.0 + hotkey_width + time_width + self.time_offset;
        let width = 2 * name_x + top_width.max(desc_size.0);
        let bounds = &mut ui.node_mut(self.widget).bounds;
        bounds.w = width;
        bounds.h = self.base_height + desc_size.1;
        ui.node_mut(self.time).bounds.x = width - name_x - time_width;
        self.last = Some(key);
    }
}

/// `SupportPowerTooltipLogic`.
pub fn support_power_tooltip(ui: &mut Ui, id: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(get_icon) = args.value::<Getter<Option<SupportPowerIcon>>>("getTooltipIcon") else {
        return cell(NoLogic);
    };
    let timestep = services(ui).ingame.borrow().as_ref().map_or(40, |i| i.world.timestep);
    let g = get_icon.clone();
    ui.set_visible_fn(id, move || g().is_some_and(|i| i.power.info().is_some()));
    let [name, hotkey, time, desc] = ["NAME", "HOTKEY", "TIME", "DESC"].map(|n| ui.child(id, n));
    let texts = [name, hotkey, time, desc].map(|l| shared_text(ui, l));
    let g = get_icon.clone();
    ui.label(time).get_color = Some(Rc::new(move || {
        if g().is_some_and(|i| !i.power.active()) {
            Color::from_argb(255, 255, 0, 0)
        } else {
            Color::from_argb(255, 255, 255, 255)
        }
    }));
    cell(SupportPowerTooltip {
        widget: id,
        get_icon,
        name,
        hotkey,
        time,
        desc,
        texts,
        base_height: ui.node(id).bounds.h,
        time_offset: ui.node(time).bounds.x,
        timestep,
        last: None,
    })
}

/// `ArmyTooltipLogic`.
struct ArmyTooltip {
    widget: WidgetId,
    get_unit: Getter<Option<Rc<ActorInfo>>>,
    name: WidgetId,
    desc: WidgetId,
    name_text: Rc<RefCell<String>>,
    desc_text: Rc<RefCell<String>>,
    desc_padding: i32,
    last: Option<Rc<ActorInfo>>,
}

impl ChromeLogic for ArmyTooltip {
    fn tick(&mut self, ui: &mut Ui) {
        let Some(actor) = (self.get_unit)() else { return };
        if self.last.as_ref().is_some_and(|l| Rc::ptr_eq(l, &actor)) {
            return;
        }
        let name = default_tooltip_name(&actor).map_or_else(|| actor.name.clone(), |k| message(ui, &k));
        let desc = actor
            .trait_info::<BuildableInfo>()
            .and_then(|bi| bi.description.clone())
            .filter(|d| !d.is_empty())
            .map(|d| message(ui, &d))
            .unwrap_or_default();
        let name_size = measure(ui, self.name, &name);
        let desc_size = measure(ui, self.desc, &desc);
        *self.name_text.borrow_mut() = name;
        *self.desc_text.borrow_mut() = desc;
        let desc_bounds = {
            let b = &mut ui.node_mut(self.desc).bounds;
            b.w = desc_size.0;
            b.h = desc_size.1 + self.desc_padding;
            *b
        };
        let name_x = ui.node(self.name).bounds.x;
        let bounds = &mut ui.node_mut(self.widget).bounds;
        bounds.w = name_size.0.max(desc_size.0) + 2 * name_x;
        bounds.h = desc_bounds.bottom() + desc_bounds.x;
        self.last = Some(actor);
    }
}

/// `ArmyTooltipLogic`.
pub fn army_tooltip(ui: &mut Ui, id: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(get_unit) = args.value::<Getter<Option<Rc<ActorInfo>>>>("getTooltipUnit") else {
        return cell(NoLogic);
    };
    let g = get_unit.clone();
    ui.set_visible_fn(id, move || g().is_some());
    let name = ui.child(id, "NAME");
    let desc = ui.child(id, "DESC");
    let name_text = shared_text(ui, name);
    let desc_text = shared_text(ui, desc);
    cell(ArmyTooltip {
        widget: id,
        get_unit,
        name,
        desc,
        name_text,
        desc_text,
        desc_padding: ui.node(desc).bounds.h,
        last: None,
    })
}
