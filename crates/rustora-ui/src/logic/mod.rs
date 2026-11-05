// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of the logic classes of `OpenRA.Mods.Common/Widgets/Logic`.

use std::cell::RefCell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::rc::Rc;

use rustora_core::miniyaml::MiniYamlNode;
use rustora_game::map_cache::MapCache;
use rustora_game::settings::Settings;

use crate::draw::Rect;
use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::net::{LobbyRules, Net};
use crate::widget::{Action, ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::LabelWidget;

pub mod color_picker;
pub mod command_bar;
pub mod connection;
pub mod ingame;
pub mod ingame_chat;
pub mod ingame_hotkeys;
pub mod ingame_menu;
pub mod lobby;
pub mod lobby_options;
pub mod lobby_utils;
pub mod main_menu;
pub mod map_chooser;
pub mod map_preview;
pub mod multiplayer;
pub mod sidebar;

use rustora_engine::sound::{AudioRules, Sound};

use crate::music::MusicPlaylist;

/// The application state that logic classes read.
pub struct Services {
    pub mod_version: String,
    pub settings: Rc<RefCell<Settings>>,
    pub maps: Rc<RefCell<MapCache>>,
    pub net: Net,
    /// Reads the lobby rules of a map. The client binary sets it.
    pub lobby_rules: RefCell<Option<LobbyRulesLookup>>,
    /// Reads the color rules of the mod. The client binary sets it.
    pub color_rules: RefCell<Option<ColorRulesLookup>>,
    /// The running game, from `Game.StartGame` on.
    pub ingame: RefCell<Option<Rc<ingame::Ingame>>>,
    /// `Game.Sound`.
    pub sound: Rc<Sound>,
    /// The audio sections of `ModRules`.
    pub mod_audio: Rc<AudioRules>,
    /// The `MusicPlaylist` of the current world.
    pub music: RefCell<Option<Rc<MusicPlaylist>>>,
}

/// The function behind `Services::lobby_rules`.
pub type LobbyRulesLookup = Rc<dyn Fn(&str) -> Option<Rc<LobbyRules>>>;

/// The function behind `Services::color_rules`.
pub type ColorRulesLookup = Rc<dyn Fn() -> Option<Rc<color_picker::ColorRules>>>;

impl Default for Services {
    fn default() -> Services {
        Services {
            mod_version: String::new(),
            settings: Rc::new(RefCell::new(Settings::load("", &[]))),
            maps: Rc::default(),
            net: Rc::default(),
            lobby_rules: RefCell::new(None),
            color_rules: RefCell::new(None),
            ingame: RefCell::new(None),
            sound: Rc::new(Sound::silent()),
            mod_audio: Rc::default(),
            music: RefCell::new(None),
        }
    }
}

/// `Game.Sound.PlayNotification(ModRules, null, "Sounds", name, null)`.
pub(crate) fn play_ui_sound(ui: &Ui, name: &str) {
    let services = services(ui);
    services.sound.play_notification(&services.mod_audio, "Sounds", name, None);
}

pub(crate) fn services(ui: &Ui) -> Rc<Services> {
    ui.services.clone().downcast::<Services>().expect("logic services")
}

pub(crate) fn message(ui: &Ui, key: &str) -> String {
    ui.assets.borrow().messages.get(key)
}

pub(crate) fn message_with(ui: &Ui, key: &str, args: &[(&str, MessageArg)]) -> String {
    ui.assets.borrow().messages.get_with(key, args)
}

/// `Game.CosmeticRandom.Next(count)`: nothing simulated depends on it.
pub(crate) fn cosmetic_random(count: usize) -> usize {
    let word = RandomState::new().build_hasher().finish();
    (word % count.max(1) as u64) as usize
}

/// Port of `ConfirmationDialogs.ButtonPrompt` with a confirm and a cancel button. `text` is already resolved.
pub(crate) fn button_prompt(ui: &mut Ui, title: &str, text: &str, on_confirm: Action, confirm_text: &str) {
    button_prompt_with_cancel(ui, title, text, on_confirm, confirm_text, None, None);
}

/// `ConfirmationDialogs.ButtonPrompt` with a cancel action and text.
pub(crate) fn button_prompt_with_cancel(
    ui: &mut Ui,
    title: &str,
    text: &str,
    on_confirm: Action,
    confirm_text: &str,
    on_cancel: Option<Action>,
    cancel_text: Option<&str>,
) {
    let prompt = ui.open_window("TWOBUTTON_PROMPT", WidgetArgs::default());
    let title = message(ui, title);
    let title_label = ui.child(prompt, "PROMPT_TITLE");
    ui.get::<LabelWidget>(title_label).get_text = Some(Rc::new(move || title.clone()));

    let template = ui.child(prompt, "PROMPT_TEXT");
    let line_height = ui.node(template).bounds.h;
    let mut header_height = 0;
    for line in text.split('\n') {
        let label = ui.clone_widget(template);
        let line = line.to_string();
        ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || line.clone()));
        ui.node_mut(label).bounds.y += header_height;
        ui.add_child(prompt, label);
        header_height += line_height;
    }
    let bounds = &mut ui.node_mut(prompt).bounds;
    bounds.h += header_height;
    bounds.y -= header_height / 2;

    let show_confirm = !confirm_text.is_empty();
    let confirm_text = message(ui, confirm_text);
    let confirm = ui.child(prompt, "CONFIRM_BUTTON");
    ui.node_mut(confirm).visible = show_confirm;
    ui.node_mut(confirm).bounds.y += header_height;
    ui.button(confirm).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        on_confirm(ui);
    }));
    ui.button(confirm).get_text = Some(Rc::new(move || confirm_text.clone()));

    let cancel = ui.child(prompt, "CANCEL_BUTTON");
    ui.node_mut(cancel).visible = true;
    ui.node_mut(cancel).bounds.y += header_height;
    ui.button(cancel).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        if let Some(f) = &on_cancel {
            f(ui);
        }
    }));
    if let Some(key) = cancel_text {
        let text = message(ui, key);
        ui.button(cancel).get_text = Some(Rc::new(move || text.clone()));
    }
}

