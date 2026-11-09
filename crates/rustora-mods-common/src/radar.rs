// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/Radar/AppearsOnRadar.cs`, `RadarColorFromTerrain.cs` and `RadarPings.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_core::world_types::{CPos, WPos};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{conditional_simple, info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("RadarPings", |n| load_info!(RadarPingsInfo, n));
    r.register("AppearsOnRadar", |n| load_info!(AppearsOnRadarInfo, n));
    r.register("RadarColorFromTerrain", |n| load_info!(RadarColorFromTerrainInfo, n));
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct RadarColorFromTerrainInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub terrain: String,
}

/// `RadarColorFromTerrain`: the radar color of the terrain type `Terrain`.
#[derive(rustora_engine::persist::Persist)]
pub struct RadarColorFromTerrain {
    color: Color,
}

register_traits!(RadarColorFromTerrain: IRadarColorModifier);
info_impl!(RadarColorFromTerrainInfo => |info, init| {
    let world = init.world();
    let terrain = &world.map().terrain;
    let color = terrain.terrain_types[terrain.terrain_index(&info.terrain) as usize].color;
    Some(Rc::new(RadarColorFromTerrain { color }))
});

impl IRadarColorModifier for RadarColorFromTerrain {
    fn radar_color_override(&self, _self_: &ActorRef, _color: Color, _local_player: Option<&PlayerRef>) -> Color {
        self.color
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct AppearsOnRadarInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub use_location: bool,
    pub valid_relationships: PlayerRelationship,
}

impl Default for AppearsOnRadarInfo {
    fn default() -> Self {
        AppearsOnRadarInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            use_location: false,
            valid_relationships: PlayerRelationship(
                PlayerRelationship::ALLY.0 | PlayerRelationship::NEUTRAL.0 | PlayerRelationship::ENEMY.0,
            ),
        }
    }
}

#[derive(rustora_engine::persist::Persist)]
pub struct AppearsOnRadar {
    pub info: Rc<AppearsOnRadarInfo>,
    state: ConditionalState,
}

register_traits!(AppearsOnRadar: INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(AppearsOnRadarInfo => |info, _init| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(AppearsOnRadar { info, state }))
});

conditional_simple!(AppearsOnRadar);

impl AppearsOnRadar {
    /// `PopulateRadarSignatureCells`, where `viewer` is the render player, or the local player.
    pub fn radar_signature_cells(
        &self,
        self_: &ActorRef,
        viewer: Option<&PlayerRef>,
        local_player: Option<&PlayerRef>,
        out: &mut Vec<(CPos, Color)>,
    ) {
        let owner = self_.owner();
        if self.state.is_trait_disabled()
            || viewer.is_some_and(|v| !self.info.valid_relationships.has_relationship(owner.relationship_with(Some(v))))
        {
            return;
        }
        let mut color = owner.color;
        if let Some(modifier) = self_.traits_implementing::<dyn IRadarColorModifier>().first() {
            color = modifier.radar_color_override(self_, color, local_player);
        }
        if self.info.use_location {
            out.push((self_.location(), color));
            return;
        }
        if let Some(os) = self_.trait_or_default::<dyn IOccupySpace>() {
            out.extend(os.occupied_cells().into_iter().map(|(c, _)| (c, color)));
        }
    }
}

/// Who sees a radar ping (`RadarPing.IsVisible`).
pub enum PingVisibility {
    All,
    AlliesOf(PlayerRef),
    /// Seen only by this player.
    Only(PlayerRef),
}

/// `RadarPing`.
pub struct RadarPing {
    pub visibility: PingVisibility,
    pub position: WPos,
    pub color: Color,
    duration: i32,
    to_radius: i32,
    shrink_speed: i32,
    rotation_speed: f32,
    radius: Cell<i32>,
    angle: Cell<f32>,
    tick: Cell<i32>,
}

impl RadarPing {
    pub fn is_visible(&self, viewer: Option<&PlayerRef>) -> bool {
        match &self.visibility {
            PingVisibility::All => true,
            PingVisibility::AlliesOf(p) => p.is_allied_with(viewer.map(|v| &**v)),
            PingVisibility::Only(p) => viewer.is_some_and(|v| Rc::ptr_eq(v, p)),
        }
    }

    fn tick(&self) -> bool {
        self.tick.set(self.tick.get() + 1);
        if self.tick.get() == self.duration {
            return false;
        }
        self.radius.set((self.radius.get() - self.shrink_speed).max(self.to_radius));
        self.angle.set(self.angle.get() - self.rotation_speed);
        true
    }

    /// `Points`: the corners of the triangle around `center`.
    pub fn points(&self, center: (f32, f32)) -> [(f32, f32); 3] {
        let r = self.radius.get() as f32;
        [0.0, 2.0, 4.0].map(|k| {
            let a = self.angle.get() + k * std::f32::consts::PI / 3.0;
            (center.0 + r * a.cos(), center.1 + r * a.sin())
        })
    }
}

/// `RadarPingsInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct RadarPingsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub from_radius: i32,
    pub to_radius: i32,
    pub shrink_speed: i32,
    pub rotation_speed: f32,
}

impl Default for RadarPingsInfo {
    fn default() -> Self {
        RadarPingsInfo {
            instance_name: None,
            from_radius: 200,
            to_radius: 15,
            shrink_speed: 4,
            rotation_speed: 0.12,
        }
    }
}

/// `RadarPings`: cosmetic state, not part of the sync hash.
#[derive(rustora_engine::persist::Persist)]
pub struct RadarPings {
    info: Rc<RadarPingsInfo>,
    #[persist(skip)]
    pub pings: RefCell<Vec<Rc<RadarPing>>>,
    pub last_ping_position: Cell<Option<WPos>>,
}

register_traits!(RadarPings: ITick);
info_impl!(RadarPingsInfo => |info, _init| Some(Rc::new(RadarPings {
    info,
    pings: RefCell::new(Vec::new()),
    last_ping_position: Cell::new(None),
})));

impl ITick for RadarPings {
    fn tick(&self, _self_: &ActorRef) {
        self.pings.borrow_mut().retain(|p| p.tick());
    }
}

impl RadarPings {
    pub fn add(
        &self,
        visibility: PingVisibility,
        viewer: Option<&PlayerRef>,
        position: WPos,
        color: Color,
        duration: i32,
    ) -> Rc<RadarPing> {
        let ping = Rc::new(RadarPing {
            visibility,
            position,
            color,
            duration,
            to_radius: self.info.to_radius,
            shrink_speed: self.info.shrink_speed,
            rotation_speed: self.info.rotation_speed,
            radius: Cell::new(self.info.from_radius),
            angle: Cell::new(0.0),
            tick: Cell::new(0),
        });
        if ping.is_visible(viewer) {
            self.last_ping_position.set(Some(position));
        }
        self.pings.borrow_mut().push(ping.clone());
        ping
    }

    /// `RadarPings.Remove`.
    pub fn remove(&self, ping: &Rc<RadarPing>) {
        self.pings.borrow_mut().retain(|p| !Rc::ptr_eq(p, ping));
    }
}

/// `RadarPings.Add` on the world actor, seen from the local player.
pub fn add_radar_ping(world: &World, visibility: PingVisibility, position: WPos, color: Color, duration: i32) {
    if let Some(pings) = world.world_actor().trait_or_default::<RadarPings>() {
        let viewer = world.sound.local_player();
        pings.add(visibility, viewer.as_ref(), position, color, duration);
    }
}
