// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `MultiplayerLogic.cs`, `ServerListLogic.cs`, `DirectConnectLogic.cs` and `ServerCreationLogic.cs`.
//! The mod version notices stay hidden, unknown maps are not downloaded and show as unavailable, and server
//! creation reports UPnP as not supported.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};

use rustora_core::fieldloader::Color;
use rustora_game::settings::MpGameFilters;
use rustora_net::beacon::{BeaconLocation, Probe};
use rustora_net::game_server::{GameServer, parse_game_list, parse_lan_game, server_list_url};

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::{button_prompt_with_cancel, cosmetic_random, lobby, map_preview, message, message_with, services};
use crate::widget::{Action, AppRequest, ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{
    CheckboxWidget, DropDownButtonWidget, ImageWidget, LabelWithTooltipWidget, MapPreviewWidget, ScrollItemWidget, SpawnOccupant,
    TextFieldWidget,
};

const LAN_BEACON: &str = "OpenRALANGame";
const STATE_WAITING: i32 = 1;
const STATE_STARTED: i32 = 2;
const STATE_SHUTTING_DOWN: i32 = 3;
const MOD_ID: &str = "ra";
const DEFAULT_PORT: u16 = 1234;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchStatus {
    Fetching,
    Failed,
    NoGames,
    Hidden,
}

/// `GameServer` with the values that its C# constructor derives from the local mod.
struct ServerEntry {
    game: GameServer,
    is_compatible: bool,
    is_joinable: bool,
    mod_label: String,
}

impl ServerEntry {
    fn new(ui: &Ui, game: GameServer) -> ServerEntry {
        let service = services(ui);
        let is_compatible = game.mod_id.as_deref() == Some(MOD_ID) && game.version.as_deref() == Some(service.mod_version.as_str());
        let map_available = game.map.as_deref().and_then(|m| service.maps.borrow().get(m)).is_some();
        let title = game
            .mod_title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| game.mod_id.clone().unwrap_or_default());
        let mod_label = format!("{title} ({})", game.version.clone().unwrap_or_default());
        ServerEntry {
            is_joinable: is_compatible && game.state == STATE_WAITING && map_available,
            is_compatible,
            mod_label,
            game,
        }
    }

    fn address(&self) -> String {
        self.game.address.clone().unwrap_or_default()
    }

    /// `GroupSortOrder`.
    fn group_sort_order(&self, version: &str) -> i32 {
        let same_mod = self.game.mod_id.as_deref() == Some(MOD_ID);
        if !self.is_compatible {
            return i32::from(same_mod);
        }
        match (same_mod, self.game.version.as_deref() == Some(version)) {
            (true, true) => 4,
            (true, false) => 3,
            _ => 2,
        }
    }

    /// `ListOrder`.
    fn list_order(&self) -> i32 {
        let g = &self.game;
        if g.state == STATE_WAITING && g.players > 0 {
            0
        } else if g.state == STATE_WAITING && g.spectators > 0 {
            1
        } else if g.state >= STATE_STARTED {
            2
        } else {
            3
        }
    }

    /// `Filtered`.
    fn filtered(&self, filters: MpGameFilters) -> bool {
        let g = &self.game;
        (g.state == STATE_STARTED && !filters.has(MpGameFilters::Started))
            || (g.state == STATE_WAITING && !filters.has(MpGameFilters::Waiting) && g.players + g.spectators > 0)
            || (g.players + g.spectators == 0 && !filters.has(MpGameFilters::Empty))
            || (!self.is_compatible && !filters.has(MpGameFilters::Incompatible))
            || (g.protected && !filters.has(MpGameFilters::Protected))
    }
}

/// The state shared by the callbacks of `ServerListLogic`.
struct ServerList {
    server_list: WidgetId,
    server_template: WidgetId,
    header_template: WidgetId,
    client_list: WidgetId,
    client_header: WidgetId,
    client_template: WidgetId,
    client_container: WidgetId,
    join_button: WidgetId,
    join_button_y: i32,
    on_join: Rc<dyn Fn(&mut Ui, &ServerEntry)>,
    current: RefCell<Option<Rc<ServerEntry>>>,
    status: Cell<SearchStatus>,
    player_count: Cell<i32>,
    occupants: RefCell<HashMap<i32, SpawnOccupant>>,
    disabled_spawns: RefCell<Vec<i32>>,
    query: RefCell<Option<Receiver<Result<String, String>>>>,
    probe: Option<Probe>,
}

impl ServerList {
    /// `RefreshServerList`.
    fn refresh(&self, ui: &Ui) {
        if self.query.borrow().is_some() {
            return;
        }
        self.status.set(SearchStatus::Fetching);
        let version = services(ui).mod_version.clone();
        let url = server_list_url(rustora_net::web::SERVER_LIST_URL, &version, MOD_ID, &version);
        let (send, receive) = channel();
        std::thread::spawn(move || {
            let _ = send.send(rustora_net::web::get(&url));
        });
        *self.query.borrow_mut() = Some(receive);
    }

    fn lan_games(&self) -> Vec<GameServer> {
        let locations: Vec<BeaconLocation> = self.probe.as_ref().map_or_else(Vec::new, Probe::locations);
        let mut games: Vec<GameServer> = Vec::new();
        for location in locations {
            if let Some(game) = parse_lan_game(&location.data, location.address.ip()) {
                games.retain(|g| g.address != game.address);
                games.push(game);
            }
        }
        games
    }

