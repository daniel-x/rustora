// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/SupportPowers`: `SupportPowerManager.cs`, `SupportPower.cs`, `DirectionalSupportPower.cs`,
//! `AirstrikePower.cs` and `ParatroopersPower.cs`. The subclasses are one [`SupportPower`] struct with a
//! [`PowerKind`]. Beacons, radar pings, sounds and order generators are left out.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::world_types::{CPos, CVec, WAngle, WDist, WRot, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::TraitNode;
use rustora_engine::actor_init::{ActorInits, CenterPositionInit, FacingInit, LocationInit, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::net_collections::NetMap;
use rustora_engine::order::Order;
use rustora_engine::player::WinState;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{self, SoundType};
use rustora_engine::target::{Target, TargetType};
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::activities::Wait;
use crate::actor_map::WorldExt;
use crate::aircraft::{AircraftInfo, Fly, RemoveSelf};
use crate::attack::AttackBase;
use crate::base::DeveloperMode;
use crate::cargo::{Cargo, ParaDrop};
use crate::gps::{GpsWatcher, ProvidesRadar, SatelliteLaunch};
use crate::interfaces::*;
use crate::production::TechTree;

pub fn register(r: &mut TraitRegistry) {
    r.register("SupportPowerManager", |n| load_info!(SupportPowerManagerInfo, n));
    r.register("AirstrikePower", |n| load_power(n, PowerKind::Airstrike));
    r.register("ParatroopersPower", |n| load_power(n, PowerKind::Paratroopers));
    r.register("GpsPower", |n| load_power(n, PowerKind::Gps));
    r.register("NukePower", |n| load_power(n, PowerKind::Nuke));
    r.register("GrantExternalConditionPower", |n| load_power(n, PowerKind::GrantExternalCondition));
    r.register("SpawnActorPower", |n| load_power(n, PowerKind::SpawnActor));
    r.register("ChronoshiftPower", |n| load_power(n, PowerKind::Chronoshift));
}

pub trait INotifySupportPower {
    fn charged(&self, self_: &ActorRef);
    fn activated(&self, self_: &ActorRef);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerKind {
    #[default]
    Airstrike,
    Paratroopers,
    Gps,
    Nuke,
    GrantExternalCondition,
    SpawnActor,
    Chronoshift,
}

impl PowerKind {
    pub fn class_name(self) -> &'static str {
        match self {
            PowerKind::Airstrike => "AirstrikePower",
            PowerKind::Paratroopers => "ParatroopersPower",
            PowerKind::Gps => "GpsPower",
            PowerKind::Nuke => "NukePower",
            PowerKind::GrantExternalCondition => "GrantExternalConditionPower",
            PowerKind::SpawnActor => "SpawnActorPower",
            PowerKind::Chronoshift => "ChronoshiftPower",
        }
    }
}

fn load_power(node: &TraitNode, kind: PowerKind) -> Result<Rc<dyn rustora_engine::traits::TraitInfo>, String> {
    let mut info = SupportPowerInfo::default();
    match kind {
        PowerKind::Airstrike => {
            info.unit_type = "badr.bomber".into();
            info.camera_remove_delay = 25;
        }
        PowerKind::Paratroopers => {
            info.unit_type = "badr".into();
            info.camera_remove_delay = 85;
        }
        PowerKind::Gps => {}
        PowerKind::Nuke => info.camera_remove_delay = 25,
        PowerKind::GrantExternalCondition | PowerKind::SpawnActor => {}
        PowerKind::Chronoshift => info.duration = 750,
    }
    info.order_name = format!("{}InfoOrder", kind.class_name());
    rustora_core::fieldloader::FieldLoad::load(&mut info, &node.yaml).map_err(|e| format!("Trait name {}: {}", node.display_name(), e))?;
    info.instance_name = node.instance_name.clone();
    info.kind = kind;
    Ok(Rc::new(info))
}

/// Union of `SupportPowerInfo` and the info fields of its subclasses.
#[derive(Clone, Debug, FieldLoad)]
pub struct SupportPowerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub display_radar_ping: bool,
    pub radar_ping_duration: i32,
    pub detected_sound: Option<String>,
    pub detected_speech_notification: Option<String>,
    pub detected_text_notification: Option<String>,
    pub begin_charge_sound: Option<String>,
    pub begin_charge_speech_notification: Option<String>,
    pub begin_charge_text_notification: Option<String>,
    pub end_charge_sound: Option<String>,
    pub end_charge_speech_notification: Option<String>,
    pub end_charge_text_notification: Option<String>,
    pub select_target_sound: Option<String>,
    pub select_target_speech_notification: Option<String>,
    pub select_target_text_notification: Option<String>,
    pub launch_sound: Option<String>,
    pub launch_speech_notification: Option<String>,
    pub launch_text_notification: Option<String>,
    pub incoming_sound: Option<String>,
    pub incoming_speech_notification: Option<String>,
    pub incoming_text_notification: Option<String>,
    #[field(ignore)]
    pub kind: PowerKind,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    pub name: Option<String>,
    pub description: Option<String>,
    pub charge_interval: i32,
    pub allow_multiple: bool,
    pub one_shot: bool,
    pub start_fully_charged: bool,
    pub prerequisites: Vec<String>,
    pub order_name: String,
    pub icon_image: String,
    pub icon: Option<String>,
    pub icon_palette: String,
    pub cursor: String,
    pub blocked_cursor: String,
    pub support_power_palette_order: i32,
    // DirectionalSupportPower
    pub use_directional_target: bool,
    // AirstrikePower, ParatroopersPower
    pub unit_type: String,
    pub squad_size: i32,
    pub squad_offset: WVec,
    pub quantized_facings: i32,
    pub cordon: WDist,
    pub camera_actor: Option<String>,
    pub camera_remove_delay: i32,
    // ParatroopersPower
    pub drop_items: Vec<String>,
    pub allow_impassable_cells: bool,
    // GpsPower
    pub reveal_delay: i32,
    pub door_image: String,
    pub door_sequence: String,
    pub requires_active_radar: bool,
    // NukePower
    pub missile_weapon: String,
    pub missile_delay: i32,
    pub spawn_offset: WVec,
    pub detonation_altitude: WDist,
    pub remove_missile_on_detonation: bool,
    pub flight_delay: i32,
    pub flight_velocity: WDist,
    pub skip_ascent: bool,
    pub camera_range: WDist,
    pub reveal_generated_shroud: bool,
    pub camera_relationships: rustora_engine::player::PlayerRelationship,
    pub camera_spawn_advance: i32,
    pub trail_image: Option<String>,
    pub trail_sequences: Vec<String>,
    pub trail_interval: i32,
    pub trail_delay: i32,
    pub condition: Option<String>,
    pub duration: i32,
    pub dimensions: CVec,
    pub footprint: String,
    pub on_fire_sound: Option<String>,
    pub valid_relationships: rustora_engine::player::PlayerRelationship,
    pub sequence: String,
    pub footprint_image: String,
    pub footprint_sequence: String,
    pub actor: Option<String>,
    pub life_time: i32,
    pub terrain: Option<Vec<String>>,
    pub allow_under_shroud: bool,
    pub deploy_sound: Option<String>,
    pub effect_image: Option<String>,
    pub effect_sequence: Option<String>,
    pub effect_palette: Option<String>,
    pub effect_palette_is_player_palette: bool,
    pub target_overlay_palette: String,
    pub valid_footprint_sequence: String,
    pub invalid_footprint_sequence: String,
    pub source_footprint_sequence: String,
    pub kill_cargo: bool,
    pub selection_cursor: String,
    pub target_cursor: String,
    pub target_blocked_cursor: String,
}

