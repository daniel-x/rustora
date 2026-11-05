// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Sound.cs`, `SoundInfo.cs`, `MusicInfo.cs` and `VoiceExts.cs`.
//!
//! Simulation code only queues [`SoundEvent`]s on the world, so it runs the same without an audio device. The client
//! drains the queue into its [`Sound`] once per frame, which applies the rules of `Sound.cs` and drives an [`AudioDevice`].

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::{MersenneTwister, WPos};
use rustora_formats::audio::{Pcm, decode_sound, sound_length_in_seconds};

use crate::actor::{Actor, ActorRef};
use crate::player::PlayerRef;
use crate::sequence::ReadFile;
use crate::world::World;

/// `SoundType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundType {
    World,
    Ui,
}

/// `SoundPool.InterruptType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptType {
    DoNotPlay,
    Interrupt,
    Overlap,
}

/// `SoundPool`.
pub struct SoundPool {
    pub volume_modifier: f32,
    pub interrupt: InterruptType,
    clips: Vec<String>,
    live_clips: RefCell<Vec<String>>,
}

impl SoundPool {
    fn new(volume_modifier: f32, interrupt: InterruptType, clips: Vec<String>) -> SoundPool {
        SoundPool {
            volume_modifier,
            interrupt,
            clips,
            live_clips: RefCell::new(Vec::new()),
        }
    }

    /// `SoundPool.GetNext`: plays each clip once before any clip repeats.
    fn next(&self, random: &mut MersenneTwister) -> Option<String> {
        let mut live = self.live_clips.borrow_mut();
        if live.is_empty() {
            live.extend(self.clips.iter().cloned());
        }
        if live.is_empty() {
            return None;
        }
        let i = random.next_max(live.len() as i32) as usize;
        Some(live.remove(i))
    }
}

