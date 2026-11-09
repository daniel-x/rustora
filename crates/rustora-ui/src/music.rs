// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `MusicPlaylist.cs`. It runs in the client, because it only talks to `Game.Sound`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_client::settings::Settings;
use rustora_core::world_types::MersenneTwister;
use rustora_engine::player::PlayerRef;
use rustora_engine::sound::{AudioRules, MusicInfo, Sound};
use rustora_engine::world::World;
use rustora_mods_common::sound::MusicPlaylistInfo;

/// `MusicPlaylist`.
pub struct MusicPlaylist {
    this: Weak<MusicPlaylist>,
    info: MusicPlaylistInfo,
    sound: Rc<Sound>,
    settings: Rc<RefCell<Settings>>,
    audio: Rc<AudioRules>,
    random: Vec<Rc<MusicInfo>>,
    playlist: Vec<Rc<MusicInfo>>,
    current_song: RefCell<Option<Rc<MusicInfo>>>,
    current_background_song: RefCell<Option<Rc<MusicInfo>>>,
    current_song_is_background: Cell<bool>,
    /// `IsMusicInstalled`.
    pub is_music_installed: bool,
}

fn same(a: &Rc<MusicInfo>, b: &Rc<MusicInfo>) -> bool {
    Rc::ptr_eq(a, b)
}

impl MusicPlaylist {
    /// `None` when the world has no `MusicPlaylist` trait.
    pub fn new(world: &Rc<World>, sound: Rc<Sound>, settings: Rc<RefCell<Settings>>) -> Option<Rc<MusicPlaylist>> {
        let info = world.world_actor().info.trait_info::<MusicPlaylistInfo>()?.as_ref().clone();
        let audio = world.rules().audio().clone();
        let installed = audio.installed_music();
        let playlist: Vec<_> = installed.iter().filter(|m| !m.hidden).cloned().collect();
        let mut random = playlist.clone();
        let mut twister = MersenneTwister::new(world.lobby_info.global_settings.random_seed ^ sound_seed());
        for i in (1..random.len()).rev() {
            random.swap(i, twister.next_max(i as i32 + 1) as usize);
        }
        let music = Rc::new_cyclic(|this| MusicPlaylist {
            this: this.clone(),
            info,
            sound,
            settings,
            audio,
            random,
            playlist,
            current_song: RefCell::new(None),
            current_background_song: RefCell::new(None),
            current_song_is_background: Cell::new(false),
            is_music_installed: !installed.is_empty(),
        });
        if music.is_music_installed {
            music.choose_first_song();
        }
        Some(music)
    }

    fn song(&self, name: &Option<String>) -> Option<Rc<MusicInfo>> {
        let m = self.audio.music.get(name.as_deref()?)?;
        m.exists.get().then(|| m.clone())
    }

    fn choose_first_song(&self) {
        if let Some(song) = self.song(&self.info.background_music) {
            *self.current_song.borrow_mut() = Some(song.clone());
            *self.current_background_song.borrow_mut() = Some(song);
            self.current_song_is_background.set(true);
        } else {
            *self.current_song.borrow_mut() = self.random.first().cloned();
        }
        if let Some(song) = self.song(&self.info.starting_music) {
            *self.current_song.borrow_mut() = Some(song);
            self.current_song_is_background.set(false);
        }
    }

    /// `IPostWorldLoaded.PostWorldLoaded`.
    pub fn post_world_loaded(&self) {
        self.sound.disable_world_sounds.set(self.info.disable_world_sounds);
        self.play_current();
    }

    /// `IsBackgroundMusicMuted`.
    pub fn is_background_music_muted(&self) -> bool {
        self.info.allow_mute_background_music && self.settings.borrow().sound.mute_background_music
    }

    /// `AllowMuteBackgroundMusic`.
    pub fn allow_mute_background_music(&self) -> bool {
        self.is_music_installed && self.info.allow_mute_background_music
    }

    /// `CurrentSongIsBackground`.
    pub fn current_song_is_background(&self) -> bool {
        self.current_song_is_background.get()
    }

    /// `IsMusicAvailable`.
    pub fn is_music_available(&self) -> bool {
        !self.playlist.is_empty()
    }

