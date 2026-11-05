// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `GameSaveBrowserLogic.cs` and `GameSaveLoadingLogic.cs`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rustora_engine::player::WinState;
use rustora_engine::world::World;
use rustora_net::game_save::GameSave;
use rustora_net::order::RawOrder;

use super::{button_prompt_with_cancel, message_with, services, text_input_prompt};
use crate::fluent::MessageArg;
use crate::input::{KeyEvent, KeyInput};
use crate::loader::cell;
use crate::widget::{Action, AppRequest, ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::production::LogicKeyListenerWidget;
use crate::widgets::{LabelWidget, LabelWithTooltipWidget, ProgressBarWidget, ScrollItemWidget, TextFieldWidget};

const SAVE_EXTENSION: &str = "orasav";

/// The saves below `dir`, the most recently written first.
fn save_paths(dir: &Path) -> Vec<PathBuf> {
    fn collect(dir: &Path, out: &mut Vec<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == SAVE_EXTENSION) {
                let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                out.push((modified, path));
            }
        }
    }
    let mut saves = Vec::new();
    collect(dir, &mut saves);
    saves.sort_by_key(|s| std::cmp::Reverse(s.0));
    saves.into_iter().map(|(_, p)| p).collect()
}

/// `GameSaveBrowserLogic.IsLoadPanelEnabled`.
pub fn is_load_panel_enabled(ui: &Ui) -> bool {
    !save_paths(&services(ui).save_dir).is_empty()
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `Path.GetInvalidFileNameChars` of Windows, which covers the other platforms.
fn invalid_file_name_char(c: char) -> bool {
    c < ' ' || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
}

fn modified_text(path: &Path) -> String {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    modified.map_or_else(String::new, |t| {
        chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string()
    })
}

struct Entry {
    path: RefCell<PathBuf>,
    item: WidgetId,
}

struct Browser {
    panel: WidgetId,
    game_list: WidgetId,
    game_template: WidgetId,
    new_template: WidgetId,
    save_field: Option<WidgetId>,
    base_path: PathBuf,
    default_filename: String,
    games: RefCell<Vec<Rc<Entry>>>,
    selected: RefCell<Option<Rc<Entry>>>,
    on_exit: Action,
    world: Option<Rc<World>>,
    /// Whether the save name is blank, updated each tick for the `SAVE_BUTTON` getter.
    save_name_empty: Cell<bool>,
}

impl Browser {
    fn is_save_panel(&self) -> bool {
        self.world.is_some()
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected.borrow().as_ref().map(|e| e.path.borrow().clone())
    }

    fn load_games(self: &Rc<Self>, ui: &mut Ui) {
        ui.remove_children(self.game_list);
        if self.is_save_panel() {
            let b = Rc::downgrade(self);
            let is_selected: Getter<bool> = Rc::new(move || b.upgrade().is_some_and(|b| b.selected.borrow().is_none()));
            let b = Rc::downgrade(self);
            let on_click: Action = Rc::new(move |ui| {
                if let Some(b) = b.upgrade() {
                    b.select(ui, None);
                }
            });
            let item = ScrollItemWidget::setup(ui, self.new_template, is_selected, on_click);
            ui.add_child(self.game_list, item);
        }
        for path in save_paths(&self.base_path) {
            self.add_game(ui, path);
        }
    }

    fn add_game(self: &Rc<Self>, ui: &mut Ui, path: PathBuf) {
        let key = path.to_string_lossy().into_owned();
        let slot: Rc<RefCell<Option<Rc<Entry>>>> = Rc::default();
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
                    if b.is_save_panel() {
                        b.save(ui);
                    } else {
                        b.load(ui);
                    }
                }
            })
        };
        let item = ScrollItemWidget::setup_with_key(ui, &key, self.game_template, is_selected, on_click, on_double_click);
        let title = ui.child(item, "TITLE");
        LabelWithTooltipWidget::truncate_to_tooltip(ui, title, &file_stem(&path));
        let date = modified_text(&path);
        let date_label = ui.child(item, "DATE");
        ui.get::<LabelWidget>(date_label).get_text = Some(Rc::new(move || date.clone()));
        let entry = Rc::new(Entry {
            path: RefCell::new(path),
            item,
        });
        *slot.borrow_mut() = Some(entry.clone());
        self.games.borrow_mut().push(entry);
        ui.add_child(self.game_list, item);
    }

    fn select(&self, ui: &mut Ui, entry: Option<Rc<Entry>>) {
        let name = entry.as_ref().map(|e| file_stem(&e.path.borrow()));
        *self.selected.borrow_mut() = entry;
        if let Some(field) = self.save_field {
            let text = name.unwrap_or_else(|| self.default_filename.clone());
            let mut f = ui.get::<TextFieldWidget>(field);
            f.set_text(&text);
            f.cursor_position = text.chars().count();
        }
    }

    fn select_first_visible(&self, ui: &mut Ui) {
        let first = if self.is_save_panel() {
            None
        } else {
            self.games.borrow().first().cloned()
        };
        self.select(ui, first);
        if let Some(field) = self.save_field {
            ui.take_keyboard_focus(field);
        }
    }

    fn rename(&self, ui: &mut Ui, entry: &Rc<Entry>, new_name: &str) {
        let old = entry.path.borrow().clone();
        let new = self.base_path.join(format!("{new_name}.{SAVE_EXTENSION}"));
        match std::fs::rename(&old, &new) {
            Ok(()) => {
                *entry.path.borrow_mut() = new;
                let title = ui.child(entry.item, "TITLE");
                let name = new_name.to_string();
                ui.get::<LabelWidget>(title).get_text = Some(Rc::new(move || name.clone()));
            }
            Err(e) => log::debug!("cannot rename {}: {e}", old.display()),
        }
    }

    fn delete(&self, ui: &mut Ui, entry: &Rc<Entry>) {
        let path = entry.path.borrow().clone();
        if let Err(e) = std::fs::remove_file(&path) {
            let text = message_with(
                ui,
                "notification-save-deletion-failed",
                &[("savePath", MessageArg::Text(&path.to_string_lossy()))],
            );
            super::chat_commands::add_debug_line(ui, text);
            log::debug!("{e}");
            return;
        }
        if self.selected.borrow().as_ref().is_some_and(|s| Rc::ptr_eq(s, entry)) {
            self.select(ui, None);
        }
        ui.remove_child(self.game_list, entry.item);
        self.games.borrow_mut().retain(|e| !Rc::ptr_eq(e, entry));
    }

    fn close(&self, ui: &mut Ui) {
        ui.close_window();
        (self.on_exit)(ui);
    }

    /// `Load`: a new local server loads the save, and the client reports ready.
    fn load(&self, ui: &mut Ui) {
        let Some(path) = self.selected_path() else { return };
        let save = match GameSave::read(&path) {
            Ok(save) => save,
            Err(e) => {
                log::warn!("cannot read the game save {}: {e}", path.display());
                return;
            }
        };
        let Some(map) = save.global_settings.map.clone() else { return };
        let maps = services(ui).maps.clone();
        if maps.borrow_mut().entry(&map).status != rustora_game::map_cache::MapStatus::Available {
            return;
        }
        ui.close_window();
        let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        ui.requests.push(AppRequest::CreateAndStartLocalServer {
            map,
            orders: vec![
                RawOrder::from_target_string("LoadGameSave", &file_name, true),
                RawOrder::command("state Ready"),
            ],
        });
    }

    /// `Save`: asks before an existing save is overwritten.
    fn save(self: &Rc<Self>, ui: &mut Ui) {
        let (Some(field), Some(world)) = (self.save_field, self.world.clone()) else {
            return;
        };
        let name = ui.get::<TextFieldWidget>(field).text();
        let filename = format!("{name}.{SAVE_EXTENSION}");
        let b = self.clone();
        let inner: Action = {
            let filename = filename.clone();
            Rc::new(move |ui: &mut Ui| {
                let orders = world.request_game_save(&filename);
                services(ui).net.borrow_mut().outgoing.extend(orders);
                b.close(ui);
            })
        };
        if self.base_path.join(&filename).exists() {
            let text = message_with(ui, "dialog-overwrite-save.prompt", &[("file", MessageArg::Text(&name))]);
            button_prompt_with_cancel(
                ui,
                "dialog-overwrite-save.title",
                &text,
                inner,
                "dialog-overwrite-save.confirm",
                Some(Rc::new(|_| {})),
                None,
            );
        } else {
            inner(ui);
        }
    }

    fn setup_buttons(self: &Rc<Self>, ui: &mut Ui) {
        let no_selection = |b: &Rc<Self>| -> Getter<bool> {
            let b = Rc::downgrade(b);
            Rc::new(move || b.upgrade().is_none_or(|b| b.selected.borrow().is_none()))
        };
        let rename = ui.child(self.panel, "RENAME_BUTTON");
        ui.button(rename).is_disabled = Some(no_selection(self));
        let b = Rc::downgrade(self);
        ui.button(rename).on_click = Some(Rc::new(move |ui| {
            let Some(b) = b.upgrade() else { return };
            let Some(entry) = b.selected.borrow().clone() else { return };
            let initial = file_stem(&entry.path.borrow());
            let (original, base) = (initial.clone(), b.base_path.clone());
            let validator = Rc::new(move |name: &str| {
                name != original && !name.trim().is_empty() && !name.chars().any(invalid_file_name_char) && !base.join(name).exists()
            });
            let b2 = b.clone();
            let on_accept = Rc::new(move |ui: &mut Ui, name: String| b2.rename(ui, &entry, &name));
            text_input_prompt(
                ui,
                "dialog-rename-save.title",
                "dialog-rename-save.prompt",
                &initial,
                on_accept,
                "dialog-rename-save.confirm",
                validator,
            );
        }));

        let delete = ui.child(self.panel, "DELETE_BUTTON");
        ui.button(delete).is_disabled = Some(no_selection(self));
        let b = Rc::downgrade(self);
        ui.button(delete).on_click = Some(Rc::new(move |ui| {
            let Some(b) = b.upgrade() else { return };
            let Some(entry) = b.selected.borrow().clone() else { return };
            let name = file_stem(&entry.path.borrow());
            let text = message_with(ui, "dialog-delete-save.prompt", &[("save", MessageArg::Text(&name))]);
            let b2 = b.clone();
            let on_confirm: Action = Rc::new(move |ui| {
                b2.delete(ui, &entry);
                if b2.games.borrow().is_empty() && !b2.is_save_panel() {
                    b2.close(ui);
                } else {
                    b2.select_first_visible(ui);
                }
            });
            button_prompt_with_cancel(
                ui,
                "dialog-delete-save.title",
                &text,
                on_confirm,
                "dialog-delete-save.confirm",
                Some(Rc::new(|_| {})),
                None,
            );
        }));

        let delete_all = ui.child(self.panel, "DELETE_ALL_BUTTON");
        let b = Rc::downgrade(self);
        ui.button(delete_all).is_disabled = Some(Rc::new(move || b.upgrade().is_none_or(|b| b.games.borrow().is_empty())));
        let b = Rc::downgrade(self);
        ui.button(delete_all).on_click = Some(Rc::new(move |ui| {
            let Some(b) = b.upgrade() else { return };
            let count = b.games.borrow().len();
            let text = message_with(ui, "dialog-delete-all-saves.prompt", &[("count", MessageArg::Number(count as i64))]);
            let b2 = b.clone();
            let on_confirm: Action = Rc::new(move |ui| {
                let games: Vec<Rc<Entry>> = b2.games.borrow().clone();
                for entry in &games {
                    b2.delete(ui, entry);
                }
                b2.close(ui);
            });
            button_prompt_with_cancel(
                ui,
                "dialog-delete-all-saves.title",
                &text,
                on_confirm,
                "dialog-delete-all-saves.confirm",
                Some(Rc::new(|_| {})),
                None,
            );
        }));
    }
}

