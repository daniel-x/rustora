// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Mods.Common/Scripting/Media.cs`: the full screen and radar videos of the scripts.

use std::rc::Rc;

use rustora_engine::world::World;

use super::ingame_menu::set_pause_state;
use super::services;
use crate::widget::{Ui, WidgetArgs, WidgetId};
use crate::widgets::video_player::{VideoAction, VideoPlayerWidget, run_after_tick};

/// Runs `f` on the video player widget `id`.
pub(crate) fn with_player<R>(ui: &mut Ui, id: WidgetId, f: impl FnOnce(&mut VideoPlayerWidget, &mut Ui) -> R) -> R {
    let kind = ui.kind(id);
    let mut kind = kind.borrow_mut();
    let player = kind
        .as_any()
        .downcast_mut::<VideoPlayerWidget>()
        .expect("a video player widget");
    f(player, ui)
}

/// `Media.PlayFMVFullscreen`.
pub fn play_fmv_fullscreen(ui: &mut Ui, world: &Rc<World>, video: &str, on_complete: VideoAction) {
    let root = ui.open_window("FMVPLAYER", WidgetArgs::default().with_value("world", world.clone()));
    let Some(player) = ui.find(root, "PLAYER") else {
        ui.close_window();
        on_complete(ui);
        return;
    };
    if !with_player(ui, player, |p, ui| p.load_and_play(ui, player, video)) {
        ui.close_window();
        on_complete(ui);
        return;
    }
    if !world.is_game_over() {
        set_pause_state(world, true);
    }
    let sound = services(ui).sound.clone();
    let music = sound.music_playing();
    if music {
        sound.pause_music();
    }
    let world = world.clone();
    with_player(ui, player, move |p, ui| {
        p.play_then(
            ui,
            Box::new(move |ui: &mut Ui| {
                if music {
                    services(ui).sound.resume_music();
                }
                ui.close_window();
                if !world.is_game_over() {
                    set_pause_state(&world, false);
                }
                on_complete(ui);
            }),
        );
    });
}

/// `Media.PlayFMVInRadar`.
pub fn play_fmv_in_radar(ui: &mut Ui, video: &str, on_complete: VideoAction) {
    match ui.find(ui.root, "PLAYER") {
        Some(player) => with_player(ui, player, |p, ui| p.load_and_play_async(ui, player, video, on_complete)),
        None => run_after_tick(ui, on_complete),
    }
}
