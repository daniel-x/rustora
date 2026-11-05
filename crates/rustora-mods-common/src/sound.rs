// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Voiced.cs`, `MusicPlaylist.cs` (its info) and the traits in `Traits/Sound`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use rustora_derive::FieldLoad;
use rustora_engine::actor::{Actor, ActorRef};
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::{ConditionalHooks, conditional_created};
use rustora_engine::conditional::{ConditionalState, ConditionalTraitInfo};
use rustora_engine::conditional_plumbing;
use rustora_engine::damage::{AttackInfo, DamageState, DamageTypeTag as DamageType};
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, IVoiced, SoundEvent, SoundType};
use rustora_engine::target::Target;

use crate::attack::{Armament, Barrel};
use crate::interfaces::{INotifyAppliedDamage, INotifyAttack, INotifyCapture, INotifyDamage, INotifyDamageStateChanged, INotifyKilled};
use rustora_engine::traits::*;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("EnemyWatcher", |n| load_info!(EnemyWatcherInfo, n));
    r.register("AnnounceOnSeen", |n| load_info!(AnnounceOnSeenInfo, n));
    r.register("Voiced", |n| load_info!(VoicedInfo, n));
    r.register("MusicPlaylist", |n| load_info!(MusicPlaylistInfo, n));
    r.register("ActorLostNotification", |n| load_info!(ActorLostNotificationInfo, n));
    r.register("AnnounceOnKill", |n| load_info!(AnnounceOnKillInfo, n));
    r.register("AttackSounds", |n| load_info!(AttackSoundsInfo, n));
    r.register("CaptureNotification", |n| load_info!(CaptureNotificationInfo, n));
    r.register("DeathSounds", |n| load_info!(DeathSoundsInfo, n));
    r.register("SoundOnDamageTransition", |n| load_info!(SoundOnDamageTransitionInfo, n));
    r.register("VoiceAnnouncement", |n| load_info!(VoiceAnnouncementInfo, n));
    r.register("StartGameNotification", |n| load_info!(StartGameNotificationInfo, n));
    r.register("BaseAttackNotifier", |n| load_info!(BaseAttackNotifierInfo, n));
    r.register("HarvesterAttackNotifier", |n| load_info!(HarvesterAttackNotifierInfo, n));
    r.register("ResourceStorageWarning", |n| load_info!(ResourceStorageWarningInfo, n));
}

/// `VoicedInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct VoicedInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub voice_set: String,
    pub volume: f32,
}

impl Default for VoicedInfo {
    fn default() -> Self {
        VoicedInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            voice_set: String::new(),
            volume: 1.0,
        }
    }
}

/// `Voiced`.
pub struct Voiced {
    info: Rc<VoicedInfo>,
    state: ConditionalState,
}

conditional_simple!(Voiced);
register_traits!(Voiced: IVoiced, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(VoicedInfo => |info, _init| Some(Rc::new(Voiced {
    state: ConditionalState::new(&info.base),
    info,
})));

impl Voiced {
    fn predefined(&self, self_: &ActorRef, phrase: &str, variant: &str, relative: bool, volume: f32) -> bool {
        if self.state.is_trait_disabled() || self.info.voice_set.is_empty() {
            return false;
        }
        self_.world().sound.push(SoundEvent::Predefined {
            sound_type: SoundType::World,
            player: None,
            voiced_actor: Some(self_.clone()),
            kind: self.info.voice_set.to_lowercase(),
            definition: phrase.to_string(),
            variant: Some(variant.to_string()),
            relative,
            pos: if relative { Default::default() } else { self_.center_position() },
            volume_modifier: volume,
            attenuate: true,
        });
        true
    }
}

impl IVoiced for Voiced {
    fn voice_set(&self) -> &str {
        &self.info.voice_set
    }

    fn play_voice(&self, self_: &ActorRef, phrase: &str, variant: &str) -> bool {
        self.predefined(self_, phrase, variant, true, self.info.volume)
    }

    fn play_voice_local(&self, self_: &ActorRef, phrase: &str, variant: &str, volume: f32) -> bool {
        self.predefined(self_, phrase, variant, false, volume)
    }

    fn has_voice(&self, self_: &Actor, voice: &str) -> bool {
        if self.state.is_trait_disabled() || self.info.voice_set.is_empty() {
            return false;
        }
        self_
            .world()
            .rules()
            .audio()
            .voices
            .get(&self.info.voice_set.to_lowercase())
            .is_some_and(|v| v.voice_pools.contains_key(voice))
    }
}

/// `MusicPlaylistInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct MusicPlaylistInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub starting_music: Option<String>,
    pub victory_music: Option<String>,
    pub defeat_music: Option<String>,
    pub background_music: Option<String>,
    pub allow_mute_background_music: bool,
    pub disable_world_sounds: bool,
}

