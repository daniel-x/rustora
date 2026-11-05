// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MusicPlayerLogic.cs`, used by `MUSIC_PANEL` and `LOBBY_MUSIC_BIN`.

use std::cell::RefCell;
use std::rc::Rc;

use rustora_engine::sound::{MusicInfo, Sound};

use super::{NoLogic, message, services};
use crate::loader::cell;
use crate::music::MusicPlaylist;
use crate::widget::{ChromeLogic, Ui, WidgetArgs, WidgetId};
use crate::widgets::production::LogicTickerWidget;
use crate::widgets::{CheckboxWidget, LabelWithTooltipWidget, ScrollItemWidget, SliderWidget};

type CurrentSong = Rc<RefCell<Option<Rc<MusicInfo>>>>;

fn same(a: &Option<Rc<MusicInfo>>, b: &Rc<MusicInfo>) -> bool {
    a.as_ref().is_some_and(|a| Rc::ptr_eq(a, b))
}

/// `Play`.
fn play(ui: &mut Ui, list: WidgetId, playlist: &MusicPlaylist, current: &CurrentSong) {
    let Some(song) = current.borrow().clone() else {
        return;
    };
    ui.scroll_to_item(list, &song.filename, false);
    playlist.play(&song);
}

/// `BuildMusicTable`.
fn build_music_table(ui: &mut Ui, list: WidgetId, template: WidgetId, playlist: &Rc<MusicPlaylist>, current: &CurrentSong) {
    if !playlist.is_music_available() {
        return;
    }
    *current.borrow_mut() = playlist.current_song();
    ui.remove_children(list);
    for song in playlist.available_playlist().to_vec() {
        let (c, s) = (current.clone(), song.clone());
        let is_selected = Rc::new(move || same(&c.borrow(), &s));
        let (c, s, p) = (current.clone(), song.clone(), playlist.clone());
        let on_click = Rc::new(move |ui: &mut Ui| {
            *c.borrow_mut() = Some(s.clone());
            play(ui, list, &p, &c);
        });
        let item = ScrollItemWidget::setup_with_key(ui, &song.filename, template, is_selected, on_click, Rc::new(|_| {}));
        let title = ui.child(item, "TITLE");
        LabelWithTooltipWidget::truncate_to_tooltip(ui, title, &song.title);
        let length = ui.child(item, "LENGTH");
        let text = format!("{}:{:02}", song.length.get() / 60, song.length.get() % 60);
        ui.label(length).get_text = Some(Rc::new(move || text.clone()));
        ui.add_child(list, item);
    }
    let song = current.borrow().clone();
    if let Some(song) = song
        && !playlist.current_song_is_background()
    {
        ui.scroll_to_item(list, &song.filename, false);
    }
}

