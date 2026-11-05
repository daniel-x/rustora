// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `CommandBarLogic.cs`, without the order voices.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::miniyaml::MiniYamlNode;
use rustora_engine::actor::ActorRef;
use rustora_engine::order::Order;
use rustora_engine::target::Target;
use rustora_mods_common::aircraft::AircraftInfo;
use rustora_mods_common::attack::AttackBaseInfo;
use rustora_mods_common::attack_move::AttackMoveInfo;
use rustora_mods_common::auto_target::{AutoTargetInfo, GuardInfo};
use rustora_mods_common::mobile::MobileInfo;
use rustora_mods_common::orders::{CommandBarBlacklistInfo, IIssueDeployOrder};

use super::{NoLogic, services};
use crate::command_orders::{AttackMoveOrderGenerator, CommandMode, ForceModifiersOrderGenerator, GuardOrderGenerator};
use crate::input::{KeyEvent, KeyInput, Modifiers};
use crate::loader::cell;
use crate::logic::ingame::Ingame;
use crate::orders::OrderGenerator;
use crate::widget::{Action, ChromeLogic, Ui, WidgetId};
use crate::widgets::ImageWidget;
use crate::widgets::production::LogicKeyListenerWidget;

/// `UpdateStateIfNecessary`: the state derived from the selected actors of the local player, cached by selection hash.
#[derive(Default)]
struct Selected {
    hash: Option<i32>,
    actors: Vec<ActorRef>,
    deploys: Vec<(ActorRef, Rc<dyn IIssueDeployOrder>)>,
    attack_move_disabled: bool,
    force_move_disabled: bool,
    force_attack_disabled: bool,
    guard_disabled: bool,
    scatter_disabled: bool,
    stop_disabled: bool,
    waypoint_mode_disabled: bool,
}

/// The fields of `CommandBarLogic` that its buttons share.
struct Bar {
    ingame: Rc<Ingame>,
    selected: RefCell<Selected>,
    /// `Game.GetModifierKeys` as of the last tick, because getters have no access to the `Ui`.
    modifiers: Cell<Modifiers>,
}

impl Bar {
    fn update(&self) {
        let hash = self.ingame.selection.hash();
        let mut s = self.selected.borrow_mut();
        if s.hash == Some(hash) {
            return;
        }
        let local = self.ingame.local_player.as_ref();
        let actors: Vec<ActorRef> = self
            .ingame
            .selection
            .actors()
            .into_iter()
            .filter(|a| local.is_some_and(|p| Rc::ptr_eq(p, &a.owner())) && a.is_in_world() && !a.is_dead())
            .collect();
        let auto_target = |a: &ActorRef| a.info.has_trait_info::<AutoTargetInfo>();
        let moves = |a: &ActorRef| a.info.has_trait_info::<MobileInfo>() || a.info.has_trait_info::<AircraftInfo>();
        let blacklist = |a: &ActorRef| a.info.trait_info::<CommandBarBlacklistInfo>();
        s.attack_move_disabled = !actors.iter().any(|a| a.info.has_trait_info::<AttackMoveInfo>() && auto_target(a));
        s.guard_disabled = !actors.iter().any(|a| a.info.has_trait_info::<GuardInfo>() && auto_target(a));
        s.force_move_disabled = !actors.iter().any(moves);
        s.force_attack_disabled = !actors.iter().any(|a| a.info.has_trait_info::<AttackBaseInfo>());
        s.scatter_disabled = !actors.iter().any(moves);
        s.stop_disabled = !actors.iter().any(|a| blacklist(a).is_none_or(|i| !i.disable_stop));
        s.waypoint_mode_disabled = !actors.iter().any(|a| blacklist(a).is_none_or(|i| !i.disable_waypoint_mode));
        s.deploys = actors
            .iter()
            .flat_map(|a| {
                a.traits_implementing::<dyn IIssueDeployOrder>()
                    .into_iter()
                    .map(move |d| (a.clone(), d))
            })
            .collect();
        s.actors = actors;
        s.hash = Some(hash);
    }

    fn read<R>(&self, f: impl FnOnce(&Selected) -> R) -> R {
        self.update();
        f(&self.selected.borrow())
    }

    fn command_mode(&self) -> Option<CommandMode> {
        self.ingame.order_generator().command_mode()
    }

    /// `IsForceModifiersActive`.
    fn is_force_modifiers_active(&self, modifiers: Modifiers) -> bool {
        let generator = self.ingame.order_generator();
        if let Some(CommandMode::ForceModifiers(m)) = generator.command_mode()
            && m.contains(modifiers)
        {
            return true;
        }
        generator.is_unit_order_generator() && self.modifiers.get().contains(modifiers)
    }

    /// `PerformKeyboardOrderOnSelection`.
    fn order_selection(&self, f: impl Fn(ActorRef) -> Order) {
        for a in self.read(|s| s.actors.clone()) {
            self.ingame.world.issue_order(f(a));
        }
    }

