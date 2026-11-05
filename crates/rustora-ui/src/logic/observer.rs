// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ObserverShroudSelectorLogic.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYamlNode;
use rustora_engine::player::PlayerRef;
use rustora_game::map_cache::MapVisibility;

use super::game_info::player_name_and_status;
use super::ingame::Ingame;
use super::sidebar::resolved_player_name;
use super::{NoLogic, message, message_with, services};
use crate::fluent::MessageArg;
use crate::input::{KeyEvent, KeyInput, Modifiers};
use crate::loader::cell;
use crate::widget::{ChromeLogic, Getter, Ui, WidgetId};
use crate::widgets::production::LogicKeyListenerWidget;
use crate::widgets::{DropDownButtonWidget, ImageWidget, ScrollItemWidget};

/// `CameraOption`. `player` is `None` for the disabled shroud view.
struct CameraOption {
    player: Option<PlayerRef>,
    label: String,
    color: Color,
    faction: Option<String>,
}

/// The state shared by the selector, its drop down and its key handler.
struct Selector {
    ingame: Rc<Ingame>,
    options: Vec<CameraOption>,
    /// The option indexes of each team, in team order.
    teams: Vec<(i32, Vec<usize>)>,
    limit_views: bool,
    selected: Cell<usize>,
    /// The `BindPlayerNameAndStatus` text of the selector label for each player option.
    label_texts: RefCell<Vec<Option<Getter<String>>>>,
    combined_view_key: String,
    world_view_key: String,
}

const COMBINED: usize = 0;
const DISABLE_SHROUD: usize = 1;