/// The sync-free world trait; the client runs the playlist from the info.
pub struct MusicPlaylistTrait;
register_traits!(MusicPlaylistTrait);
info_impl!(MusicPlaylistInfo => |_info, _init| Some(Rc::new(MusicPlaylistTrait)));

/// `StartGameNotificationInfo`. The client plays it after the world loaded.
#[derive(Clone, Debug, FieldLoad)]
pub struct StartGameNotificationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub notification: String,
    pub text_notification: Option<String>,
    pub loaded_notification: String,
    pub loaded_text_notification: Option<String>,
    pub saved_notification: String,
    pub saved_text_notification: Option<String>,
}

impl Default for StartGameNotificationInfo {
    fn default() -> Self {
        StartGameNotificationInfo {
            instance_name: None,
            notification: "StartGame".into(),
            text_notification: None,
            loaded_notification: "GameLoaded".into(),
            loaded_text_notification: None,
            saved_notification: "GameSaved".into(),
            saved_text_notification: None,
        }
    }
}

info_impl!(StartGameNotificationInfo => |_info, _init| Some(Rc::new(MusicPlaylistTrait)));

/// `ActorLostNotificationInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ActorLostNotificationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub notification: String,
    pub text_notification: Option<String>,
    pub notify_all: bool,
}

impl Default for ActorLostNotificationInfo {
    fn default() -> Self {
        ActorLostNotificationInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            notification: "UnitLost".into(),
            text_notification: None,
            notify_all: false,
        }
    }
}

/// `ActorLostNotification`.
pub struct ActorLostNotification {
    info: Rc<ActorLostNotificationInfo>,
    state: ConditionalState,
}

conditional_simple!(ActorLostNotification);
register_traits!(ActorLostNotification: INotifyKilled, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(ActorLostNotificationInfo => |info, _init| Some(Rc::new(ActorLostNotification {
    state: ConditionalState::new(&info.base),
    info,
})));

impl INotifyKilled for ActorLostNotification {
    fn killed(&self, self_: &ActorRef, _e: &AttackInfo) {
        if self.state.is_trait_disabled() {
            return;
        }
        let world = self_.world();
        let Some(local_player) = world.sound.local_player().filter(|p| !p.spectating()) else {
            return;
        };
        let owner = self_.owner();
        let player = if self.info.notify_all { &local_player } else { &owner };
        sound::play_notification(&world, Some(player), "Speech", &self.info.notification, Some(&owner.faction));
        world.sound.add_transient_line(Some(player), self.info.text_notification.as_deref());
    }
}

/// `AnnounceOnKillInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct AnnounceOnKillInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub interval: i32,
    pub voice: String,
}

impl Default for AnnounceOnKillInfo {
    fn default() -> Self {
        AnnounceOnKillInfo {
            instance_name: None,
            interval: 5000,
            voice: "Kill".into(),
        }
    }
}