struct BrowserLogic {
    browser: Rc<Browser>,
}

impl ChromeLogic for BrowserLogic {
    fn tick(&mut self, ui: &mut Ui) {
        if let Some(field) = self.browser.save_field {
            let empty = ui.get::<TextFieldWidget>(field).text().trim().is_empty();
            self.browser.save_name_empty.set(empty);
        }
    }

    fn disposed(&mut self, ui: &mut Ui) {
        services(ui).net.borrow_mut().before_game_start = None;
        ui.dispose_detached(self.browser.game_template);
        ui.dispose_detached(self.browser.new_template);
    }
}

/// `GameSaveBrowserLogic`. The save panel gets the `world` value, the load panel none.
pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let (on_exit, on_start) = (args.action("onExit"), args.action("onStart"));
    let world = args.value::<Rc<World>>("world");
    services(ui).net.borrow_mut().before_game_start = Some(Rc::new(move |ui| {
        ui.close_window();
        on_start(ui);
    }));
    let game_list = ui.child(widget, "GAME_LIST");
    let game_template = ui.child(game_list, "GAME_TEMPLATE");
    let new_template = ui.child(game_list, "NEW_TEMPLATE");
    ui.hide_child(game_list, game_template);
    ui.hide_child(game_list, new_template);
    let base_path = services(ui).save_dir.clone();

    let cancel = ui.child(widget, "CANCEL_BUTTON");
    let mut save_field = None;
    let mut default_filename = String::new();
    if let Some(world) = &world {
        let title_label = ui.child(widget, "SAVE_TITLE");
        ui.set_visible_fn(title_label, || true);
        let title = world.map().title.clone();
        default_filename = title.clone();
        let mut attempt = 0;
        while base_path.join(format!("{default_filename}.{SAVE_EXTENSION}")).exists() {
            attempt += 1;
            default_filename = format!("{title} ({attempt})");
        }
        let save_widgets = ui.child(widget, "SAVE_WIDGETS");
        ui.set_visible_fn(save_widgets, || true);
        let height = ui.node(save_widgets).bounds.h;
        ui.node_mut(game_list).bounds.h -= height;
        let field = ui.child(save_widgets, "SAVE_TEXTFIELD");
        save_field = Some(field);
    } else {
        let title_label = ui.child(widget, "LOAD_TITLE");
        ui.set_visible_fn(title_label, || true);
    }

    let browser = Rc::new(Browser {
        panel: widget,
        game_list,
        game_template,
        new_template,
        save_field,
        base_path,
        default_filename,
        games: RefCell::default(),
        selected: RefCell::default(),
        on_exit: on_exit.clone(),
        world,
        save_name_empty: Cell::new(false),
    });

    let b = Rc::downgrade(&browser);
    ui.button(cancel).on_click = Some(Rc::new(move |ui| {
        if let Some(b) = b.upgrade() {
            b.close(ui);
        }
    }));

    if let Some(field) = save_field {
        let save_button = ui.child(widget, "SAVE_BUTTON");
        ui.set_visible_fn(save_button, || true);
        let b = Rc::downgrade(&browser);
        ui.button(save_button).is_disabled = Some(Rc::new(move || b.upgrade().is_none_or(|b| b.save_name_empty.get())));
        let b = Rc::downgrade(&browser);
        ui.button(save_button).on_click = Some(Rc::new(move |ui| {
            if let Some(b) = b.upgrade() {
                b.save(ui);
            }
        }));
        let mut f = ui.get::<TextFieldWidget>(field);
        let b = Rc::downgrade(&browser);
        f.on_enter_key = Some(Rc::new(move |ui: &mut Ui, _e: &KeyInput| {
            if let Some(b) = b.upgrade()
                && !ui.get::<TextFieldWidget>(field).text().trim().is_empty()
            {
                b.save(ui);
            }
            true
        }));
        let b = Rc::downgrade(&browser);
        f.on_esc_key = Some(Rc::new(move |ui: &mut Ui, _e: &KeyInput| {
            if let Some(b) = b.upgrade() {
                b.close(ui);
            }
            true
        }));
    } else {
        let load_button = ui.child(widget, "LOAD_BUTTON");
        ui.set_visible_fn(load_button, || true);
        ui.button(load_button).is_disabled = Some(no_selection_getter(&browser));
        let b = Rc::downgrade(&browser);
        ui.button(load_button).on_click = Some(Rc::new(move |ui| {
            if let Some(b) = b.upgrade() {
                b.load(ui);
            }
        }));
    }

    browser.load_games(ui);
    browser.setup_buttons(ui);
    browser.select_first_visible(ui);
    cell(BrowserLogic { browser })
}

