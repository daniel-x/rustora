// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ReplayBrowserLogic.cs` and `ReplayUtils.cs`. Map downloads are not ported.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use rustora_game::map_cache::{MapCache, MapPreview, MapStatus};
use rustora_net::game_information::{GameInformation, GamePlayer, ReplayMetadata, WinState};

use super::map_preview::{GetMap, MAP_STATUS_PLAYABLE};
use super::{button_prompt_with_cancel, message, message_with, services, text_input_prompt};
use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::widget::{Action, AppRequest, ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::production::format_time;
use crate::widgets::{DropDownButtonWidget, ImageWidget, LabelWidget, LabelWithTooltipWidget, ScrollItemWidget, SpawnOccupant};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum GameType {
    #[default]
    Any,
    Singleplayer,
    Multiplayer,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DateType {
    #[default]
    Any,
    Today,
    LastWeek,
    LastFortnight,
    LastMonth,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DurationType {
    #[default]
    Any,
    VeryShort,
    Short,
    Medium,
    Long,
}

/// `ReplayBrowserLogic.Filter`. It outlives the panel like the static field of C#.
#[derive(Clone, Default)]
struct Filter {
    game_type: GameType,
    date: DateType,
    duration: DurationType,
    outcome: WinState,
    player_name: Option<String>,
    map_name: Option<String>,
    faction: Option<String>,
}

impl Filter {
    fn is_empty(&self) -> bool {
        let none = |s: &Option<String>| s.as_deref().is_none_or(str::is_empty);
        self.game_type == GameType::Any
            && self.date == DateType::Any
            && self.duration == DurationType::Any
            && self.outcome == WinState::Undefined
            && none(&self.player_name)
            && none(&self.map_name)
            && none(&self.faction)
    }
}

thread_local! {
    static FILTER: RefCell<Filter> = RefCell::new(Filter::default());
}

fn filter() -> Filter {
    FILTER.with(|f| f.borrow().clone())
}

fn update_filter(change: impl FnOnce(&mut Filter)) {
    FILTER.with(|f| change(&mut f.borrow_mut()));
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn sort_distinct_ignore_case(mut values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values.retain(|v| seen.insert(v.to_lowercase()));
    values.sort_by_key(|v| v.to_lowercase());
    values
}

/// A replay in the list with its scroll item.
struct ReplayEntry {
    meta: RefCell<ReplayMetadata>,
    item: WidgetId,
    visible: Cell<bool>,
}

/// `GameInformation.ResolvedPlayerName`.
fn resolved_player_name(ui: &Ui, info: &GameInformation, player: &GamePlayer) -> String {
    if !player.is_bot {
        return player.name.clone();
    }
    let name = message(ui, &player.name);
    let number = info.bot_number(player) as i64;
    message_with(
        ui,
        "enumerated-bot-name",
        &[("name", MessageArg::Text(&name)), ("number", MessageArg::Number(number))],
    )
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `EvaluateReplayVisibility`.
fn evaluate_visibility(ui: &Ui, info: &GameInformation, filter: &Filter) -> bool {
    match filter.game_type {
        GameType::Multiplayer if info.is_single_player() => return false,
        GameType::Singleplayer if !info.is_single_player() => return false,
        _ => {}
    }
    if filter.date != DateType::Any {
        let days = match filter.date {
            DateType::Today => 1,
            DateType::LastWeek => 7,
            DateType::LastFortnight => 14,
            _ => 30,
        };
        if info.start_time().unwrap_or(i64::MIN) < now_seconds() - days * 86400 {
            return false;
        }
    }
    let minutes = info.duration_seconds() as f64 / 60.0;
    let in_range = match filter.duration {
        DurationType::Any => true,
        DurationType::VeryShort => minutes < 5.0,
        DurationType::Short => (5.0..20.0).contains(&minutes),
        DurationType::Medium => (20.0..60.0).contains(&minutes),
        DurationType::Long => minutes >= 60.0,
    };
    if !in_range {
        return false;
    }
    if let Some(map) = filter.map_name.as_deref().filter(|m| !m.is_empty())
        && !eq_ignore_case(map, info.root.map_title.as_deref().unwrap_or_default())
    {
        return false;
    }
    if let Some(name) = filter.player_name.as_deref().filter(|n| !n.is_empty()) {
        let Some(player) = info
            .players
            .iter()
            .find(|p| eq_ignore_case(name, &resolved_player_name(ui, info, p)))
        else {
            return false;
        };
        if filter.outcome != WinState::Undefined && filter.outcome != player.outcome {
            return false;
        }
        if let Some(faction) = filter.faction.as_deref().filter(|f| !f.is_empty())
            && !eq_ignore_case(faction, player.faction_name.as_deref().unwrap_or_default())
        {
            return false;
        }
    }
    true
}

/// `ReplayMetadata.Read` for every `*.orarep` below `dir`, newest first.
fn load_replays(dir: &Path, cancel: &AtomicBool) -> Option<Vec<ReplayMetadata>> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "orarep") {
                files.push(path);
            }
        }
    }
    let mut replays = Vec::new();
    for file in files {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        replays.extend(ReplayMetadata::read(&file));
    }
    replays.sort_by_key(|r| std::cmp::Reverse(r.game_info.start_time().unwrap_or(i64::MIN)));
    Some(replays)
}

/// The installed mods with their versions, for `PromptConfirmReplayCompatibility`.
fn installed_mod_version(ui: &Ui, mod_id: &str) -> Option<String> {
    let service = services(ui);
    if service.mod_id == mod_id {
        return Some(service.mod_version.clone());
    }
    let package = service.installed_mods.get(mod_id)?.clone();
    rustora_game::manifest::Manifest::load(mod_id, package)
        .ok()
        .map(|m| m.metadata.version)
}

/// `ReplayUtils.PromptConfirmReplayCompatibility`: true when the replay can be watched, else shows why not.
pub fn prompt_confirm_replay_compatibility(ui: &mut Ui, meta: Option<&ReplayMetadata>, on_cancel: Action) -> bool {
    let reason = match meta {
        None => Some(("dialog-incompatible-replay.prompt", None)),
        Some(meta) => {
            let root = &meta.game_info.root;
            match (root.version.as_deref(), root.mod_id.as_deref()) {
                (None, _) => Some(("dialog-incompatible-replay.prompt-unknown-version", None)),
                (_, None) => Some(("dialog-incompatible-replay.prompt-unknown-mod", None)),
                (Some(version), Some(mod_id)) => match installed_mod_version(ui, mod_id) {
                    None => Some((
                        "dialog-incompatible-replay.prompt-unavailable-mod",
                        Some(("mod", mod_id.to_string())),
                    )),
                    Some(v) if v != version => Some((
                        "dialog-incompatible-replay.prompt-incompatible-version",
                        Some(("version", version.to_string())),
                    )),
                    Some(_) if !map_available(ui, root.map_uid.as_deref()) => Some((
                        "dialog-incompatible-replay.prompt-unavailable-map",
                        Some(("map", root.map_uid.clone().unwrap_or_default())),
                    )),
                    Some(_) => None,
                },
            }
        }
    };
    let Some((key, arg)) = reason else {
        return true;
    };
    let text = match &arg {
        Some((name, value)) => message_with(ui, key, &[(name, MessageArg::Text(value))]),
        None => message(ui, key),
    };
    button_prompt_with_cancel(
        ui,
        "dialog-incompatible-replay.title",
        &text,
        Rc::new(|_| {}),
        "",
        Some(on_cancel),
        Some("dialog-incompatible-replay.confirm"),
    );
    false
}

fn map_preview(ui: &Ui, uid: Option<&str>) -> Option<Rc<MapPreview>> {
    uid.map(|uid| services(ui).maps.borrow_mut().entry(uid))
}

fn map_available(ui: &Ui, uid: Option<&str>) -> bool {
    map_preview(ui, uid).is_some_and(|m| m.status == MapStatus::Available)
}

/// The state of an open replay browser.
struct Browser {
    replay_list: WidgetId,
    player_list: WidgetId,
    player_header: WidgetId,
    player_template: WidgetId,
    replay_template: WidgetId,
    replays: RefCell<Vec<Rc<ReplayEntry>>>,
    selected: RefCell<Option<Rc<ReplayEntry>>>,
    map: RefCell<Option<Rc<MapPreview>>>,
    /// The `DURATION` text, the spawn occupants and the disabled spawn points of the selected replay.
    duration: RefCell<String>,
    occupants: RefCell<HashMap<i32, SpawnOccupant>>,
    disabled_spawns: RefCell<Vec<i32>>,
    cancel_loading: Arc<AtomicBool>,
    loaded: RefCell<Option<Receiver<Option<Vec<ReplayMetadata>>>>>,
}

impl Browser {
    /// `ApplyFilter`.
    fn apply_filter(self: &Rc<Self>, ui: &mut Ui) {
        let filter = filter();
        for entry in self.replays.borrow().iter() {
            entry.visible.set(evaluate_visibility(ui, &entry.meta.borrow().game_info, &filter));
        }
        let selected_visible = self.selected.borrow().as_ref().is_some_and(|e| e.visible.get());
        if !selected_visible {
            self.select_first_visible(ui);
        }
        ui.adjust_children(self.replay_list);
        ui.scroll_to_selected_item(self.replay_list);
    }

    fn select_first_visible(self: &Rc<Self>, ui: &mut Ui) {
        let first = self.replays.borrow().iter().find(|e| e.visible.get()).cloned();
        self.select(ui, first);
    }

    /// The map of the selected replay as the map cache has it now.
    fn current_map(&self, maps: &RefCell<MapCache>) -> Option<Rc<MapPreview>> {
        let map = self.map.borrow();
        map.as_ref().map(|m| maps.borrow().get(&m.uid).unwrap_or_else(|| m.clone()))
    }

    /// `SelectReplay`.
    fn select(self: &Rc<Self>, ui: &mut Ui, entry: Option<Rc<ReplayEntry>>) {
        *self.selected.borrow_mut() = entry.clone();
        let info = entry.as_ref().map(|e| e.meta.borrow().game_info.clone());
        *self.map.borrow_mut() = info.as_ref().and_then(|i| map_preview(ui, i.root.map_uid.as_deref()));
        let Some(info) = info else {
            return;
        };
        let service = services(ui);
        if let Some(uid) = info.root.map_uid.as_deref()
            && service.settings.borrow().game.allow_downloading
            && self.map.borrow().as_ref().is_none_or(|m| m.status == MapStatus::Unavailable)
        {
            service.map_repository.query(&service.maps, [uid]);
        }
        let time = format_time(info.duration_seconds() as i32, 1000);
        *self.duration.borrow_mut() = message_with(ui, "label-duration", &[("time", MessageArg::Text(&time))]);
        *self.occupants.borrow_mut() = info
            .players
            .iter()
            .filter(|p| p.spawn_point != 0)
            .map(|p| {
                let occupant = SpawnOccupant {
                    color: p.color,
                    player_name: p.name.clone(),
                    team: p.team,
                    faction: p.faction_id.clone(),
                    spawn_point: p.spawn_point,
                };
                (p.spawn_point, occupant)
            })
            .collect();
        *self.disabled_spawns.borrow_mut() = info.root.disabled_spawn_points.iter().copied().collect();
        let mut teams: Vec<(i32, Vec<&GamePlayer>)> = Vec::new();
        for p in &info.players {
            match teams.iter_mut().find(|(t, _)| *t == p.team) {
                Some((_, members)) => members.push(p),
                None => teams.push((p.team, vec![p])),
            }
        }
        teams.sort_by_key(|(t, _)| *t);
        let no_teams = teams.len() == 1;
        ui.remove_children(self.player_list);
        for (team, members) in teams {
            let group = if no_teams {
                message(ui, "label-players")
            } else if team > 0 {
                message_with(ui, "label-team-name", &[("team", MessageArg::Number(team as i64))])
            } else {
                message(ui, "label-no-team")
            };
            if !group.is_empty() {
                let header = ScrollItemWidget::setup(ui, self.player_header, Rc::new(|| false), Rc::new(|_| {}));
                let label = ui.child(header, "LABEL");
                ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || group.clone()));
                ui.add_child(self.player_list, header);
            }
            for player in members {
                let item = ScrollItemWidget::setup(ui, self.player_template, Rc::new(|| false), Rc::new(|_| {}));
                let label = ui.child(item, "LABEL");
                let font = ui.label(label).font.clone();
                let width = ui.node(label).bounds.w;
                let name = ui.truncate_text(&font, &resolved_player_name(ui, &info, player), width);
                let color = player.color;
                {
                    let mut l = ui.label(label);
                    l.get_text = Some(Rc::new(move || name.clone()));
                    l.get_color = Some(Rc::new(move || color));
                }
                let faction = player
                    .faction_id
                    .clone()
                    .filter(|f| ui.assets.borrow().chrome.region_size("flags", f).is_some())
                    .unwrap_or_else(|| "Random".to_string());
                let flag = ui.child(item, "FLAG");
                let mut image = ui.get::<ImageWidget>(flag);
                image.get_image_collection = Some(Rc::new(|| "flags".to_string()));
                image.get_image_name = Some(Rc::new(move || faction.clone()));
                drop(image);
                ui.add_child(self.player_list, item);
            }
        }
    }

    /// `WatchReplay`.
    fn watch(self: &Rc<Self>, ui: &mut Ui) {
        let Some(meta) = self.selected.borrow().as_ref().map(|e| e.meta.borrow().clone()) else {
            return;
        };
        if prompt_confirm_replay_compatibility(ui, Some(&meta), Rc::new(|_| {})) {
            self.cancel_loading.store(true, Ordering::Relaxed);
            ui.requests.push(AppRequest::JoinReplay(meta.path.to_string_lossy().into_owned()));
        }
    }

    /// `AddReplay`.
    fn add_replay(self: &Rc<Self>, ui: &mut Ui, meta: ReplayMetadata) {
        let key = meta.path.to_string_lossy().into_owned();
        let title = file_stem(&meta.path);
        let slot: Rc<RefCell<Option<Rc<ReplayEntry>>>> = Rc::default();
        let is_selected: Getter<bool> = {
            let (b, s) = (Rc::downgrade(self), slot.clone());
            Rc::new(move || {
                let (Some(b), Some(e)) = (b.upgrade(), s.borrow().clone()) else {
                    return false;
                };
                b.selected.borrow().as_ref().is_some_and(|x| Rc::ptr_eq(x, &e))
            })
        };
        let on_click: Action = {
            let (b, s) = (Rc::downgrade(self), slot.clone());
            Rc::new(move |ui| {
                if let Some(b) = b.upgrade() {
                    b.select(ui, s.borrow().clone());
                }
            })
        };
        let on_double_click: Action = {
            let b = Rc::downgrade(self);
            Rc::new(move |ui| {
                if let Some(b) = b.upgrade() {
                    b.watch(ui);
                }
            })
        };
        let item = ScrollItemWidget::setup_with_key(ui, &key, self.replay_template, is_selected, on_click, on_double_click);
        let entry = Rc::new(ReplayEntry {
            meta: RefCell::new(meta),
            item,
            visible: Cell::new(true),
        });
        *slot.borrow_mut() = Some(entry.clone());
        let e = Rc::downgrade(&entry);
        ui.set_visible_fn(item, move || e.upgrade().is_some_and(|e| e.visible.get()));
        set_item_title(ui, item, &title);
        self.replays.borrow_mut().push(entry);
        ui.add_child(self.replay_list, item);
    }

    /// `RenameReplay`.
    fn rename(&self, ui: &mut Ui, entry: &Rc<ReplayEntry>, new_name: &str) {
        let old = entry.meta.borrow().path.clone();
        let new = old.with_file_name(format!("{new_name}.orarep"));
        match std::fs::rename(&old, &new) {
            Ok(()) => {
                entry.meta.borrow_mut().path = new;
                set_item_title(ui, entry.item, new_name);
            }
            Err(e) => log::debug!("cannot rename {}: {e}", old.display()),
        }
    }

    /// `DeleteReplay`.
    fn delete(self: &Rc<Self>, ui: &mut Ui, entry: &Rc<ReplayEntry>) {
        let path = entry.meta.borrow().path.clone();
        if let Err(e) = std::fs::remove_file(&path) {
            let text = message_with(
                ui,
                "notification-replay-deletion-failed",
                &[("file", MessageArg::Text(&path.to_string_lossy()))],
            );
            log::warn!("{text}");
            log::debug!("{e}");
            return;
        }
        let is_selected = self.selected.borrow().as_ref().is_some_and(|s| Rc::ptr_eq(s, entry));
        if is_selected {
            self.select(ui, None);
        }
        ui.remove_child(self.replay_list, entry.item);
        self.replays.borrow_mut().retain(|e| !Rc::ptr_eq(e, entry));
    }

    /// `OnDeleteReplay`.
    fn confirm_delete(self: &Rc<Self>, ui: &mut Ui, entry: Rc<ReplayEntry>) {
        let name = file_stem(&entry.meta.borrow().path);
        let text = message_with(ui, "dialog-delete-replay.prompt", &[("replay", MessageArg::Text(&name))]);
        let b = self.clone();
        let on_confirm: Action = Rc::new(move |ui| {
            b.delete(ui, &entry);
            if b.selected.borrow().is_none() {
                b.select_first_visible(ui);
            }
        });
        button_prompt_with_cancel(
            ui,
            "dialog-delete-replay.title",
            &text,
            on_confirm,
            "dialog-delete-replay.confirm",
            Some(Rc::new(|_| {})),
            None,
        );
    }

    /// The `MNG_DELALL_BUTTON` action.
    fn confirm_delete_visible(self: &Rc<Self>, ui: &mut Ui) {
        let list: Vec<Rc<ReplayEntry>> = self.replays.borrow().iter().filter(|e| e.visible.get()).cloned().collect();
        match list.len() {
            0 => {}
            1 => self.confirm_delete(ui, list[0].clone()),
            count => {
                let text = message_with(
                    ui,
                    "dialog-delete-all-replays.prompt",
                    &[("count", MessageArg::Number(count as i64))],
                );
                let b = self.clone();
                let on_confirm: Action = Rc::new(move |ui| {
                    for entry in &list {
                        b.delete(ui, entry);
                    }
                    if b.selected.borrow().is_none() {
                        b.select_first_visible(ui);
                    }
                });
                button_prompt_with_cancel(
                    ui,
                    "dialog-delete-all-replays.title",
                    &text,
                    on_confirm,
                    "dialog-delete-all-replays.confirm",
                    Some(Rc::new(|_| {})),
                    None,
                );
            }
        }
    }

    /// The `MNG_RENSEL_BUTTON` action.
    fn prompt_rename(self: &Rc<Self>, ui: &mut Ui) {
        let Some(entry) = self.selected.borrow().clone() else {
            return;
        };
        let path = entry.meta.borrow().path.clone();
        let initial = file_stem(&path);
        let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let b = self.clone();
        let on_accept = Rc::new(move |ui: &mut Ui, name: String| b.rename(ui, &entry, &name));
        let original = initial.clone();
        let validator = Rc::new(move |name: &str| {
            name != original && !name.trim().is_empty() && !name.chars().any(invalid_file_name_char) && !directory.join(name).exists()
        });
        text_input_prompt(
            ui,
            "dialog-rename-replay.title",
            "dialog-rename-replay.prompt",
            &initial,
            on_accept,
            "dialog-rename-replay.confirm",
            validator,
        );
    }

    /// `SetupReplayDependentFilters`.
    fn setup_replay_dependent_filters(self: &Rc<Self>, ui: &mut Ui, panel: WidgetId) {
        let infos: Vec<GameInformation> = self.replays.borrow().iter().map(|e| e.meta.borrow().game_info.clone()).collect();
        let maps = sort_distinct_ignore_case(infos.iter().filter_map(|i| i.root.map_title.clone()).collect());
        let players = sort_distinct_ignore_case(
            infos
                .iter()
                .flat_map(|i| i.players.iter().map(|p| resolved_player_name(ui, i, p)))
                .collect(),
        );
        let factions = sort_distinct_ignore_case(
            infos
                .iter()
                .flat_map(|i| i.players.iter().filter_map(|p| p.faction_name.clone()))
                .filter(|f| !f.is_empty())
                .collect(),
        );
        self.string_filter(ui, panel, "FLT_MAPNAME_DROPDOWNBUTTON", maps, false, |f| &mut f.map_name);
        self.string_filter(ui, panel, "FLT_PLAYER_DROPDOWNBUTTON", players, false, |f| &mut f.player_name);
        if let Some(ddb) = ui.find(panel, "FLT_FACTION_DROPDOWNBUTTON") {
            ui.button(ddb).is_disabled = Some(Rc::new(|| filter().player_name.as_deref().is_none_or(str::is_empty)));
        }
        self.string_filter(ui, panel, "FLT_FACTION_DROPDOWNBUTTON", factions, true, |f| &mut f.faction);
    }

    /// A drop down that filters by one of the strings of `options`. `translate` resolves them as message keys.
    fn string_filter(
        self: &Rc<Self>,
        ui: &mut Ui,
        panel: WidgetId,
        name: &str,
        options: Vec<String>,
        translate: bool,
        field: fn(&mut Filter) -> &mut Option<String>,
    ) {
        let Some(ddb) = ui.find(panel, name) else {
            return;
        };
        let any_text = message(ui, &ui.button(ddb).text.clone());
        let display: Rc<HashMap<String, String>> = Rc::new(
            options
                .iter()
                .map(|o| (o.clone(), if translate { message(ui, o) } else { o.clone() }))
                .collect(),
        );
        {
            let (any_text, display) = (any_text.clone(), display.clone());
            ui.button(ddb).get_text = Some(Rc::new(move || {
                let mut f = filter();
                match field(&mut f).as_deref().filter(|v| !v.is_empty()) {
                    Some(v) => display.get(v).cloned().unwrap_or_else(|| v.to_string()),
                    None => any_text.clone(),
                }
            }));
        }
        let b = Rc::downgrade(self);
        ui.button(ddb).on_mouse_down = Some(Rc::new(move |ui, _| {
            let choices: Vec<Option<String>> = std::iter::once(None).chain(options.iter().cloned().map(Some)).collect();
            let (b, any_text, display) = (b.clone(), any_text.clone(), display.clone());
            DropDownButtonWidget::show_dropdown(ui, ddb, "LABEL_DROPDOWN_TEMPLATE", 330, choices, move |ui, option, tpl| {
                let selected = option.clone();
                let is_selected: Getter<bool> = Rc::new(move || {
                    let mut f = filter();
                    match (field(&mut f).as_deref(), selected.as_deref()) {
                        (Some(a), Some(b)) => eq_ignore_case(a, b),
                        (None, None) => true,
                        _ => false,
                    }
                });
                let (chosen, b) = (option.clone(), b.clone());
                let on_click: Action = Rc::new(move |ui| {
                    update_filter(|f| *field(f) = chosen.clone());
                    if let Some(b) = b.upgrade() {
                        b.apply_filter(ui);
                    }
                });
                let item = ScrollItemWidget::setup(ui, tpl, is_selected, on_click);
                let text = option.map_or_else(|| any_text.clone(), |o| display.get(&o).cloned().unwrap_or(o));
                let label = ui.child(item, "LABEL");
                ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
                item
            });
        }));
    }

    /// A drop down that filters by one value of an enumeration.
    fn enum_filter<T: Copy + PartialEq + 'static>(
        self: &Rc<Self>,
        ui: &mut Ui,
        panel: WidgetId,
        name: &str,
        options: &[(T, &str)],
        field: fn(&mut Filter) -> &mut T,
    ) {
        let Some(ddb) = ui.find(panel, name) else {
            return;
        };
        let any_text = message(ui, &ui.button(ddb).text.clone());
        let options: Rc<Vec<(T, String)>> = Rc::new(
            options
                .iter()
                .enumerate()
                .map(|(i, (v, key))| (*v, if i == 0 { any_text.clone() } else { message(ui, key) }))
                .collect(),
        );
        {
            let options = options.clone();
            ui.button(ddb).get_text = Some(Rc::new(move || {
                let value = *field(&mut filter());
                options
                    .iter()
                    .find(|(v, _)| *v == value)
                    .map(|(_, t)| t.clone())
                    .unwrap_or_default()
            }));
        }
        let b = Rc::downgrade(self);
        ui.button(ddb).on_mouse_down = Some(Rc::new(move |ui, _| {
            let b = b.clone();
            DropDownButtonWidget::show_dropdown(
                ui,
                ddb,
                "LABEL_DROPDOWN_TEMPLATE",
                330,
                options.iter().cloned().collect::<Vec<_>>(),
                move |ui, (value, text), tpl| {
                    let is_selected: Getter<bool> = Rc::new(move || *field(&mut filter()) == value);
                    let b = b.clone();
                    let on_click: Action = Rc::new(move |ui| {
                        update_filter(|f| *field(f) = value);
                        if let Some(b) = b.upgrade() {
                            b.apply_filter(ui);
                        }
                    });
                    let item = ScrollItemWidget::setup(ui, tpl, is_selected, on_click);
                    let label = ui.child(item, "LABEL");
                    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
                    item
                },
            );
        }));
    }

    /// `SetupFilters`.
    fn setup_filters(self: &Rc<Self>, ui: &mut Ui, panel: WidgetId) {
        self.enum_filter(
            ui,
            panel,
            "FLT_GAMETYPE_DROPDOWNBUTTON",
            &[
                (GameType::Any, ""),
                (GameType::Singleplayer, "options-replay-type.singleplayer"),
                (GameType::Multiplayer, "options-replay-type.multiplayer"),
            ],
            |f| &mut f.game_type,
        );
        self.enum_filter(
            ui,
            panel,
            "FLT_DATE_DROPDOWNBUTTON",
            &[
                (DateType::Any, ""),
                (DateType::Today, "options-replay-date.today"),
                (DateType::LastWeek, "options-replay-date.last-week"),
                (DateType::LastFortnight, "options-replay-date.last-fortnight"),
                (DateType::LastMonth, "options-replay-date.last-month"),
            ],
            |f| &mut f.date,
        );
        self.enum_filter(
            ui,
            panel,
            "FLT_DURATION_DROPDOWNBUTTON",
            &[
                (DurationType::Any, ""),
                (DurationType::VeryShort, "options-replay-duration.very-short"),
                (DurationType::Short, "options-replay-duration.short"),
                (DurationType::Medium, "options-replay-duration.medium"),
                (DurationType::Long, "options-replay-duration.long"),
            ],
            |f| &mut f.duration,
        );
        if let Some(ddb) = ui.find(panel, "FLT_OUTCOME_DROPDOWNBUTTON") {
            ui.button(ddb).is_disabled = Some(Rc::new(|| filter().player_name.as_deref().is_none_or(str::is_empty)));
        }
        self.enum_filter(
            ui,
            panel,
            "FLT_OUTCOME_DROPDOWNBUTTON",
            &[
                (WinState::Undefined, ""),
                (WinState::Lost, "options-winstate.defeat"),
                (WinState::Won, "options-winstate.victory"),
            ],
            |f| &mut f.outcome,
        );
        let reset = ui.child(panel, "FLT_RESET_BUTTON");
        ui.button(reset).is_disabled = Some(Rc::new(|| filter().is_empty()));
        let b = Rc::downgrade(self);
        ui.button(reset).on_click = Some(Rc::new(move |ui| {
            update_filter(|f| *f = Filter::default());
            if let Some(b) = b.upgrade() {
                b.apply_filter(ui);
            }
        }));
    }

    /// `SetupManagement`.
    fn setup_management(self: &Rc<Self>, ui: &mut Ui, panel: WidgetId) {
        let no_selection = |b: &Rc<Self>| -> Getter<bool> {
            let b = Rc::downgrade(b);
            Rc::new(move || b.upgrade().is_none_or(|b| b.selected.borrow().is_none()))
        };
        let rename = ui.child(panel, "MNG_RENSEL_BUTTON");
        ui.button(rename).is_disabled = Some(no_selection(self));
        let b = Rc::downgrade(self);
        ui.button(rename).on_click = Some(Rc::new(move |ui| {
            if let Some(b) = b.upgrade() {
                b.prompt_rename(ui);
            }
        }));

        let delete = ui.child(panel, "MNG_DELSEL_BUTTON");
        ui.button(delete).is_disabled = Some(no_selection(self));
        let b = Rc::downgrade(self);
        ui.button(delete).on_click = Some(Rc::new(move |ui| {
            if let Some(b) = b.upgrade()
                && let Some(entry) = b.selected.borrow().clone()
            {
                b.confirm_delete(ui, entry);
            }
        }));

        let delete_all = ui.child(panel, "MNG_DELALL_BUTTON");
        let b = Rc::downgrade(self);
        ui.button(delete_all).is_disabled = Some(Rc::new(move || {
            b.upgrade().is_none_or(|b| !b.replays.borrow().iter().any(|e| e.visible.get()))
        }));
        let b = Rc::downgrade(self);
        ui.button(delete_all).on_click = Some(Rc::new(move |ui| {
            if let Some(b) = b.upgrade() {
                b.confirm_delete_visible(ui);
            }
        }));
    }
}