fn split_names(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn name_lists(y: &MiniYaml, key: &str) -> HashMap<String, Vec<String>> {
    let Ok(Some(node)) = y.node_with_key_or_default(key) else {
        return HashMap::new();
    };
    node.value
        .nodes
        .iter()
        .map(|n| (n.key.clone(), split_names(n.value.value_str())))
        .collect()
}

fn string_field(y: &MiniYaml, key: &str, default: &str) -> String {
    match y.node_with_key_or_default(key) {
        Ok(Some(n)) => n.value.value_str().to_string(),
        _ => default.to_string(),
    }
}

fn name_set(y: &MiniYaml, key: &str) -> HashSet<String> {
    split_names(&string_field(y, key, "")).into_iter().collect()
}

/// `SoundInfo`.
pub struct SoundInfo {
    pub variants: HashMap<String, Vec<String>>,
    pub prefixes: HashMap<String, Vec<String>>,
    pub default_variant: String,
    pub default_prefix: String,
    pub disable_variants: HashSet<String>,
    pub disable_prefixes: HashSet<String>,
    pub voice_pools: HashMap<String, SoundPool>,
    pub notification_pools: HashMap<String, SoundPool>,
}

impl SoundInfo {
    fn load(y: &MiniYaml) -> SoundInfo {
        let voice_pools = name_lists(y, "Voices")
            .into_iter()
            .map(|(k, clips)| (k, SoundPool::new(1.0, InterruptType::DoNotPlay, clips)))
            .collect();
        SoundInfo {
            variants: name_lists(y, "Variants"),
            prefixes: name_lists(y, "Prefixes"),
            default_variant: string_field(y, "DefaultVariant", ".aud"),
            default_prefix: string_field(y, "DefaultPrefix", ""),
            disable_variants: name_set(y, "DisableVariants"),
            disable_prefixes: name_set(y, "DisablePrefixes"),
            voice_pools,
            notification_pools: notification_pools(y),
        }
    }
}

/// `SoundInfo.ParseSoundPool`.
fn notification_pools(y: &MiniYaml) -> HashMap<String, SoundPool> {
    let Ok(Some(node)) = y.node_with_key_or_default("Notifications") else {
        return HashMap::new();
    };
    node.value
        .nodes
        .iter()
        .map(|t| {
            let volume = string_field(&t.value, "VolumeModifier", "1").parse().unwrap_or(1.0);
            let interrupt = match string_field(&t.value, "InterruptType", "DoNotPlay").as_str() {
                "Interrupt" => InterruptType::Interrupt,
                "Overlap" => InterruptType::Overlap,
                _ => InterruptType::DoNotPlay,
            };
            let pool = SoundPool::new(volume, interrupt, split_names(t.value.value_str()));
            (t.key.clone(), pool)
        })
        .collect()
}

/// `MusicInfo`.
pub struct MusicInfo {
    pub key: String,
    pub filename: String,
    pub title: String,
    pub hidden: bool,
    pub volume_modifier: f32,
    /// Seconds.
    pub length: Cell<i32>,
    pub exists: Cell<bool>,
}

impl MusicInfo {
    fn new(key: &str, y: &MiniYaml) -> MusicInfo {
        let ext = string_field(y, "Extension", "aud");
        MusicInfo {
            key: key.to_string(),
            filename: format!("{}.{ext}", string_field(y, "Filename", key)),
            title: y.value_str().to_string(),
            hidden: string_field(y, "Hidden", "false").eq_ignore_ascii_case("true"),
            volume_modifier: string_field(y, "VolumeModifier", "1").parse().unwrap_or(1.0),
            length: Cell::new(0),
            exists: Cell::new(false),
        }
    }

    /// `MusicInfo.Load`, with a per-process cache of the file results in `known`.
    fn load(&self, read_file: &ReadFile, known: &mut HashMap<String, Option<i32>>) {
        let length = known
            .entry(self.filename.clone())
            .or_insert_with(|| read_file(&self.filename).map(|data| sound_length_in_seconds(&data).unwrap_or(0.0) as i32));
        self.exists.set(length.is_some());
        self.length.set(length.unwrap_or(0));
    }
}

/// The audio sections of `Ruleset`: `Voices`, `Notifications` and `Music`.
#[derive(Default)]
pub struct AudioRules {
    pub voices: HashMap<String, SoundInfo>,
    pub notifications: HashMap<String, SoundInfo>,
    pub music: IndexMap<String, Rc<MusicInfo>>,
}

impl AudioRules {
    pub fn load(voices: &[MiniYamlNode], notifications: &[MiniYamlNode], music: &[MiniYamlNode]) -> AudioRules {
        let infos = |nodes: &[MiniYamlNode]| {
            nodes
                .iter()
                .map(|n| (n.key.to_lowercase(), SoundInfo::load(&n.value)))
                .collect::<HashMap<_, _>>()
        };
        AudioRules {
            voices: infos(voices),
            notifications: infos(notifications),
            music: music
                .iter()
                .map(|n| (n.key.clone(), Rc::new(MusicInfo::new(&n.key, &n.value))))
                .collect(),
        }
    }

    /// `Ruleset.InstalledMusic`, after [`Sound::load_music_files`].
    pub fn installed_music(&self) -> Vec<Rc<MusicInfo>> {
        self.music.values().filter(|m| m.exists.get()).cloned().collect()
    }
}

/// A sound started by simulation code, used to stop or move it later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundHandle(u64);

/// One queued `Game.Sound` call.
pub enum SoundEvent {
    Play {
        sound_type: SoundType,
        player: Option<PlayerRef>,
        name: String,
        head_relative: bool,
        pos: WPos,
        volume_modifier: f32,
        looped: bool,
        handle: SoundHandle,
    },
    Predefined {
        sound_type: SoundType,
        player: Option<PlayerRef>,
        voiced_actor: Option<ActorRef>,
        kind: String,
        definition: String,
        variant: Option<String>,
        relative: bool,
        pos: WPos,
        volume_modifier: f32,
        attenuate: bool,
    },
    Stop(SoundHandle),
    SetPosition(SoundHandle, WPos),
    /// `Game.RunAfterDelay` around another sound call.
    Delayed(u64, Box<SoundEvent>),
}

/// The sound calls of a world since the last drain. Disabled worlds, such as headless ones, queue nothing.
#[derive(Default)]
pub struct SoundQueue {
    enabled: Cell<bool>,
    events: RefCell<Vec<SoundEvent>>,
    next_handle: Cell<u64>,
    local_player: RefCell<Option<PlayerRef>>,
    transient_lines: RefCell<Vec<(Option<PlayerRef>, String)>>,
    system_lines: RefCell<Vec<(String, String)>>,
}

impl SoundQueue {
    /// `World.LocalPlayer` of the client. Only code that decides about sounds and messages may read it.
    pub fn local_player(&self) -> Option<PlayerRef> {
        self.local_player.borrow().clone()
    }

    pub fn set_local_player(&self, player: Option<PlayerRef>) {
        *self.local_player.borrow_mut() = player;
    }

    /// `TextNotificationsManager.AddTransientLine`: `key` names a message.
    pub fn add_transient_line(&self, player: Option<&PlayerRef>, key: Option<&str>) {
        if let Some(key) = key.filter(|k| self.enabled.get() && !k.is_empty()) {
            self.transient_lines.borrow_mut().push((player.cloned(), key.to_string()));
        }
    }

    /// `TextNotificationsManager.AddSystemLine(key, "player", player_name)`.
    pub fn add_player_system_line(&self, key: &str, player_name: &str) {
        if self.enabled.get() {
            self.system_lines.borrow_mut().push((key.to_string(), player_name.to_string()));
        }
    }

    pub fn take_system_lines(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.system_lines.borrow_mut())
    }

    pub fn take_transient_lines(&self) -> Vec<(Option<PlayerRef>, String)> {
        std::mem::take(&mut *self.transient_lines.borrow_mut())
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
        if !enabled {
            self.events.borrow_mut().clear();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.get()
    }

    pub fn push(&self, event: SoundEvent) {
        if self.enabled.get() {
            self.events.borrow_mut().push(event);
        }
    }

    pub fn take(&self) -> Vec<SoundEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    fn new_handle(&self) -> SoundHandle {
        let h = self.next_handle.get() + 1;
        self.next_handle.set(h);
        SoundHandle(h)
    }
}

fn push_play(
    world: &World,
    sound_type: SoundType,
    player: Option<&PlayerRef>,
    name: &str,
    head_relative: bool,
    pos: WPos,
    volume_modifier: f32,
    looped: bool,
) -> Option<SoundHandle> {
    if !world.sound.enabled() || name.is_empty() {
        return None;
    }
    let handle = world.sound.new_handle();
    world.sound.push(SoundEvent::Play {
        sound_type,
        player: player.cloned(),
        name: name.to_string(),
        head_relative,
        pos,
        volume_modifier,
        looped,
        handle,
    });
    Some(handle)
}

/// `Sound.Play(type, name, pos)`.
pub fn play_at(world: &World, sound_type: SoundType, name: &str, pos: WPos, volume_modifier: f32) -> Option<SoundHandle> {
    push_play(world, sound_type, None, name, false, pos, volume_modifier, false)
}

/// `Sound.Play(type, name)`.
pub fn play(world: &World, sound_type: SoundType, name: &str, volume_modifier: f32) -> Option<SoundHandle> {
    push_play(world, sound_type, None, name, true, WPos::new(0, 0, 0), volume_modifier, false)
}

/// `Sound.PlayToPlayer(type, player, name)`.
pub fn play_to_player(world: &World, sound_type: SoundType, player: &PlayerRef, name: &str) -> Option<SoundHandle> {
    push_play(world, sound_type, Some(player), name, true, WPos::new(0, 0, 0), 1.0, false)
}

/// `Sound.PlayToPlayer(type, player, name, pos)`.
pub fn play_to_player_at(world: &World, sound_type: SoundType, player: &PlayerRef, name: &str, pos: WPos) -> Option<SoundHandle> {
    push_play(world, sound_type, Some(player), name, false, pos, 1.0, false)
}

/// `Sound.PlayLooped(type, name, pos)`.
pub fn play_looped_at(world: &World, sound_type: SoundType, name: &str, pos: WPos) -> Option<SoundHandle> {
    push_play(world, sound_type, None, name, false, pos, 1.0, true)
}

/// `Sound.PlayLooped(type, name)`.
pub fn play_looped(world: &World, sound_type: SoundType, name: &str) -> Option<SoundHandle> {
    push_play(world, sound_type, None, name, true, WPos::new(0, 0, 0), 1.0, true)
}

/// Picks one of `names` with `world.LocalRandom`, like `names.Random(world.LocalRandom)`. Draws nothing when audio is off.
pub fn random_name<'a>(world: &World, names: &'a [String]) -> Option<&'a str> {
    if !world.sound.enabled() || names.is_empty() {
        return None;
    }
    let i = world.local_random.borrow_mut().next_max(names.len() as i32) as usize;
    Some(&names[i])
}