    /// The part of the query task in `RefreshServerList` that runs on the answer.
    fn finish_query(self: &Rc<Self>, ui: &mut Ui, answer: Result<String, String>) {
        let mut games = match answer.and_then(|text| parse_game_list(&text)) {
            Ok(games) => Some(games),
            Err(e) => {
                log::debug!("Failed to query server list: {e}");
                None
            }
        };
        let lan = self.lan_games();
        match &mut games {
            Some(g) => g.extend(lan),
            None if !lan.is_empty() => games = Some(lan),
            None => {}
        }
        self.refresh_inner(ui, games);
    }

    /// `RefreshServerListInner`.
    fn refresh_inner(self: &Rc<Self>, ui: &mut Ui, games: Option<Vec<GameServer>>) {
        ui.remove_children(self.server_list);
        let previous = self.current.borrow().as_ref().map(|c| c.address());
        self.select_server(ui, None);
        let Some(games) = games else {
            self.status.set(SearchStatus::Failed);
            return;
        };
        let entries: Vec<Rc<ServerEntry>> = games.into_iter().map(|g| Rc::new(ServerEntry::new(ui, g))).collect();
        let (rows, next) = self.load_game_rows(ui, &entries, previous.as_deref());
        if rows.is_empty() {
            self.status.set(SearchStatus::NoGames);
            return;
        }
        self.status.set(SearchStatus::Hidden);
        for row in rows {
            ui.add_child(self.server_list, row);
        }
        if let Some(next) = next {
            self.select_server(ui, Some(next));
        }
        self.player_count.set(entries.iter().map(|e| e.game.players).sum());
    }

    /// `LoadGameRows`.
    fn load_game_rows(
        self: &Rc<Self>,
        ui: &mut Ui,
        entries: &[Rc<ServerEntry>],
        previous: Option<&str>,
    ) -> (Vec<WidgetId>, Option<Rc<ServerEntry>>) {
        let filters = services(ui).settings.borrow().game.mp_game_filters;
        let version = services(ui).mod_version.clone();
        let mut mods: Vec<(String, Vec<Rc<ServerEntry>>)> = Vec::new();
        for e in entries {
            match mods.iter_mut().find(|(label, _)| *label == e.mod_label) {
                Some((_, group)) => group.push(e.clone()),
                None => mods.push((e.mod_label.clone(), vec![e.clone()])),
            }
        }
        mods.sort_by_key(|(_, group)| (-group[0].group_sort_order(&version), -(group.len() as i64)));

        let mut rows = Vec::new();
        let mut next = None;
        for (label, mut group) in mods {
            if group.iter().all(|g| g.filtered(filters)) {
                continue;
            }
            let header = ScrollItemWidget::setup(ui, self.header_template, Rc::new(|| false), Rc::new(|_| {}));
            let title = ui.child(header, "LABEL");
            ui.label(title).get_text = Some(Rc::new(move || label.clone()));
            rows.push(header);

            group.sort_by(|a, b| {
                let by_order = a.list_order().cmp(&b.list_order());
                let by_key = if a.list_order() == 2 {
                    b.game.started.cmp(&a.game.started)
                } else {
                    (-a.game.players).cmp(&-b.game.players)
                };
                by_order.then(by_key)
            });
            for entry in group.iter().filter(|g| !g.filtered(filters)) {
                rows.push(self.game_row(ui, entry));
                if previous == Some(entry.address().as_str()) {
                    next = Some(entry.clone());
                }
            }
        }
        (rows, next)
    }

