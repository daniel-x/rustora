// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MapChooserLogic.cs`. Remote map queries are not ported: a remote pool lists the maps found locally.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::UNIX_EPOCH;

use rustora_game::map_cache::{MapClassification, MapPreview, MapStatus, MapVisibility};

use crate::fluent::MessageArg;
use crate::loader::cell;
use crate::logic::{NoLogic, button_prompt, cosmetic_random, message, message_with, services};
use crate::widget::{Action, ChromeLogic, Getter, Ui, WidgetArgs, WidgetId};
use crate::widgets::{DropDownButtonWidget, LabelWidget, LabelWithTooltipWidget, MapPreviewWidget, ScrollItemWidget, TextFieldWidget};

/// The `onSelect` argument.
pub type OnSelectMap = Rc<dyn Fn(&mut Ui, String)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrderBy {
    Players,
    Title,
    Date,
    Size,
}

struct Chooser {
    widget: WidgetId,
    filter: MapVisibility,
    on_select: Option<OnSelectMap>,
    item_template: WidgetId,
    remote_map_pool: Option<Vec<String>>,
    game_mode_dropdown: Option<WidgetId>,
    current_tab: Cell<MapClassification>,
    scrollpanels: RefCell<HashMap<MapClassification, WidgetId>>,
    tab_maps: RefCell<HashMap<MapClassification, Vec<Rc<MapPreview>>>>,
    visible_maps: RefCell<Vec<String>>,
    selected_uid: Rc<RefCell<String>>,
    category: RefCell<Option<String>>,
    map_filter: RefCell<Option<String>>,
    order_by: Cell<OrderBy>,
    unavailable: Cell<i32>,
}

impl Chooser {
    /// Port of `RefreshMaps`.
    fn refresh_maps(&self, ui: &Ui, tab: MapClassification) {
        let cache = services(ui).maps.clone();
        let cache = cache.borrow();
        let maps = if tab != MapClassification::Remote {
            cache
                .previews
                .values()
                .filter(|m| m.status == MapStatus::Available && m.class == tab && m.visibility.intersects(self.filter))
                .cloned()
                .collect()
        } else if let Some(pool) = &self.remote_map_pool {
            let loaded: Vec<_> = pool.iter().filter_map(|uid| cache.get(uid)).collect();
            self.unavailable.set((pool.len() - loaded.len()) as i32);
            loaded
        } else {
            Vec::new()
        };
        self.tab_maps.borrow_mut().insert(tab, maps);
    }

    /// Port of `EnumerateMaps`.
    fn enumerate_maps(self: &Rc<Self>, ui: &mut Ui, tab: MapClassification) {
        let filter = self.map_filter.borrow().clone();
        let player_count_filter = filter.as_deref().and_then(|f| f.parse::<i32>().ok()).unwrap_or(-1);
        let lowered = filter.as_ref().map(|f| f.to_lowercase());
        let category = self.category.borrow().clone();
        let mut maps: Vec<Rc<MapPreview>> = self.tab_maps.borrow()[&tab]
            .iter()
            .filter(|m| {
                category.as_ref().is_none_or(|c| m.categories.contains(c))
                    && lowered.as_ref().is_none_or(|f| {
                        m.title.to_lowercase().contains(f) || m.author.to_lowercase().contains(f) || m.player_count == player_count_filter
                    })
            })
            .cloned()
            .collect();
        let title_key = |m: &MapPreview| (m.title.to_lowercase(), m.title.clone());
        match self.order_by.get() {
            OrderBy::Title => maps.sort_by_key(|m| title_key(m)),
            OrderBy::Players => maps.sort_by_key(|m| (m.player_count, title_key(m))),
            OrderBy::Size => maps.sort_by_key(|m| (m.bounds.width * m.bounds.height, title_key(m))),
            OrderBy::Date => maps.sort_by_key(|m| {
                let age = m.modified_date.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
                (std::cmp::Reverse(age), title_key(m))
            }),
        }

        let panel = self.scrollpanels.borrow()[&tab];
        ui.remove_children(panel);
        for preview in &maps {
            let item = self.map_item(ui, preview);
            ui.node_mut(item).cull_to = Some(panel);
            ui.add_child(panel, item);
        }

        if tab == self.current_tab.get() {
            *self.visible_maps.borrow_mut() = maps.iter().map(|m| m.uid.clone()).collect();
            self.setup_game_mode_dropdown(ui, tab);
        }
        let selected = self.selected_uid.borrow().clone();
        if self.visible_maps.borrow().contains(&selected) {
            ui.scroll_to_item(panel, &selected, false);
        }
    }