/// `Sound.Play(type, names, world, pos)`.
pub fn play_random_at(world: &World, sound_type: SoundType, names: &[String], pos: WPos, volume_modifier: f32) -> Option<SoundHandle> {
    play_at(world, sound_type, random_name(world, names)?, pos, volume_modifier)
}

/// `Sound.Play(type, names, world)`.
pub fn play_random(world: &World, sound_type: SoundType, names: &[String], volume_modifier: f32) -> Option<SoundHandle> {
    play(world, sound_type, random_name(world, names)?, volume_modifier)
}

/// `Sound.StopSound`.
pub fn stop(world: &World, handle: SoundHandle) {
    world.sound.push(SoundEvent::Stop(handle));
}

/// `Sound.SetPosition`.
pub fn set_position(world: &World, handle: SoundHandle, pos: WPos) {
    world.sound.push(SoundEvent::SetPosition(handle, pos));
}

/// `Sound.PlayNotification(world.Map.Rules, player, type, notification, variant)`.
pub fn play_notification(world: &World, player: Option<&PlayerRef>, kind: &str, notification: &str, variant: Option<&str>) {
    if notification.is_empty() {
        return;
    }
    world.sound.push(SoundEvent::Predefined {
        sound_type: SoundType::Ui,
        player: player.cloned(),
        voiced_actor: None,
        kind: kind.to_lowercase(),
        definition: notification.to_string(),
        variant: variant.map(str::to_string),
        relative: true,
        pos: WPos::new(0, 0, 0),
        volume_modifier: 1.0,
        attenuate: false,
    });
}

/// The usual pair of calls: `PlayNotification` to the owner in the owner's faction, then `AddTransientLine` to the owner.
pub fn notify_owner(self_: &Actor, kind: &str, notification: Option<&str>, text: Option<&str>) {
    let world = self_.world();
    if !world.sound.enabled() {
        return;
    }
    let owner = self_.owner();
    if let Some(notification) = notification {
        play_notification(&world, Some(&owner), kind, notification, Some(&owner.faction));
    }
    world.sound.add_transient_line(Some(&owner), text);
}

