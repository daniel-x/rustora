// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `ReplayControlBarLogic.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{NoLogic, services};
use crate::loader::cell;
use crate::widget::{ChromeLogic, Ui, WidgetId};

/// `PlaybackSpeed` with its timestep multiplier.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaybackSpeed {
    Regular,
    Slow,
    Fast,
    Maximum,
}

impl PlaybackSpeed {
    fn multiplier(self) -> f32 {
        match self {
            PlaybackSpeed::Regular => 1.0,
            PlaybackSpeed::Slow => 2.0,
            PlaybackSpeed::Fast => 0.5,
            PlaybackSpeed::Maximum => 0.001,
        }
    }
}

/// `ReplayControlBarLogic`: pause, play and the playback speed of a replay.
pub fn replay_control_bar(ui: &mut Ui, widget: WidgetId) -> Rc<RefCell<dyn ChromeLogic>> {
    let Some(ingame) = services(ui).ingame.borrow().clone() else {
        return cell(NoLogic);
    };
    let Some(replay_net_ticks) = ingame.replay.as_ref().map(|r| r.tick_count) else {
        return cell(NoLogic);
    };
    let container = ui.child(widget, "REPLAY_PLAYER");
    let container_height = ui.node(container).bounds.h;
    if let Some(parent) = ui.node(widget).parent
        && let Some(background) = ui.find(parent, "OBSERVER_CONTROL_BG")
    {
        ui.node_mut(background).bounds.h += container_height;
    }
    ui.set_visible_fn(container, || true);
    let speed = Rc::new(Cell::new(PlaybackSpeed::Regular));
    let world = ingame.world.clone();
    let original_timestep = world.timestep;
    let timestep_for = move |s: PlaybackSpeed| (original_timestep as f32 * s.multiplier()).ceil() as i32;
    let disabled = {
        let (world, ingame) = (world.clone(), ingame.clone());
        Rc::new(move || ingame.replay.as_ref().is_some_and(|r| r.out_of_sync.get()) || world.net_frame_number() >= replay_net_ticks)
    };

    let pause = ui.child(widget, "BUTTON_PAUSE");
    {
        let (w, d) = (world.clone(), disabled.clone());
        ui.set_visible_fn(pause, move || w.replay_timestep.get() != 0 && !d());
        let w = world.clone();
        ui.button(pause).on_click = Some(Rc::new(move |_| w.replay_timestep.set(0)));
    }
    let play = ui.child(widget, "BUTTON_PLAY");
    {
        let (w, d) = (world.clone(), disabled.clone());
        ui.set_visible_fn(play, move || w.replay_timestep.get() == 0 || d());
        let (w, s) = (world.clone(), speed.clone());
        let mut b = ui.button(play);
        b.on_click = Some(Rc::new(move |_| w.replay_timestep.set(timestep_for(s.get()))));
        let d = disabled.clone();
        b.is_disabled = Some(Rc::new(move || d()));
    }
    for (name, choice) in [
        ("BUTTON_SLOW", PlaybackSpeed::Slow),
        ("BUTTON_REGULAR", PlaybackSpeed::Regular),
        ("BUTTON_FAST", PlaybackSpeed::Fast),
        ("BUTTON_MAXIMUM", PlaybackSpeed::Maximum),
    ] {
        let button = ui.child(widget, name);
        let mut b = ui.button(button);
        let s = speed.clone();
        b.is_highlighted = Some(Rc::new(move || s.get() == choice));
        let d = disabled.clone();
        b.is_disabled = Some(Rc::new(move || d()));
        let (w, s) = (world.clone(), speed.clone());
        b.on_click = Some(Rc::new(move |_| {
            s.set(choice);
            if w.replay_timestep.get() != 0 {
                w.replay_timestep.set(timestep_for(choice));
            }
        }));
    }
    cell(NoLogic)
}
