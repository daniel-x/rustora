// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sim relevant parts of render traits: body orientation and sprite image lookup.

use std::cell::Cell;
use std::rc::Rc;

use rustora_core::world_types::{WAngle, WRot, WVec};

use indexmap::IndexMap;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::actor_init::FactionInit;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("RenderSprites", |n| load_info!(RenderSpritesInfo, n));
    r.register("BodyOrientation", |n| load_info!(BodyOrientationInfo, n));
    r.register("ClassicFacingBodyOrientation", |n| {
        let mut info = BodyOrientationInfo::default();
        rustora_core::fieldloader::FieldLoad::load(&mut info, &n.yaml).map_err(|e| format!("Trait name {}: {e}", n.display_name()))?;
        info.instance_name = n.instance_name.clone();
        Ok(Rc::new(ClassicFacingBodyOrientationInfo(info)) as Rc<dyn TraitInfo>)
    });
    r.register("QuantizeFacingsFromSequence", |n| load_info!(QuantizeFacingsFromSequenceInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RenderSpritesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub image: Option<String>,
    pub faction_images: IndexMap<String, String>,
    pub palette: Option<String>,
    pub player_palette: String,
}

impl Default for RenderSpritesInfo {
    fn default() -> Self {
        RenderSpritesInfo {
            instance_name: None,
            image: None,
            faction_images: IndexMap::new(),
            palette: None,
            player_palette: "player".into(),
        }
    }
}

impl RenderSpritesInfo {
    pub fn get_image(&self, actor: &ActorInfo, faction: &str) -> String {
        if !faction.is_empty() {
            if let Some(i) = self.faction_images.get(faction) {
                return i.to_lowercase();
            }
        }
        self.image.clone().unwrap_or_else(|| actor.name.clone()).to_lowercase()
    }
}

/// Rendering is done by the client; the sim keeps the info for image lookups.
pub struct RenderSprites;
register_traits!(RenderSprites);
info_impl!(RenderSpritesInfo => |_, _| Some(Rc::new(RenderSprites)));

#[derive(Clone, Debug, FieldLoad)]
pub struct BodyOrientationInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub quantized_facings: i32,
    pub camera_pitch: rustora_core::world_types::WAngle,
    pub use_classic_perspective_fudge: bool,
}

impl Default for BodyOrientationInfo {
    fn default() -> Self {
        BodyOrientationInfo {
            instance_name: None,
            quantized_facings: -1,
            camera_pitch: rustora_core::world_types::WAngle::from_degrees(40),
            use_classic_perspective_fudge: true,
        }
    }
}

/// `ClassicFacingBodyOrientationInfo`: same data, different facing quantization (pending).
pub struct ClassicFacingBodyOrientationInfo(pub BodyOrientationInfo);

pub struct BodyOrientation {
    pub info: BodyOrientationInfo,
    class_name: &'static str,
    self_: std::rc::Weak<rustora_engine::actor::Actor>,
    faction: String,
    quantized: Cell<Option<i32>>,
}

