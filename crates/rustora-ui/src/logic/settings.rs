// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `SettingsLogic`, `SettingsUtils` and the logic of the settings panels.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use rustora_client::settings::{
    GlProfile, ModifierFlags, MouseScrollType, SettingValue, Settings, StatusBarsType, TargetLinesType, TextNotificationPoolFilters,
    WindowMode, WorldViewport, sanitized_player_name,
};
use rustora_core::miniyaml::MiniYamlNode;

use super::{ChromeLogic, NoLogic, button_prompt_with_cancel, message, message_with, services};
use crate::fluent::MessageArg;
use crate::hotkeys::{CONTEXT_FLUENT_PREFIX, Hotkey, hotkeys_equal};
use crate::loader::cell;
use crate::widget::{Action, AppRequest, KeyCallback, Ui, WidgetArgs, WidgetId};
use crate::widgets::{
    ButtonWidget, CheckboxWidget, ColorBlockWidget, DropDownButtonWidget, HotkeyEntryWidget, LabelWidget, ScrollItemWidget, SliderWidget,
    TextFieldWidget,
};

type LeaveAction = Rc<dyn Fn(&mut Ui) -> bool>;

struct RegisteredPanel {
    id: String,
    label: String,
    leave: LeaveAction,
    reset: Action,
}

/// The state of `SettingsLogic` that the panel logics register with.
pub struct SettingsShell {
    panel_containers: HashMap<String, WidgetId>,
    tab_container: WidgetId,
    tab_template: WidgetId,
    stride: (i32, i32),
    buttons: RefCell<Vec<WidgetId>>,
    panels: RefCell<Vec<RegisteredPanel>>,
    active: Rc<RefCell<String>>,
    needs_restart: Cell<bool>,
}

impl SettingsShell {
    /// Port of `RegisterSettingsPanel`. The panel logic has initialized the panel already.
    fn register(self: &Rc<Self>, ui: &mut Ui, panel_id: &str, label: &str, leave: LeaveAction, reset: Action) {
        if self.active.borrow().is_empty() {
            *self.active.borrow_mut() = panel_id.to_string();
        }
        if let Some(&container) = self.panel_containers.get(panel_id) {
            let (active, id) = (self.active.clone(), panel_id.to_string());
            ui.set_visible_fn(container, move || *active.borrow() == id);
        }
        self.panels.borrow_mut().push(RegisteredPanel {
            id: panel_id.to_string(),
            label: label.to_string(),
            leave,
            reset,
        });
        self.add_tab(ui, panel_id, label);
    }

    fn add_tab(self: &Rc<Self>, ui: &mut Ui, id: &str, label: &str) {
        let tab = ui.clone_widget(self.tab_template);
        ui.set_visible_fn(tab, || true);
        if let Some(&last) = self.buttons.borrow().last() {
            let lb = ui.node(last).bounds;
            ui.node_mut(tab).bounds.x = lb.x + self.stride.0;
            ui.node_mut(tab).bounds.y = lb.y + self.stride.1;
        }
        ui.node_mut(tab).id = Some(id.to_string());
        let (label, active, id_text) = (label.to_string(), self.active.clone(), id.to_string());
        let shell = self.clone();
        let id_click = id.to_string();
        {
            let mut b = ui.button(tab);
            b.get_text = Some(Rc::new(move || label.clone()));
            b.is_highlighted = Some(Rc::new(move || *active.borrow() == id_text));
            b.on_click = Some(Rc::new(move |ui| {
                shell.leave_active(ui);
                save(ui);
                *shell.active.borrow_mut() = id_click.clone();
            }));
        }
        ui.add_child(self.tab_container, tab);
        self.buttons.borrow_mut().push(tab);
    }

    fn active_panel<T>(&self, f: impl Fn(&RegisteredPanel) -> T) -> Option<T> {
        let active = self.active.borrow();
        self.panels.borrow().iter().find(|p| p.id == *active).map(f)
    }

    fn leave_active(&self, ui: &mut Ui) {
        if let Some(leave) = self.active_panel(|p| p.leave.clone()) {
            let restart = leave(ui);
            self.needs_restart.set(self.needs_restart.get() | restart);
        }
    }
}

fn save(ui: &Ui) {
    if let Err(e) = services(ui).settings.borrow_mut().save() {
        log::warn!("Failed to save the settings: {e}");
    }
}

fn logic_arg<'a>(logic_args: &'a [MiniYamlNode], key: &str) -> Option<&'a MiniYamlNode> {
    logic_args.iter().find(|n| n.key == key)
}

/// `SettingsLogic`.
pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let on_exit = args.action("onExit");
    let panel_container = ui.child(widget, "PANEL_CONTAINER");
    let panel_template = ui.child(panel_container, "PANEL_TEMPLATE");
    let tab_container = ui.child(widget, "SETTINGS_TAB_CONTAINER");
    let tab_template = ui.child(tab_container, "BUTTON_TEMPLATE");
    ui.hide_child(panel_container, panel_template);
    ui.hide_child(tab_container, tab_template);
    let stride = logic_arg(logic_args, "ButtonStride").map_or((0, 0), |n| {
        let v: Vec<i32> = n.value.value_str().split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0))
    });
    let panels: Vec<(String, String)> = logic_arg(logic_args, "Panels")
        .map(|n| {
            n.value
                .nodes
                .iter()
                .map(|p| (p.key.clone(), p.value.value_str().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut panel_containers = HashMap::new();
    for (id, _) in &panels {
        let container = ui.clone_widget(panel_template);
        ui.set_visible_fn(container, || true);
        ui.node_mut(container).id = Some(id.clone());
        ui.add_child(panel_container, container);
        panel_containers.insert(id.clone(), container);
    }
    let shell = Rc::new(SettingsShell {
        panel_containers: panel_containers.clone(),
        tab_container,
        tab_template,
        stride,
        buttons: RefCell::new(Vec::new()),
        panels: RefCell::new(Vec::new()),
        active: Rc::new(RefCell::new(String::new())),
        needs_restart: Cell::new(false),
    });
    for (id, label) in &panels {
        let args = WidgetArgs::default()
            .with_value("settingsShell", shell.clone())
            .with_value("panelID", id.clone())
            .with_value("label", label.clone());
        ui.load_widget(id, Some(panel_containers[id]), &args);
    }

    let back = ui.child(widget, "BACK_BUTTON");
    let s = shell.clone();
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        s.leave_active(ui);
        save(ui);
        let on_exit = on_exit.clone();
        let close_and_exit: Action = Rc::new(move |ui| {
            ui.close_window();
            on_exit(ui);
        });
        if s.needs_restart.get() {
            let text = message(ui, "dialog-settings-save.prompt");
            button_prompt_with_cancel(
                ui,
                "dialog-settings-save.title",
                &text,
                Rc::new(|_| {}),
                "",
                Some(close_and_exit),
                Some("dialog-settings-save.cancel"),
            );
        } else {
            close_and_exit(ui);
        }
    }));

    let reset = ui.child(widget, "RESET_BUTTON");
    let s = shell.clone();
    ui.button(reset).on_click = Some(Rc::new(move |ui| {
        let Some((label, reset)) = s.active_panel(|p| (p.label.clone(), p.reset.clone())) else {
            return;
        };
        let title = message_with(ui, "dialog-settings-reset.title", &[("panel", MessageArg::Text(&label))]);
        let text = message(ui, "dialog-settings-reset.prompt");
        let on_confirm: Action = Rc::new(move |ui| {
            reset(ui);
            save(ui);
        });
        reset_prompt(ui, &title, &text, on_confirm);
    }));
    cell(NoLogic)
}

/// `ConfirmationDialogs.ButtonPrompt` with a title that has arguments.
fn reset_prompt(ui: &mut Ui, title: &str, text: &str, on_confirm: Action) {
    button_prompt_with_cancel(
        ui,
        "dialog-settings-reset.title",
        text,
        on_confirm,
        "dialog-settings-reset.confirm",
        Some(Rc::new(|_| {})),
        Some("dialog-settings-reset.cancel"),
    );
    if let Some(window) = ui.current_window()
        && let Some(label) = ui.find(window, "PROMPT_TITLE")
    {
        let title = title.to_string();
        ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || title.clone()));
    }
}

/// The arguments that `SettingsLogic` gives each panel logic.
struct PanelArgs {
    shell: Rc<SettingsShell>,
    panel_id: String,
    label: String,
}