/// `MusicPlayerLogic`.
pub fn create(ui: &mut Ui, panel: WidgetId, args: &WidgetArgs) -> Rc<RefCell<dyn ChromeLogic>> {
    let service = services(ui);
    let Some(playlist) = service.music.borrow().clone() else {
        return cell(NoLogic);
    };
    let sound: Rc<Sound> = service.sound.clone();
    let settings = service.settings.clone();
    let list = ui.child(panel, "MUSIC_LIST");
    let template = ui.child(list, "MUSIC_TEMPLATE");
    ui.hide_child(list, template);
    let current: CurrentSong = Rc::default();
    build_music_table(ui, list, template, &playlist, &current);

    let no_music = {
        let (p, c) = (playlist.clone(), current.clone());
        Rc::new(move || !p.is_music_available() || p.current_song_is_background() || c.borrow().is_none())
    };
    let no_music_label = ui.child(panel, "NO_MUSIC_LABEL");
    let p = playlist.clone();
    ui.set_visible_fn(no_music_label, move || !p.is_music_available());
    if playlist.is_music_available() {
        let mute_label = ui.child(panel, "MUTE_LABEL");
        let (muted, s) = (message(ui, "label-sound-muted"), settings.clone());
        ui.label(mute_label).get_text = Some(Rc::new(move || if s.borrow().sound.mute { muted.clone() } else { String::new() }));
    }

    let play_button = ui.child(panel, "BUTTON_PLAY");
    {
        let (p, c) = (playlist.clone(), current.clone());
        let mut b = ui.button(play_button);
        b.on_click = Some(Rc::new(move |ui: &mut Ui| play(ui, list, &p, &c)));
        b.is_disabled = Some(no_music.clone());
    }
    let s = sound.clone();
    ui.set_visible_fn(play_button, move || !s.music_playing());

    let pause_button = ui.child(panel, "BUTTON_PAUSE");
    {
        let s = sound.clone();
        let mut b = ui.button(pause_button);
        b.on_click = Some(Rc::new(move |_| s.pause_music()));
        b.is_disabled = Some(no_music.clone());
    }
    let s = sound.clone();
    ui.set_visible_fn(pause_button, move || s.music_playing());

    let stop_button = ui.child(panel, "BUTTON_STOP");
    {
        let p = playlist.clone();
        let mut b = ui.button(stop_button);
        b.on_click = Some(Rc::new(move |_| p.stop()));
        b.is_disabled = Some(no_music.clone());
    }
    for (id, reverse) in [("BUTTON_NEXT", false), ("BUTTON_PREV", true)] {
        let button = ui.child(panel, id);
        let (p, c) = (playlist.clone(), current.clone());
        let mut b = ui.button(button);
        b.on_click = Some(Rc::new(move |ui: &mut Ui| {
            *c.borrow_mut() = if reverse { p.prev_song() } else { p.next_song() };
            play(ui, list, &p, &c);
        }));
        b.is_disabled = Some(no_music.clone());
    }

    let shuffle = ui.child(panel, "SHUFFLE");
    {
        let (s, s2, p) = (settings.clone(), settings.clone(), playlist.clone());
        let mut c = ui.get::<CheckboxWidget>(shuffle);
        c.is_checked = Some(Rc::new(move || s.borrow().sound.shuffle));
        c.button.on_click = Some(Rc::new(move |_| {
            let mut settings = s2.borrow_mut();
            settings.sound.shuffle = !settings.sound.shuffle;
        }));
        c.button.is_disabled = Some(Rc::new(move || p.current_song_is_background()));
    }
    let repeat = ui.child(panel, "REPEAT");
    {
        let (s, s2, p, snd) = (settings.clone(), settings.clone(), playlist.clone(), sound.clone());
        let mut c = ui.get::<CheckboxWidget>(repeat);
        c.is_checked = Some(Rc::new(move || s.borrow().sound.repeat));
        c.button.on_click = Some(Rc::new(move |_| {
            let repeat = !s2.borrow().sound.repeat;
            s2.borrow_mut().sound.repeat = repeat;
            snd.set_music_looped(repeat);
        }));
        c.button.is_disabled = Some(Rc::new(move || p.current_song_is_background()));
    }

    let time_label = ui.child(panel, "TIME_LABEL");
    {
        let (c, p, s) = (current.clone(), playlist.clone(), sound.clone());
        ui.label(time_label).get_text = Some(Rc::new(move || {
            let song = c.borrow();
            let Some(song) = song.as_ref().filter(|_| !p.current_song_is_background()) else {
                return String::new();
            };
            let seek = s.music_seek_position() as i32;
            format!(
                "{:02}:{:02} / {:02}:{:02}",
                seek / 60,
                seek % 60,
                song.length.get() / 60,
                song.length.get() % 60
            )
        }));
    }
    if let Some(title) = ui.find(panel, "TITLE_LABEL") {
        let (no_song, c) = (message(ui, "label-no-song-playing"), current.clone());
        ui.label(title).get_text = Some(Rc::new(move || {
            c.borrow().as_ref().map_or_else(|| no_song.clone(), |s| s.title.clone())
        }));
    }

    let slider = ui.child(panel, "MUSIC_SLIDER");
    {
        let s = sound.clone();
        let mut w = ui.get::<SliderWidget>(slider);
        w.on_change = Some(Rc::new(move |_, x| s.set_music_volume(x)));
        w.value = sound.music_volume();
    }

    if let Some(watcher) = ui.find(panel, "SONG_WATCHER") {
        let (c, p, s) = (current.clone(), playlist.clone(), sound.clone());
        ui.get::<LogicTickerWidget>(watcher).on_tick = Some(Rc::new(move |_| {
            if p.current_song_is_background() && c.borrow().is_some() {
                *c.borrow_mut() = None;
            }
            let Some(music) = s.current_music() else {
                return;
            };
            if same(&c.borrow(), &music) || p.current_song_is_background() {
                return;
            }
            *c.borrow_mut() = Some(music);
        }));
    }

    if let Some(back) = ui.find(panel, "BACK_BUTTON") {
        let on_exit = args.action("onExit");
        ui.button(back).on_click = Some(Rc::new(move |ui: &mut Ui| {
            if let Err(e) = services(ui).settings.borrow_mut().save() {
                log::warn!("{e}");
            }
            ui.close_window();
            on_exit(ui);
        }));
    }
    cell(NoLogic)
}