impl TraitObject for BodyOrientation {
    fn register(self: Rc<Self>, dict: &rustora_engine::trait_dict::TraitDict, actor: &ActorRef) {
        dict.add::<BodyOrientation>(actor, self.clone());
        dict.add::<dyn ISync>(actor, self);
    }
    fn type_name(&self) -> &'static str {
        self.class_name
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn create_body(
    info: BodyOrientationInfo,
    class_name: &'static str,
    init: &rustora_engine::actor_init::ActorInitializer,
) -> Option<Rc<dyn TraitObject>> {
    let faction = init
        .inits
        .get::<FactionInit>()
        .map(|f| f.value.clone())
        .unwrap_or_else(|| init.self_.owner().faction.clone());
    Some(Rc::new(BodyOrientation {
        info,
        class_name,
        self_: Rc::downgrade(&init.self_),
        faction,
        quantized: Cell::new(None),
    }))
}

info_impl!(BodyOrientationInfo => |info, init| create_body((*info).clone(), "BodyOrientation", init));

impl TraitInfo for ClassicFacingBodyOrientationInfo {
    fn create(self: Rc<Self>, init: &rustora_engine::actor_init::ActorInitializer) -> Option<Rc<dyn TraitObject>> {
        create_body(self.0.clone(), "ClassicFacingBodyOrientation", init)
    }
    fn register_info(self: Rc<Self>, index: &rustora_engine::actor_info::InfoIndex) {
        index.add::<ClassicFacingBodyOrientationInfo>(self);
    }
    fn instance_name(&self) -> Option<&str> {
        self.0.instance_name.as_deref()
    }
    fn info_type_name(&self) -> &'static str {
        "ClassicFacingBodyOrientationInfo"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BodyOrientation {
    /// `LocalToWorld`.
    pub fn local_to_world(&self, vec: WVec) -> WVec {
        if !self.info.use_classic_perspective_fudge {
            return WVec::new(vec.y, -vec.x, vec.z);
        }
        WVec::new(vec.y, (-self.info.camera_pitch.sin()).wrapping_mul(vec.x) / 1024, vec.z)
    }

    pub fn quantize_orientation(&self, orientation: WRot) -> WRot {
        let f = self.quantized_facings();
        if f == 0 {
            return orientation;
        }
        WRot::from_yaw(self.quantize_facing_to(orientation.yaw, f))
    }

    pub fn quantize_facing(&self, facing: WAngle) -> WAngle {
        self.quantize_facing_to(facing, self.quantized_facings())
    }

    /// `QuantizeFacing(facing, facings)`, with the classic rounding of `ClassicFacingBodyOrientation`.
    pub fn quantize_facing_to(&self, facing: WAngle, facings: i32) -> WAngle {
        if self.class_name == "ClassicFacingBodyOrientation" {
            return crate::render::classic_quantize_facing(facing, facings);
        }
        crate::util::quantize_facing(facing, facings)
    }

    pub fn quantized_facings(&self) -> i32 {
        if let Some(q) = self.quantized.get() {
            return q;
        }
        let q = if self.info.quantized_facings >= 0 {
            self.info.quantized_facings
        } else {
            let a: ActorRef = self.self_.upgrade().expect("actor alive");
            let qi = a
                .info
                .trait_info::<QuantizeFacingsFromSequenceInfo>()
                .unwrap_or_else(|| panic!("Actor '{}' does not define a quantized body orientation.", a.info.name));
            let rsi = a.info.trait_info::<RenderSpritesInfo>().expect("RenderSprites");
            let image = rsi.get_image(&a.info, &self.faction);
            a.world()
                .sequence_facings(&image, &qi.sequence)
                .unwrap_or_else(|| panic!("missing sequence {image}.{}", qi.sequence))
        };
        self.quantized.set(Some(q));
        q
    }
}

impl ISync for BodyOrientation {
    fn sync_hash(&self) -> i32 {
        self.quantized_facings()
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct QuantizeFacingsFromSequenceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub sequence: String,
}

impl Default for QuantizeFacingsFromSequenceInfo {
    fn default() -> Self {
        QuantizeFacingsFromSequenceInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            sequence: "idle".into(),
        }
    }
}

pub struct QuantizeFacingsFromSequence {
    state: ConditionalState,
}

impl ConditionalHooks for QuantizeFacingsFromSequence {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

register_traits!(QuantizeFacingsFromSequence: ISync, INotifyCreated, IObservesVariables, IDisabledTrait);
info_impl!(QuantizeFacingsFromSequenceInfo => |info, _| Some(Rc::new(QuantizeFacingsFromSequence { state: ConditionalState::new(&info.base) })));

impl ISync for QuantizeFacingsFromSequence {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl INotifyCreated for QuantizeFacingsFromSequence {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl IObservesVariables for QuantizeFacingsFromSequence {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for QuantizeFacingsFromSequence {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

const SPRITE_RANGES: [i32; 32] = [
    20, 56, 88, 132, 156, 184, 212, 240, 268, 296, 324, 352, 384, 416, 452, 488, 532, 568, 604, 644, 668, 696, 724, 752, 780, 808, 836, 864, 896, 928, 964, 1000,
];

const SPRITE_FACINGS: [i32; 32] = [
    0, 40, 74, 112, 146, 172, 200, 228, 256, 284, 312, 340, 370, 402, 436, 472, 512, 552, 588, 626, 658, 684, 712, 740, 768, 796, 824, 852, 882, 914, 948, 984,
];

/// `ClassicIndexFacing` of `Mods.Cnc/Util.cs`.
pub fn classic_index_facing(facing: WAngle, num_frames: i32) -> i32 {
    if num_frames == 32 {
        return SPRITE_RANGES.iter().position(|r| facing.angle < *r).map_or(0, |i| i as i32);
    }
    crate::util::index_facing(facing, num_frames)
}

/// `ClassicQuantizeFacing` of `Mods.Cnc/Util.cs`.
pub fn classic_quantize_facing(facing: WAngle, steps: i32) -> WAngle {
    if steps == 32 {
        return WAngle::new(SPRITE_FACINGS[classic_index_facing(facing, steps) as usize]);
    }
    crate::util::quantize_facing(facing, steps)
}