/// `foreach (var s in sounds) PlayToPlayer(type, self.Owner, s, pos)`.
pub fn play_all_to_owner(self_: &Actor, sound_type: SoundType, sounds: &[String], pos: WPos) {
    let world = self_.world();
    let owner = self_.owner();
    for s in sounds {
        play_to_player_at(&world, sound_type, &owner, s, pos);
    }
}

/// `Game.RunAfterDelay(millis, () => PlayNotification(rules, player, "Speech", notification, player.Faction))`.
pub fn play_notification_after_delay(world: &World, millis: u64, player: &PlayerRef, notification: &str) {
    if notification.is_empty() {
        return;
    }
    let event = SoundEvent::Predefined {
        sound_type: SoundType::Ui,
        player: Some(player.clone()),
        voiced_actor: None,
        kind: "speech".to_string(),
        definition: notification.to_string(),
        variant: Some(player.faction.clone()),
        relative: true,
        pos: WPos::new(0, 0, 0),
        volume_modifier: 1.0,
        attenuate: false,
    };
    world.sound.push(SoundEvent::Delayed(millis, Box::new(event)));
}

/// `IVoiced`.
pub trait IVoiced {
    fn voice_set(&self) -> &str;
    fn play_voice(&self, self_: &ActorRef, phrase: &str, variant: &str) -> bool;
    fn play_voice_local(&self, self_: &ActorRef, phrase: &str, variant: &str, volume: f32) -> bool;
    fn has_voice(&self, self_: &Actor, voice: &str) -> bool;
}

/// `IOrderVoice`.
pub trait IOrderVoice {
    fn voice_phrase_for_order(&self, self_: &ActorRef, order: &crate::order::Order) -> Option<String>;
}

/// `VoiceExts.PlayVoice`.
pub fn play_voice(self_: &ActorRef, phrase: &str) {
    if !self_.world().sound.enabled() {
        return;
    }
    let faction = self_.owner().faction.clone();
    for voiced in self_.traits_implementing::<dyn IVoiced>() {
        if voiced.voice_set().is_empty() {
            return;
        }
        voiced.play_voice(self_, phrase, &faction);
    }
}

/// `VoiceExts.PlayVoiceLocal`.
pub fn play_voice_local(self_: &ActorRef, phrase: &str, volume: f32) {
    if !self_.world().sound.enabled() {
        return;
    }
    let faction = self_.owner().faction.clone();
    for voiced in self_.traits_implementing::<dyn IVoiced>() {
        if voiced.voice_set().is_empty() {
            return;
        }
        voiced.play_voice_local(self_, phrase, &faction, volume);
    }
}

/// `VoiceExts.HasVoice`.
pub fn has_voice(self_: &Actor, voice: &str) -> bool {
    self_.traits_implementing::<dyn IVoiced>().iter().any(|v| v.has_voice(self_, voice))
}

/// `VoiceExts.PlayVoiceForOrders`: the first subject with a phrase speaks.
pub fn play_voice_for_orders(orders: &[crate::order::Order]) {
    for o in orders {
        let spoke = match &o.grouped_actors {
            Some(group) => group.iter().any(|subject| play_voice_for_order(&o.from_grouped(subject.clone()))),
            None => play_voice_for_order(o),
        };
        if spoke {
            return;
        }
    }
}

fn play_voice_for_order(o: &crate::order::Order) -> bool {
    let Some(subject) = o.subject.as_ref().filter(|s| !s.disposed()) else {
        return false;
    };
    for voice in subject.traits_implementing::<dyn IVoiced>() {
        for v in subject.traits_implementing::<dyn IOrderVoice>() {
            if let Some(phrase) = v.voice_phrase_for_order(subject, o)
                && voice.play_voice(subject, &phrase, &subject.owner().faction)
            {
                return true;
            }
        }
    }
    false
}

/// Sound data for an [`AudioDevice`].
pub type Clip = Arc<Pcm>;

/// A playing sound of an [`AudioDevice`].
pub type VoiceId = u64;

/// The arguments of `ISoundEngine.Play2D`.
pub struct PlayParams {
    pub looping: bool,
    pub relative: bool,
    pub pos: WPos,
    pub volume: f32,
    pub attenuate: bool,
}

/// `ISoundEngine`.
pub trait AudioDevice {
    /// `None` when the instance limit of the sound applies or no slot is free.
    fn play(&self, clip: &Clip, params: PlayParams) -> Option<VoiceId>;
    fn stop(&self, id: VoiceId);
    fn stop_all(&self);
    fn set_paused(&self, id: VoiceId, paused: bool);
    fn set_all_paused(&self, paused: bool);
    fn set_volume(&self, id: VoiceId, volume: f32);
    fn set_position(&self, id: VoiceId, pos: WPos);
    fn set_looping(&self, id: VoiceId, looping: bool);
    fn is_complete(&self, id: VoiceId) -> bool;
    /// Seconds played, `ISound.SeekPosition`.
    fn seek_position(&self, id: VoiceId) -> f32;
    fn set_listener(&self, pos: WPos);
    /// `ISoundEngine.Volume`: 0 mutes, 1 unmutes.
    fn set_master_volume(&self, volume: f32);
    /// Advances the frame count the instance limit of `play` uses.
    fn next_frame(&self);
}

