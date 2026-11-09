// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MissionBrowserLogic.cs`. The mission data loads on the UI thread instead of a worker thread.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_core::miniyaml::MiniYaml;
use rustora_client::map_cache::{MapClassification, MapPreview, MapStatus, MapVisibility};
use rustora_net::order::RawOrder;

use super::lobby_utils::split_on_first_token;
use super::media::with_player;
use super::{button_prompt_with_cancel, message, services};
use crate::loader::cell;
use crate::net::{LobbyOption, LobbyRules};
use crate::widget::{Action, AppRequest, ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::{CheckboxWidget, DropDownButtonWidget, LabelWidget, LabelWithTooltipWidget, MapPreviewWidget, ScrollItemWidget};

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlayingVideo {
    None,
    Info,
    Briefing,
    GameStart,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelType {
    MissionInfo,
    Options,
}

/// The widgets and the state of the panel, shared by the callbacks.
struct Browser {
    on_start: Action,
    on_exit: Action,
    mission_list: WidgetId,
    header_template: WidgetId,
    template: WidgetId,
    description_panel: WidgetId,
    description: WidgetId,
    options_container: WidgetId,
    checkbox_row_template: WidgetId,
    dropdown_row_template: WidgetId,
    start_briefing_video: WidgetId,
    start_info_video: WidgetId,
    video_player: WidgetId,
    fullscreen_player: WidgetId,
    fullscreen_visible: Rc<Cell<bool>>,
    difficulty_button: Option<WidgetId>,
    game_speed_button: Option<WidgetId>,
    unset_difficulty: String,
    default_tooltip: String,
    selected: RefCell<Option<Rc<MapPreview>>>,
    playing: Cell<PlayingVideo>,
    panel: Cell<PanelType>,
    minified_options: Cell<bool>,
    mission_options: RefCell<IndexMap<String, String>>,
    selected_difficulty: RefCell<Option<String>>,
    selected_game_speed: RefCell<Option<String>>,
    cached_volumes: Cell<(f32, f32)>,
}

/// `MapCache.PickLastModifiedMap`: the most recently changed map with `visibility`.
pub fn pick_last_modified_map(ui: &Ui, visibility: MapVisibility) -> Option<String> {
    let maps = services(ui).maps.clone();
    let maps = maps.borrow();
    maps.previews
        .values()
        .filter(|p| p.status == MapStatus::Available && p.visibility.has(visibility))
        .max_by_key(|p| p.modified_date)
        .map(|p| p.uid.clone())
}

/// Whether the missions button is enabled: there is a campaign or a mission map.
pub fn has_missions(ui: &Ui) -> bool {
    let s = services(ui);
    !s.missions.is_empty()
        || s.maps
            .borrow()
            .previews
            .values()
            .any(|p| p.status == MapStatus::Available && p.visibility.has(MapVisibility::MISSION_SELECTOR))
}

fn lobby_rules(ui: &Ui, uid: &str) -> Option<Rc<LobbyRules>> {
    let lookup = services(ui).lobby_rules.borrow().clone()?;
    lookup(uid)
}

impl Browser {
    fn rc(this: &Rc<Browser>) -> Rc<Browser> {
        this.clone()
    }

    /// The campaign groups of `Manifest.Missions` and the loose mission maps.
    fn mission_groups(ui: &Ui) -> Vec<(String, Vec<Rc<MapPreview>>)> {
        let s = services(ui);
        let fs = ui.assets.borrow().fs.clone();
        let mut sources = Vec::new();
        for file in &s.missions {
            let Some(data) = fs.try_open(file) else { continue };
            match rustora_core::miniyaml::MiniYaml::from_str(&String::from_utf8_lossy(&data), file, true) {
                Ok(nodes) => sources.push(nodes),
                Err(e) => log::warn!("{e}"),
            }
        }
        let yaml = MiniYaml::merge(sources).unwrap_or_default();
        let maps = s.maps.borrow();
        let mut groups = Vec::new();
        let mut all: Vec<String> = Vec::new();
        for kv in &yaml {
            let paths: Vec<&str> = kv.value.nodes.iter().map(|n| n.key.as_str()).collect();
            let mut previews: Vec<(usize, Rc<MapPreview>)> = maps
                .previews
                .values()
                .filter(|p| p.class == MapClassification::System && p.status == MapStatus::Available)
                .filter_map(|p| {
                    let name = std::path::Path::new(&p.package_name).file_name()?.to_string_lossy().into_owned();
                    paths.iter().position(|&m| m == name).map(|i| (i, p.clone()))
                })
                .collect();
            previews.sort_by_key(|(i, _)| *i);
            if !previews.is_empty() {
                all.extend(previews.iter().map(|(_, p)| p.uid.clone()));
                groups.push((kv.key.clone(), previews.into_iter().map(|(_, p)| p).collect()));
            }
        }
        let loose: Vec<Rc<MapPreview>> = maps
            .previews
            .values()
            .filter(|p| p.status == MapStatus::Available && p.visibility.has(MapVisibility::MISSION_SELECTOR) && !all.contains(&p.uid))
            .cloned()
            .collect();
        if !loose.is_empty() {
            groups.push(("Missions".to_string(), loose));
        }
        groups
    }

    fn create_mission_group(self: &Rc<Self>, ui: &mut Ui, title: &str, previews: &[Rc<MapPreview>]) {
        let header = ScrollItemWidget::setup(ui, self.header_template, Rc::new(|| false), Rc::new(|_| {}));
        let label = ui.child(header, "LABEL");
        let title = title.to_string();
        ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || title.clone()));
        ui.add_child(self.mission_list, header);
        for preview in previews {
            let (b, uid) = (Self::rc(self), preview.uid.clone());
            let is_selected = Rc::new(move || b.selected.borrow().as_ref().is_some_and(|s| s.uid == uid));
            let (b, p) = (Self::rc(self), preview.clone());
            let on_click: Action = Rc::new(move |ui| b.select_map(ui, &p));
            let b = Self::rc(self);
            let on_double_click: Action = Rc::new(move |ui| b.start_mission_clicked(ui));
            let item = ScrollItemWidget::setup_with_key(ui, &preview.uid, self.template, is_selected, on_click, on_double_click);
            let label = ui.child(item, "TITLE");
            LabelWithTooltipWidget::truncate_to_tooltip(ui, label, &preview.title);
            ui.add_child(self.mission_list, item);
        }
    }

    fn select_map(self: &Rc<Self>, ui: &mut Ui, preview: &Rc<MapPreview>) {
        *self.selected.borrow_mut() = Some(preview.clone());
        let mission = lobby_rules(ui, &preview.uid).and_then(|r| r.mission.clone());
        let (briefing_video, info_video) = match &mission {
            Some(m) => (m.briefing_video.clone(), m.background_video.clone()),
            None => (None, None),
        };
        if let Some(m) = &mission {
            let briefing = m.briefing.clone().unwrap_or_default().replace("\\n", "\n");
            let font = ui.label(self.description).font.clone();
            let width = ui.node(self.description).bounds.w;
            let text = ui.assets.borrow_mut().wrap_text(&font, &briefing, width);
            ui.node_mut(self.description).bounds.h = ui.measure(&font, &text).1;
            ui.label(self.description).get_text = Some(Rc::new(move || text.clone()));
            ui.adjust_children(self.description_panel);
            self.panel.set(PanelType::MissionInfo);
        }
        let b = Self::rc(self);
        let visible = briefing_video.is_some();
        ui.set_visible_fn(self.start_briefing_video, move || {
            visible && b.playing.get() != PlayingVideo::Briefing
        });
        let (b, video) = (Self::rc(self), briefing_video.unwrap_or_default());
        ui.button(self.start_briefing_video).on_click = Some(Rc::new(move |ui| {
            b.play_video(ui, b.video_player, &video, PlayingVideo::Briefing, None);
        }));
        let b = Self::rc(self);
        let visible = info_video.is_some();
        ui.set_visible_fn(self.start_info_video, move || visible && b.playing.get() != PlayingVideo::Info);
        let (b, video) = (Self::rc(self), info_video.unwrap_or_default());
        ui.button(self.start_info_video).on_click = Some(Rc::new(move |ui| {
            b.play_video(ui, b.video_player, &video, PlayingVideo::Info, None);
        }));
        ui.scroll_to_top(self.description_panel, false);
        self.rebuild_options(ui);
    }

    fn rebuild_options(self: &Rc<Self>, ui: &mut Ui) {
        let Some(selected) = self.selected.borrow().clone() else { return };
        let Some(rules) = lobby_rules(ui, &selected.uid) else { return };
        self.mission_options.borrow_mut().clear();
        ui.remove_children(self.options_container);
        let mut options: Vec<LobbyOption> = rules.options.iter().filter(|o| o.is_visible).cloned().collect();
        options.sort_by_key(|o| o.display_order);
        let minified = options.iter().all(|o| o.id == "difficulty" || o.id == "gamespeed");
        self.minified_options.set(minified);
        if minified {
            self.build_minified_options(ui, &options);
        } else {
            self.build_options(ui, &options);
        }
    }

    fn set_remembered(&self, option: &LobbyOption, remembered: &RefCell<Option<String>>) {
        let value = remembered.borrow_mut().get_or_insert_with(|| option.default_value.clone()).clone();
        let value = if option.values.contains_key(&value) {
            value
        } else {
            option.default_value.clone()
        };
        self.mission_options.borrow_mut().insert(option.id.clone(), value);
    }

    fn on_option_selected(&self, id: &str, value: &str) {
        match id {
            "difficulty" => *self.selected_difficulty.borrow_mut() = Some(value.to_string()),
            "gamespeed" => *self.selected_game_speed.borrow_mut() = Some(value.to_string()),
            _ => {}
        }
        self.mission_options.borrow_mut().insert(id.to_string(), value.to_string());
    }

    fn set_tooltip(ui: &Ui, id: WidgetId, option: &LobbyOption) {
        if let Some(description) = &option.description {
            let (text, desc) = split_on_first_token(description);
            let mut b = ui.button(id);
            b.get_tooltip_text = Some(Rc::new(move || text.clone()));
            b.get_tooltip_desc = Some(Rc::new(move || desc.clone()));
        }
    }

    fn new_row(&self, ui: &mut Ui, template: WidgetId, y: &mut i32) -> (WidgetId, VecDeque<WidgetId>) {
        let row = ui.clone_widget(template);
        ui.node_mut(row).bounds.y = *y;
        *y += ui.node(row).bounds.h;
        let columns = ui
            .node(row)
            .children
            .iter()
            .copied()
            .filter(|&c| {
                let id = ui.node(c).id.clone().unwrap_or_default();
                !id.ends_with("_DESC") && ui.kind(c).borrow_mut().as_button().is_some()
            })
            .collect();
        ui.add_child(self.options_container, row);
        (row, columns)
    }

    fn build_options(self: &Rc<Self>, ui: &mut Ui, options: &[LobbyOption]) {
        let mut y = 0;
        let mut columns = VecDeque::new();
        for option in options.iter().filter(|o| o.is_boolean) {
            self.mission_options
                .borrow_mut()
                .insert(option.id.clone(), option.default_value.clone());
            if columns.is_empty() {
                (_, columns) = self.new_row(ui, self.checkbox_row_template, &mut y);
            }
            let checkbox = columns.pop_front().expect("a free column");
            let name = option.name.clone();
            ui.button(checkbox).get_text = Some(Rc::new(move || name.clone()));
            Self::set_tooltip(ui, checkbox, option);
            ui.set_visible_fn(checkbox, || true);
            let (b, id) = (Self::rc(self), option.id.clone());
            ui.get::<CheckboxWidget>(checkbox).is_checked =
                Some(Rc::new(move || b.mission_options.borrow().get(&id).is_some_and(|v| v == "True")));
            let locked = option.is_locked;
            ui.button(checkbox).is_disabled = Some(Rc::new(move || locked));
            let (b, id) = (Self::rc(self), option.id.clone());
            ui.button(checkbox).on_click = Some(Rc::new(move |_| {
                let mut options = b.mission_options.borrow_mut();
                let value = if options.get(&id).is_some_and(|v| v == "True") {
                    "False"
                } else {
                    "True"
                };
                options.insert(id.clone(), value.to_string());
            }));
        }
        columns.clear();
        let mut row = self.options_container;
        for option in options.iter().filter(|o| !o.is_boolean) {
            match option.id.as_str() {
                "difficulty" => self.set_remembered(option, &self.selected_difficulty),
                "gamespeed" => self.set_remembered(option, &self.selected_game_speed),
                _ => {
                    self.mission_options
                        .borrow_mut()
                        .insert(option.id.clone(), option.default_value.clone());
                }
            }
            if columns.is_empty() {
                (row, columns) = self.new_row(ui, self.dropdown_row_template, &mut y);
            }
            let dropdown = columns.pop_front().expect("a free column");
            self.setup_dropdown(ui, dropdown, option);
            let dropdown_id = ui.node(dropdown).id.clone().unwrap_or_default();
            if let Some(label) = ui.find(row, &format!("{dropdown_id}_DESC")) {
                let text = format!("{}:", option.name);
                ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
                ui.set_visible_fn(label, || true);
            }
        }
    }

    fn build_minified_options(self: &Rc<Self>, ui: &mut Ui, options: &[LobbyOption]) {
        if let Some(button) = self.difficulty_button {
            match options.iter().find(|o| o.id == "difficulty") {
                Some(option) => {
                    self.set_remembered(option, &self.selected_difficulty);
                    self.setup_dropdown(ui, button, option);
                }
                None => {
                    let (text, tooltip) = (self.unset_difficulty.clone(), self.default_tooltip.clone());
                    let mut b = ui.button(button);
                    b.is_disabled = Some(Rc::new(|| true));
                    b.get_text = Some(Rc::new(move || text.clone()));
                    b.get_tooltip_text = Some(Rc::new(move || tooltip.clone()));
                }
            }
        }
        if let Some(button) = self.game_speed_button {
            match options.iter().find(|o| o.id == "gamespeed") {
                Some(option) => {
                    self.set_remembered(option, &self.selected_game_speed);
                    self.setup_dropdown(ui, button, option);
                }
                None => {
                    let text = message(ui, "label-not-available");
                    let mut b = ui.button(button);
                    b.is_disabled = Some(Rc::new(|| true));
                    b.get_text = Some(Rc::new(move || text.clone()));
                }
            }
        }
    }

    fn setup_dropdown(self: &Rc<Self>, ui: &mut Ui, dropdown: WidgetId, option: &LobbyOption) {
        let not_available = message(ui, "label-not-available");
        let (b, id, values) = (Self::rc(self), option.id.clone(), option.values.clone());
        ui.button(dropdown).get_text = Some(Rc::new(move || {
            b.mission_options
                .borrow()
                .get(&id)
                .and_then(|v| values.get(v))
                .unwrap_or(&not_available)
                .clone()
        }));
        Self::set_tooltip(ui, dropdown, option);
        ui.set_visible_fn(dropdown, || true);
        let locked = option.is_locked;
        ui.button(dropdown).is_disabled = Some(Rc::new(move || locked));
        let (b, id, values) = (Self::rc(self), option.id.clone(), option.values.clone());
        ui.button(dropdown).on_mouse_down = Some(Rc::new(move |ui, _| {
            let (b, id) = (b.clone(), id.clone());
            let height = values.len() as i32 * 30;
            DropDownButtonWidget::show_dropdown(
                ui,
                dropdown,
                "LABEL_DROPDOWN_TEMPLATE",
                height,
                values.clone(),
                move |ui, (key, value), template| {
                    let (s, i, k) = (b.clone(), id.clone(), key.clone());
                    let is_selected = Rc::new(move || s.mission_options.borrow().get(&i) == Some(&k));
                    let (s, i) = (b.clone(), id.clone());
                    let on_click: Action = Rc::new(move |_| s.on_option_selected(&i, &key));
                    let item = ScrollItemWidget::setup(ui, template, is_selected, on_click);
                    let label = ui.child(item, "LABEL");
                    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || value.clone()));
                    item
                },
            );
        }));
    }

    fn mute_sounds(&self, ui: &Ui) {
        let sound = services(ui).sound.clone();
        self.cached_volumes.set((sound.sound_volume(), sound.music_volume()));
        sound.set_sound_volume(0.0);
        sound.set_music_volume(0.0);
    }

    fn unmute_sounds(&self, ui: &Ui) {
        let sound = services(ui).sound.clone();
        let (s, m) = self.cached_volumes.get();
        if s > 0.0 {
            sound.set_sound_volume(s);
        }
        if m > 0.0 {
            sound.set_music_volume(m);
        }
    }

    fn play_video(self: &Rc<Self>, ui: &mut Ui, player: WidgetId, video: &str, pv: PlayingVideo, on_complete: Option<Action>) {
        if !ui.assets.borrow().fs.exists(video) {
            let text = message(ui, "dialog-no-video.prompt");
            button_prompt_with_cancel(
                ui,
                "dialog-no-video.title",
                &text,
                Rc::new(|_| {}),
                "",
                Some(Rc::new(|_| {})),
                Some("dialog-no-video.cancel"),
            );
            return;
        }
        self.stop_video(ui, player);
        self.playing.set(pv);
        with_player(ui, player, |p, ui| {
            p.load_and_play(ui, player, video);
        });
        let loaded = with_player(ui, player, |p, _| p.video.is_some());
        if !loaded {
            self.stop_video(ui, player);
            let text = message(ui, "dialog-cant-play-video.prompt");
            button_prompt_with_cancel(
                ui,
                "dialog-cant-play-video.title",
                &text,
                Rc::new(|_| {}),
                "",
                Some(Rc::new(|_| {})),
                Some("dialog-cant-play-video.cancel"),
            );
            return;
        }
        let b = Self::rc(self);
        with_player(ui, player, move |p, ui| {
            p.play_then(
                ui,
                Box::new(move |ui: &mut Ui| {
                    b.stop_video(ui, player);
                    if let Some(f) = on_complete {
                        f(ui);
                    }
                }),
            );
        });
        self.mute_sounds(ui);
    }

    fn stop_video(&self, ui: &mut Ui, player: WidgetId) {
        if self.playing.get() == PlayingVideo::None {
            return;
        }
        self.unmute_sounds(ui);
        with_player(ui, player, |p, ui| p.stop(ui));
        self.playing.set(PlayingVideo::None);
    }

    fn start_mission_clicked(self: &Rc<Self>, ui: &mut Ui) {
        self.stop_video(ui, self.video_player);
        let Some(selected) = self.selected.borrow().clone() else { return };
        let available = services(ui)
            .maps
            .borrow()
            .get(&selected.uid)
            .is_some_and(|p| p.status == MapStatus::Available);
        if !available {
            ui.requests.push(AppRequest::Disconnect);
            ui.close_window();
            (self.on_exit)(ui);
            return;
        }
        let mut orders: Vec<RawOrder> = self
            .mission_options
            .borrow()
            .iter()
            .map(|(k, v)| RawOrder::command(&format!("option {k} {v}")))
            .collect();
        orders.push(RawOrder::command("state Ready"));
        let map = selected.uid.clone();
        let start_video = lobby_rules(ui, &map)
            .and_then(|r| r.mission.clone())
            .and_then(|m| m.start_video)
            .filter(|v| ui.assets.borrow().fs.exists(v));
        match start_video {
            Some(video) => {
                self.fullscreen_visible.set(true);
                let fs_player = self.fullscreen_player;
                let on_complete: Action = Rc::new(move |ui| {
                    ui.requests.push(AppRequest::CreateAndStartLocalServer {
                        map: map.clone(),
                        orders: orders.clone(),
                    });
                });
                self.play_video(ui, fs_player, &video, PlayingVideo::GameStart, Some(on_complete));
            }
            None => ui.requests.push(AppRequest::CreateAndStartLocalServer { map, orders }),
        }
    }
}

