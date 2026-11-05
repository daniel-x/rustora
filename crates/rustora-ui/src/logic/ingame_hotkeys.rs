// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Ingame/Hotkeys/*HotkeyLogic.cs`, `StanceSelectorLogic.cs`, `IngameTransientNotificationsLogic.cs`,
//! `DisconnectWatcherLogic.cs` and `MuteIndicatorLogic.cs`. Sounds and the player stance colors are not ported, and
//! jumping to the last event waits for `RadarPings`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::miniyaml::MiniYamlNode;
use rustora_engine::actor::ActorRef;
use rustora_engine::order::Order;
use rustora_engine::player::WinState;
use rustora_engine::target::Target;
use rustora_game::settings::StatusBarsType;
use rustora_mods_common::auto_target::{AutoTarget, UnitStance};
use rustora_mods_common::base::BaseBuilding;
use rustora_mods_common::building::Building;
use rustora_mods_common::harvester::Harvester;
use rustora_mods_common::production::{Production, is_primary_building};
use rustora_mods_common::selection::{ControlGroups, SelectableInfo, selection_class};

use super::{NoLogic, message, message_with, services};
use crate::fluent::MessageArg;
use crate::input::{KeyEvent, KeyInput};
use crate::loader::cell;
use crate::logic::connection::open_disconnected;
use crate::logic::ingame::Ingame;
use crate::net::{ConnectionState, TextNotification, TextNotificationPool};
use crate::screen_map::{priority_viewer, subset_with_highest_selection_priority};
use crate::widget::{ChromeLogic, Ui, WidgetId};
use crate::widgets::ImageWidget;
use crate::widgets::control_groups::center_on_actors;
use crate::widgets::production::{LogicKeyListenerWidget, LogicTickerWidget};
use crate::widgets::text_notifications::TextNotificationsDisplayWidget;

type Hotkey = Rc<dyn Fn(&mut Ui, &Rc<Ingame>, &KeyInput) -> bool>;

/// `SingleHotkeyBaseLogic`.
fn single_hotkey(
    ui: &mut Ui,
    id: WidgetId,
    logic_args: &[MiniYamlNode],
    arg: &str,
    parent: &str,
    on_activated: Hotkey,
) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let Some(key) = logic_args.iter().find(|n| n.key == arg).and_then(|n| n.value.value.clone()) else {
        return cell(NoLogic);
    };
    let listener = ui.child(id, parent);
    ui.get::<LogicKeyListenerWidget>(listener)
        .handlers
        .push(Rc::new(move |ui: &mut Ui, e: &KeyInput| {
            let activated = e.event == KeyEvent::Down && ui.assets.borrow().hotkeys.is_activated(&key, e);
            activated && on_activated(ui, &ingame, e)
        }));
    cell(NoLogic)
}

/// `World.RenderPlayer ?? World.LocalPlayer`, as the owner test of the cycle hotkeys.
fn is_viewed(ingame: &Ingame, a: &ActorRef) -> bool {
    ingame
        .render_player
        .as_ref()
        .or(ingame.local_player.as_ref())
        .is_some_and(|p| Rc::ptr_eq(p, &a.owner()))
}

/// Selects the actor after the first selected one, wrapping around, and centers the view on it.
fn select_next(ingame: &Ingame, actors: &[ActorRef]) {
    let Some(first) = actors.first() else {
        return;
    };
    let next = actors.iter().skip_while(|a| !ingame.selection.contains(a)).nth(1).unwrap_or(first);
    ingame.selection.combine(std::slice::from_ref(next), false, true);
    center_on_actors(ingame, &ingame.selection.actors());
}

fn cycle_bases(_ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    let mut bases: Vec<ActorRef> = ingame
        .world
        .actors_having_trait::<BaseBuilding>()
        .into_iter()
        .filter(|a| is_viewed(ingame, a))
        .collect();
    bases.sort_by_key(|a| (!is_primary_building(a), a.id));
    if bases.is_empty() {
        let building = ingame
            .world
            .actors_having_trait::<Building>()
            .into_iter()
            .find(|a| is_viewed(ingame, a) && a.info.has_trait_info::<SelectableInfo>());
        if let Some(building) = building {
            ingame.selection.combine(&[building], false, true);
            center_on_actors(ingame, &ingame.selection.actors());
        }
        return true;
    }
    select_next(ingame, &bases);
    true
}