/// `DummySoundEngine`.
pub struct NullAudioDevice;

impl AudioDevice for NullAudioDevice {
    fn play(&self, _: &Clip, _: PlayParams) -> Option<VoiceId> {
        None
    }
    fn stop(&self, _: VoiceId) {}
    fn stop_all(&self) {}
    fn set_paused(&self, _: VoiceId, _: bool) {}
    fn set_all_paused(&self, _: bool) {}
    fn set_volume(&self, _: VoiceId, _: f32) {}
    fn set_position(&self, _: VoiceId, _: WPos) {}
    fn set_looping(&self, _: VoiceId, _: bool) {}
    fn is_complete(&self, _: VoiceId) -> bool {
        true
    }
    fn seek_position(&self, _: VoiceId) -> f32 {
        0.0
    }
    fn set_listener(&self, _: WPos) {}
    fn set_master_volume(&self, _: f32) {}
    fn next_frame(&self) {}
}

/// The arguments of `Sound.PlayPredefined`, with the voiced actor reduced to its id and selection state.
pub struct Predefined<'a> {
    pub sound_type: SoundType,
    /// False when the sound belongs to another player than the local one.
    pub for_local_player: bool,
    /// `voicedActor.ActorID` and whether the local player selected it, for voices.
    pub voiced_actor: Option<(u32, bool)>,
    pub kind: &'a str,
    pub definition: &'a str,
    pub variant: Option<&'a str>,
    pub relative: bool,
    pub pos: WPos,
    pub volume_modifier: f32,
    pub attenuate: bool,
}

type MusicCallback = Rc<dyn Fn()>;

/// `Sound`: the client side of the audio system.
pub struct Sound {
    device: Box<dyn AudioDevice>,
    read_file: RefCell<ReadFile>,
    clips: RefCell<HashMap<String, Option<Clip>>>,
    music_files: RefCell<HashMap<String, Option<i32>>>,
    random: RefCell<MersenneTwister>,
    current_sounds: RefCell<HashMap<u32, VoiceId>>,
    current_notifications: RefCell<HashMap<String, VoiceId>>,
    handles: RefCell<HashMap<SoundHandle, VoiceId>>,
    pending: RefCell<Vec<(Instant, SoundEvent)>>,
    music: Cell<Option<VoiceId>>,
    video: Cell<Option<VoiceId>>,
    current_music: RefCell<Option<Rc<MusicInfo>>>,
    music_playing: Cell<bool>,
    on_music_complete: RefCell<Option<MusicCallback>>,
    sound_volume: Cell<f32>,
    music_volume: Cell<f32>,
    video_volume: Cell<f32>,
    sound_volume_modifier: Cell<f32>,
    repeat: Cell<bool>,
    /// `Sound.DisableAllSounds`.
    pub disable_all_sounds: Cell<bool>,
    /// `Sound.DisableWorldSounds`.
    pub disable_world_sounds: Cell<bool>,
}

impl Sound {
    pub fn new(device: Box<dyn AudioDevice>, read_file: ReadFile, random_seed: i32) -> Sound {
        Sound {
            device,
            read_file: RefCell::new(read_file),
            clips: RefCell::default(),
            music_files: RefCell::default(),
            random: RefCell::new(MersenneTwister::new(random_seed)),
            current_sounds: RefCell::default(),
            current_notifications: RefCell::default(),
            handles: RefCell::default(),
            pending: RefCell::default(),
            music: Cell::new(None),
            video: Cell::new(None),
            current_music: RefCell::new(None),
            music_playing: Cell::new(false),
            on_music_complete: RefCell::new(None),
            sound_volume: Cell::new(0.5),
            music_volume: Cell::new(0.5),
            video_volume: Cell::new(0.5),
            sound_volume_modifier: Cell::new(1.0),
            repeat: Cell::new(false),
            disable_all_sounds: Cell::new(false),
            disable_world_sounds: Cell::new(false),
        }
    }

    /// A sound system without an audio device, for tests and tools.
    pub fn silent() -> Sound {
        Sound::new(Box::new(NullAudioDevice), Rc::new(|_| None), 0)
    }

    /// `Sound.Initialize`: replaces the file system, for example with one that includes map files.
    pub fn set_file_system(&self, read_file: ReadFile) {
        self.stop_music();
        self.device.stop_all();
        *self.read_file.borrow_mut() = read_file;
        self.clips.borrow_mut().clear();
        self.music_files.borrow_mut().clear();
        self.current_sounds.borrow_mut().clear();
        self.current_notifications.borrow_mut().clear();
        self.handles.borrow_mut().clear();
        self.video.set(None);
    }