/// `AnnounceOnKill`.
pub struct AnnounceOnKill {
    info: Rc<AnnounceOnKillInfo>,
    last_announce: Cell<Option<Instant>>,
}

register_traits!(AnnounceOnKill: INotifyAppliedDamage);
info_impl!(AnnounceOnKillInfo => |info, _init| Some(Rc::new(AnnounceOnKill {
    info,
    last_announce: Cell::new(None),
})));

impl INotifyAppliedDamage for AnnounceOnKill {
    fn applied_damage(&self, self_: &ActorRef, damaged: &ActorRef, e: &AttackInfo) {
        let suicide = e.attacker.as_ref().is_some_and(|a| Rc::ptr_eq(a, damaged));
        if e.damage_state != DamageState::DEAD || suicide || !self_.world().sound.enabled() {
            return;
        }
        let elapsed = self
            .last_announce
            .get()
            .is_none_or(|t| t.elapsed().as_millis() > self.info.interval as u128);
        if elapsed {
            sound::play_voice(self_, &self.info.voice);
        }
        self.last_announce.set(Some(Instant::now()));
    }
}

/// `AttackDelayType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttackDelayType {
    #[default]
    Preparation,
    Attack,
}
rustora_core::field_enum!(AttackDelayType, [Preparation, Attack]);

/// `AttackSoundsInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct AttackSoundsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sounds: Vec<String>,
    pub delay: i32,
    pub delay_relative_to: AttackDelayType,
}

/// `AttackSounds`.
pub struct AttackSounds {
    info: Rc<AttackSoundsInfo>,
    state: ConditionalState,
    tick: Cell<i32>,
}

conditional_simple!(AttackSounds);
register_traits!(AttackSounds: INotifyAttack, ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(AttackSoundsInfo => |info, _init| Some(Rc::new(AttackSounds {
    state: ConditionalState::new(&info.base),
    info,
    tick: Cell::new(0),
})));

impl AttackSounds {
    fn play(&self, self_: &ActorRef) {
        sound::play_random_at(&self_.world(), SoundType::World, &self.info.sounds, self_.center_position(), 1.0);
    }

    fn start(&self, self_: &ActorRef, relative_to: AttackDelayType) {
        if self.info.delay_relative_to == relative_to {
            if self.info.delay > 0 {
                self.tick.set(self.info.delay);
            } else {
                self.play(self_);
            }
        }
    }
}

impl INotifyAttack for AttackSounds {
    fn attacking(&self, self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {
        self.start(self_, AttackDelayType::Attack);
    }

    fn preparing_attack(&self, self_: &ActorRef, _target: &Target, _a: &Armament, _barrel: &Barrel) {
        self.start(self_, AttackDelayType::Preparation);
    }
}

impl ITick for AttackSounds {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() || self.info.delay <= 0 {
            return;
        }
        self.tick.set(self.tick.get() - 1);
        if self.tick.get() == 0 {
            self.play(self_);
        }
    }
}

/// `CaptureNotificationInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct CaptureNotificationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub notification: String,
    pub text_notification: Option<String>,
    pub new_owner_voice: bool,
    pub lose_notification: Option<String>,
    pub lose_text_notification: Option<String>,
    pub lose_new_owner_voice: bool,
}

impl Default for CaptureNotificationInfo {
    fn default() -> Self {
        CaptureNotificationInfo {
            instance_name: None,
            notification: "BuildingCaptured".into(),
            text_notification: None,
            new_owner_voice: true,
            lose_notification: None,
            lose_text_notification: None,
            lose_new_owner_voice: false,
        }
    }
}

/// `CaptureNotification`.
pub struct CaptureNotification {
    info: Rc<CaptureNotificationInfo>,
}

register_traits!(CaptureNotification: INotifyCapture);
info_impl!(CaptureNotificationInfo => |info, _init| Some(Rc::new(CaptureNotification { info })));