fn panel_args(args: &WidgetArgs) -> Option<PanelArgs> {
    Some(PanelArgs {
        shell: args.value::<Rc<SettingsShell>>("settingsShell")?,
        panel_id: args.value::<String>("panelID")?,
        label: args.value::<String>("label")?,
    })
}

type Get<T> = fn(&Settings) -> T;
type Set<T> = fn(&mut Settings, T);

/// Port of `SettingsUtils.BindCheckboxPref`.
fn bind_checkbox(ui: &mut Ui, panel: WidgetId, name: &str, get: Get<bool>, set: Set<bool>) -> WidgetId {
    let id = ui.child(panel, name);
    let settings = services(ui).settings.clone();
    let s = settings.clone();
    let mut cb = ui.get::<CheckboxWidget>(id);
    cb.is_checked = Some(Rc::new(move || get(&s.borrow())));
    cb.button.on_click = Some(Rc::new(move |_| {
        let value = get(&settings.borrow());
        set(&mut settings.borrow_mut(), !value);
    }));
    drop(cb);
    id
}

/// Appends an action to the `OnClick` of a checkbox.
fn after_click(ui: &mut Ui, id: WidgetId, action: impl Fn(&mut Ui) + 'static) {
    let mut cb = ui.get::<CheckboxWidget>(id);
    let previous = cb.button.on_click.clone();
    cb.button.on_click = Some(Rc::new(move |ui| {
        if let Some(previous) = &previous {
            previous(ui);
        }
        action(ui);
    }));
}

/// Port of `SettingsUtils.BindSliderPref` with the `OnChange` of the panel.
fn bind_slider(ui: &mut Ui, panel: WidgetId, name: &str, get: Get<f32>, set: Set<f32>, on_change: impl Fn(&mut Ui, f32) + 'static) {
    let id = ui.child(panel, name);
    let settings = services(ui).settings.clone();
    let value = get(&settings.borrow());
    let mut slider = ui.get::<SliderWidget>(id);
    slider.value = value;
    slider.on_change = Some(Rc::new(move |ui, x| {
        set(&mut settings.borrow_mut(), x);
        on_change(ui, x);
    }));
}

macro_rules! checkbox {
    ($ui:expr, $panel:expr, $name:expr, $($field:ident).+) => {
        bind_checkbox($ui, $panel, $name, |s: &Settings| s.$($field).+, |s: &mut Settings, v| s.$($field).+ = v)
    };
}

macro_rules! slider {
    ($ui:expr, $panel:expr, $name:expr, $($field:ident).+) => {
        slider!($ui, $panel, $name, $($field).+, |_: &mut Ui, _: f32| {})
    };
    ($ui:expr, $panel:expr, $name:expr, $($field:ident).+, $on_change:expr) => {
        bind_slider($ui, $panel, $name, |s: &Settings| s.$($field).+ as f32, |s: &mut Settings, v| s.$($field).+ = v as _, $on_change)
    };
}

/// A drop down of labeled values, as `ShowDropDown` with `LABEL_DROPDOWN_TEMPLATE` builds it.
fn show_options<T: Clone + PartialEq + 'static>(
    ui: &mut Ui,
    dropdown: WidgetId,
    max_height: i32,
    options: Vec<(String, T)>,
    get: Rc<dyn Fn() -> T>,
    set: Rc<dyn Fn(&mut Ui, T)>,
) {
    DropDownButtonWidget::show_dropdown(
        ui,
        dropdown,
        "LABEL_DROPDOWN_TEMPLATE",
        max_height,
        options,
        |ui, (label, value), template| {
            let (get, set, selected) = (get.clone(), set.clone(), value.clone());
            let item = ScrollItemWidget::setup(
                ui,
                template,
                Rc::new(move || get() == selected),
                Rc::new(move |ui| set(ui, value.clone())),
            );
            let l = ui.child(item, "LABEL");
            ui.label(l).get_text = Some(Rc::new(move || label.clone()));
            item
        },
    );
}

/// Binds a drop down of a setting: `options` builds the choices when it opens, `text` names the current value.
fn bind_dropdown<T: Clone + PartialEq + 'static>(
    ui: &mut Ui,
    panel: WidgetId,
    name: &str,
    options: impl Fn(&Ui) -> Vec<(String, T)> + 'static,
    get: Get<T>,
    set: Rc<dyn Fn(&mut Ui, T)>,
    text: Rc<dyn Fn(&T) -> String>,
) -> WidgetId {
    let id = ui.child(panel, name);
    let settings = services(ui).settings.clone();
    let s = settings.clone();
    let mut b = ui.button(id);
    b.get_text = Some(Rc::new(move || text(&get(&s.borrow()))));
    b.on_mouse_down = Some(Rc::new(move |ui, _| {
        let s = settings.clone();
        let options = options(ui);
        show_options(ui, id, 500, options, Rc::new(move || get(&s.borrow())), set.clone());
    }));
    drop(b);
    id
}

/// A `text` for `bind_dropdown` that names a value by the options of the drop down.
fn option_text<T: PartialEq + 'static>(options: Vec<(String, T)>) -> Rc<dyn Fn(&T) -> String> {
    Rc::new(move |v| options.iter().find(|(_, o)| o == v).map(|(l, _)| l.clone()).unwrap_or_default())
}

fn set_with<T: 'static>(set: Set<T>) -> Rc<dyn Fn(&mut Ui, T)> {
    Rc::new(move |ui, v| set(&mut services(ui).settings.borrow_mut(), v))
}

/// Port of `SettingsUtils.AdjustSettingsScrollPanelLayout`.
fn adjust_scroll_panel_layout(ui: &mut Ui, scroll_panel: WidgetId) {
    for row in ui.node(scroll_panel).children.clone() {
        let children = ui.node(row).children.clone();
        if !children.is_empty() && !children.iter().any(|&c| ui.is_visible(c)) {
            ui.node_mut(row).visible = false;
        }
    }
    ui.adjust_children(scroll_panel);
}

fn set_visible(ui: &mut Ui, panel: WidgetId, name: &str, f: impl Fn() -> bool + 'static) {
    if let Some(id) = ui.find(panel, name) {
        ui.set_visible_fn(id, f);
    }
}

fn settings_getter<T: 'static>(ui: &Ui, get: Get<T>) -> impl Fn() -> T + 'static {
    let settings = services(ui).settings.clone();
    move || get(&settings.borrow())
}

/// The values that need a restart, as the static constructors of the panel logics record them on first use.
struct OriginalSettings {
    mode: WindowMode,
    video_display: i32,
    windowed_size: (i32, i32),
    fullscreen_size: (i32, i32),
    gl_profile: GlProfile,
    discover_nat_devices: bool,
}

static ORIGINAL: OnceLock<OriginalSettings> = OnceLock::new();

fn original(ui: &Ui) -> &'static OriginalSettings {
    ORIGINAL.get_or_init(|| {
        let settings_rc = services(ui).settings.clone();
        let s = settings_rc.borrow();
        OriginalSettings {
            mode: s.graphics.mode,
            video_display: s.graphics.video_display,
            windowed_size: s.graphics.windowed_size,
            fullscreen_size: s.graphics.fullscreen_size,
            gl_profile: s.graphics.gl_profile,
            discover_nat_devices: s.server.discover_nat_devices,
        }
    })
}

/// The minimum resolution of `WorldViewportSizes`.
const MIN_EFFECTIVE_RESOLUTION: (f32, f32) = (1024.0, 720.0);

/// Port of `DisplaySettingsLogic.GetViewportSizeName`.
pub fn viewport_size_name(ui: &Ui, distance: WorldViewport) -> String {
    message(
        ui,
        match distance {
            WorldViewport::Close => "options-camera.close",
            WorldViewport::Medium => "options-camera.medium",
            WorldViewport::Far => "options-camera.far",
            WorldViewport::Native => "options-camera.furthest",
        },
    )
}

/// The logic of the display panel, which tracks the values that its getters cannot read.
struct DisplayPanel {
    restart: Rc<Cell<bool>>,
    frame_limit_text: Rc<RefCell<String>>,
    window_width: WidgetId,
    window_height: WidgetId,
    original_text: (String, String),
}