    fn game_row(self: &Rc<Self>, ui: &mut Ui, entry: &Rc<ServerEntry>) -> WidgetId {
        let is_selected: Getter<bool> = {
            let (this, entry) = (self.clone(), entry.clone());
            Rc::new(move || this.current.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, &entry)))
        };
        let on_click: Action = {
            let (this, entry) = (self.clone(), entry.clone());
            Rc::new(move |ui| this.select_server(ui, Some(entry.clone())))
        };
        let on_double_click: Action = {
            let (join, entry) = (self.on_join.clone(), entry.clone());
            Rc::new(move |ui| join(ui, &entry))
        };
        let key = entry.address();
        let item = ScrollItemWidget::setup_with_key(ui, &key, self.server_template, is_selected, on_click, on_double_click);
        let can_join = entry.is_joinable;
        let game = &entry.game;
        let incompatible = ui.assets.borrow().metrics.color("IncompatibleGameColor");
        let row_color = move |c: Color| if can_join { c } else { incompatible };

        let title = ui.child(item, "TITLE");
        LabelWithTooltipWidget::truncate_to_tooltip(ui, title, game.name.as_deref().unwrap_or_default());
        {
            let mut widget = ui.get::<LabelWithTooltipWidget>(title);
            let color = row_color(widget.label.text_color);
            widget.label.get_color = Some(Rc::new(move || color));
        }

        if let Some(password) = ui.find(item, "PASSWORD_PROTECTED") {
            let protected = game.protected;
            ui.set_visible_fn(password, move || protected);
            let image = if can_join { "protected" } else { "protected-disabled" };
            ui.get::<ImageWidget>(password).get_image_name = Some(Rc::new(move || image.to_string()));
        }
        if let Some(auth) = ui.find(item, "REQUIRES_AUTHENTICATION") {
            let authentication = game.authentication;
            ui.set_visible_fn(auth, move || authentication);
            let image = if can_join { "authentication" } else { "authentication-disabled" };
            ui.get::<ImageWidget>(auth).get_image_name = Some(Rc::new(move || image.to_string()));
            if game.protected
                && let Some(password) = ui.find(item, "PASSWORD_PROTECTED")
            {
                let width = ui.node(password).bounds.w;
                ui.node_mut(auth).bounds.x -= width + 5;
            }
        }

        let players = ui.child(item, "PLAYERS");
        let spectators = if game.spectators > 0 {
            format!(" + {}", game.spectators)
        } else {
            String::new()
        };
        let label = format!("{} / {}{spectators}", game.players + game.bots, game.max_players + game.bots);
        let bot_name = message(ui, "label-bot-player");
        let others = (game.clients.len() > 10).then(|| {
            message_with(
                ui,
                "label-other-players-count",
                &[("players", MessageArg::Number(game.clients.len() as i64 - 9))],
            )
        });
        {
            let mut widget = ui.get::<LabelWithTooltipWidget>(players);
            let color = row_color(widget.label.text_color);
            widget.label.get_text = Some(Rc::new(move || label.clone()));
            widget.label.get_color = Some(Rc::new(move || color));
            let names: Vec<String> = game
                .clients
                .iter()
                .map(|c| {
                    if c.is_bot {
                        bot_name.clone()
                    } else {
                        c.name.clone().unwrap_or_default()
                    }
                })
                .collect();
            let tooltip = match others {
                Some(others) => format!("{}\n{others}", names[..9].join("\n")),
                None => names.join("\n"),
            };
            widget.get_tooltip_text = (!game.clients.is_empty()).then(|| -> Getter<String> { Rc::new(move || tooltip.clone()) });
        }

        let state = ui.child(item, "STATUS");
        let text = message(
            ui,
            if game.state >= STATE_STARTED {
                "label-playing"
            } else {
                "label-waiting"
            },
        );
        let color = state_color(ui, game, ui.label(state).text_color, !can_join);
        {
            let mut widget = ui.label(state);
            widget.get_text = Some(Rc::new(move || text.clone()));
            widget.get_color = Some(Rc::new(move || color));
        }

        let location = ui.child(item, "LOCATION");
        let font = ui.label(location).font.clone();
        let width = ui.node(location).bounds.w;
        let text = ui.truncate_text(&font, game.location.as_deref().unwrap_or_default(), width);
        {
            let mut widget = ui.label(location);
            let color = row_color(widget.text_color);
            widget.get_text = Some(Rc::new(move || text.clone()));
            widget.get_color = Some(Rc::new(move || color));
        }
        item
    }

    /// `SelectServer`.
    fn select_server(self: &Rc<Self>, ui: &mut Ui, server: Option<Rc<ServerEntry>>) {
        *self.current.borrow_mut() = server.clone();
        let Some(server) = server else {
            ui.node_mut(self.join_button).bounds.y = self.join_button_y;
            self.occupants.borrow_mut().clear();
            self.disabled_spawns.borrow_mut().clear();
            return;
        };
        let game = &server.game;
        let bot_name = message(ui, "label-bot-player");
        let map = game.map.as_deref().and_then(|m| services(ui).maps.borrow().get(m));
        let spawn_count = map.as_ref().map_or(0, |m| m.spawn_points.len() as i32);
        *self.occupants.borrow_mut() = game
            .clients
            .iter()
            .filter(|c| c.spawn_point >= 1 && c.spawn_point <= spawn_count)
            .map(|c| {
                let occupant = SpawnOccupant {
                    color: c.color,
                    player_name: if c.is_bot {
                        bot_name.clone()
                    } else {
                        c.name.clone().unwrap_or_default()
                    },
                    team: c.team,
                    faction: c.faction.clone(),
                    spawn_point: c.spawn_point,
                };
                (c.spawn_point, occupant)
            })
            .collect();
        *self.disabled_spawns.borrow_mut() = game.disabled_spawn_points.clone();

        if game.clients.is_empty() {
            ui.node_mut(self.join_button).bounds.y = self.join_button_y;
            return;
        }
        let bottom = {
            let b = ui.node(self.client_container).bounds;
            b.y + b.h
        };
        ui.node_mut(self.join_button).bounds.y = bottom;
        self.show_clients(ui, &server, &bot_name);
    }

    fn show_clients(&self, ui: &mut Ui, server: &ServerEntry, bot_name: &str) {
        let game = &server.game;
        ui.remove_children(self.client_list);
        let mut team_ids: Vec<i32> = game.clients.iter().filter(|c| !c.is_spectator).map(|c| c.team).collect();
        team_ids.sort();
        team_ids.dedup();
        let no_teams = team_ids.len() == 1;
        let mut groups: Vec<(String, Vec<_>)> = Vec::new();
        for team in team_ids {
            let label = if no_teams {
                message(ui, "label-players")
            } else if team > 0 {
                message_with(ui, "label-team-name", &[("team", MessageArg::Number(team as i64))])
            } else {
                message(ui, "label-no-team")
            };
            let members = game.clients.iter().filter(|c| !c.is_spectator && c.team == team).collect();
            groups.push((label, members));
        }
        if game.clients.iter().any(|c| c.is_spectator) {
            let members = game.clients.iter().filter(|c| c.is_spectator).collect();
            groups.push((message(ui, "label-spectators"), members));
        }
        let same_mod = game.mod_id.as_deref() == Some(MOD_ID);
        for (group, members) in groups {
            let header = ScrollItemWidget::setup(ui, self.client_header, Rc::new(|| false), Rc::new(|_| {}));
            let label = ui.child(header, "LABEL");
            ui.label(label).get_text = Some(Rc::new(move || group.clone()));
            ui.add_child(self.client_list, header);
            for client in members {
                let item = ScrollItemWidget::setup(ui, self.client_template, Rc::new(|| false), Rc::new(|_| {}));
                let name = if client.is_bot {
                    bot_name.to_string()
                } else {
                    client.name.clone().unwrap_or_default()
                };
                let flagged = !client.is_spectator && same_mod;
                let label = ui.child(item, if flagged { "LABEL" } else { "NOFLAG_LABEL" });
                let font = ui.label(label).font.clone();
                let width = ui.node(label).bounds.w;
                let text = ui.truncate_text(&font, &name, width);
                let color = if client.is_spectator {
                    Color::from_argb(255, 255, 255, 255)
                } else {
                    client.color
                };
                {
                    let mut widget = ui.label(label);
                    widget.get_text = Some(Rc::new(move || text.clone()));
                    widget.get_color = Some(Rc::new(move || color));
                }
                if flagged {
                    let flag = ui.child(item, "FLAG");
                    ui.set_visible_fn(flag, || true);
                    let faction = client.faction.clone().unwrap_or_else(|| "Random".to_string());
                    let mut image = ui.get::<ImageWidget>(flag);
                    image.get_image_collection = Some(Rc::new(|| "flags".to_string()));
                    image.get_image_name = Some(Rc::new(move || faction.clone()));
                }
                ui.add_child(self.client_list, item);
            }
        }
    }
}

