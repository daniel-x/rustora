// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Actor previews (`IRenderActorPreviewInfo`, `SpriteActorPreview`) and the info traits that choose how a building
//! looks while it is being placed (`Traits/Buildings/*PlaceBuildingPreview.cs`). Previews come from the actor rules
//! alone, with the inits of a new undamaged actor at `CPos.Zero`.

use std::rc::Rc;

use rustora_core::world_types::{CPos, CVec, WAngle, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::damage::DamageState;
use rustora_engine::player::PlayerRef;
use rustora_engine::render::{PaletteRef, Renderable, render_animation};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::Animation;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info};

use crate::building::BuildingInfo;
use crate::conditions::WithSpriteTurretInfo;
use crate::render::{
    BodyOrientationInfo, RenderSpritesInfo, WithIdleOverlayInfo, WithProductionDoorOverlayInfo, WithSpriteBodyInfo, normalize_sequence,
};
use crate::render_traits::WithBuildingBibInfo;
use crate::turreted::TurretedInfo;

pub fn register(r: &mut TraitRegistry) {
    r.register("OrderEffects", |n| load_info!(crate::effects::OrderEffectsInfo, n));
    r.register("ProductionIconOverlayManager", |n| load_info!(ProductionIconOverlayManagerInfo, n));
    r.register("WithProductionIconOverlay", |n| load_info!(WithProductionIconOverlayInfo, n));
    r.register("FootprintPlaceBuildingPreview", |n| {
        load_info!(FootprintPlaceBuildingPreviewInfo, n)
    });
    r.register("SequencePlaceBuildingPreview", |n| load_info!(SequencePlaceBuildingPreviewInfo, n));
    r.register("ActorPreviewPlaceBuildingPreview", |n| {
        load_info!(ActorPreviewPlaceBuildingPreviewInfo, n)
    });
}

/// `FootprintPlaceBuildingPreviewInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct FootprintPlaceBuildingPreviewInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub palette: String,
    pub footprint_alpha: f32,
    pub line_build_footprint_alpha: f32,
}

impl Default for FootprintPlaceBuildingPreviewInfo {
    fn default() -> Self {
        FootprintPlaceBuildingPreviewInfo {
            instance_name: None,
            palette: "terrain".into(),
            footprint_alpha: 1.0,
            line_build_footprint_alpha: 1.0,
        }
    }
}

info_impl!(FootprintPlaceBuildingPreviewInfo => |_info, _init| None);

/// `SequencePlaceBuildingPreviewInfo`. Footprint under the sprite for valid and line build cells, over it for invalid.
#[derive(Clone, Debug, FieldLoad)]
pub struct SequencePlaceBuildingPreviewInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub footprint: FootprintPlaceBuildingPreviewInfo,
    pub sequence: String,
    pub sequence_alpha: f32,
}

impl Default for SequencePlaceBuildingPreviewInfo {
    fn default() -> Self {
        SequencePlaceBuildingPreviewInfo {
            instance_name: None,
            footprint: FootprintPlaceBuildingPreviewInfo::default(),
            sequence: "idle".into(),
            sequence_alpha: 1.0,
        }
    }
}

info_impl!(SequencePlaceBuildingPreviewInfo => |_info, _init| None);

/// `ActorPreviewPlaceBuildingPreviewInfo`. Footprint under the actor for valid and line build cells, over it for invalid.
#[derive(Clone, Debug, FieldLoad)]
pub struct ActorPreviewPlaceBuildingPreviewInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub footprint: FootprintPlaceBuildingPreviewInfo,
    pub animated: bool,
    pub preview_alpha: f32,
    pub z_offset: i32,
}

impl Default for ActorPreviewPlaceBuildingPreviewInfo {
    fn default() -> Self {
        ActorPreviewPlaceBuildingPreviewInfo {
            instance_name: None,
            footprint: FootprintPlaceBuildingPreviewInfo::default(),
            animated: true,
            preview_alpha: 1.0,
            z_offset: 0,
        }
    }
}

info_impl!(ActorPreviewPlaceBuildingPreviewInfo => |_info, _init| None);

/// `SpriteActorPreview`.
pub struct ActorPreview {
    pub animation: Rc<Animation>,
    pub offset: WVec,
    pub z_offset: i32,
    pub palette: PaletteRef,
}

impl ActorPreview {
    pub fn tick(&self) {
        self.animation.tick();
    }

    pub fn render(&self, pos: rustora_core::world_types::WPos, out: &mut Vec<Renderable>) {
        render_animation(&self.animation, pos, self.offset, self.z_offset, &self.palette, out);
    }
}

fn overlay_palette(palette: Option<&str>, is_player: bool, owner: &PlayerRef, fallback: &PaletteRef) -> PaletteRef {
    match palette {
        Some(_) => PaletteRef::resolve(palette, is_player, owner),
        None => fallback.clone(),
    }
}