impl Default for SupportPowerInfo {
    fn default() -> Self {
        SupportPowerInfo {
            display_radar_ping: false,
            radar_ping_duration: 125,
            detected_sound: None,
            detected_speech_notification: None,
            detected_text_notification: None,
            begin_charge_sound: None,
            begin_charge_speech_notification: None,
            begin_charge_text_notification: None,
            end_charge_sound: None,
            end_charge_speech_notification: None,
            end_charge_text_notification: None,
            select_target_sound: None,
            select_target_speech_notification: None,
            select_target_text_notification: None,
            launch_sound: None,
            launch_speech_notification: None,
            launch_text_notification: None,
            incoming_sound: None,
            incoming_speech_notification: None,
            incoming_text_notification: None,
            instance_name: None,
            kind: PowerKind::default(),
            base: PausableConditionalTraitInfo::default(),
            name: None,
            description: None,
            charge_interval: 0,
            icon_image: "icon".into(),
            icon: None,
            icon_palette: "chrome".into(),
            cursor: "ability".into(),
            blocked_cursor: "generic-blocked".into(),
            support_power_palette_order: 9999,
            allow_multiple: false,
            one_shot: false,
            start_fully_charged: false,
            prerequisites: Vec::new(),
            order_name: String::new(),
            use_directional_target: false,
            unit_type: String::new(),
            squad_size: 1,
            squad_offset: WVec::new(-1536, 1536, 0),
            quantized_facings: 32,
            cordon: WDist::new(5120),
            camera_actor: None,
            camera_remove_delay: 0,
            drop_items: Vec::new(),
            allow_impassable_cells: false,
            reveal_delay: 0,
            door_image: "atek".into(),
            door_sequence: "active".into(),
            requires_active_radar: true,
            missile_weapon: String::new(),
            missile_delay: 0,
            spawn_offset: WVec::ZERO,
            detonation_altitude: WDist::ZERO,
            remove_missile_on_detonation: true,
            flight_delay: 400,
            flight_velocity: WDist::new(512),
            skip_ascent: false,
            camera_range: WDist::ZERO,
            reveal_generated_shroud: true,
            camera_relationships: rustora_engine::player::PlayerRelationship::ALLY,
            camera_spawn_advance: 25,
            trail_image: None,
            trail_sequences: Vec::new(),
            trail_interval: 1,
            trail_delay: 1,
            condition: None,
            duration: 0,
            dimensions: CVec::ZERO,
            footprint: String::new(),
            on_fire_sound: None,
            valid_relationships: rustora_engine::player::PlayerRelationship::ALLY,
            sequence: "active".into(),
            footprint_image: "overlay".into(),
            footprint_sequence: "target-select".into(),
            actor: None,
            life_time: 250,
            terrain: None,
            allow_under_shroud: true,
            deploy_sound: None,
            effect_image: None,
            effect_sequence: None,
            effect_palette: None,
            effect_palette_is_player_palette: false,
            target_overlay_palette: "terrain".into(),
            valid_footprint_sequence: "target-valid".into(),
            invalid_footprint_sequence: "target-invalid".into(),
            source_footprint_sequence: "target-select".into(),
            kill_cargo: true,
            selection_cursor: "chrono-select".into(),
            target_cursor: "chrono-target".into(),
            target_blocked_cursor: "move-blocked".into(),
        }
    }
}

pub struct SupportPower {
    pub info: Rc<SupportPowerInfo>,
    state: ConditionalState,
    pub self_: ActorRef,
    gps_owner: RefCell<Option<Rc<GpsWatcher>>>,
    gps_was_paused: Cell<bool>,
}

impl rustora_engine::traits::TraitObject for SupportPower {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<SupportPower>(actor, self.clone());
        dict.add::<dyn INotifyCreated>(actor, self.clone());
        dict.add::<dyn IObservesVariables>(actor, self.clone());
        dict.add::<dyn IDisabledTrait>(actor, self.clone());
        if self.info.kind == PowerKind::Gps {
            dict.add::<dyn INotifyKilled>(actor, self.clone());
            dict.add::<dyn INotifySold>(actor, self.clone());
            dict.add::<dyn INotifyOwnerChanged>(actor, self.clone());
            dict.add::<dyn ITick>(actor, self.clone());
        }
        dict.add::<dyn ISync>(actor, self);
    }

    fn type_name(&self) -> &'static str {
        self.info.kind.class_name()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

info_impl!(SupportPowerInfo => |info, init| {
    let state = ConditionalState::new_pausable(&info.base);
    let gps_owner = (info.kind == PowerKind::Gps).then(|| init.self_.owner().player_actor().trait_::<GpsWatcher>());
    if let Some(g) = &gps_owner {
        g.gps_add(&init.self_);
    }
    Some(Rc::new(SupportPower { info, state, self_: init.self_.clone(), gps_owner: RefCell::new(gps_owner), gps_was_paused: Cell::new(false) }))
});

rustora_engine::conditional_simple!(SupportPower);

