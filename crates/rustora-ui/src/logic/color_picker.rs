// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `ColorPickerLogic.cs` and `ShowColorDropDown` of `ColorPickerManager.cs`.
//! The actor preview of the chooser waits for the unit render traits.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYamlNode;
use rustora_core::world_types::MersenneTwister;
use rustora_game::color_picker::{ColorPicker, from_hsv, to_hsv};

use super::{cosmetic_random, services};
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::{ColorBlockWidget, ColorMixerWidget, DropDownButtonWidget, HueSliderWidget};

/// The `onExit` argument.
pub type OnColorChosen = Rc<dyn Fn(&mut Ui, Color)>;

/// The `onChange` argument of `ColorPickerLogic`.
type OnChange = Rc<dyn Fn(&mut Ui, Color)>;

/// The color rules of the default mod rules.
pub struct ColorRules {
    pub picker: ColorPicker,
    /// The distinct `RestrictedPlayerColors` of all tile sets.
    pub terrain_colors: Vec<Color>,
}

/// Port of `ShowColorDropDown`. Does nothing if the mod rules cannot be read.
pub fn show_color_dropdown(ui: &mut Ui, dropdown: WidgetId, initial: Color, on_exit: OnColorChosen) {
    DropDownButtonWidget::remove_panel(ui, dropdown);
    let final_color = Rc::new(Cell::new(initial));
    let chosen = final_color.clone();
    let on_change: OnChange = Rc::new(move |_, color| chosen.set(color));
    let args = WidgetArgs::default()
        .with_value("initialColor", initial)
        .with_value("onChange", on_change);
    let chooser = ui.load_widget("COLOR_CHOOSER", None, &args);
    DropDownButtonWidget::attach_panel(ui, dropdown, chooser, Some(Rc::new(move |ui| on_exit(ui, final_color.get()))));
}

static PALETTE_TAB_OPENED_LAST: AtomicBool = AtomicBool::new(false);

fn palette_tab_open() -> bool {
    PALETTE_TAB_OPENED_LAST.load(Ordering::Relaxed)
}

/// The parts of the chooser that swatches and buttons change together.
#[derive(Clone)]
struct Chooser {
    mixer: WidgetId,
    hue_slider: WidgetId,
    on_change: OnChange,
    on_hue: Rc<dyn Fn(&mut Ui, f32)>,
}

impl Chooser {
    /// `mixer.Set(color)`.
    fn set_mixer(&self, ui: &mut Ui, color: Color) {
        let changed = ui.get::<ColorMixerWidget>(self.mixer).set(color);
        if changed {
            let color = ui.get::<ColorMixerWidget>(self.mixer).color();
            (self.on_change)(ui, color);
        }
    }

    /// `hueSlider.UpdateValue(hue)`.
    fn update_hue(&self, ui: &mut Ui, hue: f32) {
        let changed = ui.get::<HueSliderWidget>(self.hue_slider).slider.set_value(hue);
        if changed {
            (self.on_hue)(ui, hue);
        }
    }

    fn set_color(&self, ui: &mut Ui, color: Color) {
        self.set_mixer(ui, color);
        self.update_hue(ui, to_hsv(color).0);
    }
}

fn logic_int(logic_args: &[MiniYamlNode], key: &str, default: i32) -> i32 {
    logic_args
        .iter()
        .find(|n| n.key == key)
        .and_then(|n| n.value.value_str().trim().parse().ok())
        .unwrap_or(default)
}

struct ColorPickerLogic {
    palette_tab_highlighted: Rc<Cell<i32>>,
}

impl ChromeLogic for ColorPickerLogic {
    fn tick(&mut self, _ui: &mut Ui) {
        let highlighted = self.palette_tab_highlighted.get();
        if highlighted > 0 {
            self.palette_tab_highlighted.set(highlighted - 1);
        }
    }
}