/// `RenderSpritesInfo.RenderPreview` for the sprite traits that RA buildings use.
pub fn render_previews(world: &World, ai: &ActorInfo, owner: &PlayerRef, faction: &str) -> Vec<ActorPreview> {
    let Some(rsi) = ai.trait_info::<RenderSpritesInfo>() else {
        return Vec::new();
    };
    let sequences = world.map().sequences().clone();
    let image = rsi.get_image(ai, faction);
    let default_palette = PaletteRef::resolve(
        rsi.palette.as_deref().or(Some(rsi.player_palette.as_str())),
        rsi.palette.is_none(),
        owner,
    );
    let new_animation = |image: &str, facing: Option<Rc<dyn Fn() -> WAngle>>| Animation::new(sequences.clone(), image, facing, None);
    let mut out = Vec::new();

    for info in ai.trait_infos::<WithSpriteBodyInfo>() {
        if !info.base.base.enabled_by_default() {
            continue;
        }
        let animation = new_animation(&image, None);
        animation.play_repeating(&normalize_sequence(&animation, DamageState::UNDAMAGED, &info.sequence));
        out.push(ActorPreview {
            animation,
            offset: WVec::ZERO,
            z_offset: 0,
            palette: overlay_palette(info.palette.as_deref(), info.is_player_palette, owner, &default_palette),
        });
    }

    for info in ai.trait_infos::<WithBuildingBibInfo>() {
        let Some(bi) = ai.trait_info::<BuildingInfo>() else {
            continue;
        };
        out.extend(bib_previews(world, &info, &bi, &image, &sequences));
    }

    let body = ai.trait_info::<BodyOrientationInfo>();
    for info in ai.trait_infos::<WithSpriteTurretInfo>() {
        if !info.base.enabled_by_default() {
            continue;
        }
        let (Some(body), Some(turret)) = (
            body.as_ref(),
            ai.trait_infos::<TurretedInfo>().into_iter().find(|t| t.turret == info.turret),
        ) else {
            continue;
        };
        let facing = turret.initial_facing;
        let animation = new_animation(&image, Some(Rc::new(move || facing)));
        animation.play(&normalize_sequence(&animation, DamageState::UNDAMAGED, &info.sequence));
        let offset = body.local_to_world(turret.offset);
        out.push(ActorPreview {
            animation,
            offset,
            z_offset: -(offset.y + offset.z) + 1,
            palette: overlay_palette(info.palette.as_deref(), info.is_player_palette, owner, &default_palette),
        });
    }

    for info in ai.trait_infos::<WithIdleOverlayInfo>() {
        if !info.base.base.enabled_by_default() {
            continue;
        }
        let Some(body) = body.as_ref() else {
            continue;
        };
        let overlay_image = info.image.clone().unwrap_or_else(|| image.clone());
        let animation = new_animation(&overlay_image, Some(Rc::new(|| WAngle::ZERO)));
        animation.is_decoration.set(info.is_decoration);
        animation.play_repeating(&normalize_sequence(&animation, DamageState::UNDAMAGED, &info.sequence));
        let offset = body.local_to_world(info.offset);
        out.push(ActorPreview {
            animation,
            offset,
            z_offset: if info.is_decoration { 0 } else { -(offset.y + offset.z) + 1 },
            palette: overlay_palette(info.palette.as_deref(), info.is_player_palette, owner, &default_palette),
        });
    }

    for info in ai.trait_infos::<WithProductionDoorOverlayInfo>() {
        let Some(bi) = ai.trait_info::<BuildingInfo>() else {
            continue;
        };
        let animation = new_animation(&image, None);
        animation.play_fetch_index(
            &normalize_sequence(&animation, DamageState::UNDAMAGED, &info.sequence),
            Rc::new(|| 0),
        );
        out.push(ActorPreview {
            animation,
            offset: WVec::ZERO,
            z_offset: bi.center_offset(world).y + 512,
            palette: default_palette.clone(),
        });
    }
    out
}

/// `WithBuildingBibInfo.RenderPreviewSprites`.
fn bib_previews(
    world: &World,
    info: &WithBuildingBibInfo,
    bi: &BuildingInfo,
    image: &str,
    sequences: &Rc<rustora_engine::sequence::SequenceSet>,
) -> Vec<ActorPreview> {
    let rows = if info.has_minibib { 1 } else { 2 };
    let width = bi.dimensions.x;
    let bib_offset = bi.dimensions.y - rows;
    let center_offset = bi.center_offset(world);
    let map = world.map();
    let mut out = Vec::new();
    for i in 0..rows * width {
        let animation = Animation::new(sequences.clone(), image, None, None);
        let cell = CPos::ZERO + CVec::new(i % width, i / width + bib_offset);
        let mut sequence = info.sequence.clone();
        if map.contains(cell) {
            let test = format!("{}-{}", info.sequence, map.terrain_type_info(cell).ty);
            if animation.has_sequence(&test) {
                sequence = test;
            }
        }
        animation.play_fetch_index(&sequence, Rc::new(move || i));
        animation.is_decoration.set(true);
        let offset = map.center_of_cell(cell) - map.center_of_cell(CPos::ZERO) - center_offset;
        out.push(ActorPreview {
            animation,
            offset,
            z_offset: -(offset.y + center_offset.y + 512),
            palette: PaletteRef::named(&info.palette),
        });
    }
    out
}

/// `ProductionIconOverlayManagerInfo`. The sprite comes from the client, and `TechTree.HasPrerequisites` replaces the
/// tech tree watchers of the C# trait.
#[derive(Clone, Debug, FieldLoad)]
pub struct ProductionIconOverlayManagerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(name = "Type")]
    pub ty: String,
    pub image: String,
    pub sequence: Option<String>,
    pub palette: String,
}

impl Default for ProductionIconOverlayManagerInfo {
    fn default() -> Self {
        ProductionIconOverlayManagerInfo {
            instance_name: None,
            ty: String::new(),
            image: String::new(),
            sequence: None,
            palette: "chrome".into(),
        }
    }
}

info_impl!(ProductionIconOverlayManagerInfo => |_info, _init| None);

/// `WithProductionIconOverlayInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct WithProductionIconOverlayInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub types: Vec<String>,
    pub prerequisites: Vec<String>,
}

info_impl!(WithProductionIconOverlayInfo => |_info, _init| None);