impl ChromeLogic for DisplayPanel {
    fn tick(&mut self, ui: &mut Ui) {
        let o = original(ui);
        let s = services(ui).settings.borrow().graphics.clone();
        let width = ui.get::<TextFieldWidget>(self.window_width).text();
        let height = ui.get::<TextFieldWidget>(self.window_height).text();
        self.restart.set(
            s.mode != o.mode
                || s.video_display != o.video_display
                || s.gl_profile != o.gl_profile
                || (s.mode == WindowMode::Windowed && (self.original_text.0 != width || self.original_text.1 != height)),
        );
        *self.frame_limit_text.borrow_mut() = message_with(
            ui,
            "checkbox-frame-limiter",
            &[("fps", MessageArg::Number(i64::from(s.max_framerate)))],
        );
    }
}

/// `DisplaySettingsLogic`.
pub fn display(ui: &mut Ui, panel: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(pa) = panel_args(args) else {
        return cell(NoLogic);
    };
    original(ui);
    let in_game = services(ui).ingame.borrow().is_some();
    let scroll_panel = ui.child(panel, "SETTINGS_SCROLLPANEL");
    checkbox!(ui, panel, "CURSORDOUBLE_CHECKBOX", graphics.cursor_double);
    checkbox!(ui, panel, "VSYNC_CHECKBOX", graphics.vsync);
    let frame_limit = checkbox!(ui, panel, "FRAME_LIMIT_CHECKBOX", graphics.cap_framerate);
    checkbox!(ui, panel, "FRAME_LIMIT_GAMESPEED_CHECKBOX", graphics.cap_framerate_to_game_fps);
    slider!(ui, panel, "FRAME_LIMIT_SLIDER", graphics.max_framerate);
    checkbox!(ui, panel, "PLAYER_STANCE_COLORS_CHECKBOX", game.use_player_stance_colors);
    if ui.find(panel, "PAUSE_SHELLMAP_CHECKBOX").is_some() {
        checkbox!(ui, panel, "PAUSE_SHELLMAP_CHECKBOX", game.pause_shellmap);
    }
    checkbox!(ui, panel, "HIDE_REPLAY_CHAT_CHECKBOX", game.hide_replay_chat);

    let modes = vec![
        (message(ui, "options-display-mode.fullscreen"), WindowMode::PseudoFullscreen),
        (message(ui, "options-display-mode.legacy-fullscreen"), WindowMode::Fullscreen),
        (message(ui, "options-display-mode.windowed"), WindowMode::Windowed),
    ];
    let mode_options = modes.clone();
    bind_dropdown(
        ui,
        panel,
        "MODE_DROPDOWN",
        move |_| mode_options.clone(),
        |s| s.graphics.mode,
        Rc::new(move |ui, v| {
            services(ui).settings.borrow_mut().graphics.mode = v;
            adjust_scroll_panel_layout(ui, scroll_panel);
        }),
        option_text(modes),
    );

    let display_label = message_with(ui, "label-video-display-index", &[("number", MessageArg::Number(1))]);
    let display = bind_dropdown(
        ui,
        panel,
        "DISPLAY_SELECTION_DROPDOWN",
        |_| vec![("Display 1".to_string(), 0)],
        |s| s.graphics.video_display,
        set_with(|s, v| s.graphics.video_display = v),
        Rc::new(move |_| display_label.clone()),
    );
    ui.button(display).is_disabled = Some(Rc::new(|| true));

    let gl_profile = bind_dropdown(
        ui,
        panel,
        "GL_PROFILE_DROPDOWN",
        |_| vec![("Automatic".to_string(), GlProfile::Automatic)],
        |s| s.graphics.gl_profile,
        set_with(|s, v| s.graphics.gl_profile = v),
        Rc::new(|p: &GlProfile| p.format_setting()),
    );
    let disable_profile = services(ui).settings.borrow().graphics.gl_profile == GlProfile::Automatic;
    ui.button(gl_profile).is_disabled = Some(Rc::new(move || disable_profile));

    let status_bars = vec![
        (message(ui, "options-status-bars.standard"), StatusBarsType::Standard),
        (message(ui, "options-status-bars.show-on-damage"), StatusBarsType::DamageShow),
        (message(ui, "options-status-bars.always-show"), StatusBarsType::AlwaysShow),
    ];
    let o = status_bars.clone();
    bind_dropdown(
        ui,
        panel,
        "STATUS_BAR_DROPDOWN",
        move |_| o.clone(),
        |s| s.game.status_bars,
        set_with(|s, v| s.game.status_bars = v),
        option_text(status_bars),
    );

    let target_lines = vec![
        (message(ui, "options-target-lines.automatic"), TargetLinesType::Automatic),
        (message(ui, "options-target-lines.manual"), TargetLinesType::Manual),
        (message(ui, "options-target-lines.disabled"), TargetLinesType::Disabled),
    ];
    let o = target_lines.clone();
    bind_dropdown(
        ui,
        panel,
        "TARGET_LINES_DROPDOWN",
        move |_| o.clone(),
        |s| s.game.target_lines,
        set_with(|s, v| s.game.target_lines = v),
        option_text(target_lines),
    );

    let camera_names: Vec<(String, WorldViewport)> = [
        WorldViewport::Close,
        WorldViewport::Medium,
        WorldViewport::Far,
        WorldViewport::Native,
    ]
    .into_iter()
    .map(|v| (viewport_size_name(ui, v), v))
    .collect();
    let all_cameras = camera_names.clone();
    bind_dropdown(
        ui,
        panel,
        "BATTLEFIELD_CAMERA_DROPDOWN",
        move |ui| {
            let window_height = ui.resolution.1 as f32;
            all_cameras
                .iter()
                .filter(|(_, v)| crate::viewport::valid_viewport_distance(*v, window_height))
                .cloned()
                .collect()
        },
        |s| s.graphics.viewport_distance,
        set_with(|s, v| s.graphics.viewport_distance = v),
        option_text(camera_names),
    );

    bind_text_notification_pool_filters(ui, panel);

    let scale_options = |ui: &Ui| -> Vec<(String, f32)> {
        let scale = services(ui).settings.borrow().graphics.ui_scale;
        let window = (ui.resolution.0 as f32 * scale, ui.resolution.1 as f32 * scale);
        let max_scale = (window.0 / MIN_EFFECTIVE_RESOLUTION.0).min(window.1 / MIN_EFFECTIVE_RESOLUTION.1);
        [1.0f32, 1.25, 1.5, 1.75, 2.0]
            .into_iter()
            .filter(|&x| x <= max_scale)
            .map(|x| (format!("{}%", (100.0 * x) as i32), x))
            .collect()
    };
    let ui_scale = bind_dropdown(
        ui,
        panel,
        "UI_SCALE_DROPDOWN",
        scale_options,
        |s| s.graphics.ui_scale,
        Rc::new(|ui: &mut Ui, v: f32| {
            if services(ui).settings.borrow().graphics.ui_scale != v {
                services(ui).settings.borrow_mut().graphics.ui_scale = v;
                ui.requests.push(AppRequest::SetUiScale(v));
            }
        }),
        Rc::new(|s: &f32| format!("{}%", (100.0 * s) as i32)),
    );
    let scale = services(ui).settings.borrow().graphics.ui_scale;
    let disable_ui_scale = in_game
        || ui.resolution.0 as f32 * scale < 1.25 * MIN_EFFECTIVE_RESOLUTION.0
        || ui.resolution.1 as f32 * scale < 1.25 * MIN_EFFECTIVE_RESOLUTION.1;
    ui.button(ui_scale).is_disabled = Some(Rc::new(move || disable_ui_scale));

    let mode = settings_getter(ui, |s| s.graphics.mode);
    set_visible(ui, panel, "DISPLAY_SELECTION_CONTAINER", move || mode() != WindowMode::Windowed);
    let mode = settings_getter(ui, |s| s.graphics.mode);
    set_visible(ui, panel, "WINDOW_RESOLUTION_CONTAINER", move || mode() == WindowMode::Windowed);

    let (w, h) = services(ui).settings.borrow().graphics.windowed_size;
    let window_width = ui.child(panel, "WINDOW_WIDTH");
    let window_height = ui.child(panel, "WINDOW_HEIGHT");
    ui.get::<TextFieldWidget>(window_width).set_text(&w.to_string());
    ui.get::<TextFieldWidget>(window_height).set_text(&h.to_string());
    let restart = Rc::new(Cell::new(false));
    let r = restart.clone();
    set_visible(ui, panel, "VIDEO_RESTART_REQUIRED_DESC", move || r.get());

    let frame_limit_text = Rc::new(RefCell::new(String::new()));
    let t = frame_limit_text.clone();
    let gamespeed = settings_getter(ui, |s| s.graphics.cap_framerate_to_game_fps);
    {
        let mut cb = ui.get::<CheckboxWidget>(frame_limit);
        cb.button.get_text = Some(Rc::new(move || t.borrow().clone()));
        cb.button.is_disabled = Some(Rc::new(gamespeed));
    }
    let limit_on = settings_getter(ui, |s| s.graphics.cap_framerate);
    let gamespeed_on = settings_getter(ui, |s| s.graphics.cap_framerate_to_game_fps);
    let slider = ui.child(panel, "FRAME_LIMIT_SLIDER");
    ui.get::<SliderWidget>(slider).is_disabled = Some(Rc::new(move || !limit_on() || gamespeed_on()));

    let name_field = ui.child(panel, "PLAYERNAME");
    let esc_pressed = Rc::new(Cell::new(false));
    let player_name = services(ui).settings.borrow().player.name.clone();
    {
        let mut f = ui.get::<TextFieldWidget>(name_field);
        f.is_disabled = Some(Rc::new(move || in_game));
        f.set_text(&sanitized_player_name(&player_name));
        let esc = esc_pressed.clone();
        f.on_lose_focus = Some(Rc::new(move |ui| {
            if esc.get() {
                esc.set(false);
                return;
            }
            let text = ui.get::<TextFieldWidget>(name_field).text().trim().to_string();
            let settings = services(ui).settings.clone();
            if text.is_empty() {
                let name = sanitized_player_name(&settings.borrow().player.name);
                ui.get::<TextFieldWidget>(name_field).set_text(&name);
            } else {
                let name = sanitized_player_name(&text);
                ui.get::<TextFieldWidget>(name_field).set_text(&name);
                settings.borrow_mut().player.name = name;
            }
        }));
        let on_enter: KeyCallback = Rc::new(move |ui, _| {
            ui.yield_keyboard_focus(name_field);
            true
        });
        f.on_enter_key = Some(on_enter);
        let esc = esc_pressed.clone();
        let on_esc: KeyCallback = Rc::new(move |ui, _| {
            let name = sanitized_player_name(&services(ui).settings.borrow().player.name);
            ui.get::<TextFieldWidget>(name_field).set_text(&name);
            esc.set(true);
            ui.yield_keyboard_focus(name_field);
            true
        });
        f.on_esc_key = Some(on_esc);
    }

    let color_dropdown = ui.child(panel, "PLAYERCOLOR");
    {
        let mut b = ui.button(color_dropdown);
        b.is_disabled = Some(Rc::new(move || in_game));
        b.on_mouse_down = Some(Rc::new(move |ui, _| {
            let color = services(ui).settings.borrow().player.color;
            super::color_picker::show_color_dropdown(
                ui,
                color_dropdown,
                color,
                Rc::new(|ui, color| {
                    services(ui).settings.borrow_mut().player.color = color;
                    save(ui);
                }),
            );
        }));
    }
    let color_block = ui.child(color_dropdown, "COLORBLOCK");
    let color = settings_getter(ui, |s| s.player.color);
    ui.get::<ColorBlockWidget>(color_block).get_color = Some(Rc::new(color));

    adjust_scroll_panel_layout(ui, scroll_panel);

    let leave: LeaveAction = Rc::new(move |ui| {
        let x = ui.get::<TextFieldWidget>(window_width).text().parse().unwrap_or(0);
        let y = ui.get::<TextFieldWidget>(window_height).text().parse().unwrap_or(0);
        ui.yield_keyboard_focus(name_field);
        let o = original(ui);
        let settings_rc = services(ui).settings.clone();
        let mut settings = settings_rc.borrow_mut();
        let g = &mut settings.graphics;
        g.windowed_size = (x, y);
        g.mode != o.mode
            || g.video_display != o.video_display
            || g.windowed_size != o.windowed_size
            || g.fullscreen_size != o.fullscreen_size
            || g.gl_profile != o.gl_profile
    });
    let reset: Action = Rc::new(|ui| {
        let settings_rc = services(ui).settings.clone();
        let mut settings = settings_rc.borrow_mut();
        let d = Settings::load("", &[]);
        let g = &mut settings.graphics;
        g.cap_framerate = d.graphics.cap_framerate;
        g.max_framerate = d.graphics.max_framerate;
        g.cap_framerate_to_game_fps = d.graphics.cap_framerate_to_game_fps;
        g.gl_profile = d.graphics.gl_profile;
        g.mode = d.graphics.mode;
        g.video_display = d.graphics.video_display;
        g.windowed_size = d.graphics.windowed_size;
        g.cursor_double = d.graphics.cursor_double;
        g.viewport_distance = d.graphics.viewport_distance;
        g.ui_scale = d.graphics.ui_scale;
        settings.player.color = d.player.color;
        settings.player.name = d.player.name;
        settings.game.text_notification_pool_filters = d.game.text_notification_pool_filters;
    });
    pa.shell.register(ui, &pa.panel_id, &pa.label, leave, reset);
    cell(DisplayPanel {
        restart,
        frame_limit_text,
        window_width,
        window_height,
        original_text: (w.to_string(), h.to_string()),
    })
}

