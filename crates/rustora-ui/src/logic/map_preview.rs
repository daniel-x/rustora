// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MapPreviewLogic.cs` without map downloads: a map that is not local shows as unavailable.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustora_game::map_cache::{MapPreview, MapStatus};

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::message_with;
use crate::widget::{ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{LabelWithTooltipWidget, MapPreviewWidget, SpawnOccupant};

const MAP_STATUS_PLAYABLE: i32 = 2;
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
    Unavailable,
}

struct MapPreviewLogic {
    get_map: GetMap,
    titles: [WidgetId; 2],
    author: WidgetId,
    shown_uid: Option<String>,
    preview_widgets: HashMap<PreviewStatus, Vec<WidgetId>>,
    visible: Rc<RefCell<Vec<WidgetId>>>,
    current: Cell<Option<PreviewStatus>>,
}

impl MapPreviewLogic {
    /// Sets the texts of a map, which draw code cannot measure.
    fn show_map_texts(&mut self, ui: &mut Ui, map: &MapPreview) {
        if self.shown_uid.as_deref() == Some(&map.uid) {
            return;
        }
        self.shown_uid = Some(map.uid.clone());
        for title in self.titles {
            LabelWithTooltipWidget::truncate_to_tooltip(ui, title, &map.title);
        }
        let text = message_with(ui, "label-created-by", &[("author", MessageArg::Text(&map.author))]);
        let width = ui.node(self.author).bounds.w;
        let font = ui.label(self.author).font.clone();
        let truncated = ui.truncate_text(&font, &text, width);
        ui.label(self.author).get_text = Some(Rc::new(move || truncated.clone()));
    }
}

impl ChromeLogic for MapPreviewLogic {
    fn tick(&mut self, ui: &mut Ui) {
        let (map, server_status) = (self.get_map)();
        if let Some(map) = &map {
            self.show_map_texts(ui, map);
        }
        let status = match map {
            None => PreviewStatus::Unknown,
            Some(map) if map.status != MapStatus::Available => PreviewStatus::Unavailable,
            Some(_) if server_status & MAP_STATUS_PLAYABLE != 0 => PreviewStatus::Playable,
            Some(_) if server_status & MAP_STATUS_INCOMPATIBLE != 0 => PreviewStatus::Incompatible,
            Some(_) if server_status & MAP_STATUS_VALIDATING != 0 => PreviewStatus::Validating,
            Some(_) => return,
        };
        if self.current.get() == Some(status) {
            return;
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
    let available = ui.child(widget, "MAP_AVAILABLE");
    let author = setup_author_and_map_type(ui, available, &get_map);

    let incompatible = ui.child(widget, "MAP_INCOMPATIBLE");
    let validating = ui.child(widget, "MAP_VALIDATING");
    let unavailable = ui.child(widget, "MAP_UNAVAILABLE");
    let preview_widgets = HashMap::from([
        (PreviewStatus::Unknown, vec![large]),
        (PreviewStatus::Playable, vec![large, available]),
        (PreviewStatus::Incompatible, vec![large, incompatible]),
        (PreviewStatus::Validating, vec![small, validating]),
        (PreviewStatus::Unavailable, vec![small, unavailable]),
    ]);
    for name in [
        "MAP_UPDATE_AVAILABLE",
        "MAP_UPDATE",
        "MAP_DOWNLOAD_AVAILABLE",
        "MAP_UPDATE_DOWNLOAD_AVAILABLE",
        "MAP_SEARCHING",
        "MAP_DOWNLOADING",
        "MAP_RETRY",
        "MAP_ERROR",
    ] {
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
        author,
        shown_uid: None,
        preview_widgets,
        visible: Rc::default(),
        current: Cell::new(None),
    })
}