    /// `PerformDeployOrderOnSelection`.
    fn deploy_selection(&self, queued: bool) {
        for (a, d) in self.read(|s| s.deploys.clone()) {
            if d.can_issue_deploy_order(&a, queued)
                && let Some(o) = d.issue_deploy_order(&a, queued)
            {
                self.ingame.world.issue_order(o);
            }
        }
    }
}

struct CommandBarLogic {
    bar: Rc<Bar>,
    highlights: Vec<Rc<Cell<i32>>>,
}

impl ChromeLogic for CommandBarLogic {
    fn tick(&mut self, ui: &mut Ui) {
        self.bar.modifiers.set(ui.modifiers);
        for h in &self.highlights {
            h.set((h.get() - 1).max(0));
        }
    }
}

/// `WidgetUtils.BindButtonIcon`.
fn bind_button_icon(ui: &mut Ui, button: WidgetId) {
    if let Some(icon) = ui.find(button, "ICON") {
        ui.get::<ImageWidget>(icon).state_button = Some(button);
    }
}

/// Binds a button that toggles an input mode: a click cancels the mode, a key press leaves it on.
fn bind_toggle(
    ui: &mut Ui,
    id: WidgetId,
    name: &str,
    bar: &Rc<Bar>,
    disabled: impl Fn(&Selected) -> bool + 'static,
    highlighted: impl Fn(&Bar) -> bool + 'static,
    generator: impl Fn(&Bar) -> Rc<dyn OrderGenerator> + 'static,
    key_cancels: bool,
) -> Option<WidgetId> {
    let button = ui.find(id, name)?;
    bind_button_icon(ui, button);
    let highlighted = Rc::new(highlighted);
    let generator = Rc::new(generator);
    let toggle = |allow_cancel: bool| -> Action {
        let (bar, highlighted, generator) = (bar.clone(), highlighted.clone(), generator.clone());
        Rc::new(move |_ui: &mut Ui| {
            if highlighted(&bar) {
                if allow_cancel {
                    bar.ingame.cancel_input_mode();
                }
            } else {
                bar.ingame.set_order_generator(generator(&bar));
            }
        })
    };
    let (on_click, on_key_press) = (toggle(true), toggle(key_cancels));
    let mut b = ui.button(button);
    let d = bar.clone();
    b.is_disabled = Some(Rc::new(move || d.read(&disabled)));
    let (h, f) = (bar.clone(), highlighted.clone());
    b.is_highlighted = Some(Rc::new(move || f(&h)));
    b.on_click = Some(on_click);
    b.on_key_press = Some(on_key_press);
    drop(b);
    Some(button)
}

/// Binds a button that gives its order right away and shows a highlight for two ticks when pressed.
fn bind_instant(
    ui: &mut Ui,
    id: WidgetId,
    name: &str,
    highlights: &mut Vec<Rc<Cell<i32>>>,
    highlight_on_button_press: bool,
    disabled: impl Fn() -> bool + 'static,
    on_click: impl Fn(&Ui) + 'static,
) -> Option<WidgetId> {
    let button = ui.find(id, name)?;
    bind_button_icon(ui, button);
    let highlight = Rc::new(Cell::new(0));
    highlights.push(highlight.clone());
    let on_click = Rc::new(on_click);
    let mut b = ui.button(button);
    b.is_disabled = Some(Rc::new(disabled));
    let h = highlight.clone();
    b.is_highlighted = Some(Rc::new(move || h.get() > 0));
    let (h, f) = (highlight.clone(), on_click.clone());
    b.on_click = Some(Rc::new(move |ui: &mut Ui| {
        if highlight_on_button_press {
            h.set(2);
        }
        f(ui);
    }));
    b.on_key_press = Some(Rc::new(move |ui: &mut Ui| {
        highlight.set(2);
        on_click(ui);
    }));
    drop(b);
    Some(button)
}

fn force_modifiers(bar: &Bar, modifiers: Modifiers, cancel_on_first_use: bool) -> Rc<dyn OrderGenerator> {
    Rc::new(ForceModifiersOrderGenerator {
        base: bar.ingame.unit_order_generator(),
        modifiers,
        cancel_on_first_use,
    })
}

/// `button.OnKeyPress(e)`, deferred as in `ButtonWidget.handle_key`.
fn press_key(ui: &mut Ui, button: WidgetId) {
    let action = ui.button(button).on_key_press.clone();
    if let Some(action) = action {
        ui.defer(move |ui| action(ui));
    }
}