fn same_player(a: Option<&PlayerRef>, b: Option<&PlayerRef>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

impl Selector {
    fn is_selected(&self, index: usize) -> bool {
        same_player(self.ingame.render_player().as_ref(), self.options[index].player.as_ref())
    }

    /// `CameraOption.OnClick`.
    fn click(&self, index: usize) {
        let option = &self.options[index];
        self.ingame.set_render_player(option.player.clone());
        self.selected.set(index);
        if option.faction.is_some() {
            self.ingame.selection.clear();
        }
    }

    /// `HandleKeyPress`.
    fn handle_key(&self, ui: &Ui, e: &KeyInput) -> bool {
        if e.event != KeyEvent::Down || e.repeat {
            return false;
        }
        let hotkeys = &ui.assets.borrow().hotkeys;
        if hotkeys.is_activated(&self.combined_view_key, e) && !self.limit_views {
            self.click(COMBINED);
            return true;
        }
        if hotkeys.is_activated(&self.world_view_key, e) && !self.limit_views {
            self.click(DISABLE_SHROUD);
            return true;
        }
        let Some(key) = e.key.strip_prefix("NUMBER_").and_then(|k| k.parse::<i32>().ok()) else {
            return false;
        };
        let Some((_, team)) = self.teams.iter().find(|(t, _)| *t == key) else {
            return false;
        };
        let mut team = team.clone();
        if team.is_empty() {
            return false;
        }
        if e.modifiers == Modifiers::SHIFT {
            team.reverse();
        }
        let selected_player = self.options[self.selected.get()].player.clone();
        let next = team
            .iter()
            .skip_while(|&&i| !same_player(self.options[i].player.as_ref(), selected_player.as_ref()))
            .nth(1)
            .or(team.first())
            .copied();
        if let Some(next) = next {
            self.click(next);
        }
        true
    }
}

impl ChromeLogic for Rc<Selector> {
    fn tick(&mut self, _ui: &mut Ui) {
        let render_player = self.ingame.render_player();
        if same_player(render_player.as_ref(), self.options[self.selected.get()].player.as_ref()) {
            return;
        }
        if let Some(index) = (0..self.options.len()).find(|&i| self.is_selected(i)) {
            self.click(index);
        }
    }
}

/// Binds the `FLAG`, `LABEL` and `NOFLAG_LABEL` children of a selector or drop down item to an option.
fn bind_option_widgets(
    ui: &mut Ui,
    parent: WidgetId,
    flag_visible: Getter<bool>,
    faction: Getter<String>,
    text: Getter<String>,
    color: Getter<Color>,
) {
    let label = ui.child(parent, "LABEL");
    let v = flag_visible.clone();
    ui.set_visible_fn(label, move || v());
    {
        let mut l = ui.label(label);
        l.get_text = Some(text.clone());
        l.get_color = Some(color.clone());
    }
    let flag = ui.child(parent, "FLAG");
    let v = flag_visible.clone();
    ui.set_visible_fn(flag, move || v());
    {
        let mut f = ui.get::<ImageWidget>(flag);
        f.get_image_collection = Some(Rc::new(|| "flags".to_string()));
        f.get_image_name = Some(faction);
    }
    let alt = ui.child(parent, "NOFLAG_LABEL");
    ui.set_visible_fn(alt, move || !flag_visible());
    let mut l = ui.label(alt);
    l.get_text = Some(text);
    l.get_color = Some(color);
}

/// `ObserverShroudSelectorLogic`.
pub fn shroud_selector(ui: &mut Ui, widget: WidgetId, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let world = ingame.world.clone();
    let net = services(ui).net.clone();
    let arg = |key: &str| {
        logic_args
            .iter()
            .find(|n| n.key == key)
            .and_then(|n| n.value.value.clone())
            .unwrap_or_default()
    };
    let limit_views = services(ui)
        .maps
        .borrow()
        .get(&world.map().uid)
        .is_some_and(|m| m.visibility.has(MapVisibility::MISSION_SELECTOR));
    let white = Color::from_argb(255, 255, 255, 255);
    let everyone = world.players().into_iter().find(|p| p.internal_name == "Everyone");
    let mut options = vec![
        CameraOption {
            player: everyone,
            label: message(ui, "options-shroud-selector.all-players"),
            color: white,
            faction: None,
        },
        CameraOption {
            player: None,
            label: message(ui, "options-shroud-selector.disable-shroud"),
            color: white,
            faction: None,
        },
    ];
    let mut teams: Vec<(i32, Vec<usize>)> = Vec::new();
    for p in world.players().into_iter().filter(|p| !p.non_combatant && p.playable) {
        let team = world.lobby_info.client_with_index(p.client_index).map_or(0, |c| c.team);
        options.push(CameraOption {
            label: resolved_player_name(ui, &world, &p),
            color: p.color,
            faction: Some(p.faction.clone()),
            player: Some(p),
        });
        let index = options.len() - 1;
        match teams.iter_mut().find(|(t, _)| *t == team) {
            Some((_, list)) => list.push(index),
            None => teams.push((team, vec![index])),
        }
    }
    teams.sort_by_key(|(t, _)| *t);

    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    if !limit_views {
        groups.push((message(ui, "options-shroud-selector.other"), vec![COMBINED, DISABLE_SHROUD]));
    }
    let no_teams = teams.len() == 1;
    let mut total_players = 0;
    for (team, list) in &teams {
        total_players += list.len();
        let label = if no_teams {
            message(ui, "label-players")
        } else if *team > 0 {
            message_with(ui, "label-team-name", &[("team", MessageArg::Number(*team as i64))])
        } else {
            message(ui, "label-no-team")
        };
        groups.push((label, list.clone()));
    }

    let shroud_label = ui.child(widget, "LABEL");
    let label_texts = options
        .iter()
        .map(|o| {
            o.player
                .as_ref()
                .filter(|_| o.faction.is_some())
                .map(|p| player_name_and_status(ui, shroud_label, &world, p, &o.label, &net))
        })
        .collect();
    let selector = Rc::new(Selector {
        ingame: ingame.clone(),
        options,
        teams,
        limit_views,
        selected: Cell::new(0),
        label_texts: RefCell::new(label_texts),
        combined_view_key: arg("CombinedViewKey"),
        world_view_key: arg("WorldViewKey"),
    });

    let disabled = limit_views && total_players < 2;
    ui.button(widget).is_disabled = Some(Rc::new(move || disabled));
    {
        let s = selector.clone();
        ui.button(widget).on_mouse_down = Some(Rc::new(move |ui: &mut Ui, _| {
            let s2 = s.clone();
            let net = services(ui).net.clone();
            let world = s.ingame.world.clone();
            let setup = move |ui: &mut Ui, index: usize, template: WidgetId| {
                let (s3, s4) = (s2.clone(), s2.clone());
                let item = ScrollItemWidget::setup(
                    ui,
                    template,
                    Rc::new(move || s3.is_selected(index)),
                    Rc::new(move |_| s4.click(index)),
                );
                let option = &s2.options[index];
                let show_flag = option.faction.is_some();
                let label = ui.child(item, "LABEL");
                let text: Getter<String> = match option.player.as_ref().filter(|_| show_flag) {
                    Some(p) => player_name_and_status(ui, label, &world, p, &option.label, &net),
                    None => {
                        let l = option.label.clone();
                        Rc::new(move || l.clone())
                    }
                };
                let (faction, color) = (option.faction.clone().unwrap_or_default(), option.color);
                bind_option_widgets(
                    ui,
                    item,
                    Rc::new(move || show_flag),
                    Rc::new(move || faction.clone()),
                    text,
                    Rc::new(move || color),
                );
                item
            };
            DropDownButtonWidget::show_grouped_dropdown(ui, widget, "SPECTATOR_DROPDOWN_TEMPLATE", 400, groups.clone(), setup);
        }));
    }

    let s = selector.clone();
    let flag_visible: Getter<bool> = Rc::new(move || s.options[s.selected.get()].faction.is_some());
    let s = selector.clone();
    let faction: Getter<String> = Rc::new(move || s.options[s.selected.get()].faction.clone().unwrap_or_default());
    let s = selector.clone();
    let text: Getter<String> = Rc::new(move || {
        let index = s.selected.get();
        match &s.label_texts.borrow()[index] {
            Some(text) => text(),
            None => s.options[index].label.clone(),
        }
    });
    let s = selector.clone();
    let color: Getter<Color> = Rc::new(move || s.options[s.selected.get()].color);
    bind_option_widgets(ui, widget, flag_visible, faction, text, color);

    let key_handler = ui.child(widget, "SHROUD_KEYHANDLER");
    let s = selector.clone();
    ui.get::<LogicKeyListenerWidget>(key_handler)
        .handlers
        .push(Rc::new(move |ui: &mut Ui, e: &KeyInput| s.handle_key(ui, e)));

    let initial = if limit_views {
        selector.teams.first().and_then(|(_, t)| t.first()).copied().unwrap_or(COMBINED)
    } else if world.world_actor().owner().shroud().explore_map_enabled() {
        COMBINED
    } else {
        DISABLE_SHROUD
    };
    selector.click(initial);
    ingame.viewport.borrow_mut().unlock_minimum_zoom(0.5);
    cell(selector)
}