impl SupportPower {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    fn key(&self) -> String {
        if self.info.allow_multiple {
            format!("{}_{}", self.info.order_name, self.self_.id)
        } else {
            self.info.order_name.clone()
        }
    }

    /// The sound, speech and text calls that `Charging`, `Charged` and `SupportPowerInstance.Target` share.
    pub fn notify_power_event(&self, sound: &Option<String>, speech: &Option<String>, text: &Option<String>) {
        let world = self.self_.world();
        if let Some(s) = sound {
            sound::play_to_player(&world, SoundType::Ui, &self.self_.owner(), s);
        }
        sound::notify_owner(&self.self_, "Speech", speech.as_deref(), text.as_deref());
    }

    /// `PlayLaunchSounds`.
    fn play_launch_sounds(&self) {
        let world = self.self_.world();
        let Some(local_player) = world.sound.local_player().filter(|p| !p.spectating()) else {
            return;
        };
        let info = &self.info;
        let allied = self.self_.owner().is_allied_with(Some(&local_player));
        let (launch, speech, text) = if allied {
            (&info.launch_sound, &info.launch_speech_notification, &info.launch_text_notification)
        } else {
            (
                &info.incoming_sound,
                &info.incoming_speech_notification,
                &info.incoming_text_notification,
            )
        };
        if let Some(s) = launch {
            sound::play(&world, SoundType::Ui, s, 1.0);
        }
        if let Some(n) = speech {
            sound::play_notification(&world, Some(&local_player), "Speech", n, Some(&local_player.faction));
        }
        world.sound.add_transient_line(Some(&local_player), text.as_deref());
    }

    fn charged(&self, self_: &ActorRef, key: &str) {
        self.notify_power_event(
            &self.info.end_charge_sound,
            &self.info.end_charge_speech_notification,
            &self.info.end_charge_text_notification,
        );
        if self.info.kind == PowerKind::Gps {
            let spm = self_.owner().player_actor().trait_::<SupportPowerManager>();
            let p = spm.powers.borrow().get(&key.to_string()).cloned().expect("power");
            p.activate(&Order::new("", None, Target::Invalid, false));
            return;
        }
        for n in self_.traits_implementing::<dyn INotifySupportPower>() {
            n.charged(self_);
        }
    }

    /// `SupportPower.CellsMatching` for this power's footprint.
    pub fn cells_matching(&self, location: CPos) -> Vec<CPos> {
        let dimensions = self.info.dimensions;
        let footprint: Vec<char> = self.info.footprint.chars().filter(|c| !c.is_whitespace()).collect();
        let (x, y) = (location.x() - (dimensions.x - 1) / 2, location.y() - (dimensions.y - 1) / 2);
        let mut cells = Vec::new();
        let mut index = 0;
        for j in 0..dimensions.y {
            for i in 0..dimensions.x {
                if footprint[index] == 'x' {
                    cells.push(CPos::new(x + i, y + j));
                }
                index += 1;
            }
        }
        cells
    }

    fn can_grant(&self, a: &ActorRef) -> bool {
        let source = self.self_.id as usize;
        a.traits_implementing::<crate::experience::ExternalCondition>()
            .iter()
            .any(|t| Some(&t.info.condition) == self.info.condition.as_ref() && t.can_grant_condition(source))
    }

    /// `GrantExternalConditionPower.UnitsInRange`.
    pub fn units_in_range(&self, xy: CPos) -> Vec<ActorRef> {
        let world = self.self_.world();
        let mut units: Vec<ActorRef> = Vec::new();
        for t in self.cells_matching(xy) {
            for a in world.actor_map().get_actors_at(t) {
                if !units.iter().any(|u| u.id == a.id) {
                    units.push(a);
                }
            }
        }
        let owner = self.self_.owner();
        units.retain(|a| {
            self.info
                .valid_relationships
                .has_relationship(owner.relationship_with(Some(&a.owner())))
                && self.can_grant(a)
        });
        units
    }

    fn grant_external_condition(&self, self_: &ActorRef, order: &Order) {
        if let Some(wsb) = self_.trait_or_default::<crate::render::WithSpriteBody>()
            && wsb.default_animation.has_sequence(&self.info.sequence)
        {
            wsb.play_custom_animation(self_, &self.info.sequence, None);
        }
        let world = self_.world();
        let pos = order.target.center_position();
        if let Some(s) = &self.info.on_fire_sound {
            sound::play_at(&world, SoundType::World, s, pos, 1.0);
        }
        let source = self_.id as usize;
        for a in self.units_in_range(world.map().cell_containing(pos)) {
            let external = a
                .traits_implementing::<crate::experience::ExternalCondition>()
                .into_iter()
                .find(|t| Some(&t.info.condition) == self.info.condition.as_ref() && t.can_grant_condition(source));
            if let Some(e) = external {
                e.grant_condition(&a, source, self.info.duration, 0);
            }
        }
    }

    /// `ChronoshiftPower.UnitsInRange`.
    pub fn chronoshift_units_in_range(&self, xy: CPos) -> Vec<ActorRef> {
        let world = self.self_.world();
        let mut units: Vec<ActorRef> = Vec::new();
        for t in self.cells_matching(xy) {
            for a in world.actor_map().get_actors_at(t) {
                if !units.iter().any(|u| u.id == a.id) {
                    units.push(a);
                }
            }
        }
        units.retain(|a| {
            a.traits_implementing::<crate::chrono::Chronoshiftable>()
                .iter()
                .any(|c| !c.is_trait_disabled())
        });
        units
    }

    fn chronoshift(&self, self_: &ActorRef, order: &Order) {
        let world = self_.world();
        let target_delta = world.map().cell_containing(order.target.center_position()) - order.extra_location;
        let shroud = self_.owner().shroud();
        for target in self.chronoshift_units_in_range(order.extra_location) {
            let cs = target
                .traits_implementing::<crate::chrono::Chronoshiftable>()
                .into_iter()
                .find(|c| !c.is_trait_disabled());
            let Some(cs) = cs else {
                continue;
            };
            let target_cell = target.location() + target_delta;
            if shroud.is_explored(target_cell) && cs.can_chronoshift_to(target_cell) {
                cs.teleport(target_cell, self.info.duration, self.info.kill_cargo, self_);
            }
        }
    }