/// Port of `DisplaySettingsLogic.BindTextNotificationPoolFilterSettings`.
pub fn bind_text_notification_pool_filters(ui: &mut Ui, panel: WidgetId) {
    for (name, flag) in [
        ("UI_FEEDBACK_CHECKBOX", TextNotificationPoolFilters::Feedback),
        ("TRANSIENTS_CHECKBOX", TextNotificationPoolFilters::Transients),
    ] {
        let Some(id) = ui.find(panel, name) else {
            continue;
        };
        let settings = services(ui).settings.clone();
        let s = settings.clone();
        let mut cb = ui.get::<CheckboxWidget>(id);
        cb.is_checked = Some(Rc::new(move || s.borrow().game.text_notification_pool_filters.has(flag)));
        cb.button.on_click = Some(Rc::new(move |ui| {
            settings.borrow_mut().game.text_notification_pool_filters.0 ^= flag.0;
            save(ui);
        }));
    }
}

/// `AudioSettingsLogic`. Only the default output device is offered.
pub fn audio(ui: &mut Ui, panel: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(pa) = panel_args(args) else {
        return cell(NoLogic);
    };
    let scroll_panel = ui.child(panel, "SETTINGS_SCROLLPANEL");
    checkbox!(ui, panel, "CASH_TICKS", sound.cash_ticks);
    let mute = checkbox!(ui, panel, "MUTE_SOUND", sound.mute);
    let mute_music = checkbox!(ui, panel, "MUTE_BACKGROUND_MUSIC", sound.mute_background_music);
    slider!(ui, panel, "SOUND_VOLUME", sound.sound_volume, |ui: &mut Ui, x| services(ui)
        .sound
        .set_sound_volume(x));
    slider!(ui, panel, "MUSIC_VOLUME", sound.music_volume, |ui: &mut Ui, x| services(ui)
        .sound
        .set_music_volume(x));
    slider!(ui, panel, "VIDEO_VOLUME", sound.video_volume, |ui: &mut Ui, x| services(ui)
        .sound
        .set_video_volume(x));
    after_click(ui, mute, |ui| {
        let sound = services(ui).sound.clone();
        if services(ui).settings.borrow().sound.mute {
            sound.mute_audio();
        } else {
            sound.unmute_audio();
        }
    });
    after_click(ui, mute_music, |ui| {
        let Some(music) = services(ui).music.borrow().clone() else {
            return;
        };
        if music.allow_mute_background_music() && music.current_song_is_background() {
            music.stop();
        }
    });
    if let Some(id) = ui.find(panel, "NO_AUDIO_DEVICE_CONTAINER") {
        ui.node_mut(id).visible = false;
    }
    let devices = vec![("Default Output".to_string(), None::<String>)];
    let default_label = devices[0].0.clone();
    let device = ui.child(panel, "AUDIO_DEVICE");
    {
        let font = ui.button(device).font.clone();
        let width = ui.node(device).bounds.w;
        let label = ui.truncate_text(&font, &default_label, width);
        let mut b = ui.button(device);
        b.get_text = Some(Rc::new(move || label.clone()));
        b.on_mouse_down = Some(Rc::new(move |ui, _| {
            show_options(
                ui,
                device,
                500,
                devices.clone(),
                Rc::new(|| None),
                Rc::new(move |ui, _| adjust_scroll_panel_layout(ui, scroll_panel)),
            );
        }));
    }
    set_visible(ui, panel, "AUDIO_RESTART_REQUIRED_DESC", || false);
    adjust_scroll_panel_layout(ui, scroll_panel);

    let leave: LeaveAction = Rc::new(|ui| {
        services(ui).settings.borrow_mut().sound.device = None;
        false
    });
    let reset: Action = Rc::new(move |ui| {
        let d = Settings::load("", &[]).sound;
        {
            let settings_rc = services(ui).settings.clone();
            let mut settings = settings_rc.borrow_mut();
            let s = &mut settings.sound;
            s.sound_volume = d.sound_volume;
            s.music_volume = d.music_volume;
            s.video_volume = d.video_volume;
            s.cash_ticks = d.cash_ticks;
            s.mute = d.mute;
            s.mute_background_music = d.mute_background_music;
            s.device = d.device.clone();
        }
        for (name, value) in [
            ("SOUND_VOLUME", d.sound_volume),
            ("MUSIC_VOLUME", d.music_volume),
            ("VIDEO_VOLUME", d.video_volume),
        ] {
            let id = ui.child(panel, name);
            ui.get::<SliderWidget>(id).value = value;
        }
        let sound = services(ui).sound.clone();
        sound.set_sound_volume(d.sound_volume);
        sound.set_music_volume(d.music_volume);
        sound.set_video_volume(d.video_volume);
        sound.unmute_audio();
    });
    pa.shell.register(ui, &pa.panel_id, &pa.label, leave, reset);
    cell(NoLogic)
}