/// Port of the constructor of `ColorPickerLogic`.
pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let initial = args.value::<Color>("initialColor").unwrap_or_default();
    let on_change = args.value::<OnChange>("onChange").unwrap_or_else(|| Rc::new(|_, _| {}));
    let rules = services(ui)
        .color_rules
        .borrow()
        .clone()
        .and_then(|lookup| lookup())
        .unwrap_or_else(|| {
            Rc::new(ColorRules {
                picker: ColorPicker::default(),
                terrain_colors: Vec::new(),
            })
        });
    let random = Rc::new(RefCell::new(MersenneTwister::new(cosmetic_random(i32::MAX as usize) as i32)));
    let palette_tab_highlighted = Rc::new(Cell::new(0));

    let mixer = ui.child(widget, "MIXER");
    let hue_slider = ui.child(widget, "HUE_SLIDER");
    let ([s_min, s_max], [v_min, v_max]) = (rules.picker.saturation_range, rules.picker.value_range);
    {
        let mut kind = ui.get::<ColorMixerWidget>(mixer);
        kind.set_color_limits(s_min, s_max, v_min, v_max, None);
        let on_change = on_change.clone();
        kind.on_change = Some(Rc::new(move |ui| {
            let color = ui.get::<ColorMixerWidget>(mixer).color();
            on_change(ui, color);
        }));
    }

    let on_change_hue = on_change.clone();
    let on_hue: Rc<dyn Fn(&mut Ui, f32)> = Rc::new(move |ui, h| {
        let color = {
            let mut kind = ui.get::<ColorMixerWidget>(mixer);
            kind.set_color_limits(s_min, s_max, v_min, v_max, Some(h));
            let (_, s, v) = to_hsv(kind.color());
            kind.set(from_hsv(h, s, v));
            kind.color()
        };
        on_change_hue(ui, color);
    });
    ui.get::<HueSliderWidget>(hue_slider).slider.on_change = Some(on_hue.clone());

    let chooser = Chooser {
        mixer,
        hue_slider,
        on_change: on_change.clone(),
        on_hue,
    };
    chooser.set_mixer(ui, initial);
    chooser.update_hue(ui, to_hsv(initial).0);

    let random_button = ui.child(widget, "RANDOM_BUTTON");
    {
        let (chooser, rules, random) = (chooser.clone(), rules.clone(), random.clone());
        ui.button(random_button).on_click = Some(Rc::new(move |ui| {
            let color = rules
                .picker
                .random_valid_color(&mut random.borrow_mut(), &rules.terrain_colors, &[]);
            chooser.set_color(ui, color);
        }));
    }

    on_change(ui, initial);

    let mixer_tab = ui.child(widget, "MIXER_TAB");
    let palette_tab = ui.child(widget, "PALETTE_TAB");
    let palette_panel = ui.child(widget, "PALETTE_TAB_PANEL");
    let mixer_tab_button = ui.child(widget, "MIXER_TAB_BUTTON");
    let palette_tab_button = ui.child(widget, "PALETTE_TAB_BUTTON");
    let preset_area = ui.child(palette_panel, "PRESET_AREA");
    let custom_area = ui.child(palette_panel, "CUSTOM_AREA");
    let preset_template = ui.child(palette_panel, "COLORPRESET");
    let custom_template = ui.child(palette_panel, "COLORCUSTOM");

    ui.set_visible_fn(mixer_tab, || !palette_tab_open());
    {
        let mut button = ui.button(mixer_tab_button);
        button.on_click = Some(Rc::new(|_| PALETTE_TAB_OPENED_LAST.store(false, Ordering::Relaxed)));
        button.is_highlighted = Some(Rc::new(|| !palette_tab_open()));
    }
    ui.set_visible_fn(palette_tab, palette_tab_open);
    {
        let mut button = ui.button(palette_tab_button);
        button.on_click = Some(Rc::new(|_| PALETTE_TAB_OPENED_LAST.store(true, Ordering::Relaxed)));
        let highlighted = palette_tab_highlighted.clone();
        button.is_highlighted = Some(Rc::new(move || palette_tab_open() || highlighted.get() > 0));
    }

    let palette_cols = logic_int(logic_args, "PaletteColumns", 8);
    let palette_preset_rows = logic_int(logic_args, "PalettePresetRows", 2);
    let palette_custom_rows = logic_int(logic_args, "PaletteCustomRows", 1);

    let add_swatch = |ui: &mut Ui, template: WidgetId, area: WidgetId, i: i32, j: i32| {
        let swatch = ui.clone_widget(template);
        let (w, h) = {
            let bounds = ui.node(swatch).bounds;
            (bounds.w, bounds.h)
        };
        ui.node_mut(swatch).bounds.x = i * w;
        ui.node_mut(swatch).bounds.y = j * h;
        ui.add_child(area, swatch);
        swatch
    };

    for j in 0..palette_preset_rows {
        for i in 0..palette_cols {
            let Some(&color) = rules.picker.preset_colors.get((j * palette_cols + i) as usize) else {
                break;
            };
            let swatch = add_swatch(ui, preset_template, preset_area, i, j);
            ui.set_visible_fn(swatch, || true);
            let chooser = chooser.clone();
            let mut block = ui.get::<ColorBlockWidget>(swatch);
            block.get_color = Some(Rc::new(move || color));
            block.on_mouse_up = Some(Rc::new(move |ui, _| chooser.set_color(ui, color)));
        }
    }

    let settings = services(ui).settings.clone();
    for j in 0..palette_custom_rows {
        for i in 0..palette_cols {
            let index = (j * palette_cols + i) as usize;
            let swatch = add_swatch(ui, custom_template, custom_area, i, j);
            let visible_settings = settings.clone();
            ui.set_visible_fn(swatch, move || visible_settings.borrow().player.custom_colors.len() > index);
            let (color_settings, picker, random) = (settings.clone(), rules.clone(), random.clone());
            let get_color: Rc<dyn Fn() -> Color> = Rc::new(move || {
                let stored = color_settings.borrow().player.custom_colors.get(index).copied().unwrap_or_default();
                picker.picker.make_valid(stored, &mut random.borrow_mut(), &[], &[]).0
            });
            let chooser = chooser.clone();
            let mut block = ui.get::<ColorBlockWidget>(swatch);
            block.get_color = Some(get_color.clone());
            block.on_mouse_up = Some(Rc::new(move |ui, _| chooser.set_color(ui, get_color())));
        }
    }

    let store_button = ui.child(widget, "STORE_BUTTON");
    {
        let (settings, highlighted) = (settings.clone(), palette_tab_highlighted.clone());
        ui.button(store_button).on_click = Some(Rc::new(move |ui| {
            let color = ui.get::<ColorMixerWidget>(mixer).color();
            let mut settings = settings.borrow_mut();
            let colors = &mut settings.player.custom_colors;
            colors.retain(|c| *c != color);
            colors.push(color);
            let keep = (palette_custom_rows * palette_cols).max(0) as usize;
            let excess = colors.len().saturating_sub(keep);
            colors.drain(..excess);
            if let Err(e) = settings.save() {
                log::warn!("Cannot save the settings: {e}");
            }
            if !palette_tab_open() {
                highlighted.set(4);
            }
        }));
    }

    crate::loader::cell(ColorPickerLogic { palette_tab_highlighted })
}