struct MissionBrowserLogic {
    browser: Rc<Browser>,
}

impl ChromeLogic for MissionBrowserLogic {
    fn disposed(&mut self, ui: &mut Ui) {
        services(ui).net.borrow_mut().before_game_start = None;
        ui.dispose_detached(self.browser.header_template);
        ui.dispose_detached(self.browser.template);
        ui.dispose_detached(self.browser.checkbox_row_template);
        ui.dispose_detached(self.browser.dropdown_row_template);
    }
}

/// `MissionBrowserLogic`.
pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (on_exit, on_start) = (args.action("onExit"), args.action("onStart"));
    let initial_map = args.value::<Option<String>>("initialMap").flatten();
    let find = |ui: &Ui, name: &str| ui.find(widget, name).unwrap_or_else(|| panic!("{name} in the mission browser"));
    let mission_list = find(ui, "MISSION_LIST");
    let header_template = find(ui, "HEADER");
    let template = find(ui, "TEMPLATE");
    ui.hide_child(mission_list, header_template);
    ui.hide_child(mission_list, template);
    let mission_detail = find(ui, "MISSION_DETAIL");
    let description_panel = find(ui, "MISSION_DESCRIPTION_PANEL");
    let description = find(ui, "MISSION_DESCRIPTION");
    let options_container = ui.find(mission_detail, "MISSION_OPTIONS").expect("MISSION_OPTIONS");
    let checkbox_row_template = ui.find(options_container, "CHECKBOX_ROW_TEMPLATE").expect("checkbox row");
    let dropdown_row_template = ui.find(options_container, "DROPDOWN_ROW_TEMPLATE").expect("dropdown row");
    ui.hide_child(options_container, checkbox_row_template);
    ui.hide_child(options_container, dropdown_row_template);
    let mini_options = ui.find(widget, "MISSION_MINIFIED_OPTIONS");
    let difficulty_button = mini_options.and_then(|m| ui.find(m, "DIFFICULTY"));
    let game_speed_button = mini_options.and_then(|m| ui.find(m, "GAMESPEED"));
    let (unset_difficulty, default_tooltip) = match difficulty_button {
        Some(b) => {
            let (text, tooltip) = {
                let button = ui.button(b);
                (button.text.clone(), button.tooltip_text.clone())
            };
            (message(ui, &text), message(ui, &tooltip))
        }
        None => (String::new(), String::new()),
    };
    let world = args.value::<Rc<rustora_engine::world::World>>("world");
    let mut fs_args = WidgetArgs::default();
    if let Some(world) = world {
        fs_args = fs_args.with_value("world", world);
    }
    let fullscreen = ui.load_widget("FULLSCREEN_PLAYER", Some(ui.root), &fs_args);
    let fullscreen_visible = Rc::new(Cell::new(false));
    {
        let v = fullscreen_visible.clone();
        ui.set_visible_fn(fullscreen, move || v.get());
    }
    let fullscreen_player = ui.child(fullscreen, "PLAYER");
    let browser = Rc::new(Browser {
        on_start: on_start.clone(),
        on_exit: on_exit.clone(),
        mission_list,
        header_template,
        template,
        description_panel,
        description,
        options_container,
        checkbox_row_template,
        dropdown_row_template,
        start_briefing_video: find(ui, "START_BRIEFING_VIDEO_BUTTON"),
        start_info_video: find(ui, "START_INFO_VIDEO_BUTTON"),
        video_player: find(ui, "MISSION_VIDEO"),
        fullscreen_player,
        fullscreen_visible,
        difficulty_button,
        game_speed_button,
        unset_difficulty,
        default_tooltip,
        selected: RefCell::new(None),
        playing: Cell::new(PlayingVideo::None),
        panel: Cell::new(PanelType::MissionInfo),
        minified_options: Cell::new(true),
        mission_options: RefCell::new(IndexMap::new()),
        selected_difficulty: RefCell::new(None),
        selected_game_speed: RefCell::new(None),
        cached_volumes: Cell::new((0.0, 0.0)),
    });
    {
        let start = browser.on_start.clone();
        services(ui).net.borrow_mut().before_game_start = Some(Rc::new(move |ui| {
            ui.close_window();
            start(ui);
        }));
    }
    if let Some(title) = ui.find(widget, "MISSIONBROWSER_TITLE") {
        let text = ui.label(title).text.clone().unwrap_or_default();
        let text = message(ui, &text);
        let b = browser.clone();
        ui.label(title).get_text = Some(Rc::new(move || {
            if b.playing.get() != PlayingVideo::None
                && let Some(s) = b.selected.borrow().as_ref()
            {
                return s.title.clone();
            }
            text.clone()
        }));
    }
    let b = browser.clone();
    ui.set_visible_fn(find(ui, "MISSION_INFO"), move || b.selected.borrow().is_some());
    let preview_widget = find(ui, "MISSION_PREVIEW");
    let b = browser.clone();
    ui.get::<MapPreviewWidget>(preview_widget).preview = Some(Rc::new(move || b.selected.borrow().clone()));
    let b = browser.clone();
    ui.set_visible_fn(preview_widget, move || b.playing.get() == PlayingVideo::None);
    let b = browser.clone();
    ui.set_visible_fn(find(ui, "MISSION_BIN"), move || b.playing.get() != PlayingVideo::None);
    let b = browser.clone();
    ui.set_visible_fn(description_panel, move || b.panel.get() == PanelType::MissionInfo);
    let b = browser.clone();
    ui.set_visible_fn(options_container, move || b.panel.get() == PanelType::Options);
    for (button, playing) in [
        ("STOP_BRIEFING_VIDEO_BUTTON", PlayingVideo::Briefing),
        ("STOP_INFO_VIDEO_BUTTON", PlayingVideo::Info),
    ] {
        let id = find(ui, button);
        let b = browser.clone();
        ui.set_visible_fn(id, move || b.playing.get() == playing);
        let b = browser.clone();
        ui.button(id).on_click = Some(Rc::new(move |ui| b.stop_video(ui, b.video_player)));
    }
    if let Some(m) = mini_options {
        let b = browser.clone();
        ui.set_visible_fn(m, move || b.minified_options.get());
    }

    ui.remove_children(mission_list);
    let groups = Browser::mission_groups(ui);
    for (title, previews) in &groups {
        browser.create_mission_group(ui, title, previews);
    }
    let all: Vec<Rc<MapPreview>> = groups.iter().flat_map(|(_, p)| p.iter().cloned()).collect();
    if let Some(first) = all.first() {
        let initial = initial_map
            .and_then(|uid| services(ui).maps.borrow().get(&uid))
            .filter(|p| p.visibility.has(MapVisibility::MISSION_SELECTOR));
        match initial {
            Some(map) => {
                browser.select_map(ui, &map);
                ui.scroll_to_selected_item(mission_list);
            }
            None => browser.select_map(ui, first),
        }
    }

    let start_button = find(ui, "STARTGAME_BUTTON");
    let b = browser.clone();
    ui.button(start_button).on_click = Some(Rc::new(move |ui| b.start_mission_clicked(ui)));
    let b = browser.clone();
    ui.button(start_button).is_disabled = Some(Rc::new(move || b.selected.borrow().is_none()));
    let back = find(ui, "BACK_BUTTON");
    let b = browser.clone();
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        b.stop_video(ui, b.video_player);
        ui.close_window();
        (b.on_exit)(ui);
    }));
    let tabs = find(ui, "MISSION_TABS");
    let b = browser.clone();
    ui.set_visible_fn(tabs, move || !b.minified_options.get());
    for (tab, panel) in [("OPTIONS_TAB", PanelType::Options), ("MISSIONINFO_TAB", PanelType::MissionInfo)] {
        let id = ui.find(tabs, tab).expect("mission tab");
        let b = browser.clone();
        ui.button(id).is_highlighted = Some(Rc::new(move || b.panel.get() == panel));
        let b = browser.clone();
        ui.button(id).on_click = Some(Rc::new(move |_| b.panel.set(panel)));
    }
    cell(MissionBrowserLogic { browser })
}