/// `GetStateColor`.
fn state_color(ui: &Ui, game: &GameServer, default: Color, darkened: bool) -> Color {
    let assets = ui.assets.borrow();
    let metric = |plain: &str, dark: &str| assets.metrics.color(if darkened { dark } else { plain });
    if game.state == STATE_WAITING {
        if game.protected {
            metric("ProtectedGameColor", "IncompatibleProtectedGameColor")
        } else {
            metric("WaitingGameColor", "IncompatibleWaitingGameColor")
        }
    } else if game.state == STATE_STARTED {
        metric("GameStartedColor", "IncompatibleGameStartedColor")
    } else {
        default
    }
}

/// `GetStateLabel`.
fn state_label(ui: &Ui, game: &GameServer) -> String {
    match game.state {
        STATE_STARTED => {
            let minutes = (game.play_time as f64 / 60.0).ceil() as i64;
            message_with(ui, "label-in-progress-for", &[("minutes", MessageArg::Number(minutes))])
        }
        STATE_WAITING if game.protected => message(ui, "label-password-protected"),
        STATE_WAITING => message(ui, "label-waiting-for-players"),
        STATE_SHUTTING_DOWN => message(ui, "label-server-shutting-down"),
        _ => message(ui, "label-unknown-server-state"),
    }
}

/// `PlayerLabel`.
fn player_label(ui: &Ui, game: &GameServer) -> String {
    let mut label = message_with(ui, "label-players-count", &[("players", MessageArg::Number(game.players as i64))]);
    if game.bots > 0 {
        label += " ";
        label += &message_with(ui, "label-bots-count", &[("bots", MessageArg::Number(game.bots as i64))]);
    }
    if game.spectators > 0 {
        label += " ";
        label += &message_with(
            ui,
            "label-spectators-count",
            &[("spectators", MessageArg::Number(game.spectators as i64))],
        );
    }
    label
}

/// The values of the selected server that getters read, updated each tick.
#[derive(Default)]
struct SelectedTexts {
    ip: String,
    status: String,
    status_color: Option<Color>,
    version: String,
    version_compatible: bool,
    players: String,
    map_title: String,
}

/// Port of `MultiplayerLogic` with `ServerListLogic`.
struct MultiplayerLogic {
    list: Rc<ServerList>,
    selected: Rc<RefCell<SelectedTexts>>,
    selected_status: WidgetId,
    selected_map: WidgetId,
    selected_version: WidgetId,
}

impl ChromeLogic for MultiplayerLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let answer = self.list.query.borrow().as_ref().and_then(|r| r.try_recv().ok());
        if let Some(answer) = answer {
            *self.list.query.borrow_mut() = None;
            self.list.finish_query(ui, answer);
        }
        let current = self.list.current.borrow().clone();
        let mut selected = self.selected.borrow_mut();
        selected.map_title = message(ui, "label-no-server-selected");
        let Some(current) = current else {
            return;
        };
        let game = &current.game;
        selected.ip = current.address();
        selected.status = state_label(ui, game);
        let default = ui.label(self.selected_status).text_color;
        selected.status_color = Some(state_color(ui, game, default, false));
        let font = ui.label(self.selected_version).font.clone();
        let width = ui.node(self.selected_version).bounds.w;
        selected.version = ui.truncate_text(&font, &current.mod_label, width);
        selected.version_compatible = current.is_compatible;
        selected.players = player_label(ui, game);
        let map = game.map.as_deref().and_then(|m| services(ui).maps.borrow().get(m));
        selected.map_title = match map {
            None => message(ui, "label-map-classification-unknown"),
            Some(map) => {
                let font = ui.get::<LabelWithTooltipWidget>(self.selected_map).label.font.clone();
                let width = ui.node(self.selected_map).bounds.w;
                ui.truncate_text(&font, &map.title, width)
            }
        };
    }
}

fn set_filter(ui: &mut Ui, list: &Rc<ServerList>, panel: WidgetId, name: &str, flag: MpGameFilters) {
    let Some(checkbox) = ui.find(panel, name) else {
        return;
    };
    let settings = services(ui).settings.clone();
    let list = list.clone();
    let mut widget = ui.get::<CheckboxWidget>(checkbox);
    let read = settings.clone();
    widget.is_checked = Some(Rc::new(move || read.borrow().game.mp_game_filters.has(flag)));
    widget.button.on_click = Some(Rc::new(move |ui| {
        {
            let mut s = settings.borrow_mut();
            s.game.mp_game_filters = MpGameFilters(s.game.mp_game_filters.0 ^ flag.0);
            if let Err(e) = s.save() {
                log::warn!("Cannot save the settings: {e}");
            }
        }
        list.refresh(ui);
    }));
}