impl INotifyCapture for CaptureNotification {
    fn on_capture(
        &self,
        self_: &ActorRef,
        _captor: &ActorRef,
        old_owner: &PlayerRef,
        new_owner: &PlayerRef,
        _capture_types: BitSet<crate::capture::CaptureType>,
    ) {
        let world = self_.world();
        let faction = if self.info.new_owner_voice {
            &new_owner.faction
        } else {
            &old_owner.faction
        };
        sound::play_notification(&world, Some(new_owner), "Speech", &self.info.notification, Some(faction));
        world
            .sound
            .add_transient_line(Some(new_owner), self.info.text_notification.as_deref());
        let lose_faction = if self.info.lose_new_owner_voice {
            &new_owner.faction
        } else {
            &old_owner.faction
        };
        if let Some(lose) = &self.info.lose_notification {
            sound::play_notification(&world, Some(old_owner), "Speech", lose, Some(lose_faction));
        }
        world
            .sound
            .add_transient_line(Some(old_owner), self.info.lose_text_notification.as_deref());
    }
}

/// `DeathSoundsInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct DeathSoundsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub voice: String,
    pub volume_multiplier: f32,
    pub death_types: BitSet<DamageType>,
}

impl Default for DeathSoundsInfo {
    fn default() -> Self {
        DeathSoundsInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            voice: "Die".into(),
            volume_multiplier: 1.0,
            death_types: BitSet::default(),
        }
    }
}

/// `DeathSounds`.
pub struct DeathSounds {
    info: Rc<DeathSoundsInfo>,
    state: ConditionalState,
}

conditional_simple!(DeathSounds);
register_traits!(DeathSounds: INotifyKilled, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(DeathSoundsInfo => |info, _init| Some(Rc::new(DeathSounds {
    state: ConditionalState::new(&info.base),
    info,
})));

impl INotifyKilled for DeathSounds {
    fn killed(&self, self_: &ActorRef, e: &AttackInfo) {
        if self.state.is_trait_disabled() {
            return;
        }
        if self.info.death_types.is_empty() || e.damage.damage_types.overlaps(&self.info.death_types) {
            sound::play_voice_local(self_, &self.info.voice, self.info.volume_multiplier);
        }
    }
}

/// `SoundOnDamageTransitionInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct SoundOnDamageTransitionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub damaged_sounds: Vec<String>,
    pub destroyed_sounds: Vec<String>,
    pub damage_types: BitSet<DamageType>,
}

/// `SoundOnDamageTransition`.
pub struct SoundOnDamageTransition {
    info: Rc<SoundOnDamageTransitionInfo>,
}

register_traits!(SoundOnDamageTransition: INotifyDamageStateChanged);
info_impl!(SoundOnDamageTransitionInfo => |info, _init| Some(Rc::new(SoundOnDamageTransition { info })));

impl INotifyDamageStateChanged for SoundOnDamageTransition {
    fn damage_state_changed(&self, self_: &ActorRef, e: &AttackInfo) {
        if !self.info.damage_types.is_empty() && !e.damage.damage_types.overlaps(&self.info.damage_types) {
            return;
        }
        let world = self_.world();
        if e.damage_state == DamageState::DEAD {
            sound::play_random_at(&world, SoundType::World, &self.info.destroyed_sounds, self_.center_position(), 1.0);
        } else if e.damage_state >= DamageState::HEAVY && e.previous_damage_state < DamageState::HEAVY {
            sound::play_random_at(&world, SoundType::World, &self.info.damaged_sounds, self_.center_position(), 1.0);
        }
    }
}

/// `VoiceAnnouncementInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct VoiceAnnouncementInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub voice: String,
    pub valid_relationships: PlayerRelationship,
    pub play_to_owner: bool,
    pub one_shot: bool,
}

impl Default for VoiceAnnouncementInfo {
    fn default() -> Self {
        VoiceAnnouncementInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            voice: String::new(),
            valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
            play_to_owner: true,
            one_shot: false,
        }
    }
}