/// The handler of `MODIFIER_OVERRIDES`, which lets buttons trigger while the queue or assault modifiers are held.
fn modifier_overrides(
    ui: &mut Ui,
    e: &KeyInput,
    no_shift_buttons: &[WidgetId],
    key_up_buttons: &[WidgetId],
    attack_move: Option<WidgetId>,
) -> bool {
    let is_activated = |ui: &Ui, b: WidgetId, e: &KeyInput| {
        let key = ui.button(b).key.clone();
        key.is_some_and(|key| ui.assets.borrow().hotkeys.is_activated(&key, e))
    };
    if e.modifiers.shift {
        let no_shift = KeyInput {
            modifiers: Modifiers {
                shift: false,
                ..e.modifiers
            },
            ..e.clone()
        };
        for &b in no_shift_buttons {
            if ui.button(b).disabled_now() || !is_activated(ui, b, &no_shift) {
                continue;
            }
            let disable_key_repeat = ui.button(b).disable_key_repeat;
            if disable_key_repeat == e.repeat || (e.event == KeyEvent::Up && !key_up_buttons.contains(&b)) {
                continue;
            }
            press_key(ui, b);
            return true;
        }
    }
    let no_mods = KeyInput {
        modifiers: Modifiers {
            ctrl: false,
            shift: false,
            ..e.modifiers
        },
        ..e.clone()
    };
    if let Some(b) = attack_move
        && !ui.button(b).disabled_now()
        && is_activated(ui, b, &no_mods)
    {
        press_key(ui, b);
        return true;
    }
    false
}

pub fn command_bar(ui: &mut Ui, id: WidgetId, logic_args: &[MiniYamlNode]) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let highlight_on_button_press = logic_args
        .iter()
        .find(|n| n.key == "HighlightOnButtonPress")
        .and_then(|n| n.value.value.as_deref())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let bar = Rc::new(Bar {
        ingame,
        selected: RefCell::default(),
        modifiers: Cell::default(),
    });
    let mut highlights = Vec::new();

    let attack_move = bind_toggle(
        ui,
        id,
        "ATTACK_MOVE",
        &bar,
        |s| s.attack_move_disabled,
        |b| b.command_mode() == Some(CommandMode::AttackMove),
        |b| Rc::new(AttackMoveOrderGenerator::new(b.read(|s| s.actors.clone()))),
        false,
    );
    let force_move_disabled = |s: &Selected| s.force_move_disabled;
    bind_toggle(
        ui,
        id,
        "FORCE_MOVE",
        &bar,
        force_move_disabled,
        move |b| !b.read(force_move_disabled) && b.is_force_modifiers_active(Modifiers::ALT),
        |b| force_modifiers(b, Modifiers::ALT, true),
        true,
    );
    let force_attack_disabled = |s: &Selected| s.force_attack_disabled;
    bind_toggle(
        ui,
        id,
        "FORCE_ATTACK",
        &bar,
        force_attack_disabled,
        move |b| {
            !b.read(force_attack_disabled)
                && b.is_force_modifiers_active(Modifiers::CTRL)
                && b.command_mode() != Some(CommandMode::AttackMove)
        },
        |b| force_modifiers(b, Modifiers::CTRL, true),
        true,
    );
    let guard = bind_toggle(
        ui,
        id,
        "GUARD",
        &bar,
        |s| s.guard_disabled,
        |b| b.command_mode() == Some(CommandMode::Guard),
        |b| Rc::new(GuardOrderGenerator::new(b.read(|s| s.actors.clone()))),
        false,
    );
    let waypoint_disabled = |s: &Selected| s.waypoint_mode_disabled;
    bind_toggle(
        ui,
        id,
        "QUEUE_ORDERS",
        &bar,
        waypoint_disabled,
        move |b| !b.read(waypoint_disabled) && b.is_force_modifiers_active(Modifiers::SHIFT),
        |b| force_modifiers(b, Modifiers::SHIFT, false),
        true,
    );

    let (b1, b2) = (bar.clone(), bar.clone());
    let scatter = bind_instant(
        ui,
        id,
        "SCATTER",
        &mut highlights,
        highlight_on_button_press,
        move || b1.read(|s| s.scatter_disabled),
        move |ui| {
            let queued = ui.modifiers.shift;
            b2.order_selection(|a| Order::new("Scatter", Some(a), Target::Invalid, queued));
        },
    );
    let (b1, b2) = (bar.clone(), bar.clone());
    let deploy = bind_instant(
        ui,
        id,
        "DEPLOY",
        &mut highlights,
        highlight_on_button_press,
        move || {
            let queued = b1.modifiers.get().shift;
            !b1.read(|s| s.deploys.clone())
                .iter()
                .any(|(a, d)| d.can_issue_deploy_order(a, queued))
        },
        move |ui| b2.deploy_selection(ui.modifiers.shift),
    );
    let (b1, b2) = (bar.clone(), bar.clone());
    bind_instant(
        ui,
        id,
        "STOP",
        &mut highlights,
        highlight_on_button_press,
        move || b1.read(|s| s.stop_disabled),
        move |_ui| b2.order_selection(|a| Order::new("Stop", Some(a), Target::Invalid, false)),
    );

    if let Some(listener) = ui.find(id, "MODIFIER_OVERRIDES") {
        let no_shift_buttons: Vec<WidgetId> = [guard, deploy, scatter, attack_move].into_iter().flatten().collect();
        let key_up_buttons: Vec<WidgetId> = [guard, attack_move].into_iter().flatten().collect();
        ui.get::<LogicKeyListenerWidget>(listener)
            .handlers
            .push(Rc::new(move |ui: &mut Ui, e: &KeyInput| {
                modifier_overrides(ui, e, &no_shift_buttons, &key_up_buttons, attack_move)
            }));
    }
    cell(CommandBarLogic { bar, highlights })
}