fn cycle_harvesters(_ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    let harvesters: Vec<ActorRef> = ingame
        .world
        .actors_having_trait::<Harvester>()
        .into_iter()
        .filter(|a| a.is_in_world() && is_viewed(ingame, a))
        .collect();
    select_next(ingame, &harvesters);
    true
}

fn cycle_production_actors(_ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    let first_produces = |a: &ActorRef| {
        a.traits_implementing::<Production>()
            .into_iter()
            .find(|t| !t.is_trait_disabled())
            .and_then(|t| t.info.produces.first().cloned())
    };
    let mut facilities: Vec<(String, ActorRef)> = ingame
        .world
        .actors_having_trait::<Production>()
        .into_iter()
        .filter(|a| {
            is_viewed(ingame, a)
                && a.trait_or_default::<dyn rustora_engine::traits::IOccupySpace>().is_some()
                && a.trait_or_default::<BaseBuilding>().is_none()
        })
        .filter_map(|a| first_produces(&a).map(|p| (p, a)))
        .collect();
    facilities.sort_by(|a, b| a.0.cmp(&b.0));
    let facilities: Vec<ActorRef> = facilities.into_iter().map(|(_, a)| a).collect();
    select_next(ingame, &facilities);
    true
}

fn jump_to_selected(_ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    center_on_actors(ingame, &ingame.selection.actors());
    true
}

fn reset_zoom(_ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    ingame.viewport.borrow_mut().toggle_zoom();
    true
}

fn cycle_status_bars(ui: &mut Ui, _ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    let settings = services(ui).settings.clone();
    let mut settings = settings.borrow_mut();
    settings.game.status_bars = match settings.game.status_bars {
        StatusBarsType::Standard => StatusBarsType::DamageShow,
        StatusBarsType::DamageShow => StatusBarsType::AlwaysShow,
        StatusBarsType::AlwaysShow => StatusBarsType::Standard,
    };
    true
}

fn pause(ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    let is_host = services(ui).net.borrow().is_host();
    if !is_host && ingame.local_player.as_ref().is_none_or(|p| p.win_state.get() == WinState::Lost) {
        return false;
    }
    let mut o = Order::new("PauseGame", None, Target::Invalid, false);
    o.target_string = Some(if ingame.world.paused() { "UnPause" } else { "Pause" }.to_string());
    ingame.world.issue_order(o);
    true
}

fn remove_from_control_group(_ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    let Some(groups) = ControlGroups::of(&ingame.world) else {
        return true;
    };
    let local = ingame.local_player.as_ref();
    for a in ingame.selection.actors() {
        if local.is_some_and(|p| Rc::ptr_eq(p, &a.owner())) && a.is_in_world() && !a.is_dead() {
            groups.remove_from_control_group(&a);
        }
    }
    true
}

/// `SelectionUtils.SelectActorsInWorld`.
fn select_actors_in_world(ingame: &Ingame, classes: &[String], players: &[rustora_engine::player::PlayerRef]) -> Vec<ActorRef> {
    ingame
        .world
        .actors()
        .into_iter()
        .filter(|a| {
            let owner = a.owner();
            a.is_in_world()
                && players.iter().any(|p| Rc::ptr_eq(p, &owner))
                && selection_class(a).is_some_and(|c| classes.is_empty() || classes.contains(&c))
        })
        .collect()
}

fn add_feedback_line(ui: &mut Ui, text: String) {
    services(ui).net.borrow_mut().notifications.push(TextNotification {
        pool: TextNotificationPool::Feedback,
        prefix: None,
        text,
        prefix_color: None,
        text_color: None,
        time: (0, 0),
    });
}

fn feedback_units(ui: &mut Ui, key: &str, count: usize) {
    let text = message_with(ui, key, &[("units", MessageArg::Number(count as i64))]);
    add_feedback_line(ui, text);
}

/// The players and on-screen actors that the select hotkeys start from.
fn eligible_on_screen(ui: &Ui, ingame: &Rc<Ingame>, classes: &[String]) -> (Vec<rustora_engine::player::PlayerRef>, Vec<ActorRef>) {
    let settings = services(ui).settings.clone();
    let settings = settings.borrow();
    ingame.with_order_context(&settings, |ctx| {
        let sel = ctx.selection_context();
        let players = sel.players_to_include_in_selection();
        let viewport = ingame.viewport.borrow();
        let on_screen = sel.select_actors_on_screen(viewport.top_left(), viewport.bottom_right(), classes, &players);
        (players, on_screen)
    })
}