/// `ModifiersExts.DisplayString`.
fn modifier_display(m: ModifierFlags) -> String {
    if m == ModifierFlags::Meta && cfg!(target_os = "macos") {
        "Cmd".into()
    } else {
        m.format_setting()
    }
}

/// `InputSettingsLogic`. `LockMouseWindow` is stored but not applied to the window.
pub fn input(ui: &mut Ui, panel: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(pa) = panel_args(args) else {
        return cell(NoLogic);
    };
    let scroll_panel = ui.child(panel, "SETTINGS_SCROLLPANEL");
    checkbox!(ui, panel, "ALTERNATE_SCROLL_CHECKBOX", game.use_alternate_scroll_button);
    checkbox!(ui, panel, "EDGESCROLL_CHECKBOX", game.viewport_edge_scroll);
    checkbox!(ui, panel, "LOCKMOUSE_CHECKBOX", game.lock_mouse_window);
    slider!(ui, panel, "ZOOMSPEED_SLIDER", game.zoom_speed);
    slider!(ui, panel, "SCROLLSPEED_SLIDER", game.viewport_edge_scroll_step);
    slider!(ui, panel, "UI_SCROLLSPEED_SLIDER", game.ui_scroll_speed);

    let control = vec![
        (message(ui, "options-control-scheme.classic"), true),
        (message(ui, "options-control-scheme.modern"), false),
    ];
    let o = control.clone();
    bind_dropdown(
        ui,
        panel,
        "MOUSE_CONTROL_DROPDOWN",
        move |_| o.clone(),
        |s| s.game.use_classic_mouse_style,
        set_with(|s, v| s.game.use_classic_mouse_style = v),
        option_text(control),
    );
    let scroll_types = vec![
        (message(ui, "options-mouse-scroll-type.disabled"), MouseScrollType::Disabled),
        (message(ui, "options-mouse-scroll-type.standard"), MouseScrollType::Standard),
        (message(ui, "options-mouse-scroll-type.inverted"), MouseScrollType::Inverted),
        (message(ui, "options-mouse-scroll-type.joystick"), MouseScrollType::Joystick),
    ];
    bind_dropdown(
        ui,
        panel,
        "MOUSE_SCROLL_TYPE_DROPDOWN",
        move |_| scroll_types.clone(),
        |s| s.game.mouse_scroll,
        set_with(|s, v| s.game.mouse_scroll = v),
        Rc::new(|v: &MouseScrollType| v.format_setting()),
    );

    let classic = settings_getter(ui, |s| s.game.use_classic_mouse_style);
    set_visible(ui, panel, "MOUSE_CONTROL_DESC_CLASSIC", classic);
    let classic = settings_getter(ui, |s| s.game.use_classic_mouse_style);
    set_visible(ui, panel, "MOUSE_CONTROL_DESC_MODERN", move || !classic());
    for name in ["MOUSE_CONTROL_DESC_CLASSIC", "MOUSE_CONTROL_DESC_MODERN"] {
        let container = ui.child(panel, name);
        let scroll = settings_getter(ui, |s| s.game.use_classic_mouse_style ^ s.game.use_alternate_scroll_button);
        set_visible(ui, container, "DESC_SCROLL_RIGHT", scroll);
        let scroll = settings_getter(ui, |s| !s.game.use_classic_mouse_style ^ s.game.use_alternate_scroll_button);
        set_visible(ui, container, "DESC_SCROLL_MIDDLE", scroll);
        let zoom = settings_getter(ui, |s| s.game.zoom_modifier);
        set_visible(ui, container, "DESC_ZOOM", move || zoom() == ModifierFlags::None);
        let zoom = settings_getter(ui, |s| s.game.zoom_modifier);
        set_visible(ui, container, "DESC_ZOOM_MODIFIER", move || zoom() != ModifierFlags::None);
        let modifier_label = ui.child(container, "DESC_ZOOM_MODIFIER");
        let key = ui.label(modifier_label).text.clone().unwrap_or_default();
        let template = message(ui, &key);
        let zoom = settings_getter(ui, |s| s.game.zoom_modifier);
        ui.label(modifier_label).get_text = Some(Rc::new(move || template.replace("MODIFIER", &zoom().format_setting())));
        let edge = settings_getter(ui, |s| s.game.viewport_edge_scroll);
        set_visible(ui, container, "DESC_EDGESCROLL", edge);
    }

    let modifiers: Vec<(String, ModifierFlags)> = [
        ModifierFlags::Alt,
        ModifierFlags::Ctrl,
        ModifierFlags::Meta,
        ModifierFlags::Shift,
        ModifierFlags::None,
    ]
    .into_iter()
    .map(|m| (modifier_display(m), m))
    .collect();
    bind_dropdown(
        ui,
        panel,
        "ZOOM_MODIFIER",
        move |_| modifiers.clone(),
        |s| s.game.zoom_modifier,
        set_with(|s, v| s.game.zoom_modifier = v),
        Rc::new(|m: &ModifierFlags| m.format_setting()),
    );
    adjust_scroll_panel_layout(ui, scroll_panel);

    let reset: Action = Rc::new(move |ui| {
        let d = Settings::load("", &[]).game;
        let (step, speed) = {
            let settings_rc = services(ui).settings.clone();
            let mut settings = settings_rc.borrow_mut();
            let g = &mut settings.game;
            g.use_classic_mouse_style = d.use_classic_mouse_style;
            g.mouse_scroll = d.mouse_scroll;
            g.use_alternate_scroll_button = d.use_alternate_scroll_button;
            g.lock_mouse_window = d.lock_mouse_window;
            g.viewport_edge_scroll = d.viewport_edge_scroll;
            g.viewport_edge_scroll_step = d.viewport_edge_scroll_step;
            g.zoom_speed = d.zoom_speed;
            g.ui_scroll_speed = d.ui_scroll_speed;
            g.zoom_modifier = d.zoom_modifier;
            (g.viewport_edge_scroll_step, g.ui_scroll_speed)
        };
        let id = ui.child(panel, "SCROLLSPEED_SLIDER");
        ui.get::<SliderWidget>(id).value = step;
        let id = ui.child(panel, "UI_SCROLLSPEED_SLIDER");
        ui.get::<SliderWidget>(id).value = speed;
    });
    pa.shell.register(ui, &pa.panel_id, &pa.label, Rc::new(|_| false), reset);
    cell(NoLogic)
}