/// Port of `SimpleTooltipLogic`.
struct SimpleTooltipLogic {
    widget: WidgetId,
    label: WidgetId,
    spacing: Rect,
    horizontal_padding: i32,
    get_text: Getter<String>,
    cached_text: String,
}

impl ChromeLogic for SimpleTooltipLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let text = (self.get_text)();
        if text == self.cached_text {
            return;
        }
        let font = ui.get::<LabelWidget>(self.label).font.clone();
        let text_width = ui.measure(&font, &text).0;
        ui.remove_children(self.widget);
        let mut bottom = 0;
        for (i, line_text) in text.split('\n').enumerate() {
            let line = ui.clone_widget(self.label);
            let line_text = line_text.to_string();
            ui.get::<LabelWidget>(line).get_text = Some(Rc::new(move || line_text.clone()));
            let bounds = &mut ui.node_mut(line).bounds;
            bounds.y += self.spacing.y + i as i32 * self.spacing.h;
            bounds.w = text_width;
            bottom = bounds.bottom();
            ui.add_child(self.widget, line);
        }
        let bounds = &mut ui.node_mut(self.widget).bounds;
        bounds.w = self.horizontal_padding + text_width;
        bounds.h = bottom + self.spacing.y;
        self.cached_text = text;
    }

    fn disposed(&mut self, ui: &mut Ui) {
        ui.dispose_detached(self.label);
    }
}

fn simple_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let label = ui.child(widget, "LABEL");
    let label_copy = ui.clone_widget(label);
    let spacing_widget = ui.child(widget, "LINE_HEIGHT");
    let spacing = ui.node(spacing_widget).bounds;
    ui.remove_children(widget);
    let label_bounds = ui.node(label_copy).bounds;
    let padding = label_bounds.w - ui.node(widget).bounds.w;
    let horizontal_padding = if padding <= 0 { 2 * label_bounds.x } else { padding };
    let get_text = args.value::<Getter<String>>("getText").unwrap_or_else(|| Rc::new(String::new));
    let mut logic = SimpleTooltipLogic {
        widget,
        label: label_copy,
        spacing,
        horizontal_padding,
        get_text,
        cached_text: String::new(),
    };
    logic.tick(ui);
    cell(logic)
}

/// A logic class without per frame state.
pub struct NoLogic;
impl ChromeLogic for NoLogic {}

/// Port of `ButtonTooltipLogic`. The hotkey label stays hidden until hotkeys are ported.
fn button_tooltip(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let label = ui.child(widget, "LABEL");
    let font = ui.get::<LabelWidget>(label).font.clone();
    let text: String = args.value("text").unwrap_or_default();
    let label_width = ui.measure(&font, &text).0;
    let shown = text.clone();
    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || shown.clone()));
    ui.node_mut(label).bounds.w = label_width;
    let label_x = ui.node(label).bounds.x;
    ui.node_mut(widget).bounds.w = 2 * label_x + label_width;

    let desc: String = args.value("desc").unwrap_or_default();
    if !desc.is_empty() {
        let template = ui.child(widget, "DESC");
        let parent = ui.node(template).parent.expect("description parent");
        ui.node_mut(parent).children.retain(|&c| c != template);
        let desc_font = ui.get::<LabelWidget>(template).font.clone();
        let template_bounds = ui.node(template).bounds;
        let mut desc_width = 0;
        let mut desc_offset = template_bounds.y;
        for line in desc.split('\n') {
            desc_width = desc_width.max(ui.measure(&desc_font, line).0);
            let line_label = ui.clone_widget(template);
            let line = line.to_string();
            ui.get::<LabelWidget>(line_label).get_text = Some(Rc::new(move || line.clone()));
            ui.node_mut(line_label).bounds.y = desc_offset;
            ui.add_child(widget, line_label);
            desc_offset += template_bounds.h;
        }
        ui.dispose_detached(template);
        let bounds = &mut ui.node_mut(widget).bounds;
        bounds.w = bounds.w.max(template_bounds.x * 2 + desc_width);
        bounds.h += desc_offset - template_bounds.y + template_bounds.x;
    }
    cell(NoLogic)
}

