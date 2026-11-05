// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Infos of the palette and palette effect traits of the world actor. The client builds the palettes from them.

use std::rc::Rc;

use indexmap::IndexMap;
use rustora_derive::FieldLoad;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("PaletteFromFile", |n| load_info!(PaletteFromFileInfo, n));
    r.register("PaletteFromRGBA", |n| load_info!(PaletteFromRgbaInfo, n));
    r.register("IndexedPalette", |n| load_info!(IndexedPaletteInfo, n));
    r.register("IndexedPlayerPalette", |n| load_info!(IndexedPlayerPaletteInfo, n));
    r.register("PlayerColorPalette", |n| load_info!(PlayerColorPaletteInfo, n));
    r.register("ShroudPalette", |n| load_info!(ShroudPaletteInfo, n));
    r.register("RotationPaletteEffect", |n| load_info!(RotationPaletteEffectInfo, n));
    r.register("LightPaletteRotator", |n| load_info!(LightPaletteRotatorInfo, n));
    r.register("ShroudRenderer", |n| load_info!(ShroudRendererInfo, n));
}

/// A trait without runtime state, whose info is read by the client.
pub struct PaletteTrait;
register_traits!(PaletteTrait);

macro_rules! palette_info {
    ($($info:ty),*) => {
        $(info_impl!($info => |_info, _init| Some(Rc::new(PaletteTrait)));)*
    };
}
palette_info!(
    PaletteFromFileInfo,
    PaletteFromRgbaInfo,
    IndexedPaletteInfo,
    IndexedPlayerPaletteInfo,
    PlayerColorPaletteInfo,
    ShroudPaletteInfo,
    RotationPaletteEffectInfo,
    LightPaletteRotatorInfo,
    ShroudRendererInfo
);

/// `ShroudRendererInfo`.
#[derive(Clone, Debug, FieldLoad)]
pub struct ShroudRendererInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub sequence: String,
    pub shroud_variants: Vec<String>,
    pub fog_variants: Vec<String>,
    pub shroud_palette: String,
    pub fog_palette: String,
    pub index: Vec<i32>,
    pub use_extended_index: bool,
    pub override_full_shroud: Option<String>,
    pub override_shroud_index: i32,
    pub override_full_fog: Option<String>,
    pub override_fog_index: i32,
}

impl Default for ShroudRendererInfo {
    fn default() -> Self {
        ShroudRendererInfo {
            instance_name: None,
            sequence: "shroud".into(),
            shroud_variants: vec!["shroud".into()],
            fog_variants: vec!["fog".into()],
            shroud_palette: "shroud".into(),
            fog_palette: "fog".into(),
            index: vec![12, 9, 8, 3, 1, 6, 4, 2, 13, 11, 7, 14],
            use_extended_index: false,
            override_full_shroud: None,
            override_shroud_index: 15,
            override_full_fog: None,
            override_fog_index: 15,
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PaletteFromFileInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    pub tileset: Option<String>,
    pub filename: String,
    pub transparent_index: Vec<i32>,
    pub shadow_index: Vec<i32>,
    pub allow_modifiers: bool,
}

impl Default for PaletteFromFileInfo {
    fn default() -> Self {
        PaletteFromFileInfo {
            instance_name: None,
            name: String::new(),
            tileset: None,
            filename: String::new(),
            transparent_index: vec![0],
            shadow_index: Vec::new(),
            allow_modifiers: true,
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PaletteFromRgbaInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    pub tileset: Option<String>,
    pub r: i32,
    pub g: i32,
    pub b: i32,
    pub a: i32,
    pub allow_modifiers: bool,
}

impl Default for PaletteFromRgbaInfo {
    fn default() -> Self {
        PaletteFromRgbaInfo {
            instance_name: None,
            name: String::new(),
            tileset: None,
            r: 0,
            g: 0,
            b: 0,
            a: 0,
            allow_modifiers: true,
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct IndexedPaletteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    pub base_palette: String,
    pub index: Vec<i32>,
    pub replace_index: Vec<i32>,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct IndexedPlayerPaletteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub base_palette: String,
    pub base_name: String,
    pub remap_index: Vec<i32>,
    pub player_index: IndexMap<String, Vec<i32>>,
}

impl Default for IndexedPlayerPaletteInfo {
    fn default() -> Self {
        IndexedPlayerPaletteInfo {
            instance_name: None,
            base_palette: String::new(),
            base_name: "player".into(),
            remap_index: Vec::new(),
            player_index: IndexMap::new(),
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PlayerColorPaletteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub base_palette: String,
    pub base_name: String,
    pub remap_index: Vec<i32>,
}

impl Default for PlayerColorPaletteInfo {
    fn default() -> Self {
        PlayerColorPaletteInfo {
            instance_name: None,
            base_palette: String::new(),
            base_name: "player".into(),
            remap_index: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ShroudPaletteInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub name: String,
    pub fog: bool,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct RotationPaletteEffectInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub palettes: Vec<String>,
    pub tilesets: Vec<String>,
    pub exclude_palettes: Vec<String>,
    pub exclude_tilesets: Vec<String>,
    pub rotation_base: i32,
    pub rotation_range: i32,
    pub rotation_step: f32,
}

impl Default for RotationPaletteEffectInfo {
    fn default() -> Self {
        RotationPaletteEffectInfo {
            instance_name: None,
            palettes: Vec::new(),
            tilesets: Vec::new(),
            exclude_palettes: Vec::new(),
            exclude_tilesets: Vec::new(),
            rotation_base: 0x60,
            rotation_range: 7,
            rotation_step: 0.25,
        }
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct LightPaletteRotatorInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub exclude_palettes: Vec<String>,
    pub time_step: f32,
    pub modify_index: i32,
    pub rotation_indices: Vec<i32>,
}

impl Default for LightPaletteRotatorInfo {
    fn default() -> Self {
        LightPaletteRotatorInfo {
            instance_name: None,
            exclude_palettes: Vec::new(),
            time_step: 0.5,
            modify_index: 103,
            rotation_indices: vec![
                230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 238, 237, 236, 235, 234, 233, 232, 231,
            ],
        }
    }
}