fn setup_filters(ui: &mut Ui, widget: WidgetId, list: &Rc<ServerList>) {
    let Some(button) = ui.find(widget, "FILTERS_DROPDOWNBUTTON") else {
        return;
    };
    let panel = ui.load_widget("MULTIPLAYER_FILTER_PANEL", None, &WidgetArgs::default());
    set_filter(ui, list, panel, "WAITING_FOR_PLAYERS", MpGameFilters::Waiting);
    set_filter(ui, list, panel, "EMPTY", MpGameFilters::Empty);
    set_filter(ui, list, panel, "ALREADY_STARTED", MpGameFilters::Started);
    set_filter(ui, list, panel, "PASSWORD_PROTECTED", MpGameFilters::Protected);
    set_filter(ui, list, panel, "INCOMPATIBLE_VERSION", MpGameFilters::Incompatible);
    let status = list.clone();
    let mut dropdown = ui.get::<DropDownButtonWidget>(button);
    dropdown.button.is_disabled = Some(Rc::new(move || status.status.get() == SearchStatus::Fetching));
    dropdown.button.on_click = Some(Rc::new(move |ui| {
        DropDownButtonWidget::remove_panel(ui, button);
        DropDownButtonWidget::attach_panel(ui, button, panel, None);
    }));
}

fn setup_selected_labels(
    ui: &mut Ui,
    widget: WidgetId,
    list: &Rc<ServerList>,
    selected: &Rc<RefCell<SelectedTexts>>,
) -> (WidgetId, WidgetId, WidgetId) {
    let has_current = |list: &Rc<ServerList>| {
        let list = list.clone();
        move || list.current.borrow().is_some()
    };
    let text = |selected: &Rc<RefCell<SelectedTexts>>, read: fn(&SelectedTexts) -> String| -> Getter<String> {
        let selected = selected.clone();
        Rc::new(move || read(&selected.borrow()))
    };

    let map_preview = ui.child(widget, "SELECTED_MAP_PREVIEW");
    {
        let maps = services(ui).maps.clone();
        let (preview_list, occupants_list, disabled_list) = (list.clone(), list.clone(), list.clone());
        let mut preview = ui.get::<MapPreviewWidget>(map_preview);
        preview.preview = Some(Rc::new(move || {
            let current = preview_list.current.borrow();
            current.as_ref()?.game.map.as_deref().and_then(|m| maps.borrow().get(m))
        }));
        preview.spawn_occupants = Some(Rc::new(move || occupants_list.occupants.borrow().clone()));
        preview.disabled_spawn_points = Some(Rc::new(move || disabled_list.disabled_spawns.borrow().clone()));
    }

    let map_title = ui.child(widget, "SELECTED_MAP");
    ui.get::<LabelWithTooltipWidget>(map_title).label.get_text = Some(text(selected, |s| s.map_title.clone()));

    let ip = ui.child(widget, "SELECTED_IP");
    ui.set_visible_fn(ip, has_current(list));
    ui.label(ip).get_text = Some(text(selected, |s| s.ip.clone()));

    let status = ui.child(widget, "SELECTED_STATUS");
    ui.set_visible_fn(status, has_current(list));
    let default = ui.label(status).text_color;
    let read = selected.clone();
    ui.label(status).get_text = Some(text(selected, |s| s.status.clone()));
    ui.label(status).get_color = Some(Rc::new(move || read.borrow().status_color.unwrap_or(default)));

    let version = ui.child(widget, "SELECTED_MOD_VERSION");
    ui.set_visible_fn(version, has_current(list));
    let (plain, incompatible) = (
        ui.label(version).text_color,
        ui.assets.borrow().metrics.color("IncompatibleVersionColor"),
    );
    let read = selected.clone();
    ui.label(version).get_text = Some(text(selected, |s| s.version.clone()));
    ui.label(version).get_color = Some(Rc::new(move || if read.borrow().version_compatible { plain } else { incompatible }));

    let players = ui.child(widget, "SELECTED_PLAYERS");
    let current = list.clone();
    ui.set_visible_fn(players, move || {
        current.current.borrow().as_ref().is_some_and(|c| c.game.clients.is_empty())
    });
    ui.label(players).get_text = Some(text(selected, |s| s.players.clone()));

    (status, map_title, version)
}

/// `OpenLobby` of `MultiplayerLogic`.
fn open_lobby_from_browser(ui: &mut Ui, on_start: Action, on_exit: Action) {
    ui.close_window();
    let (start, exit) = (on_start.clone(), on_exit.clone());
    let on_lobby_exit: Action = Rc::new(move |ui| {
        open_multiplayer_panel(ui, start.clone(), exit.clone());
        ui.requests.push(AppRequest::Disconnect);
    });
    lobby::open_lobby(ui, on_lobby_exit, on_start.clone(), false);
}

/// Opens `MULTIPLAYER_PANEL`.
pub fn open_multiplayer_panel(ui: &mut Ui, on_start: Action, on_exit: Action) {
    let args = WidgetArgs::default()
        .with_action("onStart", on_start)
        .with_action("onExit", on_exit);
    ui.open_window("MULTIPLAYER_PANEL", args);
}

pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (on_start, on_exit) = (args.action("onStart"), args.action("onExit"));
    let open_lobby: Action = {
        let (start, exit) = (on_start.clone(), on_exit.clone());
        Rc::new(move |ui| open_lobby_from_browser(ui, start.clone(), exit.clone()))
    };

    let direct = ui.child(widget, "DIRECTCONNECT_BUTTON");
    let lobby_action = open_lobby.clone();
    ui.button(direct).on_click = Some(Rc::new(move |ui| {
        let args = WidgetArgs::default()
            .with_action("openLobby", lobby_action.clone())
            .with_action("onExit", Rc::new(|_| {}));
        ui.open_window("DIRECTCONNECT_PANEL", args);
    }));

    let create_button = ui.child(widget, "CREATE_BUTTON");
    let lobby_action = open_lobby.clone();
    ui.button(create_button).on_click = Some(Rc::new(move |ui| {
        let args = WidgetArgs::default()
            .with_action("openLobby", lobby_action.clone())
            .with_action("onExit", Rc::new(|_| {}));
        ui.open_window("MULTIPLAYER_CREATESERVER_PANEL", args);
    }));

    let back = ui.child(widget, "BACK_BUTTON");
    let exit = on_exit.clone();
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        exit(ui);
    }));

    let server_list = ui.child(widget, "SERVER_LIST");
    let header_template = ui.child(server_list, "HEADER_TEMPLATE");
    let server_template = ui.child(server_list, "SERVER_TEMPLATE");
    let join_button = ui.child(widget, "JOIN_BUTTON");
    let join_button_y = ui.node(join_button).bounds.y;
    let client_container = ui.child(widget, "CLIENT_LIST_CONTAINER");
    let client_list = ui.load_widget("MULTIPLAYER_CLIENT_LIST", Some(client_container), &WidgetArgs::default());
    let client_header = ui.child(client_list, "HEADER");
    let client_template = ui.child(client_list, "TEMPLATE");
    ui.hide_child(client_list, client_header);
    ui.hide_child(client_list, client_template);
    ui.hide_child(server_list, header_template);
    ui.hide_child(server_list, server_template);

    let lobby_action = open_lobby.clone();
    let on_join: Rc<dyn Fn(&mut Ui, &ServerEntry)> = Rc::new(move |ui, server| {
        if !server.is_joinable {
            return;
        }
        crate::logic::connection::connect(ui, &server.address(), "", lobby_action.clone(), Rc::new(|_| {}));
    });
    let probe = Probe::start(LAN_BEACON).map_err(|e| log::debug!("BeaconLib.Probe: {e}")).ok();
    let list = Rc::new(ServerList {
        server_list,
        server_template,
        header_template,
        client_list,
        client_header,
        client_template,
        client_container,
        join_button,
        join_button_y,
        on_join,
        current: RefCell::new(None),
        status: Cell::new(SearchStatus::Fetching),
        player_count: Cell::new(0),
        occupants: RefCell::new(HashMap::new()),
        disabled_spawns: RefCell::new(Vec::new()),
        query: RefCell::new(None),
        probe,
    });

    let visible_client_list = list.clone();
    ui.set_visible_fn(client_list, move || {
        visible_client_list
            .current
            .borrow()
            .as_ref()
            .is_some_and(|c| !c.game.clients.is_empty())
    });

    let current = list.clone();
    ui.set_visible_fn(join_button, move || current.current.borrow().is_some());
    let (disabled, join) = (list.clone(), list.clone());
    {
        let mut button = ui.button(join_button);
        button.is_disabled = Some(Rc::new(move || !disabled.current.borrow().as_ref().is_some_and(|c| c.is_joinable)));
        button.on_click = Some(Rc::new(move |ui| {
            let current = join.current.borrow().clone();
            if let Some(current) = current {
                (join.on_join)(ui, &current);
            }
        }));
    }

    let notices = ui.child(widget, "NOTICE_CONTAINER");
    ui.set_visible_fn(notices, || false);

    let progress = ui.child(widget, "PROGRESS_LABEL");
    let status = list.clone();
    ui.set_visible_fn(progress, move || status.status.get() != SearchStatus::Hidden);
    let failed = message(ui, "label-search-status-failed");
    let no_games = message(ui, "label-search-status-no-games");
    let status = list.clone();
    ui.label(progress).get_text = Some(Rc::new(move || match status.status.get() {
        SearchStatus::Failed => failed.clone(),
        SearchStatus::NoGames => no_games.clone(),
        _ => String::new(),
    }));

    setup_filters(ui, widget, &list);

    let reload = ui.child(widget, "RELOAD_BUTTON");
    let (disabled, refresh) = (list.clone(), list.clone());
    {
        let mut button = ui.button(reload);
        button.is_disabled = Some(Rc::new(move || disabled.status.get() == SearchStatus::Fetching));
        button.on_click = Some(Rc::new(move |ui| refresh.refresh(ui)));
    }

    if let Some(players) = ui.find(widget, "PLAYER_COUNT") {
        let count = list.clone();
        ui.set_visible_fn(players, move || count.player_count.get() != 0);
        let template = message_with(ui, "label-players-online-count", &[("players", MessageArg::Text("{}"))]);
        let count = list.clone();
        ui.label(players).get_text = Some(Rc::new(move || template.replace("{}", &count.player_count.get().to_string())));
    }

    let selected = Rc::new(RefCell::new(SelectedTexts::default()));
    let (selected_status, selected_map, selected_version) = setup_selected_labels(ui, widget, &list, &selected);

    list.refresh(ui);
    cell(MultiplayerLogic {
        list,
        selected,
        selected_status,
        selected_map,
        selected_version,
    })
}