/// `AdvancedSettingsLogic`.
pub fn advanced(ui: &mut Ui, panel: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(pa) = panel_args(args) else {
        return cell(NoLogic);
    };
    original(ui);
    let scroll_panel = ui.child(panel, "SETTINGS_SCROLLPANEL");
    checkbox!(ui, panel, "NAT_DISCOVERY", server.discover_nat_devices);
    checkbox!(ui, panel, "PERFTEXT_CHECKBOX", debug.perf_text);
    checkbox!(ui, panel, "PERFGRAPH_CHECKBOX", debug.perf_graph);
    checkbox!(ui, panel, "FETCH_NEWS_CHECKBOX", game.fetch_news);
    let sysinfo = checkbox!(ui, panel, "SENDSYSINFO_CHECKBOX", debug.send_system_information);
    checkbox!(ui, panel, "CHECK_VERSION_CHECKBOX", debug.check_version);
    let fetch_news = settings_getter(ui, |s| s.game.fetch_news);
    ui.get::<CheckboxWidget>(sysinfo).button.is_disabled = Some(Rc::new(move || !fetch_news()));
    checkbox!(ui, panel, "BOTDEBUG_CHECKBOX", debug.bot_debug);
    checkbox!(ui, panel, "LUADEBUG_CHECKBOX", debug.lua_debug);
    checkbox!(ui, panel, "REPLAY_COMMANDS_CHECKBOX", debug.enable_debug_commands_in_replays);
    checkbox!(ui, panel, "CHECKUNSYNCED_CHECKBOX", debug.sync_check_unsynced_code);
    checkbox!(ui, panel, "CHECKBOTSYNC_CHECKBOX", debug.sync_check_bot_module_code);
    checkbox!(ui, panel, "PERFLOGGING_CHECKBOX", debug.enable_simulation_perf_logging);
    for name in [
        "BOTDEBUG_CHECKBOX_CONTAINER",
        "CHECKUNSYNCED_CHECKBOX_CONTAINER",
        "CHECKBOTSYNC_CHECKBOX_CONTAINER",
        "LUADEBUG_CHECKBOX_CONTAINER",
        "REPLAY_COMMANDS_CHECKBOX_CONTAINER",
        "PERFLOGGING_CHECKBOX_CONTAINER",
    ] {
        let developer = settings_getter(ui, |s| s.debug.display_developer_settings);
        set_visible(ui, panel, name, developer);
    }
    let developer = settings_getter(ui, |s| s.debug.display_developer_settings);
    set_visible(ui, panel, "DEBUG_HIDDEN_CONTAINER", move || !developer());
    adjust_scroll_panel_layout(ui, scroll_panel);

    let leave: LeaveAction = Rc::new(|ui| services(ui).settings.borrow().server.discover_nat_devices != original(ui).discover_nat_devices);
    let reset: Action = Rc::new(|ui| {
        let d = Settings::load("", &[]);
        let settings_rc = services(ui).settings.clone();
        let mut settings = settings_rc.borrow_mut();
        settings.server.discover_nat_devices = d.server.discover_nat_devices;
        let s = &mut settings.debug;
        s.perf_text = d.debug.perf_text;
        s.perf_graph = d.debug.perf_graph;
        s.sync_check_unsynced_code = d.debug.sync_check_unsynced_code;
        s.sync_check_bot_module_code = d.debug.sync_check_bot_module_code;
        s.bot_debug = d.debug.bot_debug;
        s.lua_debug = d.debug.lua_debug;
        s.send_system_information = d.debug.send_system_information;
        s.check_version = d.debug.check_version;
        s.enable_debug_commands_in_replays = d.debug.enable_debug_commands_in_replays;
        s.enable_simulation_perf_logging = d.debug.enable_simulation_perf_logging;
    });
    pa.shell.register(ui, &pa.panel_id, &pa.label, leave, reset);
    cell(NoLogic)
}

/// `HotkeyDefinition.ContextFluentPrefix + "-any"`.
fn any_context() -> String {
    format!("{CONTEXT_FLUENT_PREFIX}-any")
}

/// The state of `HotkeysSettingsLogic`.
struct HotkeysPanel {
    panel: WidgetId,
    hotkey_list: WidgetId,
    header_template: WidgetId,
    template: WidgetId,
    entry: WidgetId,
    filter: WidgetId,
    groups: Vec<(String, Vec<String>)>,
    contexts: Vec<String>,
    current_context: RefCell<String>,
    selected: RefCell<Option<String>>,
    selected_button: Cell<Option<WidgetId>>,
    duplicate: RefCell<Option<String>>,
    is_valid: Rc<Cell<bool>>,
    is_default: Rc<Cell<bool>>,
    valid_width: Cell<i32>,
    invalid_width: Cell<i32>,
    label_text: Rc<RefCell<String>>,
    duplicate_text: Rc<RefCell<String>>,
    original_text: Rc<RefCell<String>>,
    selected_readonly: Rc<Cell<bool>>,
    duplicate_readonly: Rc<Cell<bool>>,
    entry_valid_key: Rc<Cell<bool>>,
}

fn current_hotkey(ui: &Ui, name: &str) -> Option<Hotkey> {
    ui.assets.borrow().hotkeys.get(name)
}

fn hotkey_text(ui: &Ui, hotkey: Option<&Hotkey>) -> String {
    let unknown = Hotkey {
        key: "UNKNOWN".into(),
        modifiers: Default::default(),
    };
    hotkey.unwrap_or(&unknown).display_string(&ui.assets.borrow().messages)
}

impl HotkeysPanel {
    fn set_entry_key(&self, ui: &mut Ui, key: Option<Hotkey>) {
        self.entry_valid_key.set(key.is_some());
        ui.get::<HotkeyEntryWidget>(self.entry).key = key;
    }

    /// Port of `BindHotkeyPref`.
    fn bind_hotkey_pref(self: &Rc<Self>, ui: &mut Ui, name: &str) {
        let (description, has_duplicates) = {
            let assets = ui.assets.borrow();
            let d = assets.hotkeys.definition(name).expect("a defined hotkey");
            (d.description.clone(), d.has_duplicates)
        };
        let key = ui.clone_widget(self.template);
        ui.node_mut(key).id = Some(name.to_string());
        ui.set_visible_fn(key, || true);
        let desc = format!("{}:", message(ui, &description));
        let function = ui.child(key, "FUNCTION");
        ui.label(function).get_text = Some(Rc::new(move || desc.clone()));
        let remap = ui.child(key, "HOTKEY");
        let text = hotkey_text(ui, current_hotkey(ui, name).as_ref());
        ButtonWidget::truncate_to_tooltip(ui, remap, &text);
        let (valid_color, invalid_color) = {
            let m = &ui.assets.borrow().metrics;
            (m.color("HotkeyColor"), m.color("HotkeyColorInvalid"))
        };
        let duplicates = Rc::new(Cell::new(has_duplicates));
        let this = Rc::downgrade(self);
        let n = name.to_string();
        {
            let mut b = ui.button(remap);
            let d = duplicates.clone();
            b.get_color = Some(Rc::new(move || if d.get() { invalid_color } else { valid_color }));
            let this2 = this.clone();
            let n2 = n.clone();
            b.is_highlighted = Some(Rc::new(move || {
                this2.upgrade().is_some_and(|t| t.selected.borrow().as_deref() == Some(n2.as_str()))
            }));
            b.on_click = Some(Rc::new(move |ui| {
                let Some(this) = this.upgrade() else { return };
                this.select(ui, &n, remap);
                if this.selected_readonly.get() {
                    ui.yield_keyboard_focus(this.entry);
                } else {
                    ui.take_keyboard_focus(this.entry);
                }
            }));
        }
        DUPLICATE_FLAGS.with(|f| f.borrow_mut().push((name.to_string(), duplicates)));
        if self.selected.borrow().as_deref() == Some(name) {
            self.select(ui, name, remap);
        }
        ui.add_child(self.hotkey_list, key);
    }

    fn select(&self, ui: &mut Ui, name: &str, button: WidgetId) {
        *self.selected.borrow_mut() = Some(name.to_string());
        self.selected_button.set(Some(button));
        let (description, readonly, default) = {
            let assets = ui.assets.borrow();
            let d = assets.hotkeys.definition(name).expect("a defined hotkey");
            (d.description.clone(), d.readonly, d.default.clone())
        };
        self.selected_readonly.set(readonly);
        *self.label_text.borrow_mut() = format!("{}:", message(ui, &description));
        *self.original_text.borrow_mut() = message_with(
            ui,
            "label-original-notice",
            &[("key", MessageArg::Text(&hotkey_text(ui, default.as_ref())))],
        );
        let key = current_hotkey(ui, name);
        self.set_entry_key(ui, key.clone());
        self.validate(ui, key);
    }

