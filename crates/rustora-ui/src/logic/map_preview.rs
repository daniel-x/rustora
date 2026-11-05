// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MapPreviewLogic.cs`. Not ported: map updates (`MapCache.GetUpdatedMap`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_game::map_cache::{MapPreview, MapStatus};

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::{message, message_with, services};
use crate::widget::{ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{LabelWithTooltipWidget, MapPreviewWidget, SpawnOccupant};

pub(crate) const MAP_STATUS_PLAYABLE: i32 = 2;
const MAP_STATUS_INCOMPATIBLE: i32 = 4;
const MAP_STATUS_VALIDATING: i32 = 1;

/// The `getMap` argument.
pub type GetMap = Getter<(Option<Rc<MapPreview>>, i32)>;
/// The `onMouseDown` argument.
pub type OnPreviewMouseDown = Rc<dyn Fn(&mut Ui, WidgetId, Rc<MapPreview>, &crate::input::MouseInput)>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PreviewStatus {
    Unknown,
    Playable,
    Incompatible,
    Validating,
    DownloadAvailable,
    Searching,
    Downloading,
    DownloadError,
    Unavailable,
}

const BLINK_TICK_LENGTH: i32 = 10;

struct MapPreviewLogic {
    get_map: GetMap,
    titles: [WidgetId; 2],
    authors: Vec<WidgetId>,
    shown: Option<(String, String, String)>,
    preview_widgets: HashMap<PreviewStatus, Vec<WidgetId>>,
    visible: Rc<RefCell<Vec<WidgetId>>>,
    current: Cell<Option<PreviewStatus>>,
    blink: Rc<Cell<bool>>,
    blink_tick: i32,
    retry: Rc<dyn Fn(&mut Ui)>,
    retry_triggered: Rc<Cell<bool>>,
    download_text: Rc<RefCell<String>>,
    download_state: Option<(i64, i32)>,
}

impl MapPreviewLogic {
    /// Sets the texts of a map, which draw code cannot measure.
    fn show_map_texts(&mut self, ui: &mut Ui, map: &MapPreview) {
        let key = (map.uid.clone(), map.title.clone(), map.author.clone());
        if self.shown.as_ref() == Some(&key) {
            return;
        }
        self.shown = Some(key);
        for title in self.titles {
            LabelWithTooltipWidget::truncate_to_tooltip(ui, title, &map.title);
        }
        let text = message_with(ui, "label-created-by", &[("author", MessageArg::Text(&map.author))]);
        for &author in &self.authors {
            let width = ui.node(author).bounds.w;
            let font = ui.label(author).font.clone();
            let truncated = ui.truncate_text(&font, &text, width);
            ui.label(author).get_text = Some(Rc::new(move || truncated.clone()));
        }
    }

    /// The text of `MAP_STATUS_DOWNLOADING`.
    fn update_download_text(&mut self, ui: &Ui, map: &MapPreview) {
        let state = (map.download_bytes, map.download_percentage);
        if self.download_state.replace(state) == Some(state) {
            return;
        }
        let size = MessageArg::Number(map.download_bytes / 1024);
        *self.download_text.borrow_mut() = if map.download_bytes == 0 {
            message(ui, "label-connecting")
        } else if map.download_percentage == 0 {
            message_with(ui, "label-downloading-map", &[("size", size)])
        } else {
            let progress = MessageArg::Number(map.download_percentage as i64);
            message_with(ui, "label-downloading-map-progress", &[("size", size), ("progress", progress)])
        };
    }

    fn status(map: &Option<Rc<MapPreview>>, server_status: i32) -> Option<PreviewStatus> {
        let Some(map) = map else {
            return Some(PreviewStatus::Unknown);
        };
        Some(match map.status {
            MapStatus::Available if server_status & MAP_STATUS_PLAYABLE != 0 => PreviewStatus::Playable,
            MapStatus::Available if server_status & MAP_STATUS_INCOMPATIBLE != 0 => PreviewStatus::Incompatible,
            MapStatus::Available if server_status & MAP_STATUS_VALIDATING != 0 => PreviewStatus::Validating,
            MapStatus::Available => return None,
            MapStatus::DownloadAvailable => PreviewStatus::DownloadAvailable,
            MapStatus::Searching => PreviewStatus::Searching,
            MapStatus::Unavailable => PreviewStatus::Unavailable,
            MapStatus::DownloadError => PreviewStatus::DownloadError,
            MapStatus::Downloading => PreviewStatus::Downloading,
        })
    }
}

impl ChromeLogic for MapPreviewLogic {
    fn tick(&mut self, ui: &mut Ui) {
        self.blink_tick += 1;
        if self.blink_tick >= BLINK_TICK_LENGTH {
            self.blink.set(!self.blink.get());
            self.blink_tick = 0;
        }
        let (map, server_status) = (self.get_map)();
        if let Some(map) = &map {
            self.show_map_texts(ui, map);
            self.update_download_text(ui, map);
        }
        let Some(mut status) = Self::status(&map, server_status) else {
            return;
        };
        if self.current.get() == Some(status) {
            return;
        }
        if status == PreviewStatus::Unavailable {
            if self.retry_triggered.get() {
                self.retry_triggered.set(false);
            } else {
                (self.retry)(ui);
                let uid = map.as_ref().map(|m| m.uid.clone()).unwrap_or_default();
                if services(ui)
                    .maps
                    .borrow()
                    .get(&uid)
                    .is_some_and(|m| m.status == MapStatus::Searching)
                {
                    status = PreviewStatus::Searching;
                }
            }
        } else if status != PreviewStatus::Searching {
            self.retry_triggered.set(false);
        }
        self.current.set(Some(status));
        for &widget in self.visible.borrow().iter() {
            ui.set_visible_fn(widget, || false);
        }
        *self.visible.borrow_mut() = self.preview_widgets[&status].clone();
        for &widget in self.visible.borrow().iter() {
            ui.set_visible_fn(widget, || true);
        }
    }
}

/// The `onSuccess` action of `MapPreview.Install` in the lobby.
fn install(ui: &mut Ui, uid: &str) {
    let on_success: Box<dyn FnOnce(&mut Ui)> = Box::new(|ui: &mut Ui| {
        let net = services(ui).net.clone();
        if net.borrow().local_client().is_some() {
            net.borrow_mut().command("state NotReady");
        }
    });
    let repository = services(ui).map_repository.clone();
    repository.install(ui, uid, Some(on_success));
}

/// `SetUpInstallButton`.
fn setup_install_button(ui: &mut Ui, parent: WidgetId, get_map: &GetMap, blink: &Rc<Cell<bool>>) {
    let button = ui.child(parent, "MAP_INSTALL");
    let blink = blink.clone();
    let map = get_map.clone();
    let mut b = ui.button(button);
    b.is_highlighted = Some(Rc::new(move || blink.get()));
    b.on_click = Some(Rc::new(move |ui: &mut Ui| {
        if let Some(m) = map().0 {
            install(ui, &m.uid);
        }
    }));
}

/// `SetUpDownloadProgress`.
fn setup_download_progress(ui: &mut Ui, parent: WidgetId, get_map: &GetMap, download_text: &Rc<RefCell<String>>) {
    let bar = ui.child(parent, "MAP_PROGRESSBAR");
    {
        let (a, b) = (get_map.clone(), get_map.clone());
        let mut progress = ui.get::<crate::widgets::ProgressBarWidget>(bar);
        progress.is_indeterminate = Some(Rc::new(move || a().0.is_none_or(|m| m.download_percentage == 0)));
        progress.get_percentage = Some(Rc::new(move || b().0.map_or(0, |m| m.download_percentage)));
    }
    let label = ui.child(parent, "MAP_STATUS_DOWNLOADING");
    let text = download_text.clone();
    ui.label(label).get_text = Some(Rc::new(move || text.borrow().clone()));
}

/// Port of `SetupMapPreview`. Returns the title label.
fn setup_map_preview(ui: &mut Ui, parent: WidgetId, get_map: &GetMap, args: &WidgetArgs) -> WidgetId {
    let preview = ui.child(parent, "MAP_PREVIEW");
    {
        let mut widget = ui.get::<MapPreviewWidget>(preview);
        let map = get_map.clone();
        widget.preview = Some(Rc::new(move || map().0));
        if let Some(on_mouse_down) = args.value::<OnPreviewMouseDown>("onMouseDown") {
            let map = get_map.clone();
            widget.on_mouse_down = Some(Rc::new(move |ui, id, mi| {
                if let Some(current) = map().0 {
                    on_mouse_down(ui, id, current, mi);
                }
            }));
        }
        widget.spawn_occupants = args.value::<Getter<HashMap<i32, SpawnOccupant>>>("getSpawnOccupants");
        widget.disabled_spawn_points = args.value::<Getter<Vec<i32>>>("getDisabledSpawnPoints");
        widget.show_unoccupied_spawnpoints = args.value::<bool>("showUnoccupiedSpawnpoints").unwrap_or(true);
    }

    let title = ui.child(parent, "MAP_TITLE");
    let map = get_map.clone();
    ui.set_visible_fn(title, move || map().0.is_some());
    title
}

/// Port of `SetupAuthorAndMapType`. Returns the author label.
fn setup_author_and_map_type(ui: &mut Ui, parent: WidgetId, get_map: &GetMap) -> WidgetId {
    let type_label = ui.child(parent, "MAP_TYPE");
    let map = get_map.clone();
    ui.label(type_label).get_text = Some(Rc::new(move || {
        map().0.and_then(|m| m.categories.first().cloned()).unwrap_or_default()
    }));
    ui.child(parent, "MAP_AUTHOR")
}

pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let get_map = args.value::<GetMap>("getMap").unwrap_or_else(|| Rc::new(|| (None, 0)));
    let large = ui.child(widget, "MAP_LARGE");
    let small = ui.child(widget, "MAP_SMALL");
    let titles = [
        setup_map_preview(ui, large, &get_map, args),
        setup_map_preview(ui, small, &get_map, args),
    ];
    let blink = Rc::new(Cell::new(false));
    let available = ui.child(widget, "MAP_AVAILABLE");
    let download_available = ui.child(widget, "MAP_DOWNLOAD_AVAILABLE");
    let authors = vec![
        setup_author_and_map_type(ui, available, &get_map),
        setup_author_and_map_type(ui, download_available, &get_map),
    ];
    setup_install_button(ui, download_available, &get_map, &blink);
    let downloading = ui.child(widget, "MAP_DOWNLOADING");
    let download_text = Rc::new(RefCell::new(String::new()));
    setup_download_progress(ui, downloading, &get_map, &download_text);

    let retry_triggered = Rc::new(Cell::new(false));
    let retry_button = ui.child(widget, "MAP_RETRY");
    let retry: Rc<dyn Fn(&mut Ui)> = {
        let (map, triggered) = (get_map.clone(), retry_triggered.clone());
        Rc::new(move |ui: &mut Ui| {
            triggered.set(true);
            let Some(m) = map().0 else {
                return;
            };
            let services = services(ui);
            let status = services.maps.borrow().get(&m.uid).map_or(m.status, |m| m.status);
            if status == MapStatus::DownloadError {
                install(ui, &m.uid);
            } else if status == MapStatus::Unavailable {
                services.map_repository.query(&services.maps, [m.uid.as_str()]);
            }
        })
    };
    {
        let (retry_install, retry_search) = (message(ui, "button-retry-install"), message(ui, "button-retry-search"));
        let map = get_map.clone();
        let click = retry.clone();
        let mut b = ui.button(retry_button);
        b.get_text = Some(Rc::new(move || {
            if map().0.is_some_and(|m| m.status == MapStatus::DownloadError) {
                retry_install.clone()
            } else {
                retry_search.clone()
            }
        }));
        b.on_click = Some(Rc::new(move |ui: &mut Ui| click(ui)));
    }

    let child = |ui: &Ui, name: &str| ui.child(widget, name);
    let preview_widgets = HashMap::from([
        (PreviewStatus::Unknown, vec![large]),
        (PreviewStatus::Playable, vec![large, available]),
        (PreviewStatus::Incompatible, vec![large, child(ui, "MAP_INCOMPATIBLE")]),
        (PreviewStatus::Validating, vec![small, child(ui, "MAP_VALIDATING")]),
        (PreviewStatus::DownloadAvailable, vec![small, download_available]),
        (PreviewStatus::Searching, vec![small, child(ui, "MAP_SEARCHING")]),
        (PreviewStatus::Downloading, vec![small, downloading]),
        (PreviewStatus::Unavailable, vec![small, child(ui, "MAP_UNAVAILABLE"), retry_button]),
        (PreviewStatus::DownloadError, vec![small, child(ui, "MAP_ERROR"), retry_button]),
    ]);
    for name in ["MAP_UPDATE_AVAILABLE", "MAP_UPDATE", "MAP_UPDATE_DOWNLOAD_AVAILABLE"] {
        if let Some(id) = ui.find(widget, name) {
            ui.set_visible_fn(id, || false);
        }
    }
    for widgets in preview_widgets.values() {
        for &id in widgets {
            ui.set_visible_fn(id, || false);
        }
    }
    cell(MapPreviewLogic {
        get_map,
        titles,
        authors,
        shown: None,
        preview_widgets,
        visible: Rc::default(),
        current: Cell::new(None),
        blink,
        blink_tick: 0,
        retry,
        retry_triggered,
        download_text,
        download_state: None,
    })
}