/// `Path.GetInvalidFileNameChars` of Windows, which covers the other platforms.
fn invalid_file_name_char(c: char) -> bool {
    c < ' ' || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn set_item_title(ui: &mut Ui, item: WidgetId, title: &str) {
    let text = title.to_string();
    ui.button(item).get_text = Some(Rc::new(move || text.clone()));
    let label = ui.child(item, "TITLE");
    LabelWithTooltipWidget::truncate_to_tooltip(ui, label, title);
}

/// The chrome logic handle: receives the loaded replays and releases the templates.
struct BrowserLogic {
    browser: Rc<Browser>,
    panel: WidgetId,
}

impl ChromeLogic for BrowserLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let result = match self.browser.loaded.borrow().as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => result,
            Some(Err(TryRecvError::Empty)) | None => return,
            Some(Err(TryRecvError::Disconnected)) => None,
        };
        *self.browser.loaded.borrow_mut() = None;
        let Some(replays) = result else {
            return;
        };
        ui.remove_children(self.browser.replay_list);
        for meta in replays {
            self.browser.add_replay(ui, meta);
        }
        self.browser.setup_replay_dependent_filters(ui, self.panel);
        self.browser.apply_filter(ui);
    }

    fn disposed(&mut self, ui: &mut Ui) {
        self.browser.cancel_loading.store(true, Ordering::Relaxed);
        services(ui).net.borrow_mut().before_game_start = None;
        for template in [
            self.browser.player_header,
            self.browser.player_template,
            self.browser.replay_template,
        ] {
            ui.dispose_detached(template);
        }
    }
}