    /// `SpawnActorPower.Validate`; the shroud check of C# looks at the render player, so it can't affect the simulation.
    pub fn validate_spawn(&self, world: &World, cell: CPos) -> bool {
        world.map().contains(cell)
            && self
                .info
                .terrain
                .as_ref()
                .is_none_or(|t| t.contains(&world.map().terrain_type_info(cell).ty))
    }

    fn spawn_actor(&self, self_: &ActorRef, pos: rustora_core::world_types::WPos, cell: CPos) {
        let (info, owner) = (self.info.clone(), self_.owner());
        self_.world().add_frame_end_task(Box::new(move |w| {
            if let Some(s) = &info.deploy_sound {
                sound::play_at(w, SoundType::World, s, pos, 1.0);
            }
            if let (Some(sequence), Some(palette)) = (info.effect_sequence.as_deref(), info.effect_palette.as_deref())
                && !sequence.is_empty()
                && !palette.is_empty()
            {
                let palette = rustora_engine::render::PaletteRef::resolve(Some(palette), info.effect_palette_is_player_palette, &owner);
                let image = info.effect_image.as_deref().unwrap_or_default();
                w.add_effect(crate::effects::SpriteEffect::at(w, pos, WAngle::ZERO, image, sequence, palette));
            }
            let actor = w.create_actor(
                info.actor.as_deref().expect("SpawnActorPower requires Actor"),
                ActorInits::new().with(LocationInit::new(cell)).with(OwnerInit::from_player(&owner)),
            );
            if info.life_time > -1 {
                actor.queue_activity(Wait::new(info.life_time));
                actor.queue_activity(RemoveSelf::new());
            }
        }));
    }

    fn activate(&self, self_: &ActorRef, order: &Order) {
        let spawn_at = (self.info.kind == PowerKind::SpawnActor).then(|| order.target.center_position());
        let spawn_cell = spawn_at.map(|p| self_.world().map().cell_containing(p));
        if let Some(cell) = spawn_cell
            && !self.validate_spawn(&self_.world(), cell)
        {
            return;
        }
        if self.info.display_radar_ping
            && let Some(player) = order.player()
        {
            crate::radar::add_radar_ping(
                &self_.world(),
                crate::radar::PingVisibility::AlliesOf(player.clone()),
                order.target.center_position(),
                player.color,
                self.info.radar_ping_duration,
            );
        }
        for n in self_.traits_implementing::<dyn INotifySupportPower>() {
            n.activated(self_);
        }
        self.play_launch_sounds();
        match self.info.kind {
            PowerKind::Airstrike => {
                let facing = self.directional_facing(order);
                self.send_airstrike(self_, order.target.center_position(), facing);
            }
            PowerKind::Paratroopers => {
                let facing = self.directional_facing(order);
                self.send_paratroopers(self_, order.target.center_position(), facing);
            }
            PowerKind::Nuke => self.launch_nuke(self_, order.target.center_position()),
            PowerKind::GrantExternalCondition => self.grant_external_condition(self_, order),
            PowerKind::Chronoshift => self.chronoshift(self_, order),
            PowerKind::SpawnActor => self.spawn_actor(self_, spawn_at.expect("set for this kind"), spawn_cell.expect("set for this kind")),
            PowerKind::Gps => {
                let (a, info) = (self_.clone(), self.info.clone());
                self_.world().add_frame_end_task(Box::new(move |w| {
                    w.add_effect(SatelliteLaunch::new(
                        w,
                        &a,
                        &info.door_image,
                        &info.door_sequence,
                        info.reveal_delay,
                    ));
                }));
            }
        }
    }

    fn directional_facing(&self, order: &Order) -> Option<WAngle> {
        (self.info.use_directional_target && order.extra_data != u32::MAX).then(|| WAngle::from_facing(order.extra_data as i32))
    }

    fn random_facing(&self, w: &World) -> WAngle {
        let q = self.info.quantized_facings;
        WAngle::new(1024 * w.shared_random.borrow_mut().next_max(q) / q)
    }

    /// Start and finish positions of a fly-over, like `SendAirstrike` and `SendParatroopers`.
    fn flight_path(
        &self,
        w: &World,
        target: rustora_core::world_types::WPos,
        facing: WAngle,
    ) -> (
        rustora_core::world_types::WPos,
        rustora_core::world_types::WPos,
        rustora_core::world_types::WPos,
        WRot,
    ) {
        let unit_type = self.info.unit_type.to_lowercase();
        let ai = w
            .rules()
            .actor(&unit_type)
            .unwrap_or_else(|| panic!("Actors ruleset does not include the entry '{unit_type}'"));
        let altitude = ai.trait_info::<AircraftInfo>().expect("AircraftInfo").cruise_altitude.length;
        let rotation = WRot::from_yaw(facing);
        let delta = WVec::new(0, -1024, 0).rotate(&rotation);
        let target = target + WVec::new(0, 0, altitude);
        let start = target - delta * (w.map().distance_to_edge(target, -delta) + self.info.cordon).length / 1024;
        let finish = target + delta * (w.map().distance_to_edge(target, delta) + self.info.cordon).length / 1024;
        (target, start, finish, rotation)
    }

    fn squad_indices(&self) -> Vec<i32> {
        let n = self.info.squad_size;
        (-n / 2..=n / 2).filter(|&i| !(i == 0 && n & 1 == 0)).collect()
    }