/// Port of `DirectConnectLogic`.
pub fn create_direct_connect(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (on_exit, open_lobby) = (args.action("onExit"), args.action("openLobby"));
    let (ip_field, port_field) = (ui.child(widget, "IP"), ui.child(widget, "PORT"));
    let last = services(ui).settings.borrow().player.last_server.clone();
    let (ip, port) = last.rsplit_once(':').unwrap_or(("localhost", "1234"));
    ui.get::<TextFieldWidget>(ip_field).set_text(ip);
    ui.get::<TextFieldWidget>(port_field).set_text(port);

    let join = ui.child(widget, "JOIN_BUTTON");
    let empty_ip = ip_field;
    let kind = ui.kind(empty_ip);
    ui.button(join).is_disabled = Some(Rc::new(move || {
        kind.borrow_mut()
            .as_any()
            .downcast_mut::<TextFieldWidget>()
            .is_none_or(|f| f.text().is_empty())
    }));
    ui.button(join).on_click = Some(Rc::new(move |ui| {
        let ip = ui.get::<TextFieldWidget>(ip_field).text();
        let port = ui.get::<TextFieldWidget>(port_field).text().parse::<u16>().unwrap_or(DEFAULT_PORT);
        let target = format!("{ip}:{port}");
        {
            let settings = services(ui).settings.clone();
            let mut settings = settings.borrow_mut();
            settings.player.last_server = target.clone();
            if let Err(e) = settings.save() {
                log::warn!("Cannot save the settings: {e}");
            }
        }
        let lobby = open_lobby.clone();
        let on_connect: Action = Rc::new(move |ui| {
            ui.close_window();
            lobby(ui);
        });
        crate::logic::connection::connect(ui, &target, "", on_connect, Rc::new(|_| {}));
    }));
    let back = ui.child(widget, "BACK_BUTTON");
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        on_exit(ui);
    }));
    cell(crate::logic::NoLogic)
}

struct ServerCreation {
    panel: WidgetId,
    map: RefCell<Option<String>>,
    advertise_online: Cell<bool>,
    on_create: Action,
    on_exit: Action,
}

impl ServerCreation {
    /// `BuildNotices`.
    fn build_notices(&self, ui: &mut Ui) {
        let labels = (
            ui.find(self.panel, "NOTICES_HEADER_A"),
            ui.find(self.panel, "NOTICES_HEADER_B"),
            ui.find(self.panel, "NOTICES_HEADER_C"),
        );
        let (Some(a), Some(b), Some(c)) = labels else {
            return;
        };
        if self.advertise_online.get() {
            let text_a = message(ui, "label-internet-server-nat-A") + " ";
            let font = ui.label(a).font.clone();
            let width = ui.measure(&font, &text_a).0;
            let a_right = {
                let node = ui.node_mut(a);
                node.bounds.w = width;
                node.bounds.x + width
            };
            ui.label(a).get_text = Some(Rc::new(move || text_a.clone()));
            let text_b = message(ui, "label-internet-server-nat-B-not-supported");
            let color = ui.assets.borrow().metrics.color("NoticeErrorColor");
            let font = ui.label(b).font.clone();
            let width = ui.measure(&font, &text_b).0;
            let b_right = {
                let node = ui.node_mut(b);
                node.bounds.x = a_right;
                node.bounds.w = width;
                a_right + width
            };
            ui.label(b).get_text = Some(Rc::new(move || text_b.clone()));
            ui.label(b).text_color = color;
            let text_c = message(ui, "label-internet-server-nat-C");
            ui.label(c).get_text = Some(Rc::new(move || text_c.clone()));
            ui.node_mut(c).bounds.x = b_right;
            ui.set_visible_fn(b, || true);
            ui.set_visible_fn(c, || true);
        } else {
            let text_a = message(ui, "label-local-server");
            ui.label(a).get_text = Some(Rc::new(move || text_a.clone()));
            ui.set_visible_fn(b, || false);
            ui.set_visible_fn(c, || false);
        }
    }

    /// `CreateAndJoin`.
    fn create_and_join(&self, ui: &mut Ui) {
        let Some(map) = self.map.borrow().clone() else {
            return;
        };
        let name_field = ui.child(self.panel, "SERVER_NAME");
        let name = ui.get::<TextFieldWidget>(name_field).text();
        let settings = services(ui).settings.clone();
        let name = settings.borrow().sanitized_server_name(&name);
        let port_field = ui.child(self.panel, "LISTEN_PORT");
        let port = ui.get::<TextFieldWidget>(port_field).text().parse::<u16>().unwrap_or(DEFAULT_PORT);
        let password = match ui.find(self.panel, "PASSWORD") {
            Some(field) => ui.get::<TextFieldWidget>(field).text(),
            None => String::new(),
        };
        {
            let mut s = settings.borrow_mut();
            s.server.name = name.clone();
            s.server.listen_port = i32::from(port);
            s.server.advertise_online = self.advertise_online.get();
            s.server.map = Some(map.clone());
            s.server.password = password.clone();
            if let Err(e) = s.save() {
                log::warn!("Cannot save the settings: {e}");
            }
        }
        if let Err(e) = std::net::TcpListener::bind(("0.0.0.0", port)) {
            let mut text = message_with(
                ui,
                "dialog-server-creation-failed.prompt",
                &[("port", MessageArg::Number(i64::from(port)))],
            );
            text.push('\n');
            text += &if e.kind() == std::io::ErrorKind::AddrInUse {
                message(ui, "dialog-server-creation-failed.prompt-port-used")
            } else {
                message_with(
                    ui,
                    "dialog-server-creation-failed.prompt-error",
                    &[
                        ("message", MessageArg::Text(&e.to_string())),
                        ("code", MessageArg::Number(i64::from(e.raw_os_error().unwrap_or(0)))),
                    ],
                )
            };
            button_prompt_with_cancel(
                ui,
                "dialog-server-creation-failed.title",
                &text,
                Rc::new(|_| {}),
                "",
                None,
                Some("dialog-server-creation-failed.cancel"),
            );
            return;
        }
        ui.requests.push(AppRequest::CreateServer {
            name,
            port,
            password: password.clone(),
            advertise_online: self.advertise_online.get(),
            map,
        });
        ui.close_window();
        crate::logic::connection::connect(
            ui,
            &format!("localhost:{port}"),
            &password,
            self.on_create.clone(),
            self.on_exit.clone(),
        );
    }
}