    /// Port of `ValidateHotkey`.
    fn validate(&self, ui: &mut Ui, key: Option<Hotkey>) {
        let Some(selected) = self.selected.borrow().clone() else {
            return;
        };
        self.entry_valid_key.set(key.is_some());
        let (duplicate, readonly, is_default, duplicate_text) = {
            let assets = ui.assets.borrow();
            let d = assets.hotkeys.definition(&selected).expect("a defined hotkey");
            let duplicate = assets.hotkeys.first_duplicate(&selected, key.as_ref()).cloned();
            let is_default = hotkeys_equal(key.as_ref(), d.default.as_ref()) || (key.is_none() && d.default.is_none());
            let duplicate_text = duplicate.as_ref().map(|dup| {
                let context = dup.contexts.iter().find(|c| d.contexts.contains(c)).cloned().unwrap_or_default();
                (dup.description.clone(), context)
            });
            (duplicate, d.readonly, is_default, duplicate_text)
        };
        *self.duplicate.borrow_mut() = duplicate.as_ref().map(|d| d.name.clone());
        self.duplicate_readonly.set(duplicate.as_ref().is_some_and(|d| d.readonly));
        *self.duplicate_text.borrow_mut() = duplicate_text.map_or_else(String::new, |(description, context)| {
            let (description, context) = (message(ui, &description), message(ui, &context));
            message_with(
                ui,
                "label-duplicate-notice",
                &[("key", MessageArg::Text(&description)), ("context", MessageArg::Text(&context))],
            )
        });
        self.is_valid.set(duplicate.is_none() || readonly);
        self.is_default.set(is_default);
        if self.is_valid.get() {
            ui.node_mut(self.entry).bounds.w = self.valid_width.get();
            self.save_hotkey(ui, &selected, key, readonly);
        } else {
            ui.node_mut(self.entry).bounds.w = if self.duplicate_readonly.get() {
                self.valid_width.get()
            } else {
                self.invalid_width.get()
            };
            ui.take_keyboard_focus(self.entry);
        }
    }

    /// Port of `SaveHotkey`.
    fn save_hotkey(&self, ui: &mut Ui, name: &str, key: Option<Hotkey>, readonly: bool) {
        if readonly {
            return;
        }
        if let Some(button) = self.selected_button.get() {
            let text = hotkey_text(ui, key.as_ref());
            ButtonWidget::truncate_to_tooltip(ui, button, &text);
        }
        set_hotkey(ui, name, key);
        save(ui);
    }

    /// Port of `InitHotkeyList`.
    fn init_hotkey_list(self: &Rc<Self>, ui: &mut Ui) {
        ui.remove_children(self.hotkey_list);
        DUPLICATE_FLAGS.with(|f| f.borrow_mut().clear());
        *self.selected.borrow_mut() = None;
        let filter = ui.get::<TextFieldWidget>(self.filter).text().to_lowercase();
        let context = self.current_context.borrow().clone();
        let definitions: Vec<(String, String, Vec<String>, Vec<String>)> = ui
            .assets
            .borrow()
            .hotkeys
            .definitions()
            .map(|d| (d.name.clone(), d.description.clone(), d.types.clone(), d.contexts.clone()))
            .collect();
        let visible = |ui: &Ui, description: &str, contexts: &[String]| {
            (filter.trim().is_empty() || message(ui, description).to_lowercase().contains(&filter))
                && (context == any_context() || contexts.contains(&context))
        };
        for (group, types) in &self.groups {
            let keys: Vec<&(String, String, Vec<String>, Vec<String>)> = definitions
                .iter()
                .filter(|(_, description, t, contexts)| visible(ui, description, contexts) && t.iter().any(|t| types.contains(t)))
                .collect();
            if keys.is_empty() {
                continue;
            }
            let header = ui.clone_widget(self.header_template);
            let group_name = message(ui, group);
            let label = ui.child(header, "LABEL");
            ui.label(label).get_text = Some(Rc::new(move || group_name.clone()));
            ui.add_child(self.hotkey_list, header);
            let mut added: Vec<&str> = Vec::new();
            for ty in types {
                for (name, ..) in keys.iter().filter(|k| k.2.contains(ty)).map(|k| &**k) {
                    if !added.contains(&name.as_str()) {
                        added.push(name);
                        if self.selected.borrow().is_none() {
                            *self.selected.borrow_mut() = Some(name.clone());
                        }
                        self.bind_hotkey_pref(ui, name);
                    }
                }
            }
        }
        let has_selection = self.selected.borrow().is_some();
        let empty = ui.child(self.panel, "HOTKEY_EMPTY_LIST");
        ui.node_mut(empty).visible = !has_selection;
        let dialog = ui.child(self.panel, "HOTKEY_REMAP_DIALOG");
        ui.node_mut(dialog).visible = has_selection;
        ui.scroll_to_top(self.hotkey_list, false);
    }
}

thread_local! {
    /// The `HasDuplicates` flag that each listed hotkey button shows, refreshed after each change.
    static DUPLICATE_FLAGS: RefCell<Vec<(String, Rc<Cell<bool>>)>> = const { RefCell::new(Vec::new()) };
}

/// `HotkeyManager.Set` with the refresh of the duplicate flags of the listed buttons.
fn set_hotkey(ui: &mut Ui, name: &str, key: Option<Hotkey>) {
    let settings = services(ui).settings.clone();
    let mut assets = ui.assets.borrow_mut();
    assets.hotkeys.set(name, key, &mut settings.borrow_mut().keys);
    DUPLICATE_FLAGS.with(|f| {
        for (n, flag) in f.borrow().iter() {
            flag.set(assets.hotkeys.definition(n).is_some_and(|d| d.has_duplicates));
        }
    });
}

impl ChromeLogic for Rc<HotkeysPanel> {}