    fn send_airstrike(&self, self_: &ActorRef, target: rustora_core::world_types::WPos, facing: Option<WAngle>) -> Vec<ActorRef> {
        let w = self_.world();
        let facing = facing.unwrap_or_else(|| self.random_facing(&w));
        let (target, start_edge, finish_edge, rotation) = self.flight_path(&w, target, facing);
        let tracker = FlyOverTracker::new(self.info.clone(), self_, w.map().cell_containing(target));
        let so = self.info.squad_offset;
        let mut aircraft = Vec::new();
        for i in self.squad_indices() {
            let spawn_offset = WVec::new(i * so.y, -i.abs() * so.x, 0).rotate(&rotation);
            let target_offset = WVec::new(i * so.y, 0, 0).rotate(&rotation);
            let a = w.create_actor_add(
                false,
                &self.info.unit_type,
                ActorInits::new()
                    .with(CenterPositionInit::new(start_edge + spawn_offset))
                    .with(OwnerInit::from_player(&self_.owner()))
                    .with(FacingInit::new(facing)),
            );
            tracker.in_range.borrow_mut().push((a.clone(), false));
            let attack = a
                .traits_implementing::<AttackBase>()
                .into_iter()
                .find(|t| t.info.kind == crate::attack::AttackKind::Bomber)
                .expect("AttackBomber");
            attack.set_bomber_target(target + target_offset);
            let (t1, t2, t3) = (tracker.clone(), tracker.clone(), tracker.clone());
            attack
                .on_entered_attack_range
                .borrow_mut()
                .push(Rc::new(move |a: &ActorRef| t1.on_enter_range(a, false)));
            attack
                .on_exited_attack_range
                .borrow_mut()
                .push(Rc::new(move |a: &ActorRef| t2.on_exit_range(a)));
            attack
                .on_removed_from_world
                .borrow_mut()
                .push(Rc::new(move |a: &ActorRef| t3.on_removed_from_world(a)));
            aircraft.push(a);
        }
        let (list, indices) = (aircraft.clone(), self.squad_indices());
        w.add_frame_end_task(Box::new(move |w| {
            for (a, i) in list.iter().zip(indices) {
                let spawn_offset = WVec::new(i * so.y, -i.abs() * so.x, 0).rotate(&rotation);
                w.add(a);
                a.queue_activity(Fly::new(a, Target::from_pos(target + spawn_offset)));
                a.queue_activity(Fly::new(a, Target::from_pos(finish_edge + spawn_offset)));
                a.queue_activity(RemoveSelf::new());
            }
        }));
        aircraft
    }

    fn send_paratroopers(&self, self_: &ActorRef, target: rustora_core::world_types::WPos, facing: Option<WAngle>) {
        let w = self_.world();
        let facing = facing.unwrap_or_else(|| self.random_facing(&w));
        let (target, start_edge, finish_edge, rotation) = self.flight_path(&w, target, facing);
        let tracker = FlyOverTracker::new(self.info.clone(), self_, w.map().cell_containing(target));
        let so = self.info.squad_offset;
        let owner = self_.owner();
        let mut aircraft = Vec::new();
        for i in self.squad_indices() {
            let spawn_offset = WVec::new(i * so.y, -i.abs() * so.x, 0).rotate(&rotation);
            aircraft.push(
                w.create_actor_add(
                    false,
                    &self.info.unit_type,
                    ActorInits::new()
                        .with(CenterPositionInit::new(start_edge + spawn_offset))
                        .with(OwnerInit::from_player(&owner))
                        .with(FacingInit::new(facing)),
                ),
            );
        }
        let units: Vec<ActorRef> = self
            .info
            .drop_items
            .iter()
            .map(|p| w.create_actor_add(false, &p.to_lowercase(), ActorInits::new().with(OwnerInit::from_player(&owner))))
            .collect();
        let (info, indices) = (self.info.clone(), self.squad_indices());
        w.add_frame_end_task(Box::new(move |w| {
            let per_plane = ((info.drop_items.len() as i32 + info.squad_size - 1) / info.squad_size) as usize;
            let mut added = 0;
            for (a, i) in aircraft.iter().zip(indices) {
                let spawn_offset = WVec::new(i * so.y, -i.abs() * so.x, 0).rotate(&rotation);
                let target_offset = WVec::new(i * so.y, 0, 0).rotate(&rotation);
                w.add(a);
                let drop = a.trait_::<ParaDrop>();
                drop.set_lz(w.map().cell_containing(target + target_offset), !info.allow_impassable_cells);
                let (t1, t2, t3) = (tracker.clone(), tracker.clone(), tracker.clone());
                drop.on_entered_drop_range
                    .borrow_mut()
                    .push(Rc::new(move |a: &ActorRef| t1.on_enter_range(a, true)));
                drop.on_exited_drop_range
                    .borrow_mut()
                    .push(Rc::new(move |a: &ActorRef| t2.on_exit_range(a)));
                drop.on_removed_from_world
                    .borrow_mut()
                    .push(Rc::new(move |a: &ActorRef| t3.on_removed_from_world(a)));
                let cargo = a.trait_::<Cargo>();
                for unit in units.iter().skip(added).take(per_plane) {
                    cargo.load(a, unit);
                    added += 1;
                }
                a.queue_activity(Fly::new(a, Target::from_pos(target + spawn_offset)));
                a.queue_activity(Fly::new(a, Target::from_pos(finish_edge + spawn_offset)));
                a.queue_activity(RemoveSelf::new());
                tracker.in_range.borrow_mut().push((a.clone(), false));
            }
            for u in units.iter().skip(added) {
                u.dispose();
            }
        }));
    }
}

/// The camera bookkeeping closures of `SendAirstrike` and `SendParatroopers`.
struct FlyOverTracker {
    info: Rc<SupportPowerInfo>,
    owner_actor: ActorRef,
    camera_cell: rustora_core::world_types::CPos,
    camera: RefCell<Option<ActorRef>>,
    in_range: RefCell<Vec<(ActorRef, bool)>>,
    me: Weak<FlyOverTracker>,
}

impl FlyOverTracker {
    fn new(info: Rc<SupportPowerInfo>, owner_actor: &ActorRef, camera_cell: rustora_core::world_types::CPos) -> Rc<Self> {
        Rc::new_cyclic(|me| FlyOverTracker {
            info,
            owner_actor: owner_actor.clone(),
            camera_cell,
            camera: RefCell::new(None),
            in_range: RefCell::new(Vec::new()),
            me: me.clone(),
        })
    }

    fn set(&self, a: &ActorRef, v: bool) {
        let mut list = self.in_range.borrow_mut();
        match list.iter_mut().find(|(x, _)| x.id == a.id) {
            Some(e) => e.1 = v,
            None => list.push((a.clone(), v)),
        }
    }

    fn any_in_range(&self) -> bool {
        self.in_range.borrow().iter().any(|(_, v)| *v)
    }

    fn on_enter_range(&self, a: &ActorRef, _paratroopers: bool) {
        if let Some(cam) = self.info.camera_actor.clone()
            && self.camera.borrow().is_none()
            && !self.any_in_range()
        {
            let (me, owner, cell) = (self.me.clone(), self.owner_actor.owner(), self.camera_cell);
            self.owner_actor.world().add_frame_end_task(Box::new(move |w| {
                let c = w.create_actor(
                    &cam,
                    ActorInits::new().with(LocationInit::new(cell)).with(OwnerInit::from_player(&owner)),
                );
                if let Some(t) = me.upgrade() {
                    *t.camera.borrow_mut() = Some(c);
                }
            }));
        }
        self.set(a, true);
    }