/// `VoiceAnnouncement`.
pub struct VoiceAnnouncement {
    info: Rc<VoiceAnnouncementInfo>,
    state: ConditionalState,
    triggered: Cell<bool>,
}

register_traits!(VoiceAnnouncement: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(VoiceAnnouncementInfo => |info, _init| Some(Rc::new(VoiceAnnouncement {
    state: ConditionalState::new(&info.base),
    info,
    triggered: Cell::new(false),
})));
conditional_plumbing!(VoiceAnnouncement);

impl INotifyCreated for VoiceAnnouncement {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ConditionalHooks for VoiceAnnouncement {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_enabled(&self, self_: &ActorRef) {
        if self.info.one_shot && self.triggered.get() {
            return;
        }
        self.triggered.set(true);
        let Some(player) = self_.world().sound.local_player() else {
            return;
        };
        let owner = self_.owner();
        let valid = self
            .info
            .valid_relationships
            .has_relationship(owner.relationship_with(Some(&player)));
        if valid || (self.info.play_to_owner && Rc::ptr_eq(&owner, &player)) {
            sound::play_voice(self_, &self.info.voice);
        }
    }
}

/// Real-time intervals of `Game.RunTime` for notifications. The first check after creation is always due.
#[derive(Default)]
struct AdviceTimer(Cell<Option<Instant>>);

impl AdviceTimer {
    fn due(&self, interval_ms: i32) -> bool {
        self.0.get().is_none_or(|t| t.elapsed().as_millis() > interval_ms as u128)
    }

    fn restart(&self) {
        self.0.set(Some(Instant::now()));
    }
}

/// `BaseAttackNotifierInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct BaseAttackNotifierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub notify_interval: i32,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
    pub ally_notification: Option<String>,
    pub ally_text_notification: Option<String>,
}

impl Default for BaseAttackNotifierInfo {
    fn default() -> Self {
        BaseAttackNotifierInfo {
            instance_name: None,
            notify_interval: 30000,
            notification: Some("BaseAttack".into()),
            text_notification: None,
            ally_notification: None,
            ally_text_notification: None,
        }
    }
}

/// `BaseAttackNotifier`, without the radar ping.
pub struct BaseAttackNotifier {
    info: Rc<BaseAttackNotifierInfo>,
    last_attack: AdviceTimer,
}

register_traits!(BaseAttackNotifier: INotifyDamage);
info_impl!(BaseAttackNotifierInfo => |info, _init| Some(Rc::new(BaseAttackNotifier { info, last_attack: AdviceTimer::default() })));

impl INotifyDamage for BaseAttackNotifier {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        let world = self_.world();
        let Some(local_player) = world.sound.local_player().filter(|p| !p.spectating()) else {
            return;
        };
        let Some(attacker) = &e.attacker else { return };
        let (owner, attacker_owner) = (self_.owner(), attacker.owner());
        if Rc::ptr_eq(&attacker_owner, &owner) || Rc::ptr_eq(attacker, world.world_actor()) {
            return;
        }
        if !self_.info.has_trait_info::<crate::building::BuildingInfo>() {
            return;
        }
        if attacker_owner.is_allied_with(Some(&owner)) && e.damage.value <= 0 {
            return;
        }
        if !self.last_attack.due(self.info.notify_interval) {
            return;
        }
        if Rc::ptr_eq(&owner, &local_player) {
            sound::notify_owner(
                self_,
                "Speech",
                self.info.notification.as_deref(),
                self.info.text_notification.as_deref(),
            );
        } else if local_player.is_allied_with(Some(&owner)) && !Rc::ptr_eq(&local_player, &attacker_owner) {
            if let Some(n) = &self.info.ally_notification {
                sound::play_notification(&world, Some(&local_player), "Speech", n, Some(&local_player.faction));
            }
            world
                .sound
                .add_transient_line(Some(&local_player), self.info.ally_text_notification.as_deref());
        }
        self.last_attack.restart();
    }
}

/// `HarvesterAttackNotifierInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct HarvesterAttackNotifierInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub notify_interval: i32,
    pub exclude_damage_types: BitSet<DamageType>,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
}

impl Default for HarvesterAttackNotifierInfo {
    fn default() -> Self {
        HarvesterAttackNotifierInfo {
            instance_name: None,
            notify_interval: 30000,
            exclude_damage_types: BitSet::default(),
            notification: Some("HarvesterAttack".into()),
            text_notification: None,
        }
    }
}

/// `HarvesterAttackNotifier`, without the radar ping.
pub struct HarvesterAttackNotifier {
    info: Rc<HarvesterAttackNotifierInfo>,
    last_attack: AdviceTimer,
}

register_traits!(HarvesterAttackNotifier: INotifyDamage);
info_impl!(HarvesterAttackNotifierInfo => |info, _init| Some(Rc::new(HarvesterAttackNotifier { info, last_attack: AdviceTimer::default() })));

impl INotifyDamage for HarvesterAttackNotifier {
    fn damaged(&self, self_: &ActorRef, e: &AttackInfo) {
        if !self.info.exclude_damage_types.is_empty() && e.damage.damage_types.overlaps(&self.info.exclude_damage_types) {
            return;
        }
        if e.attacker.as_ref().is_some_and(|a| Rc::ptr_eq(&a.owner(), &self_.owner())) || e.damage.value < 0 {
            return;
        }
        if !self_.info.has_trait_info::<crate::harvester::HarvesterInfo>() || !self.last_attack.due(self.info.notify_interval) {
            return;
        }
        sound::notify_owner(
            self_,
            "Speech",
            self.info.notification.as_deref(),
            self.info.text_notification.as_deref(),
        );
        self.last_attack.restart();
    }
}

/// `ResourceStorageWarningInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ResourceStorageWarningInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub advice_interval: i32,
    pub threshold: i32,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
}

impl Default for ResourceStorageWarningInfo {
    fn default() -> Self {
        ResourceStorageWarningInfo {
            instance_name: None,
            advice_interval: 20000,
            threshold: 80,
            notification: Some("SilosNeeded".into()),
            text_notification: None,
        }
    }
}

/// `ResourceStorageWarning`.
pub struct ResourceStorageWarning {
    info: Rc<ResourceStorageWarningInfo>,
    last_advice: AdviceTimer,
}

register_traits!(ResourceStorageWarning: ITick);
info_impl!(ResourceStorageWarningInfo => |info, _init| Some(Rc::new(ResourceStorageWarning { info, last_advice: AdviceTimer::default() })));

impl ITick for ResourceStorageWarning {
    fn tick(&self, self_: &ActorRef) {
        if !self_.world().sound.enabled() || !self.last_advice.due(self.info.advice_interval) {
            return;
        }
        let resources = self_.trait_::<crate::player::PlayerResources>();
        if resources.resources.get() > self.info.threshold * resources.resource_capacity.get() / 100 {
            sound::notify_owner(
                self_,
                "Speech",
                self.info.notification.as_deref(),
                self.info.text_notification.as_deref(),
            );
        }
        self.last_advice.restart();
    }
}

/// `INotifyDiscovered`.
pub trait INotifyDiscovered {
    fn on_discovered(&self, self_: &ActorRef, discoverer: &PlayerRef, play_notification: bool);
}

#[derive(Clone, Debug, FieldLoad)]
pub struct EnemyWatcherInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub scan_interval: i32,
    pub notification_interval: i32,
}

impl Default for EnemyWatcherInfo {
    fn default() -> Self {
        EnemyWatcherInfo {
            instance_name: None,
            scan_interval: 25,
            notification_interval: 750,
        }
    }
}