/// Port of `ObjectCreator.CreateObject<ChromeLogic>`.
pub fn create(
    name: &str,
    ui: &mut Ui,
    id: WidgetId,
    args: &WidgetArgs,
    logic_args: &[MiniYamlNode],
) -> Option<Rc<RefCell<dyn ChromeLogic>>> {
    if let Some(logic) = ingame_hotkeys::create(name, ui, id, logic_args) {
        return Some(logic);
    }
    Some(match name {
        "StanceSelectorLogic" => ingame_hotkeys::stance_selector(ui, id),
        "IngameTransientNotificationsLogic" => ingame_hotkeys::transient_notifications(ui, id),
        "DisconnectWatcherLogic" => ingame_hotkeys::disconnect_watcher(ui, id),
        "MuteIndicatorLogic" => ingame_hotkeys::mute_indicator(ui, id),
        "MainMenuLogic" => main_menu::create(ui, id, args),
        "AddFactionSuffixLogic" => sidebar::add_faction_suffix(ui, id),
        "ClassicProductionLogic" => sidebar::classic_production(ui, id),
        "IngameCashCounterLogic" => sidebar::cash_counter(ui, id),
        "IngamePowerCounterLogic" => sidebar::power_counter(ui, id),
        "SupportPowerBinLogic" => sidebar::support_power_bin(ui, id),
        "IngameRadarDisplayLogic" => sidebar::radar_display(ui, id),
        "MenuButtonsChromeLogic" => ingame_menu::menu_buttons(ui, id),
        "LoadIngameChatLogic" => ingame_chat::load_ingame_chat(ui, id),
        "IngameChatLogic" => ingame_chat::ingame_chat(ui, id, args, logic_args),
        "GameTimerLogic" => ingame_chat::game_timer(ui, id),
        "WorldTooltipLogic" => sidebar::world_tooltip(ui, id, args),
        "CommandBarLogic" => command_bar::command_bar(ui, id, logic_args),
        "IngameMenuLogic" => ingame_menu::ingame_menu(ui, id, args, logic_args),
        "ProductionTooltipLogic" => sidebar::production_tooltip(ui, id, args),
        "SellOrderButtonLogic" => sidebar::bind_order_button(ui, id, crate::button_orders::ButtonOrder::Sell, "sell"),
        "RepairOrderButtonLogic" => sidebar::bind_order_button(ui, id, crate::button_orders::ButtonOrder::Repair, "repair"),
        "PowerdownOrderButtonLogic" => sidebar::bind_order_button(ui, id, crate::button_orders::ButtonOrder::PowerDown, "power"),
        "VersionLabelLogic" => {
            let version = services(ui).mod_version.clone();
            ui.get::<LabelWidget>(id).get_text = Some(Rc::new(move || version.clone()));
            cell(NoLogic)
        }
        "ButtonTooltipLogic" => button_tooltip(ui, id, args),
        "SimpleTooltipLogic" => simple_tooltip(ui, id, args),
        "MapChooserLogic" => map_chooser::create(ui, id, args),
        "LoadIngamePlayerOrObserverUILogic" => ingame::load_player_or_observer_ui(ui, id),
        "ColorPickerLogic" => color_picker::create(ui, id, args, logic_args),
        "ConnectionLogic" => connection::create_connection(ui, id, args),
        "ConnectionFailedLogic" => connection::create_connection_failed(ui, id, args),
        "LobbyLogic" => lobby::create(ui, id, args, logic_args),
        "MultiplayerLogic" => multiplayer::create(ui, id, args),
        "DirectConnectLogic" => multiplayer::create_direct_connect(ui, id, args),
        "ServerCreationLogic" => multiplayer::create_server_creation(ui, id, args),
        "MapPreviewLogic" => map_preview::create(ui, id, args),
        "LobbyOptionsLogic" => lobby_options::create(ui, id, args),
        "LatencyTooltipLogic" => lobby_utils::latency_tooltip(ui, id, args),
        "SpawnSelectorTooltipLogic" => lobby_utils::spawn_tooltip(ui, id, args),
        "KickClientLogic" => lobby_utils::kick_client(ui, id, args),
        "KickSpectatorsLogic" => lobby_utils::kick_spectators(ui, id, args),
        "PerfDebugLogic" => {
            for name in ["GRAPH_BG", "PERF_TEXT"] {
                let widget = ui.child(id, name);
                ui.set_visible_fn(widget, || false);
            }
            cell(NoLogic)
        }
        _ => return None,
    })
}