    /// `CurrentSong`.
    pub fn current_song(&self) -> Option<Rc<MusicInfo>> {
        self.current_song.borrow().clone()
    }

    /// `AvailablePlaylist`.
    pub fn available_playlist(&self) -> &[Rc<MusicInfo>] {
        &self.playlist
    }

    /// `IGameOver.GameOver`. An observer counts as a loser, as in C#.
    pub fn game_over(&self, local_player: Option<&PlayerRef>) {
        let won = local_player.is_some_and(|p| p.win_state.get() == rustora_engine::player::WinState::Won);
        let song = if won {
            self.song(&self.info.victory_music)
        } else {
            self.song(&self.info.defeat_music)
        };
        if let Some(song) = song {
            *self.current_background_song.borrow_mut() = Some(song);
            self.stop();
        }
    }

    fn play_current(&self) {
        let Some(song) = self.current_song().filter(|s| s.exists.get()) else {
            return;
        };
        if self.current_song_is_background.get() && self.is_background_music_muted() {
            return;
        }
        self.sound.set_repeat(self.settings.borrow().sound.repeat);
        let this = self.this.clone();
        self.sound.play_music_then(
            &song,
            Rc::new(move || {
                if let Some(this) = this.upgrade() {
                    this.play_next_song();
                }
            }),
        );
    }

    fn play_next_song(&self) {
        if !self.current_song_is_background.get() {
            *self.current_song.borrow_mut() = self.next_song();
        }
        self.play_current();
    }

    /// `Play(MusicInfo)`.
    pub fn play(&self, music: &Rc<MusicInfo>) {
        *self.current_song.borrow_mut() = Some(music.clone());
        self.current_song_is_background.set(false);
        self.play_current();
    }

    /// `Play(MusicInfo, Action)`.
    pub fn play_then(&self, music: &Rc<MusicInfo>, on_complete: Rc<dyn Fn()>) {
        *self.current_song.borrow_mut() = Some(music.clone());
        self.current_song_is_background.set(false);
        self.sound.set_repeat(self.settings.borrow().sound.repeat);
        self.sound.play_music_then(music, on_complete);
    }

    /// `SetBackgroundMusic`.
    pub fn set_background_music(&self, music: Option<Rc<MusicInfo>>) {
        *self.current_background_song.borrow_mut() = music;
        if self.current_song_is_background.get() {
            self.stop();
        }
    }

    /// `GetNextSong`.
    pub fn next_song(&self) -> Option<Rc<MusicInfo>> {
        self.song_after(false)
    }

    /// `GetPrevSong`.
    pub fn prev_song(&self) -> Option<Rc<MusicInfo>> {
        self.song_after(true)
    }

    fn song_after(&self, reverse: bool) -> Option<Rc<MusicInfo>> {
        if self.playlist.is_empty() {
            return None;
        }
        let songs = if self.settings.borrow().sound.shuffle {
            &self.random
        } else {
            &self.playlist
        };
        let current = self.current_song();
        let position = current.as_ref().and_then(|c| songs.iter().position(|m| same(m, c)));
        let next = match (position, reverse) {
            (Some(i), false) => songs.get(i + 1).or(songs.first()),
            (Some(i), true) if i > 0 => songs.get(i - 1),
            _ if reverse => songs.last(),
            _ => songs.first(),
        };
        next.filter(|m| m.exists.get()).cloned()
    }

    /// `Stop`.
    pub fn stop(&self) {
        *self.current_song.borrow_mut() = None;
        self.sound.stop_music();
        let background = self.current_background_song.borrow().clone();
        if let Some(background) = background {
            *self.current_song.borrow_mut() = Some(background);
            self.current_song_is_background.set(true);
            if !self.is_background_music_muted() {
                self.play_current();
            }
        }
    }

    /// `INotifyActorDisposing.Disposing`.
    pub fn dispose(&self) {
        if self.current_song.borrow().is_some() {
            self.sound.stop_music();
        }
        self.sound.disable_world_sounds.set(false);
    }
}

fn sound_seed() -> i32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as i32
}