    fn map_item(self: &Rc<Self>, ui: &mut Ui, preview: &Rc<MapPreview>) -> WidgetId {
        let uid = preview.uid.clone();
        let selected = self.selected_uid.clone();
        let is_selected: Getter<bool> = {
            let (selected, uid) = (selected.clone(), uid.clone());
            Rc::new(move || *selected.borrow() == uid)
        };
        let on_click: Action = {
            let uid = uid.clone();
            Rc::new(move |_| *selected.borrow_mut() = uid.clone())
        };
        let on_double_click: Action = {
            let on_select = self.on_select.clone();
            let uid = uid.clone();
            Rc::new(move |ui| {
                if let Some(on_select) = &on_select {
                    ui.close_window();
                    on_select(ui, uid.clone());
                }
            })
        };
        let item = ScrollItemWidget::setup_with_key(ui, &uid, self.item_template, is_selected, on_click, on_double_click);

        let title = ui.child(item, "TITLE");
        LabelWithTooltipWidget::truncate_to_tooltip(ui, title, &preview.title);

        let preview_widget = ui.child(item, "PREVIEW");
        let shown = preview.clone();
        ui.get::<MapPreviewWidget>(preview_widget).preview = Some(Rc::new(move || Some(shown.clone())));

        if let Some(details_widget) = ui.find(item, "DETAILS") {
            let mut details = preview.categories.first().map_or_else(String::new, |c| format!("{c} "));
            details += &message_with(
                ui,
                "label-player-count",
                &[("players", MessageArg::Number(preview.player_count as i64))],
            );
            ui.label(details_widget).get_text = Some(Rc::new(move || details.clone()));
        }

        if let Some(author_widget) = ui.find(item, "AUTHOR")
            && !preview.author.is_empty()
        {
            let text = message_with(ui, "label-created-by", &[("author", MessageArg::Text(&preview.author))]);
            LabelWithTooltipWidget::truncate_to_tooltip(ui, author_widget, &text);
        }

        if let Some(size_widget) = ui.find(item, "SIZE") {
            let (w, h) = (preview.bounds.width, preview.bounds.height);
            let cells = w * h;
            let key = if cells >= 120 * 120 {
                "label-map-size-huge"
            } else if cells >= 90 * 90 {
                "label-map-size-large"
            } else if cells >= 60 * 60 {
                "label-map-size-medium"
            } else {
                "label-map-size-small"
            };
            let size = format!("{w}x{h} {}", message(ui, key));
            ui.label(size_widget).get_text = Some(Rc::new(move || size.clone()));
        }
        item
    }

    /// Port of `SetupGameModeDropdown`.
    fn setup_game_mode_dropdown(self: &Rc<Self>, ui: &mut Ui, tab: MapClassification) {
        let Some(dropdown) = self.game_mode_dropdown else {
            return;
        };
        let mut counts: BTreeMap<String, i32> = BTreeMap::new();
        let total = {
            let tab_maps = self.tab_maps.borrow();
            for map in &tab_maps[&tab] {
                for category in &map.categories {
                    *counts.entry(category.clone()).or_default() += 1;
                }
            }
            tab_maps[&tab].len() as i32
        };
        let mut categories: Vec<(Option<String>, i32)> = vec![(None, total)];
        categories.extend(counts.into_iter().map(|(c, n)| (Some(c), n)));
        let all_maps = message(ui, "label-all-maps");
        let no_matches = message(ui, "label-no-matches");
        let show = {
            let all_maps = all_maps.clone();
            move |category: &Option<String>, count: i32| format!("{} ({count})", category.as_deref().unwrap_or(&all_maps))
        };
        let this = self.clone();
        let options = categories.clone();
        let show_item = show.clone();
        ui.button(dropdown).on_click = Some(Rc::new(move |ui| {
            let this = this.clone();
            let show_item = show_item.clone();
            DropDownButtonWidget::show_dropdown(
                ui,
                dropdown,
                "LABEL_DROPDOWN_TEMPLATE",
                210,
                options.clone(),
                move |ui, (category, count), template| {
                    let (a, b) = (this.clone(), category.clone());
                    let is_selected: Getter<bool> = Rc::new(move || *a.category.borrow() == b);
                    let (a, b) = (this.clone(), category.clone());
                    let on_click: Action = Rc::new(move |ui| {
                        *a.category.borrow_mut() = b.clone();
                        a.enumerate_maps(ui, tab);
                    });
                    let item = ScrollItemWidget::setup(ui, template, is_selected, on_click);
                    let label = ui.child(item, "LABEL");
                    let text = show_item(&category, count);
                    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
                    item
                },
            );
        }));
        let this = self.clone();
        ui.button(dropdown).get_text = Some(Rc::new(move || {
            let category = this.category.borrow();
            match categories.iter().find(|(c, _)| *c == *category) {
                Some((c, count)) => show(c, *count),
                None => format!("{no_matches} (0)"),
            }
        }));
    }