    fn on_exit_range(&self, a: &ActorRef) {
        self.set(a, false);
        if !self.any_in_range() {
            self.remove_camera();
        }
    }

    fn on_removed_from_world(&self, a: &ActorRef) {
        self.set(a, false);
        if self.in_range.borrow().iter().all(|(x, _)| !x.is_in_world()) {
            self.remove_camera();
        }
    }

    fn remove_camera(&self) {
        let Some(c) = self.camera.borrow().clone() else { return };
        c.queue_activity(Wait::new(self.info.camera_remove_delay));
        c.queue_activity(RemoveSelf::new());
    }
}

/// `SupportPowerInstance`.
pub struct SupportPowerInstance {
    manager: Weak<SupportPowerManager>,
    pub key: String,
    pub instances: RefCell<Vec<Rc<SupportPower>>>,
    pub total_ticks: i32,
    remaining_sub_ticks: Cell<i32>,
    active: Cell<bool>,
    instances_enabled: Cell<bool>,
    prereqs_available: Cell<bool>,
    one_shot_fired: Cell<bool>,
    notified_charging: Cell<bool>,
    notified_ready: Cell<bool>,
}

impl SupportPowerInstance {
    /// `ResetTimer`.
    pub fn reset_timer(&self) {
        self.remaining_sub_ticks.set(self.total_ticks * 100);
    }

    fn new(key: String, info: &SupportPowerInfo, manager: Weak<SupportPowerManager>) -> Rc<Self> {
        Rc::new(SupportPowerInstance {
            manager,
            key,
            instances: RefCell::new(Vec::new()),
            total_ticks: info.charge_interval,
            remaining_sub_ticks: Cell::new(if info.start_fully_charged { 0 } else { info.charge_interval * 100 }),
            active: Cell::new(false),
            instances_enabled: Cell::new(false),
            prereqs_available: Cell::new(true),
            one_shot_fired: Cell::new(false),
            notified_charging: Cell::new(false),
            notified_ready: Cell::new(false),
        })
    }

    fn manager(&self) -> Rc<SupportPowerManager> {
        self.manager.upgrade().expect("manager")
    }

    pub fn remaining_ticks(&self) -> i32 {
        self.remaining_sub_ticks.get() / 100
    }

    pub fn ready(&self) -> bool {
        self.active.get() && self.remaining_ticks() == 0
    }

    /// `Active`.
    pub fn active(&self) -> bool {
        self.active.get()
    }

    /// The instance `Target` uses: the first that is not paused.
    pub fn target_instance(&self) -> Option<Rc<SupportPower>> {
        if !self.ready() {
            return None;
        }
        self.instances.borrow().iter().find(|i| !i.is_trait_paused()).cloned()
    }

    pub fn disabled(&self) -> bool {
        let m = self.manager();
        m.self_.owner().win_state.get() == WinState::Lost
            || (!self.prereqs_available.get() && !m.dev_mode.all_tech())
            || !self.instances_enabled.get()
            || self.one_shot_fired.get()
    }

    pub fn info(&self) -> Option<Rc<SupportPowerInfo>> {
        self.instances.borrow().first().map(|i| i.info.clone())
    }

    pub fn prerequisites_available(&self, available: bool) {
        self.prereqs_available.set(available);
        if !available {
            self.remaining_sub_ticks.set(self.total_ticks * 100);
        }
    }

    fn tick(&self) {
        let instances = self.instances.borrow().clone();
        self.instances_enabled.set(instances.iter().any(|i| !i.is_trait_disabled()));
        if !self.instances_enabled.get() {
            self.remaining_sub_ticks.set(self.total_ticks * 100);
        }
        self.active.set(!self.disabled() && instances.iter().any(|i| !i.is_trait_paused()));
        if !self.active.get() {
            return;
        }
        let power = &instances[0];
        if self.manager().dev_mode.fast_charge() && self.remaining_sub_ticks.get() > 2500 {
            self.remaining_sub_ticks.set(2500);
        }
        if self.remaining_sub_ticks.get() > 0 {
            self.remaining_sub_ticks
                .set((self.remaining_sub_ticks.get() - 100).clamp(0, self.total_ticks * 100));
        }
        if !self.notified_charging.get() {
            power.notify_power_event(
                &power.info.begin_charge_sound,
                &power.info.begin_charge_speech_notification,
                &power.info.begin_charge_text_notification,
            );
            self.notified_charging.set(true);
        }
        if self.remaining_ticks() == 0 && !self.notified_ready.get() {
            power.charged(&power.self_, &self.key);
            self.notified_ready.set(true);
        }
    }