    fn internal_volume(&self) -> f32 {
        self.sound_volume.get() * self.sound_volume_modifier.get()
    }

    fn clip(&self, name: &str) -> Option<Clip> {
        if let Some(c) = self.clips.borrow().get(name) {
            return c.clone();
        }
        let clip = self.load_pcm(name).map(Arc::new);
        self.clips.borrow_mut().insert(name.to_string(), clip.clone());
        clip
    }

    fn load_pcm(&self, name: &str) -> Option<Pcm> {
        let read_file = self.read_file.borrow().clone();
        let Some(data) = read_file(name) else {
            log::info!("LoadSound, file does not exist: {name}");
            return None;
        };
        let pcm = decode_sound(&data);
        if pcm.is_none() {
            log::warn!("{name} is not a valid sound file");
        }
        pcm
    }

    /// `Sound.Play(type, player, name, headRelative, pos, volumeModifier, loop)`.
    pub fn play(
        &self,
        sound_type: SoundType,
        for_local_player: bool,
        name: &str,
        head_relative: bool,
        pos: WPos,
        volume_modifier: f32,
        looping: bool,
    ) -> Option<VoiceId> {
        if name.is_empty()
            || self.disable_all_sounds.get()
            || (self.disable_world_sounds.get() && sound_type == SoundType::World)
            || !for_local_player
        {
            return None;
        }
        let clip = self.clip(name)?;
        self.device.play(
            &clip,
            PlayParams {
                looping,
                relative: head_relative,
                pos,
                volume: self.internal_volume() * volume_modifier,
                attenuate: true,
            },
        )
    }

    /// `Sound.PlayPredefined`: returns true when the sound played.
    pub fn play_predefined(&self, rules: &AudioRules, p: &Predefined) -> bool {
        if self.disable_all_sounds.get() || (self.disable_world_sounds.get() && p.sound_type == SoundType::World) {
            return false;
        }
        let set = if p.voiced_actor.is_some() {
            &rules.voices
        } else {
            &rules.notifications
        };
        let Some(info) = set.get(p.kind) else {
            return false;
        };
        let pools = if p.voiced_actor.is_some() {
            &info.voice_pools
        } else {
            &info.notification_pools
        };
        let Some(pool) = pools.get(p.definition) else {
            log::warn!("Can't find {} in the sound pool of {}.", p.definition, p.kind);
            return false;
        };
        let Some(clip) = pool.next(&mut self.random.borrow_mut()) else {
            return false;
        };
        let name = predefined_name(info, &clip, p);
        if name.is_empty() || !p.for_local_player {
            return false;
        }
        let actor_id = match p.voiced_actor {
            None | Some((_, true)) => 0,
            Some((id, false)) => id,
        };
        if pool.interrupt != InterruptType::Overlap && !self.resolve_interrupt(p, pool.interrupt, &name, actor_id) {
            return false;
        }
        let Some(data) = self.clip(&name) else {
            return false;
        };
        let voice = self.device.play(
            &data,
            PlayParams {
                looping: false,
                relative: p.relative,
                pos: p.pos,
                volume: self.internal_volume() * p.volume_modifier * pool.volume_modifier,
                attenuate: p.attenuate,
            },
        );
        let Some(voice) = voice else {
            return false;
        };
        if pool.interrupt == InterruptType::Overlap {
            return true;
        }
        if p.voiced_actor.is_some() {
            self.current_sounds.borrow_mut().insert(actor_id, voice);
        } else {
            self.current_notifications.borrow_mut().insert(name, voice);
        }
        true
    }

    /// False when a sound of the same owner still plays and the pool says not to play over it.
    fn resolve_interrupt(&self, p: &Predefined, interrupt: InterruptType, name: &str, actor_id: u32) -> bool {
        let current = if p.voiced_actor.is_some() {
            self.current_sounds.borrow().get(&actor_id).copied()
        } else {
            self.current_notifications.borrow().get(name).copied()
        };
        let Some(current) = current else {
            return true;
        };
        if self.device.is_complete(current) {
            return true;
        }
        if interrupt == InterruptType::Interrupt {
            self.device.stop(current);
            return true;
        }
        false
    }

    /// `Sound.PlayNotification(rules, null, type, notification, variant)`.
    pub fn play_notification(&self, rules: &AudioRules, kind: &str, notification: &str, variant: Option<&str>) -> bool {
        self.play_predefined(
            rules,
            &Predefined {
                sound_type: SoundType::Ui,
                for_local_player: true,
                voiced_actor: None,
                kind: &kind.to_lowercase(),
                definition: notification,
                variant,
                relative: true,
                pos: WPos::new(0, 0, 0),
                volume_modifier: 1.0,
                attenuate: false,
            },
        )
    }