    /// Port of `SetupOrderByDropdown`.
    fn setup_order_by_dropdown(self: &Rc<Self>, ui: &mut Ui) {
        let Some(dropdown) = ui.find(self.widget, "ORDERBY") else {
            return;
        };
        let options: Vec<(String, OrderBy)> = [
            ("options-order-maps.player-count", OrderBy::Players),
            ("options-order-maps.title", OrderBy::Title),
            ("options-order-maps.date", OrderBy::Date),
            ("options-order-maps.size", OrderBy::Size),
        ]
        .iter()
        .map(|(key, order)| (message(ui, key), *order))
        .collect();
        let this = self.clone();
        let list = options.clone();
        ui.button(dropdown).on_click = Some(Rc::new(move |ui| {
            let this = this.clone();
            DropDownButtonWidget::show_dropdown(
                ui,
                dropdown,
                "LABEL_DROPDOWN_TEMPLATE",
                500,
                list.clone(),
                move |ui, (text, order), template| {
                    let a = this.clone();
                    let is_selected: Getter<bool> = Rc::new(move || a.order_by.get() == order);
                    let a = this.clone();
                    let on_click: Action = Rc::new(move |ui| {
                        a.order_by.set(order);
                        a.enumerate_maps(ui, a.current_tab.get());
                    });
                    let item = ScrollItemWidget::setup(ui, template, is_selected, on_click);
                    let label = ui.child(item, "LABEL");
                    ui.get::<LabelWidget>(label).get_text = Some(Rc::new(move || text.clone()));
                    item
                },
            );
        }));
        let this = self.clone();
        ui.button(dropdown).get_text = Some(Rc::new(move || {
            let order = this.order_by.get();
            options
                .iter()
                .find(|(_, o)| *o == order)
                .map_or_else(String::new, |(text, _)| text.clone())
        }));
    }

    /// Port of `SetupMapTab`.
    fn setup_map_tab(self: &Rc<Self>, ui: &mut Ui, tab: MapClassification, button: &str, container: &str) {
        let tab_container = ui.child(self.widget, container);
        let this = self.clone();
        ui.set_visible_fn(tab_container, move || this.current_tab.get() == tab);
        let panel = ui.child(tab_container, "MAP_LIST");
        ui.set_grid_layout(panel);
        self.scrollpanels.borrow_mut().insert(tab, panel);

        let tab_button = ui.child(self.widget, button);
        let this = self.clone();
        ui.button(tab_button).is_highlighted = Some(Rc::new(move || this.current_tab.get() == tab));
        if self.remote_map_pool.is_some() {
            let is_remote = tab == MapClassification::Remote;
            ui.set_visible_fn(tab_button, move || is_remote);
        } else {
            let this = self.clone();
            ui.set_visible_fn(tab_button, move || this.tab_maps.borrow().get(&tab).is_some_and(|m| !m.is_empty()));
        }
        let this = self.clone();
        ui.button(tab_button).on_click = Some(Rc::new(move |ui| {
            this.current_tab.set(tab);
            this.enumerate_maps(ui, tab);
        }));
        self.refresh_maps(ui, tab);
    }

    /// Port of `DeleteMap`.
    fn delete_map(&self, ui: &mut Ui, uid: &str) -> String {
        let result = services(ui).maps.borrow_mut().delete(uid);
        match result {
            Ok(()) => {
                if *self.selected_uid.borrow() == uid {
                    let first = self.tab_maps.borrow()[&self.current_tab.get()]
                        .iter()
                        .map(|m| m.uid.clone())
                        .find(|u| u != uid);
                    let chosen = services(ui)
                        .maps
                        .borrow()
                        .choose_initial_map(first.as_deref(), &mut cosmetic_random);
                    *self.selected_uid.borrow_mut() = chosen;
                }
            }
            Err(e) => log::warn!("Failed to delete map {uid}: {e}"),
        }
        self.selected_uid.borrow().clone()
    }