    fn activate(&self, order: &Order) {
        if !self.ready() {
            return;
        }
        let instances = self.instances.borrow().clone();
        let candidates = instances.iter().filter(|i| !i.is_trait_paused() && !i.is_trait_disabled());
        let key = |a: &Rc<SupportPower>| -> i64 {
            if a.self_.cache().occupies_space.is_none() || order.target.target_type() == TargetType::Invalid {
                return 0;
            }
            (a.self_.center_position() - order.target.center_position()).horizontal_length_squared()
        };
        let mut best: Option<(&Rc<SupportPower>, i64)> = None;
        for c in candidates {
            let k = key(c);
            if best.is_none_or(|(_, bk)| k < bk) {
                best = Some((c, k));
            }
        }
        let Some((power, _)) = best else { return };
        power.activate(&power.self_, order);
        self.remaining_sub_ticks.set(self.total_ticks * 100);
        self.notified_charging.set(false);
        self.notified_ready.set(false);
        if self.info().is_some_and(|i| i.one_shot) {
            self.prerequisites_available(false);
            self.one_shot_fired.set(true);
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct SupportPowerManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

pub struct SupportPowerManager {
    pub self_: ActorRef,
    pub powers: RefCell<NetMap<String, Rc<SupportPowerInstance>>>,
    dev_mode: Rc<DeveloperMode>,
    tech_tree: Rc<TechTree>,
    me: Weak<SupportPowerManager>,
}

register_traits!(SupportPowerManager: ITick, IResolveOrder);
info_impl!(SupportPowerManagerInfo => |_, init| {
    let self_ = init.self_.clone();
    let m = Rc::new_cyclic(|me| SupportPowerManager {
        dev_mode: self_.trait_::<DeveloperMode>(),
        tech_tree: self_.trait_::<TechTree>(),
        self_,
        powers: RefCell::new(NetMap::new()),
        me: me.clone(),
    });
    let w = init.world();
    let (wa, wr) = (m.me.clone(), m.me.clone());
    w.on_actor_added(Rc::new(move |a: &ActorRef| {
        if let Some(m) = wa.upgrade() {
            m.actor_added(a);
        }
    }));
    w.on_actor_removed(Rc::new(move |a: &ActorRef| {
        if let Some(m) = wr.upgrade() {
            m.actor_removed(a);
        }
    }));
    Some(m)
});

impl SupportPowerManager {
    /// `GetPowersForActor`.
    pub fn get_powers_for_actor(&self, a: &ActorRef) -> Vec<Rc<SupportPowerInstance>> {
        if !Rc::ptr_eq(&a.owner(), &self.self_.owner()) {
            return Vec::new();
        }
        let powers = self.powers.borrow();
        a.traits_implementing::<SupportPower>()
            .iter()
            .filter_map(|t| powers.get(&t.key()).cloned())
            .collect()
    }

    fn owns(&self, a: &ActorRef) -> bool {
        a.owner_opt().is_some_and(|o| Rc::ptr_eq(&o, &self.self_.owner()))
    }

    fn actor_added(&self, a: &ActorRef) {
        if !self.owns(a) {
            return;
        }
        for t in a.traits_implementing::<SupportPower>() {
            let key = t.key();
            let existing = self.powers.borrow().get(&key).cloned();
            let spi = match existing {
                Some(s) => s,
                None => {
                    let s = SupportPowerInstance::new(key.clone(), &t.info, self.me.clone());
                    self.powers.borrow_mut().insert(key.clone(), s.clone());
                    if !t.info.prerequisites.is_empty() {
                        let me: Rc<dyn ITechTreeElement> = self.me.upgrade().expect("manager");
                        self.tech_tree.add(&key, t.info.prerequisites.clone(), 0, me);
                        self.tech_tree.update();
                    }
                    s
                }
            };
            spi.instances.borrow_mut().push(t);
        }
    }

    fn actor_removed(&self, a: &ActorRef) {
        if !self.owns(a) || !a.info.has_trait_info::<SupportPowerInfo>() {
            return;
        }
        for t in a.traits_implementing::<SupportPower>() {
            let key = t.key();
            let spi = self.powers.borrow().get(&key).cloned().expect("power instance");
            spi.instances.borrow_mut().retain(|i| !Rc::ptr_eq(i, &t));
            if spi.instances.borrow().is_empty() && !spi.disabled() {
                self.powers.borrow_mut().remove(&key);
                self.tech_tree.remove_key(&key);
                self.tech_tree.update();
            }
        }
    }
}

impl ITick for SupportPowerManager {
    fn tick(&self, _self_: &ActorRef) {
        let powers: Vec<Rc<SupportPowerInstance>> = self.powers.borrow().values().cloned().collect();
        for p in powers {
            p.tick();
        }
    }
}

impl IResolveOrder for SupportPowerManager {
    fn resolve_order(&self, _self_: &ActorRef, order: &Order) {
        let sp = self.powers.borrow().get(&order.order_string).cloned();
        if let Some(sp) = sp {
            sp.activate(order);
        }
    }
}

impl ITechTreeElement for SupportPowerManager {
    fn prerequisites_available(&self, key: &str) {
        let sp = self.powers.borrow().get(&key.to_string()).cloned();
        if let Some(sp) = sp {
            sp.prerequisites_available(true);
        }
    }

    fn prerequisites_unavailable(&self, key: &str) {
        let sp = self.powers.borrow().get(&key.to_string()).cloned();
        if let Some(sp) = sp {
            sp.prerequisites_available(false);
        }
    }

    fn prerequisites_item_hidden(&self, _key: &str) {}

    fn prerequisites_item_visible(&self, _key: &str) {}
}

impl SupportPower {
    fn remove_gps(&self, self_: &ActorRef) {
        let g = self.gps_owner.borrow().clone();
        if let Some(g) = g {
            g.gps_remove(self_);
        }
    }

    fn no_active_radar(&self, self_: &ActorRef) -> bool {
        let owner = self_.owner();
        !self_
            .world()
            .actors_with_trait::<ProvidesRadar>()
            .iter()
            .any(|(a, r)| !r.is_trait_disabled() && Rc::ptr_eq(&a.owner(), &owner))
    }
}

impl INotifyKilled for SupportPower {
    fn killed(&self, self_: &ActorRef, _e: &rustora_engine::damage::AttackInfo) {
        self.remove_gps(self_);
    }
}

impl INotifySold for SupportPower {
    fn selling(&self, _self_: &ActorRef) {}

    fn sold(&self, self_: &ActorRef) {
        self.remove_gps(self_);
    }
}

impl INotifyOwnerChanged for SupportPower {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &rustora_engine::player::PlayerRef, new_owner: &rustora_engine::player::PlayerRef) {
        self.remove_gps(self_);
        let g = new_owner.player_actor().trait_::<GpsWatcher>();
        *self.gps_owner.borrow_mut() = Some(g.clone());
        g.gps_add(self_);
    }
}

impl ITick for SupportPower {
    fn tick(&self, self_: &ActorRef) {
        let blocked = self.is_trait_paused() || (self.info.requires_active_radar && self.no_active_radar(self_));
        if !self.gps_was_paused.get() && blocked {
            self.gps_was_paused.set(true);
            self.remove_gps(self_);
        } else if self.gps_was_paused.get() && !self.is_trait_paused() && !(self.info.requires_active_radar && self.no_active_radar(self_))
        {
            self.gps_was_paused.set(false);
            let g = self.gps_owner.borrow().clone().expect("gps watcher");
            g.gps_add(self_);
        }
    }
}

impl SupportPower {
    /// `NukePower.Activate(Actor, WPos)`.
    pub fn launch_nuke(&self, self_: &ActorRef, target_position: rustora_core::world_types::WPos) {
        let info = &self.info;
        let w = self_.world();
        let weapon = w
            .rules()
            .weapon(&info.missile_weapon.to_lowercase())
            .unwrap_or_else(|| panic!("Weapons Ruleset does not contain an entry '{}'", info.missile_weapon.to_lowercase()));
        let body = self_.trait_or_default::<crate::render::BodyOrientation>();
        let skip_ascent = info.skip_ascent || body.is_none();
        let launch_pos = match (&body, skip_ascent) {
            (Some(b), false) => self_.center_position() + b.local_to_world(info.spawn_offset),
            _ => rustora_core::world_types::WPos::ZERO,
        };
        let missile = NukeLaunch::new(self_.owner(), weapon, launch_pos, target_position, info, skip_ascent);
        w.add_frame_end_task(Box::new(move |w| w.add_effect(missile)));
        if info.camera_range != WDist::ZERO {
            let ty = if info.reveal_generated_shroud {
                rustora_engine::shroud::SourceType::Visibility
            } else {
                rustora_engine::shroud::SourceType::PassiveVisibility
            };
            let (owner, range, rel, delay, duration) = (
                self_.owner(),
                info.camera_range,
                info.camera_relationships,
                info.flight_delay - info.camera_spawn_advance,
                info.camera_spawn_advance + info.camera_remove_delay,
            );
            w.add_frame_end_task(Box::new(move |w| {
                w.add_effect(crate::death::RevealShroudEffect::new(
                    target_position,
                    range,
                    ty,
                    owner,
                    rel,
                    delay,
                    duration,
                ))
            }));
        }
    }
}

/// `Projectiles/NukeLaunch.cs`, without the missile animation.
pub struct NukeLaunch {
    me: Weak<NukeLaunch>,
    fired_by: rustora_engine::player::PlayerRef,
    weapon: Rc<rustora_engine::weapon::WeaponInfo>,
    ascend_source: rustora_core::world_types::WPos,
    ascend_target: rustora_core::world_types::WPos,
    descend_source: rustora_core::world_types::WPos,
    descend_target: rustora_core::world_types::WPos,
    detonation_altitude: WDist,
    remove_on_detonation: bool,
    impact_delay: i32,
    turn: i32,
    trail_image: Option<String>,
    trail_sequences: Vec<String>,
    trail_interval: i32,
    pos: Cell<rustora_core::world_types::WPos>,
    ticks: Cell<i32>,
    trail_ticks: Cell<i32>,
    launch_delay: Cell<i32>,
    detonated: Cell<bool>,
}

impl NukeLaunch {
    fn new(
        fired_by: rustora_engine::player::PlayerRef,
        weapon: Rc<rustora_engine::weapon::WeaponInfo>,
        launch_pos: rustora_core::world_types::WPos,
        target_pos: rustora_core::world_types::WPos,
        info: &SupportPowerInfo,
        skip_ascent: bool,
    ) -> rustora_engine::effect::EffectRef {
        let impact_delay = info.flight_delay;
        let turn = if skip_ascent { 0 } else { impact_delay / 2 };
        let offset = WVec::new(0, 0, info.flight_velocity.length * (impact_delay - turn));
        let descend_source = target_pos + offset;
        Rc::new_cyclic(|me| NukeLaunch {
            me: me.clone(),
            fired_by,
            weapon,
            ascend_source: launch_pos,
            ascend_target: launch_pos + offset,
            descend_source,
            descend_target: target_pos,
            detonation_altitude: info.detonation_altitude,
            remove_on_detonation: info.remove_missile_on_detonation,
            impact_delay,
            turn,
            trail_image: info.trail_image.clone().filter(|t| !t.is_empty()),
            trail_sequences: info.trail_sequences.clone(),
            trail_interval: info.trail_interval,
            pos: Cell::new(if skip_ascent { descend_source } else { launch_pos }),
            ticks: Cell::new(0),
            trail_ticks: Cell::new(info.trail_delay),
            launch_delay: Cell::new(info.missile_delay),
            detonated: Cell::new(false),
        })
    }

    fn explode(&self, world: &Rc<World>, remove: bool) {
        if remove {
            let me: rustora_engine::effect::EffectRef = self.me.upgrade().expect("effect");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
        if self.detonated.get() {
            return;
        }
        let target = Target::from_pos(self.pos.get());
        let args = rustora_engine::weapon::WarheadArgs {
            weapon: self.weapon.clone(),
            damage_modifiers: Vec::new(),
            source: Some(target.center_position()),
            impact_orientation: WRot::NONE,
            impact_position: rustora_core::world_types::WPos::ZERO,
            source_actor: self.fired_by.player_actor().clone(),
            weapon_target: target.clone(),
        };
        rustora_engine::weapon::weapon_impact_args(&self.weapon, &target, &args);
        self.detonated.set(true);
    }
}

impl rustora_engine::effect::Effect for NukeLaunch {
    fn tick(&self, world: &Rc<World>) {
        let d = self.launch_delay.get();
        self.launch_delay.set(d - 1);
        if d > 0 {
            return;
        }
        if d == 0 {
            sound::play_random_at(world, SoundType::World, &self.weapon.report, self.pos.get(), 1.0);
        }
        let ticks = self.ticks.get();
        let descending = ticks >= self.turn;
        let lerp = rustora_core::world_types::WPos::lerp_quadratic;
        let pos = if !descending {
            lerp(self.ascend_source, self.ascend_target, WAngle::ZERO, ticks, self.turn)
        } else {
            lerp(
                self.descend_source,
                self.descend_target,
                WAngle::ZERO,
                ticks - self.turn,
                self.impact_delay - self.turn,
            )
        };
        self.pos.set(pos);
        if self.trail_image.is_some() {
            self.trail_ticks.set(self.trail_ticks.get() - 1);
            if self.trail_ticks.get() < 0 {
                if !self.trail_sequences.is_empty() {
                    world.shared_random.borrow_mut().next_max(self.trail_sequences.len() as i32);
                }
                self.trail_ticks.set(self.trail_interval);
            }
        }
        let dat = world.map().distance_above_terrain(pos);
        if ticks == self.impact_delay || (descending && dat <= self.detonation_altitude) {
            self.explode(world, ticks == self.impact_delay || self.remove_on_detonation);
        }
        self.ticks.set(ticks + 1);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
