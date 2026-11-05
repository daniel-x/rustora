// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The stop, scatter and deploy buttons of `CommandBarLogic.cs`. Attack move, force move, force attack, guard,
//! queue orders, the stateful button icons, order voices and `HighlightOnButtonPress` are not ported yet.
//! `CommandBarBlacklist` is read by name and taken as disabling stop.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_engine::actor::ActorRef;
use rustora_engine::order::Order;
use rustora_engine::target::Target;
use rustora_mods_common::aircraft::AircraftInfo;
use rustora_mods_common::mobile::MobileInfo;
use rustora_mods_common::orders::IIssueDeployOrder;

use super::{NoLogic, services};
use crate::loader::cell;
use crate::logic::ingame::Ingame;
use crate::widget::{ChromeLogic, Ui, WidgetId};

/// `UpdateStateIfNecessary`: the selected actors of the local player, cached by selection hash.
#[derive(Default)]
struct State {
    hash: Option<i32>,
    actors: Vec<ActorRef>,
}

fn selected(ingame: &Ingame, state: &RefCell<State>) -> Vec<ActorRef> {
    let hash = ingame.selection.hash();
    let mut s = state.borrow_mut();
    if s.hash != Some(hash) {
        s.actors = ingame
            .selection
            .actors()
            .into_iter()
            .filter(|a| ingame.local_player.as_ref().is_some_and(|p| Rc::ptr_eq(p, &a.owner())) && a.is_in_world() && !a.is_dead())
            .collect();
        s.hash = Some(hash);
    }
    s.actors.clone()
}

pub fn command_bar(ui: &mut Ui, id: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let state: Rc<RefCell<State>> = Rc::default();
    if let Some(b) = ui.find(id, "SCATTER") {
        let (i, s) = (ingame.clone(), state.clone());
        let disabled = move || {
            !selected(&i, &s)
                .iter()
                .any(|a| a.info.has_trait_info::<MobileInfo>() || a.info.has_trait_info::<AircraftInfo>())
        };
        let (i, s) = (ingame.clone(), state.clone());
        let mut button = ui.button(b);
        button.is_disabled = Some(Rc::new(disabled));
        button.on_click = Some(Rc::new(move |ui: &mut Ui| {
            let queued = ui.modifiers.shift;
            for a in selected(&i, &s) {
                i.world.issue_order(Order::new("Scatter", Some(a), Target::Invalid, queued));
            }
        }));
    }
    if let Some(b) = ui.find(id, "STOP") {
        let (i, s) = (ingame.clone(), state.clone());
        let disabled = move || selected(&i, &s).iter().all(|a| a.info.has_trait_info_named("CommandBarBlacklist"));
        let (i, s) = (ingame.clone(), state.clone());
        let mut button = ui.button(b);
        button.is_disabled = Some(Rc::new(disabled));
        button.on_click = Some(Rc::new(move |_ui: &mut Ui| {
            for a in selected(&i, &s) {
                i.world.issue_order(Order::new("Stop", Some(a), Target::Invalid, false));
            }
        }));
    }
    if let Some(b) = ui.find(id, "DEPLOY") {
        let deploys = |i: &Ingame, s: &RefCell<State>| -> Vec<(ActorRef, Rc<dyn IIssueDeployOrder>)> {
            selected(i, s)
                .into_iter()
                .flat_map(|a| {
                    a.traits_implementing::<dyn IIssueDeployOrder>()
                        .into_iter()
                        .map(move |d| (a.clone(), d))
                })
                .collect()
        };
        let (i, s) = (ingame.clone(), state.clone());
        let modifiers_shift = Rc::new(std::cell::Cell::new(false));
        let m = modifiers_shift.clone();
        let disabled = move || !deploys(&i, &s).iter().any(|(a, d)| d.can_issue_deploy_order(a, m.get()));
        let (i, s) = (ingame.clone(), state.clone());
        let mut button = ui.button(b);
        button.is_disabled = Some(Rc::new(disabled));
        button.on_click = Some(Rc::new(move |ui: &mut Ui| {
            let queued = ui.modifiers.shift;
            modifiers_shift.set(queued);
            for (a, d) in deploys(&i, &s) {
                if d.can_issue_deploy_order(&a, queued)
                    && let Some(o) = d.issue_deploy_order(&a, queued)
                {
                    i.world.issue_order(o);
                }
            }
        }));
    }
    cell(NoLogic)
}