fn select_all_units(ui: &mut Ui, ingame: &Rc<Ingame>, e: &KeyInput) -> bool {
    if ingame.world.is_game_over() {
        return false;
    }
    let (players, on_screen) = eligible_on_screen(ui, ingame, &[]);
    let viewer = priority_viewer(ingame.local_player.as_ref(), ingame.render_player.as_ref());
    let subset = |actors| subset_with_highest_selection_priority(actors, viewer.as_ref(), e.modifiers);
    let mut new_selection = subset(on_screen);
    if new_selection.len() > ingame.selection.actors().len() {
        feedback_units(ui, "selected-units-across-screen", new_selection.len());
    } else {
        new_selection = subset(select_actors_in_world(ingame, &[], &players));
        feedback_units(ui, "selected-units-across-map", new_selection.len());
    }
    ingame.selection.combine(&new_selection, false, false);
    true
}

fn select_units_by_type(ui: &mut Ui, ingame: &Rc<Ingame>, _e: &KeyInput) -> bool {
    if ingame.world.is_game_over() {
        return false;
    }
    let selected = ingame.selection.actors();
    if selected.is_empty() {
        let text = message(ui, "nothing-selected");
        add_feedback_line(ui, text);
        return false;
    }
    let (players, _) = eligible_on_screen(ui, ingame, &[]);
    let mut classes: Vec<String> = Vec::new();
    for a in selected.iter().filter(|a| {
        let owner = a.owner();
        !a.is_dead() && players.iter().any(|p| Rc::ptr_eq(p, &owner))
    }) {
        if let Some(class) = selection_class(a).filter(|c| !classes.contains(c)) {
            classes.push(class);
        }
    }
    if classes.is_empty() {
        return false;
    }
    let (_, mut new_selection) = eligible_on_screen(ui, ingame, &classes);
    if new_selection.len() > selected.len() {
        feedback_units(ui, "selected-units-across-screen", new_selection.len());
    } else {
        new_selection = select_actors_in_world(ingame, &classes, &players);
        feedback_units(ui, "selected-units-across-map", new_selection.len());
    }
    ingame.selection.combine(&new_selection, true, false);
    true
}

/// The hotkey logic of `name`, if it is one.
pub fn create(name: &str, ui: &mut Ui, id: WidgetId, logic_args: &[MiniYamlNode]) -> Option<Rc<RefCell<dyn ChromeLogic>>> {
    let (arg, parent, handler): (&str, &str, Hotkey) = match name {
        "CycleBasesHotkeyLogic" => ("CycleBasesKey", "WORLD_KEYHANDLER", Rc::new(cycle_bases)),
        "CycleHarvestersHotkeyLogic" => ("CycleHarvestersKey", "WORLD_KEYHANDLER", Rc::new(cycle_harvesters)),
        "CycleProductionActorsHotkeyLogic" => ("CycleProductionActorsKey", "WORLD_KEYHANDLER", Rc::new(cycle_production_actors)),
        "JumpToSelectedActorsHotkeyLogic" => ("JumpToSelectedActorsKey", "WORLD_KEYHANDLER", Rc::new(jump_to_selected)),
        "ResetZoomHotkeyLogic" => ("ResetZoomKey", "WORLD_KEYHANDLER", Rc::new(reset_zoom)),
        "CycleStatusBarsHotkeyLogic" => ("CycleStatusBarsKey", "WORLD_KEYHANDLER", Rc::new(cycle_status_bars)),
        "PauseHotkeyLogic" => ("PauseKey", "WORLD_KEYHANDLER", Rc::new(pause)),
        "SelectAllUnitsHotkeyLogic" => ("SelectAllUnitsKey", "WORLD_KEYHANDLER", Rc::new(select_all_units)),
        "SelectUnitsByTypeHotkeyLogic" => ("SelectUnitsByTypeKey", "WORLD_KEYHANDLER", Rc::new(select_units_by_type)),
        "RemoveFromControlGroupHotkeyLogic" => ("RemoveFromControlGroupKey", "PLAYER_KEYHANDLER", Rc::new(remove_from_control_group)),
        _ => return None,
    };
    Some(single_hotkey(ui, id, logic_args, arg, parent, handler))
}