/// `ReplayBrowserLogic`.
pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (on_exit, on_start) = (args.action("onExit"), args.action("onStart"));
    let player_list = ui.child(widget, "PLAYER_LIST");
    let player_header = ui.child(player_list, "HEADER");
    let player_template = ui.child(player_list, "TEMPLATE");
    ui.hide_child(player_list, player_header);
    ui.hide_child(player_list, player_template);
    ui.remove_children(player_list);
    let replay_list = ui.child(widget, "REPLAY_LIST");
    let replay_template = ui.child(replay_list, "REPLAY_TEMPLATE");
    ui.hide_child(replay_list, replay_template);

    let browser = Rc::new(Browser {
        replay_list,
        player_list,
        player_header,
        player_template,
        replay_template,
        replays: RefCell::default(),
        selected: RefCell::default(),
        map: RefCell::default(),
        duration: RefCell::default(),
        occupants: RefCell::default(),
        disabled_spawns: RefCell::default(),
        cancel_loading: Arc::new(AtomicBool::new(false)),
        loaded: RefCell::default(),
    });

    services(ui).net.borrow_mut().before_game_start = Some(Rc::new(move |ui| {
        ui.close_window();
        on_start(ui);
    }));
    let cancel = ui.child(widget, "CANCEL_BUTTON");
    let b = Rc::downgrade(&browser);
    ui.button(cancel).on_click = Some(Rc::new(move |ui| {
        if let Some(b) = b.upgrade() {
            b.cancel_loading.store(true, Ordering::Relaxed);
        }
        ui.close_window();
        on_exit(ui);
    }));

    let dir: PathBuf = services(ui).replay_dir.clone();
    if dir.is_dir() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let cancel = browser.cancel_loading.clone();
        std::thread::spawn(move || {
            let _ = sender.send(load_replays(&dir, &cancel));
        });
        *browser.loaded.borrow_mut() = Some(receiver);
    }

    let watch = ui.child(widget, "WATCH_BUTTON");
    {
        let (b, maps) = (Rc::downgrade(&browser), services(ui).maps.clone());
        ui.button(watch).is_disabled = Some(Rc::new(move || {
            b.upgrade()
                .is_none_or(|b| b.selected.borrow().is_none() || b.current_map(&maps).is_none_or(|m| m.status != MapStatus::Available))
        }));
        let b = Rc::downgrade(&browser);
        ui.button(watch).on_click = Some(Rc::new(move |ui| {
            if let Some(b) = b.upgrade() {
                b.watch(ui);
            }
        }));
    }

    let preview_root = ui.child(widget, "MAP_PREVIEW_ROOT");
    for name in ["MAP_PREVIEW_ROOT", "REPLAY_INFO"] {
        let id = ui.child(widget, name);
        let b = Rc::downgrade(&browser);
        ui.set_visible_fn(id, move || b.upgrade().is_some_and(|b| b.selected.borrow().is_some()));
    }
    let get_map: GetMap = {
        let (b, maps) = (Rc::downgrade(&browser), services(ui).maps.clone());
        Rc::new(move || (b.upgrade().and_then(|b| b.current_map(&maps)), MAP_STATUS_PLAYABLE))
    };
    let get_occupants: Getter<HashMap<i32, SpawnOccupant>> = {
        let b = Rc::downgrade(&browser);
        Rc::new(move || b.upgrade().map(|b| b.occupants.borrow().clone()).unwrap_or_default())
    };
    let get_disabled: Getter<Vec<i32>> = {
        let b = Rc::downgrade(&browser);
        Rc::new(move || b.upgrade().map(|b| b.disabled_spawns.borrow().clone()).unwrap_or_default())
    };
    let preview_args = WidgetArgs::default()
        .with_value("getMap", get_map)
        .with_value("getSpawnOccupants", get_occupants)
        .with_value("getDisabledSpawnPoints", get_disabled)
        .with_value("showUnoccupiedSpawnpoints", false);
    ui.load_widget("MAP_PREVIEW", Some(preview_root), &preview_args);

    let duration = ui.child(widget, "DURATION");
    let b = Rc::downgrade(&browser);
    ui.label(duration).get_text = Some(Rc::new(move || {
        b.upgrade().map(|b| b.duration.borrow().clone()).unwrap_or_default()
    }));

    browser.setup_filters(ui, widget);
    browser.setup_management(ui, widget);
    cell(BrowserLogic { browser, panel: widget })
}