/// Port of `ServerCreationLogic`.
pub fn create_server_creation(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (on_exit, on_create) = (args.action("onExit"), args.action("openLobby"));
    let service = services(ui);
    let initial = {
        let settings = service.settings.borrow();
        service
            .maps
            .borrow()
            .choose_initial_map(settings.server.map.as_deref(), &mut cosmetic_random)
    };
    let (advertise, password, port, server_name) = {
        let settings = service.settings.borrow();
        let s = &settings.server;
        (
            s.advertise_online,
            s.password.clone(),
            s.listen_port,
            settings.sanitized_server_name(&s.name),
        )
    };
    let state = Rc::new(ServerCreation {
        panel: widget,
        map: RefCell::new(Some(initial)),
        advertise_online: Cell::new(advertise),
        on_create,
        on_exit: on_exit.clone(),
    });

    let root = ui.child(widget, "MAP_PREVIEW_ROOT");
    let get_map: map_preview::GetMap = {
        let (state, maps) = (state.clone(), service.maps.clone());
        Rc::new(move || {
            let map = state.map.borrow().as_deref().and_then(|m| maps.borrow().get(m));
            (map, map_preview::MAP_STATUS_PLAYABLE)
        })
    };
    let preview_args = WidgetArgs::default()
        .with_value("getMap", get_map)
        .with_value("showUnoccupiedSpawnpoints", false);
    ui.load_widget("MAP_PREVIEW", Some(root), &preview_args);

    let back = ui.child(widget, "BACK_BUTTON");
    ui.button(back).on_click = Some(Rc::new(move |ui| {
        ui.close_window();
        on_exit(ui);
    }));
    let create = ui.child(widget, "CREATE_BUTTON");
    let creator = state.clone();
    ui.button(create).on_click = Some(Rc::new(move |ui| creator.create_and_join(ui)));

    if let Some(map_button) = ui.find(widget, "MAP_BUTTON") {
        let chooser = state.clone();
        ui.button(map_button).on_click = Some(Rc::new(move |ui| {
            let select = chooser.clone();
            let on_select: Rc<dyn Fn(&mut Ui, String)> = Rc::new(move |_, uid| *select.map.borrow_mut() = Some(uid));
            let args = WidgetArgs::default()
                .with_value("initialMap", chooser.map.borrow().clone())
                .with_action("onExit", Rc::new(|_| {}))
                .with_value("onSelect", on_select)
                .with_action("onStart", Rc::new(|_| {}));
            ui.open_window("MAPCHOOSER_PANEL", args);
        }));
    }

    let name_field = ui.child(widget, "SERVER_NAME");
    let settings = service.settings.clone();
    {
        let mut field = ui.get::<TextFieldWidget>(name_field);
        field.set_text(&server_name);
        field.on_enter_key = Some(Rc::new(move |ui, _| {
            ui.yield_keyboard_focus(name_field);
            true
        }));
        field.on_lose_focus = Some(Rc::new(move |ui| {
            let text = ui.get::<TextFieldWidget>(name_field).text();
            let clean = settings.borrow().sanitized_server_name(&text);
            ui.get::<TextFieldWidget>(name_field).set_text(&clean);
            settings.borrow_mut().server.name = clean;
        }));
    }
    let port_field = ui.child(widget, "LISTEN_PORT");
    ui.get::<TextFieldWidget>(port_field).set_text(&port.to_string());

    let checkbox = ui.child(widget, "ADVERTISE_CHECKBOX");
    {
        let (read, toggle) = (state.clone(), state.clone());
        let mut widget = ui.get::<CheckboxWidget>(checkbox);
        widget.is_checked = Some(Rc::new(move || read.advertise_online.get()));
        widget.button.on_click = Some(Rc::new(move |ui| {
            toggle.advertise_online.set(!toggle.advertise_online.get());
            toggle.build_notices(ui);
        }));
    }
    if let Some(field) = ui.find(widget, "PASSWORD") {
        ui.get::<TextFieldWidget>(field).set_text(&password);
    }
    if let Some(no_upnp) = ui.find(widget, "NOTICES_NO_UPNP") {
        let read = state.clone();
        ui.set_visible_fn(no_upnp, move || read.advertise_online.get());
        for name in ["SETTINGS_A", "SETTINGS_B"] {
            if let Some(label) = ui.find(no_upnp, name) {
                ui.set_visible_fn(label, || false);
            }
        }
    }
    if let Some(upnp) = ui.find(widget, "NOTICES_UPNP") {
        ui.set_visible_fn(upnp, || false);
    }
    if let Some(lan) = ui.find(widget, "NOTICES_LAN") {
        let read = state.clone();
        ui.set_visible_fn(lan, move || !read.advertise_online.get());
    }
    state.build_notices(ui);
    cell(crate::logic::NoLogic)
}