/// `HotkeysSettingsLogic`.
pub fn hotkeys(ui: &mut Ui, panel: WidgetId, args: &WidgetArgs, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(pa) = panel_args(args) else {
        return cell(NoLogic);
    };
    let hotkey_list = ui.child(panel, "HOTKEY_LIST");
    ui.set_grid_layout(hotkey_list);
    let header = ui.child(hotkey_list, "HEADER");
    let template = ui.child(hotkey_list, "TEMPLATE");
    let header_template = ui.clone_widget(header);
    let template_copy = ui.clone_widget(template);
    ui.remove_children(hotkey_list);
    let mut contexts = vec![any_context()];
    for d in ui.assets.borrow().hotkeys.definitions() {
        for c in &d.contexts {
            if !contexts.contains(c) {
                contexts.push(c.clone());
            }
        }
    }
    let groups: Vec<(String, Vec<String>)> = logic_arg(logic_args, "HotkeyGroups")
        .map(|n| {
            n.value
                .nodes
                .iter()
                .filter_map(|g| {
                    let types = g.value.nodes.iter().find(|t| t.key == "Types")?;
                    let mut set: Vec<String> = Vec::new();
                    for t in types.value.value_str().split(',').map(str::trim).filter(|t| !t.is_empty()) {
                        if !set.iter().any(|s| s == t) {
                            set.push(t.to_string());
                        }
                    }
                    Some((g.key.clone(), set))
                })
                .collect()
        })
        .unwrap_or_default();
    let has_groups = logic_arg(logic_args, "HotkeyGroups").is_some();
    let entry = ui.child(panel, "HOTKEY_ENTRY");
    let filter = ui.child(panel, "FILTER_INPUT");
    let this = Rc::new(HotkeysPanel {
        panel,
        hotkey_list,
        header_template,
        template: template_copy,
        entry,
        filter,
        groups,
        contexts,
        current_context: RefCell::new(any_context()),
        selected: RefCell::new(None),
        selected_button: Cell::new(None),
        duplicate: RefCell::new(None),
        is_valid: Rc::new(Cell::new(false)),
        is_default: Rc::new(Cell::new(false)),
        valid_width: Cell::new(0),
        invalid_width: Cell::new(0),
        label_text: Rc::new(RefCell::new(String::new())),
        duplicate_text: Rc::new(RefCell::new(String::new())),
        original_text: Rc::new(RefCell::new(String::new())),
        selected_readonly: Rc::new(Cell::new(false)),
        duplicate_readonly: Rc::new(Cell::new(false)),
        entry_valid_key: Rc::new(Cell::new(false)),
    });

    {
        let weak = Rc::downgrade(&this);
        let mut f = ui.get::<TextFieldWidget>(filter);
        let w = weak.clone();
        f.on_text_edited = Some(Rc::new(move |ui| {
            if let Some(t) = w.upgrade() {
                t.init_hotkey_list(ui);
            }
        }));
        let on_esc: KeyCallback = Rc::new(move |ui, _| {
            if ui.get::<TextFieldWidget>(filter).text().is_empty() {
                ui.yield_keyboard_focus(filter);
            } else {
                ui.get::<TextFieldWidget>(filter).set_text("");
                if let Some(t) = weak.upgrade() {
                    t.init_hotkey_list(ui);
                }
            }
            true
        });
        f.on_esc_key = Some(on_esc);
    }

    if let Some(dropdown) = ui.find(panel, "CONTEXT_DROPDOWN") {
        let weak = Rc::downgrade(&this);
        let w = weak.clone();
        let names: Vec<(String, String)> = this.contexts.iter().map(|c| (message(ui, c), c.clone())).collect();
        let text_names = names.clone();
        let mut b = ui.button(dropdown);
        b.get_text = Some(Rc::new(move || {
            let current = w.upgrade().map(|t| t.current_context.borrow().clone()).unwrap_or_default();
            text_names
                .iter()
                .find(|(_, c)| *c == current)
                .map(|(n, _)| n.clone())
                .unwrap_or_default()
        }));
        b.on_mouse_down = Some(Rc::new(move |ui, _| {
            let Some(t) = weak.upgrade() else { return };
            ui.yield_keyboard_focus(t.entry);
            let (get_t, set_t) = (t.clone(), Rc::downgrade(&t));
            show_options(
                ui,
                dropdown,
                280,
                names.clone(),
                Rc::new(move || get_t.current_context.borrow().clone()),
                Rc::new(move |ui, context| {
                    if let Some(t) = set_t.upgrade() {
                        *t.current_context.borrow_mut() = context;
                        t.init_hotkey_list(ui);
                    }
                }),
            );
        }));
    }

    if has_groups {
        init_remap_dialog(ui, &this);
        this.init_hotkey_list(ui);
    }

    let t = this.clone();
    let leave: LeaveAction = Rc::new(move |ui| {
        let key = t.selected.borrow().as_deref().and_then(|n| current_hotkey(ui, n));
        t.set_entry_key(ui, key);
        HotkeyEntryWidget::force_yield_keyboard_focus(ui, t.entry);
        false
    });
    let reset: Action = Rc::new(move |ui| {
        let definitions: Vec<(String, Option<Hotkey>)> = ui
            .assets
            .borrow()
            .hotkeys
            .definitions()
            .map(|d| (d.name.clone(), d.default.clone()))
            .collect();
        for (name, default) in definitions {
            set_hotkey(ui, &name, default.clone());
            if let Some(button) = ui.find(panel, &name).and_then(|k| ui.find(k, "HOTKEY")) {
                let text = hotkey_text(ui, default.as_ref());
                ButtonWidget::truncate_to_tooltip(ui, button, &text);
            }
        }
    });
    pa.shell.register(ui, &pa.panel_id, &pa.label, leave, reset);
    cell(this)
}

/// Port of `InitHotkeyRemapDialog`.
fn init_remap_dialog(ui: &mut Ui, this: &Rc<HotkeysPanel>) {
    let panel = this.panel;
    let (error_color, info_color) = {
        let m = &ui.assets.borrow().metrics;
        (m.color("NoticeErrorColor"), m.color("NoticeInfoColor"))
    };
    let label = ui.child(panel, "HOTKEY_LABEL");
    let t = this.clone();
    ui.set_visible_fn(label, move || t.selected.borrow().is_some());
    let text = this.label_text.clone();
    ui.label(label).get_text = Some(Rc::new(move || text.borrow().clone()));

    let duplicate_notice = ui.child(panel, "DUPLICATE_NOTICE");
    ui.label(duplicate_notice).text_color = error_color;
    let valid = this.is_valid.clone();
    ui.set_visible_fn(duplicate_notice, move || !valid.get());
    let text = this.duplicate_text.clone();
    ui.label(duplicate_notice).get_text = Some(Rc::new(move || text.borrow().clone()));

    let original_notice = ui.child(panel, "ORIGINAL_NOTICE");
    ui.label(original_notice).text_color = info_color;
    let (valid, default) = (this.is_valid.clone(), this.is_default.clone());
    ui.set_visible_fn(original_notice, move || valid.get() && !default.get());
    let text = this.original_text.clone();
    ui.label(original_notice).get_text = Some(Rc::new(move || text.borrow().clone()));

    let readonly_notice = ui.child(panel, "READONLY_NOTICE");
    ui.label(readonly_notice).text_color = info_color;
    let readonly = this.selected_readonly.clone();
    ui.set_visible_fn(readonly_notice, move || readonly.get());

    let reset_button = ui.child(panel, "RESET_HOTKEY_BUTTON");
    let (default, readonly) = (this.is_default.clone(), this.selected_readonly.clone());
    ui.button(reset_button).is_disabled = Some(Rc::new(move || default.get() || readonly.get()));
    let t = Rc::downgrade(this);
    ui.button(reset_button).on_click = Some(Rc::new(move |ui| {
        let Some(t) = t.upgrade() else { return };
        let default = t
            .selected
            .borrow()
            .as_deref()
            .and_then(|n| ui.assets.borrow().hotkeys.definition(n).and_then(|d| d.default.clone()));
        t.set_entry_key(ui, default);
        ui.yield_keyboard_focus(t.entry);
    }));

    let clear_button = ui.child(panel, "CLEAR_HOTKEY_BUTTON");
    let (readonly, valid_key) = (this.selected_readonly.clone(), this.entry_valid_key.clone());
    ui.button(clear_button).is_disabled = Some(Rc::new(move || readonly.get() || !valid_key.get()));
    let t = Rc::downgrade(this);
    ui.button(clear_button).on_click = Some(Rc::new(move |ui| {
        let Some(t) = t.upgrade() else { return };
        t.set_entry_key(ui, None);
        ui.yield_keyboard_focus(t.entry);
    }));

    let override_button = ui.child(panel, "OVERRIDE_HOTKEY_BUTTON");
    let valid = this.is_valid.clone();
    ui.button(override_button).is_disabled = Some(Rc::new(move || valid.get()));
    let (valid, duplicate_readonly) = (this.is_valid.clone(), this.duplicate_readonly.clone());
    ui.set_visible_fn(override_button, move || !valid.get() && !duplicate_readonly.get());
    let t = Rc::downgrade(this);
    ui.button(override_button).on_click = Some(Rc::new(move |ui| {
        let Some(t) = t.upgrade() else { return };
        let Some(duplicate) = t.duplicate.borrow().clone() else { return };
        if let Some(button) = ui.find(t.hotkey_list, &duplicate).and_then(|k| ui.find(k, "HOTKEY")) {
            let text = hotkey_text(ui, None);
            ButtonWidget::truncate_to_tooltip(ui, button, &text);
        }
        set_hotkey(ui, &duplicate, None);
        save(ui);
        ui.yield_keyboard_focus(t.entry);
    }));

    let valid = this.is_valid.clone();
    let (readonly, w1, w2) = (this.selected_readonly.clone(), Rc::downgrade(this), Rc::downgrade(this));
    {
        let mut e = ui.get::<HotkeyEntryWidget>(this.entry);
        e.is_valid = Some(Rc::new(move || valid.get()));
        e.on_lose_focus = Some(Rc::new(move |ui, key| {
            if let Some(t) = w1.upgrade() {
                t.validate(ui, key);
            }
        }));
        e.on_esc_key = Some(Rc::new(move |ui| {
            let t = w2.upgrade()?;
            let selected = t.selected.borrow().clone()?;
            current_hotkey(ui, &selected)
        }));
        e.is_disabled = Some(Rc::new(move || readonly.get()));
    }
    let valid_width = ui.node(this.entry).bounds.w;
    this.valid_width.set(valid_width);
    this.invalid_width
        .set(valid_width - (ui.node(clear_button).bounds.x - ui.node(override_button).bounds.x));
}