    /// The common end of deleting one or all maps.
    fn after_delete(self: &Rc<Self>, ui: &mut Ui, new_uid: &str, switch_always: bool) {
        let current = self.current_tab.get();
        self.refresh_maps(ui, current);
        self.enumerate_maps(ui, current);
        if switch_always || self.tab_maps.borrow()[&current].is_empty() {
            let class = services(ui).maps.borrow().get(new_uid).map(|m| m.class);
            if let Some(class) = class.filter(|c| self.scrollpanels.borrow().contains_key(c)) {
                self.current_tab.set(class);
                self.enumerate_maps(ui, class);
            }
        }
    }
}

pub fn create(ui: &mut Ui, widget: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let on_exit = args.action("onExit");
    let on_select = args.value::<OnSelectMap>("onSelect");
    let initial_map = args.value::<Option<String>>("initialMap").flatten();
    let initial_tab = args.value::<MapClassification>("initialTab").unwrap_or(MapClassification::System);
    let remote_map_pool = args.value::<Option<Vec<String>>>("remoteMapPool").flatten();
    let filter = args.value::<MapVisibility>("filter").unwrap_or(MapVisibility::LOBBY);

    let item_template = ui.child(widget, "MAP_TEMPLATE");
    let parent = ui.node(item_template).parent.expect("template parent");
    ui.node_mut(parent).children.retain(|&c| c != item_template);

    let this = Rc::new(Chooser {
        widget,
        filter,
        on_select,
        item_template,
        remote_map_pool,
        game_mode_dropdown: ui.find(widget, "GAMEMODE_FILTER"),
        current_tab: Cell::new(MapClassification::Unknown),
        scrollpanels: RefCell::default(),
        tab_maps: RefCell::default(),
        visible_maps: RefCell::default(),
        selected_uid: Rc::default(),
        category: RefCell::new(None),
        map_filter: RefCell::new(None),
        order_by: Cell::new(OrderBy::Players),
        unavailable: Cell::new(0),
    });

    let approving: Action = {
        let this = this.clone();
        Rc::new(move |ui| {
            ui.close_window();
            if let Some(on_select) = &this.on_select {
                on_select(ui, this.selected_uid.borrow().clone());
            }
        })
    };
    let canceling: Action = Rc::new(move |ui| {
        ui.close_window();
        on_exit(ui);
    });

    let ok_button = ui.child(widget, "BUTTON_OK");
    ui.button(ok_button).disabled = this.on_select.is_none();
    ui.button(ok_button).on_click = Some(approving.clone());
    let cancel_button = ui.child(widget, "BUTTON_CANCEL");
    ui.button(cancel_button).on_click = Some(canceling.clone());

    this.setup_order_by_dropdown(ui);

    if let Some(input) = ui.find(widget, "MAPFILTER_INPUT") {
        ui.take_keyboard_focus(input);
        let mut field = ui.get::<TextFieldWidget>(input);
        let a = this.clone();
        let cancel = canceling.clone();
        field.on_esc_key = Some(Rc::new(move |ui, _| {
            if ui.get::<TextFieldWidget>(input).text().is_empty() {
                cancel(ui);
            } else {
                ui.get::<TextFieldWidget>(input).set_text("");
                *a.map_filter.borrow_mut() = None;
                a.enumerate_maps(ui, a.current_tab.get());
            }
            true
        }));
        let approve = approving.clone();
        field.on_enter_key = Some(Rc::new(move |ui, _| {
            approve(ui);
            true
        }));
        let a = this.clone();
        field.on_text_edited = Some(Rc::new(move |ui| {
            let text = ui.get::<TextFieldWidget>(input).text();
            *a.map_filter.borrow_mut() = Some(text);
            a.enumerate_maps(ui, a.current_tab.get());
        }));
    }

    if let Some(random) = ui.find(widget, "RANDOMMAP_BUTTON") {
        let a = this.clone();
        ui.button(random).on_click = Some(Rc::new(move |ui| {
            let uid = {
                let visible = a.visible_maps.borrow();
                if visible.is_empty() {
                    return;
                }
                visible[cosmetic_random(visible.len())].clone()
            };
            *a.selected_uid.borrow_mut() = uid.clone();
            let panel = a.scrollpanels.borrow()[&a.current_tab.get()];
            ui.scroll_to_item(panel, &uid, true);
        }));
        let a = this.clone();
        ui.button(random).is_disabled = Some(Rc::new(move || a.visible_maps.borrow().is_empty()));
    }

    let delete_button = ui.child(widget, "DELETE_MAP_BUTTON");
    let a = this.clone();
    ui.button(delete_button).is_disabled = Some(Rc::new(move || a.current_tab.get() != MapClassification::User));
    let a = this.clone();
    ui.set_visible_fn(delete_button, move || a.current_tab.get() == MapClassification::User);
    let a = this.clone();
    ui.button(delete_button).on_click = Some(Rc::new(move |ui| {
        let uid = a.selected_uid.borrow().clone();
        let title = services(ui).maps.borrow().get(&uid).map_or_else(String::new, |m| m.title.clone());
        let b = a.clone();
        let on_confirm: Action = Rc::new(move |ui| {
            let new_uid = b.delete_map(ui, &uid);
            b.after_delete(ui, &new_uid, false);
        });
        let text = message_with(ui, "dialog-delete-map.prompt", &[("title", MessageArg::Text(&title))]);
        button_prompt(ui, "dialog-delete-map.title", &text, on_confirm, "dialog-delete-map.confirm");
    }));

    let delete_all_button = ui.child(widget, "DELETE_ALL_MAPS_BUTTON");
    let a = this.clone();
    ui.set_visible_fn(delete_all_button, move || a.current_tab.get() == MapClassification::User);
    let a = this.clone();
    ui.button(delete_all_button).on_click = Some(Rc::new(move |ui| {
        let b = a.clone();
        let on_confirm: Action = Rc::new(move |ui| {
            let maps = b.visible_maps.borrow().clone();
            for uid in &maps {
                b.delete_map(ui, uid);
            }
            let new_uid = services(ui).maps.borrow().choose_initial_map(None, &mut cosmetic_random);
            b.after_delete(ui, &new_uid, true);
        });
        let text = message(ui, "dialog-delete-all-maps.prompt");
        button_prompt(
            ui,
            "dialog-delete-all-maps.title",
            &text,
            on_confirm,
            "dialog-delete-all-maps.confirm",
        );
    }));

    let remote_label = ui.child(widget, "REMOTE_MAP_LABEL");
    let a = this.clone();
    ui.set_visible_fn(remote_label, move || a.remote_map_pool.is_some() && a.unavailable.get() > 0);

    this.setup_map_tab(ui, MapClassification::User, "USER_MAPS_TAB_BUTTON", "USER_MAPS_TAB");
    this.setup_map_tab(ui, MapClassification::System, "SYSTEM_MAPS_TAB_BUTTON", "SYSTEM_MAPS_TAB");
    this.setup_map_tab(ui, MapClassification::Remote, "REMOTE_MAPS_TAB_BUTTON", "REMOTE_MAPS_TAB");

    let count = this.unavailable.get() as i64;
    let remote_text = message_with(ui, "label-map-unavailable-count", &[("count", MessageArg::Number(count))]);
    ui.get::<LabelWidget>(remote_label).get_text = Some(Rc::new(move || remote_text.clone()));

    let cache = services(ui).maps.clone();
    let first_of_initial = this
        .tab_maps
        .borrow()
        .get(&initial_tab)
        .and_then(|m| m.first().map(|p| p.uid.clone()));
    if this.remote_map_pool.is_some() {
        this.current_tab.set(MapClassification::Remote);
        *this.selected_uid.borrow_mut() = initial_map.unwrap_or_default();
    } else if let (None, Some(first)) = (&initial_map, first_of_initial) {
        *this.selected_uid.borrow_mut() = cache.borrow().choose_initial_map(Some(&first), &mut cosmetic_random);
        this.current_tab.set(initial_tab);
    } else {
        let selected = cache.borrow().choose_initial_map(initial_map.as_deref(), &mut cosmetic_random);
        let tab = this
            .tab_maps
            .borrow()
            .iter()
            .find(|(_, maps)| maps.iter().any(|m| m.uid == selected))
            .map_or(MapClassification::System, |(tab, _)| *tab);
        *this.selected_uid.borrow_mut() = selected;
        this.current_tab.set(tab);
    }
    let current = this.current_tab.get();
    this.enumerate_maps(ui, current);
    cell(NoLogic)
}