    /// Applies the sound calls of `world`. `local_player` and `is_selected` are the state of the client.
    pub fn process_events(
        &self,
        rules: &AudioRules,
        world: &World,
        local_player: Option<&PlayerRef>,
        is_selected: &dyn Fn(&ActorRef) -> bool,
    ) {
        let now = Instant::now();
        let mut events = world.sound.take();
        let mut pending = self.pending.borrow_mut();
        let (due, later): (Vec<_>, Vec<_>) = pending.drain(..).partition(|(at, _)| *at <= now);
        *pending = later;
        drop(pending);
        events.extend(due.into_iter().map(|(_, e)| e));
        let is_local = |p: &Option<PlayerRef>| p.as_ref().is_none_or(|p| local_player.is_some_and(|l| Rc::ptr_eq(p, l)));
        for e in events {
            match e {
                SoundEvent::Delayed(millis, event) => {
                    self.pending.borrow_mut().push((now + Duration::from_millis(millis), *event));
                }
                SoundEvent::Play {
                    sound_type,
                    player,
                    name,
                    head_relative,
                    pos,
                    volume_modifier,
                    looped,
                    handle,
                } => {
                    let voice = self.play(sound_type, is_local(&player), &name, head_relative, pos, volume_modifier, looped);
                    if let Some(voice) = voice
                        && looped
                    {
                        self.handles.borrow_mut().insert(handle, voice);
                    }
                }
                SoundEvent::Predefined {
                    sound_type,
                    player,
                    voiced_actor,
                    kind,
                    definition,
                    variant,
                    relative,
                    pos,
                    volume_modifier,
                    attenuate,
                } => {
                    self.play_predefined(
                        rules,
                        &Predefined {
                            sound_type,
                            for_local_player: is_local(&player),
                            voiced_actor: voiced_actor.as_ref().map(|a| (a.id, is_selected(a))),
                            kind: &kind,
                            definition: &definition,
                            variant: variant.as_deref(),
                            relative,
                            pos,
                            volume_modifier,
                            attenuate,
                        },
                    );
                }
                SoundEvent::Stop(handle) => {
                    if let Some(voice) = self.handles.borrow_mut().remove(&handle) {
                        self.device.stop(voice);
                    }
                }
                SoundEvent::SetPosition(handle, pos) => {
                    if let Some(&voice) = self.handles.borrow().get(&handle) {
                        self.device.set_position(voice, pos);
                    }
                }
            }
        }
    }

    /// `Sound.SetListenerPosition`.
    pub fn set_listener_position(&self, pos: WPos) {
        self.device.set_listener(pos);
    }

    /// `Sound.MuteAudio`.
    pub fn mute_audio(&self) {
        self.device.set_master_volume(0.0);
    }

    /// `Sound.UnmuteAudio`.
    pub fn unmute_audio(&self) {
        self.device.set_master_volume(1.0);
    }

    /// `Sound.StopAudio`.
    pub fn stop_audio(&self) {
        self.device.stop_all();
        self.pending.borrow_mut().clear();
        self.music.set(None);
        self.current_music.borrow_mut().take();
        self.music_playing.set(false);
        self.video.set(None);
        self.handles.borrow_mut().clear();
    }

    /// `Sound.StopSound`.
    pub fn stop_sound(&self, voice: VoiceId) {
        self.device.stop(voice);
    }

    /// `Sound.Tick`. A callback may start the next song.
    pub fn tick(&self) {
        self.device.next_frame();
        if self.music_playing.get() && self.music.get().is_none_or(|m| self.device.is_complete(m)) {
            self.stop_music();
            let callback = self.on_music_complete.borrow().clone();
            if let Some(f) = callback {
                f();
            }
        }
    }

    /// `MusicInfo.Load` for every track. Only the client calls it, because it reads all music files.
    pub fn load_music_files(&self, rules: &AudioRules) {
        let read_file = self.read_file.borrow().clone();
        let mut known = self.music_files.borrow_mut();
        for m in rules.music.values() {
            m.load(&read_file, &mut known);
        }
    }

    /// `Sound.MusicPlaying`.
    pub fn music_playing(&self) -> bool {
        self.music_playing.get()
    }

    /// `Sound.CurrentMusic`.
    pub fn current_music(&self) -> Option<Rc<MusicInfo>> {
        self.current_music.borrow().clone()
    }

    /// `Sound.MusicSeekPosition`.
    pub fn music_seek_position(&self) -> f32 {
        self.music.get().map_or(0.0, |m| self.device.seek_position(m))
    }

    /// `Settings.Sound.Repeat`.
    pub fn set_repeat(&self, repeat: bool) {
        self.repeat.set(repeat);
    }

    /// `Sound.SetMusicLooped`, without the settings update that the caller makes.
    pub fn set_music_looped(&self, looped: bool) {
        self.repeat.set(looped);
        if let Some(music) = self.music.get() {
            self.device.set_looping(music, looped);
        }
    }