pub struct EnemyWatcher {
    info: Rc<EnemyWatcherInfo>,
    discovered_players: RefCell<Vec<PlayerRef>>,
    rescan_interval: Cell<i32>,
    ticks_before_next_notification: Cell<i32>,
    last_known_actor_ids: RefCell<HashSet<u32>>,
}

register_traits!(EnemyWatcher: ITick);
info_impl!(EnemyWatcherInfo => |info, _init| Some(Rc::new(EnemyWatcher {
    info,
    discovered_players: RefCell::new(Vec::new()),
    rescan_interval: Cell::new(0),
    ticks_before_next_notification: Cell::new(0),
    last_known_actor_ids: RefCell::new(HashSet::new()),
})));

impl ITick for EnemyWatcher {
    fn tick(&self, self_: &ActorRef) {
        let owner = self_.owner();
        if owner.shroud().disabled() || owner.is_bot || !owner.playable || owner.spectating() {
            return;
        }
        self.rescan_interval.set(self.rescan_interval.get() - 1);
        self.ticks_before_next_notification
            .set(self.ticks_before_next_notification.get() - 1);
        if self.rescan_interval.get() > 0 {
            return;
        }
        self.rescan_interval.set(self.info.scan_interval);
        let mut announced_any = false;
        let mut visible = HashSet::new();
        let mut played = HashSet::new();
        for (actor, announce) in self_.world().actors_with_trait::<AnnounceOnSeen>() {
            if crate::util::appears_friendly_to(&actor, self_)
                || actor.is_dead()
                || !actor.is_in_world()
                || !actor.can_be_viewed_by_player(Some(&owner))
            {
                continue;
            }
            visible.insert(actor.id);
            if self.last_known_actor_ids.borrow().contains(&actor.id) {
                continue;
            }
            let id = (announce.info.notification.clone(), announce.info.text_notification.clone());
            let play = !played.contains(&id) && self.ticks_before_next_notification.get() <= 0;
            for t in actor.traits_implementing::<dyn INotifyDiscovered>() {
                t.on_discovered(&actor, &owner, play);
            }
            let discovered = actor.owner();
            if !self.discovered_players.borrow().iter().any(|p| Rc::ptr_eq(p, &discovered)) {
                for t in discovered.player_actor().traits_implementing::<dyn INotifyDiscovered>() {
                    t.on_discovered(&actor, &owner, false);
                }
                self.discovered_players.borrow_mut().push(discovered);
            }
            if play {
                played.insert(id);
                announced_any = true;
            }
        }
        if announced_any {
            self.ticks_before_next_notification.set(self.info.notification_interval);
        }
        *self.last_known_actor_ids.borrow_mut() = visible;
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct AnnounceOnSeenInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub notification: Option<String>,
    pub text_notification: Option<String>,
    pub announce_neutrals: bool,
}

pub struct AnnounceOnSeen {
    info: Rc<AnnounceOnSeenInfo>,
}

register_traits!(AnnounceOnSeen: INotifyDiscovered);
info_impl!(AnnounceOnSeenInfo => |info, _init| Some(Rc::new(AnnounceOnSeen { info })));

impl INotifyDiscovered for AnnounceOnSeen {
    fn on_discovered(&self, self_: &ActorRef, discoverer: &PlayerRef, play_notification: bool) {
        let world = self_.world();
        let is_render_player = world.sound.local_player().is_some_and(|p| Rc::ptr_eq(&p, discoverer));
        if !play_notification || !is_render_player {
            return;
        }
        if !self.info.announce_neutrals && !crate::util::appears_hostile_to(self_, discoverer.player_actor()) {
            return;
        }
        if let Some(n) = self.info.notification.as_deref() {
            sound::play_notification(&world, Some(discoverer), "Speech", n, Some(&discoverer.faction));
        }
        world
            .sound
            .add_transient_line(Some(discoverer), self.info.text_notification.as_deref());
    }
}