fn no_selection_getter(browser: &Rc<Browser>) -> Getter<bool> {
    let b = Rc::downgrade(browser);
    Rc::new(move || b.upgrade().is_none_or(|b| b.selected.borrow().is_none()))
}

/// Whether any playable player has not finished: the `SAVE_GAME` button of the ingame menu needs one.
pub fn can_save(world: &World) -> bool {
    world
        .players()
        .iter()
        .any(|p| p.playable && p.win_state.get() == WinState::Undefined)
}

struct LoadingLogic {
    world: Option<Rc<World>>,
    percentage: Rc<Cell<i32>>,
}

impl ChromeLogic for LoadingLogic {
    fn tick(&mut self, _ui: &mut Ui) {
        if let Some(world) = &self.world {
            let last = world.game_save_last_frame();
            if last > 0 {
                self.percentage.set(world.net_frame_number() * 100 / last);
            }
        }
    }
}

/// `GameSaveLoadingLogic`.
pub fn loading(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(world) = args.value::<Rc<World>>("world") else {
        return cell(LoadingLogic {
            world: None,
            percentage: Rc::default(),
        });
    };
    let progress = ui.child(widget, "PROGRESS");
    let percentage = Rc::new(Cell::new(0));
    {
        let p = percentage.clone();
        ui.get::<ProgressBarWidget>(progress).get_percentage = Some(Rc::new(move || p.get()));
    }
    let handler = ui.child(widget, "CANCEL_HANDLER");
    ui.get::<LogicKeyListenerWidget>(handler)
        .handlers
        .push(Rc::new(|ui: &mut Ui, e: &KeyInput| {
            if e.event == KeyEvent::Down && e.key == "ESCAPE" {
                ui.requests.push(AppRequest::LeaveGame);
                return true;
            }
            false
        }));
    cell(LoadingLogic {
        world: Some(world),
        percentage,
    })
}