    /// `Sound.PlayMusicThen`.
    pub fn play_music_then(&self, m: &Rc<MusicInfo>, then: MusicCallback) {
        if !m.exists.get() {
            return;
        }
        *self.on_music_complete.borrow_mut() = Some(then);
        let same = self.current_music.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, m));
        if same && let Some(music) = self.music.get() {
            self.device.set_paused(music, false);
            self.music_playing.set(true);
            return;
        }
        self.play_music(m, self.repeat.get());
    }

    /// `Sound.PlayMusic(m, looped)`.
    pub fn play_music(&self, m: &Rc<MusicInfo>, looped: bool) {
        if !m.exists.get() {
            return;
        }
        self.stop_music();
        let voice = self.load_pcm(&m.filename).and_then(|pcm| {
            self.device.play(
                &Arc::new(pcm),
                PlayParams {
                    looping: looped,
                    relative: true,
                    pos: WPos::new(0, 0, 0),
                    volume: self.music_volume.get() * m.volume_modifier,
                    attenuate: false,
                },
            )
        });
        let Some(voice) = voice else {
            self.on_music_complete.borrow_mut().take();
            return;
        };
        self.music.set(Some(voice));
        *self.current_music.borrow_mut() = Some(m.clone());
        self.music_playing.set(true);
    }

    /// `Sound.PlayMusic()`: resumes paused music.
    pub fn resume_music(&self) {
        if let Some(music) = self.music.get() {
            self.music_playing.set(true);
            self.device.set_paused(music, false);
        }
    }

    /// `Sound.StopMusic`.
    pub fn stop_music(&self) {
        if let Some(music) = self.music.take() {
            self.device.stop(music);
        }
        self.current_music.borrow_mut().take();
        self.music_playing.set(false);
    }

    /// `Sound.PauseMusic`.
    pub fn pause_music(&self) {
        if let Some(music) = self.music.get() {
            self.music_playing.set(false);
            self.device.set_paused(music, true);
        }
    }

    /// `Sound.PlayVideo(raw, channels, sampleBits, sampleRate)` with decoded samples.
    pub fn play_video(&self, pcm: Pcm) {
        self.stop_video();
        let voice = self.device.play(
            &Arc::new(pcm),
            PlayParams {
                looping: false,
                relative: true,
                pos: WPos::new(0, 0, 0),
                volume: self.internal_volume(),
                attenuate: false,
            },
        );
        self.video.set(voice);
    }

    /// `Sound.PlayVideo()` and `Sound.PauseVideo`.
    pub fn set_video_paused(&self, paused: bool) {
        if let Some(video) = self.video.get() {
            self.device.set_paused(video, paused);
        }
    }

    /// `Sound.StopVideo`.
    pub fn stop_video(&self) {
        if let Some(video) = self.video.take() {
            self.device.stop(video);
        }
    }

    /// `Sound.VideoSeekPosition`.
    pub fn video_seek_position(&self) -> f32 {
        self.video.get().map_or(0.0, |v| self.device.seek_position(v))
    }

    /// `Sound.SoundVolumeModifier`.
    pub fn set_sound_volume_modifier(&self, modifier: f32) {
        self.sound_volume_modifier.set(modifier);
    }

    /// `Sound.SoundVolume`.
    pub fn sound_volume(&self) -> f32 {
        self.sound_volume.get()
    }

    /// `Sound.SoundVolume`: changes new sounds and a playing video.
    pub fn set_sound_volume(&self, volume: f32) {
        self.sound_volume.set(volume);
        if let Some(video) = self.video.get() {
            self.device.set_volume(video, self.internal_volume());
        }
    }

    /// `Sound.MusicVolume`.
    pub fn music_volume(&self) -> f32 {
        self.music_volume.get()
    }

    /// `Sound.MusicVolume`.
    pub fn set_music_volume(&self, volume: f32) {
        self.music_volume.set(volume);
        if let Some(music) = self.music.get() {
            let modifier = self.current_music.borrow().as_ref().map_or(1.0, |m| m.volume_modifier);
            self.device.set_volume(music, volume * modifier);
        }
    }

    /// `Sound.VideoVolume`.
    pub fn video_volume(&self) -> f32 {
        self.video_volume.get()
    }

    /// `Sound.VideoVolume`.
    pub fn set_video_volume(&self, volume: f32) {
        self.video_volume.set(volume);
        if let Some(video) = self.video.get() {
            self.device.set_volume(video, volume);
        }
    }
}

/// The file name part of `Sound.PlayPredefined`: `prefix + clip + suffix`.
fn predefined_name(info: &SoundInfo, clip: &str, p: &Predefined) -> String {
    let id = p.voiced_actor.map_or(0, |(id, _)| id) as usize;
    let mut suffix = info.default_variant.as_str();
    let mut prefix = info.default_prefix.as_str();
    if let Some(variant) = p.variant {
        if let Some(list) = info.variants.get(variant)
            && !list.is_empty()
            && !info.disable_variants.contains(p.definition)
        {
            suffix = &list[id % list.len()];
        }
        if let Some(list) = info.prefixes.get(variant)
            && !list.is_empty()
            && !info.disable_prefixes.contains(p.definition)
        {
            prefix = &list[id % list.len()];
        }
    }
    format!("{prefix}{clip}{suffix}")
}