/// `StanceSelectorLogic`.
pub fn stance_selector(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    type Stances = Rc<RefCell<(Option<i32>, Vec<(ActorRef, Rc<AutoTarget>)>)>>;
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let stances: Stances = Rc::default();
    let read = {
        let (ingame, stances) = (ingame.clone(), stances.clone());
        Rc::new(move || {
            let hash = ingame.selection.hash();
            let mut s = stances.borrow_mut();
            if s.0 != Some(hash) {
                let local = ingame.local_player.as_ref();
                s.1 = ingame
                    .selection
                    .actors()
                    .into_iter()
                    .filter(|a| local.is_some_and(|p| Rc::ptr_eq(p, &a.owner())) && a.is_in_world())
                    .flat_map(|a| {
                        a.traits_implementing::<AutoTarget>()
                            .into_iter()
                            .filter(|t| t.info.enable_stances)
                            .map(move |t| (a.clone(), t))
                    })
                    .collect();
                s.0 = Some(hash);
            }
            s.1.clone()
        })
    };
    for (name, stance, extra) in [
        ("STANCE_HOLDFIRE", UnitStance::HoldFire, 0u32),
        ("STANCE_RETURNFIRE", UnitStance::ReturnFire, 1),
        ("STANCE_DEFEND", UnitStance::Defend, 2),
        ("STANCE_ATTACKANYTHING", UnitStance::AttackAnything, 3),
    ] {
        let Some(button) = ui.find(id, name) else {
            continue;
        };
        if let Some(icon) = ui.find(button, "ICON") {
            ui.get::<ImageWidget>(icon).state_button = Some(button);
        }
        let mut b = ui.button(button);
        let r = read.clone();
        b.is_disabled = Some(Rc::new(move || r().is_empty()));
        let r = read.clone();
        b.is_highlighted = Some(Rc::new(move || {
            r().iter()
                .any(|(_, t)| !t.is_trait_disabled() && t.predicted_stance.get() == stance)
        }));
        let (r, world) = (read.clone(), ingame.world.clone());
        b.on_click = Some(Rc::new(move |_ui: &mut Ui| {
            for (a, t) in r() {
                if !t.is_trait_disabled() {
                    t.predicted_stance.set(stance);
                }
                let mut o = Order::new("SetUnitStance", Some(a), Target::Invalid, false);
                o.extra_data = extra;
                world.issue_order(o);
            }
        }));
    }
    cell(NoLogic)
}

/// `IngameTransientNotificationsLogic`.
struct TransientNotifications {
    display: WidgetId,
    seen: usize,
    last_line: Option<TextNotification>,
    repetitions: u32,
}

impl ChromeLogic for TransientNotifications {
    fn tick(&mut self, ui: &mut Ui) {
        let fresh = {
            let net = services(ui).net.clone();
            let net = net.borrow();
            let start = self.seen.min(net.notifications.len());
            self.seen = net.notifications.len();
            net.notifications[start..].to_vec()
        };
        for notification in fresh {
            if !matches!(notification.pool, TextNotificationPool::Transients | TextNotificationPool::Feedback) {
                continue;
            }
            let mut line = notification.clone();
            let duplicate = self
                .last_line
                .as_ref()
                .is_some_and(|l| (l.pool, &l.prefix, &l.text) == (notification.pool, &notification.prefix, &notification.text));
            if duplicate && !ui.node(self.display).children.is_empty() {
                self.repetitions += 1;
                line.text = format!("{} ({})", notification.text, self.repetitions + 1);
                TextNotificationsDisplayWidget::remove_most_recent(ui, self.display);
            } else {
                self.repetitions = 0;
            }
            self.last_line = Some(notification);
            TextNotificationsDisplayWidget::add_notification(ui, self.display, &line);
        }
    }
}

pub fn transient_notifications(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let seen = services(ui).net.borrow().notifications.len();
    cell(TransientNotifications {
        display: ui.child(id, "TRANSIENTS_DISPLAY"),
        seen,
        last_line: None,
        repetitions: 0,
    })
}

/// `DisconnectWatcherLogic`.
pub fn disconnect_watcher(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let net = services(ui).net.clone();
    let ticker = ui.child(id, "DISCONNECT_WATCHER");
    let disconnected = Cell::new(false);
    ui.get::<LogicTickerWidget>(ticker).on_tick = Some(Rc::new(move |ui: &mut Ui| {
        if disconnected.get() || net.borrow().connection_state != ConnectionState::NotConnected {
            return;
        }
        disconnected.set(true);
        let password = net.borrow().password.clone();
        ui.run_after_tick(move |ui| open_disconnected(ui, &password));
    }));
    cell(NoLogic)
}

/// `MuteIndicatorLogic`.
pub fn mute_indicator(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let settings = services(ui).settings.clone();
    ui.set_visible_fn(id, move || settings.borrow().sound.mute);
    cell(NoLogic)
}
