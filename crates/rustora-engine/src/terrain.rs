// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Terrain data of a tileset (`Mods.Common/Terrain/DefaultTerrain.cs`).

use std::collections::{BTreeMap, HashMap};

use indexmap::IndexSet;
use rustora_core::fieldloader::{Color, get_value};
use rustora_core::miniyaml::MiniYamlNode;
use rustora_derive::FieldLoad;

use crate::bitset::BitSet;
use crate::map::TerrainTile;

/// Tag of `BitSet<TargetableType>`.
pub struct TargetableType;

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct TerrainTypeInfo {
    #[field(name = "Type")]
    pub ty: String,
    pub target_types: BitSet<TargetableType>,
    pub accepts_smudge_type: IndexSet<String>,
    pub color: Color,
    pub restrict_player_color: bool,
}

/// One tile of a template.
#[derive(Clone, Debug)]
pub struct TileInfo {
    pub terrain_type: u8,
    pub min_color: Color,
    pub max_color: Color,
}

#[derive(Clone, Debug)]
pub struct TerrainTemplate {
    pub id: u16,
    pub size: (i32, i32),
    pub pick_any: bool,
    /// Indexed by tile index; missing entries of sparse templates are `None`.
    pub tiles: Vec<Option<TileInfo>>,
    pub images: Vec<String>,
    pub frames: Option<Vec<i32>>,
    pub palette: Option<String>,
}

pub struct TerrainInfo {
    pub name: String,
    pub id: String,
    pub sheet_size: i32,
    pub palette: Option<String>,
    /// `HeightDebugColors`.
    pub height_debug_colors: Vec<Color>,
    pub terrain_types: Vec<TerrainTypeInfo>,
    index_by_type: HashMap<String, u8>,
    default_walkable: u8,
    pub templates: BTreeMap<u16, TerrainTemplate>,
    first_template: u16,
}

impl TerrainInfo {
    pub fn load(yaml: &[MiniYamlNode]) -> Result<TerrainInfo, String> {
        let section = |k: &str| {
            yaml.iter()
                .find(|n| n.key == k)
                .map(|n| &n.value)
                .ok_or_else(|| format!("tileset lacks {k}"))
        };
        let general = section("General")?;
        let g = |k: &str| general.nodes.iter().find(|n| n.key == k).map(|n| n.value.value_str().to_string());
        let mut terrain_types = Vec::new();
        for n in &section("Terrain")?.nodes {
            terrain_types.push(rustora_core::fieldloader::load::<TerrainTypeInfo>(&n.value).map_err(|e| e.to_string())?);
        }
        terrain_types.sort_by(|a, b| a.ty.cmp(&b.ty));
        let index_by_type: HashMap<String, u8> = terrain_types.iter().enumerate().map(|(i, t)| (t.ty.clone(), i as u8)).collect();
        let default_walkable = *index_by_type.get("Clear").ok_or("tileset lacks Clear")?;
        let mut templates = BTreeMap::new();
        let mut first_template = None;
        for n in &section("Templates")?.nodes {
            let t = &n.value;
            let f = |k: &str| t.nodes.iter().find(|n| n.key == k);
            let id: u16 = get_value("Id", f("Id").map(|n| n.value.value_str())).map_err(|e| e.to_string())?;
            let size = match f("Size") {
                Some(s) => {
                    let v: rustora_core::world_types::Int2 = get_value("Size", Some(s.value.value_str())).map_err(|e| e.to_string())?;
                    (v.x, v.y)
                }
                None => (0, 0),
            };
            let pick_any = f("PickAny").is_some_and(|n| n.value.value_str().eq_ignore_ascii_case("true"));
            let tiles_node = f("Tiles").ok_or("template lacks Tiles")?;
            let len = if pick_any {
                tiles_node.value.nodes.len()
            } else {
                (size.0 * size.1) as usize
            };
            let mut tiles = vec![None; len];
            for tn in &tiles_node.value.nodes {
                let key: usize = tn.key.trim().parse().map_err(|_| format!("bad tile key {}", tn.key))?;
                let type_name = tn.value.value_str();
                let terrain_type = *index_by_type
                    .get(type_name)
                    .ok_or_else(|| format!("unknown terrain type {type_name}"))?;
                let base = terrain_types[terrain_type as usize].color;
                let col = |k: &str| -> Color {
                    tn.value
                        .nodes
                        .iter()
                        .find(|n| n.key == k)
                        .and_then(|n| Color::parse(n.value.value_str()))
                        .filter(|c| *c != Color::default())
                        .unwrap_or(base)
                };
                tiles[key] = Some(TileInfo {
                    terrain_type,
                    min_color: col("MinColor"),
                    max_color: col("MaxColor"),
                });
            }
            first_template.get_or_insert(id);
            let strs = |k: &str| -> Vec<String> {
                f(k).map(|n| {
                    n.value
                        .value_str()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
            };
            templates.insert(
                id,
                TerrainTemplate {
                    id,
                    size,
                    pick_any,
                    tiles,
                    images: strs("Images"),
                    frames: f("Frames").and_then(|n| get_value::<Vec<i32>>("Frames", Some(n.value.value_str())).ok()),
                    palette: f("Palette").map(|n| n.value.value_str().to_string()),
                },
            );
        }
        Ok(TerrainInfo {
            name: g("Name").unwrap_or_default(),
            id: g("Id").unwrap_or_default(),
            sheet_size: g("SheetSize").and_then(|s| s.parse().ok()).unwrap_or(512),
            palette: g("Palette"),
            height_debug_colors: g("HeightDebugColors")
                .map(|v| v.split(',').filter_map(|c| Color::parse(c.trim())).collect::<Vec<_>>())
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| vec![Color::from_argb(255, 255, 0, 0)]),
            terrain_types,
            index_by_type,
            default_walkable,
            templates,
            first_template: first_template.ok_or("tileset without templates")?,
        })
    }

    pub fn terrain_index(&self, ty: &str) -> u8 {
        *self
            .index_by_type
            .get(ty)
            .unwrap_or_else(|| panic!("Tileset '{}' lacks terrain type '{ty}'", self.id))
    }

    pub fn tile_info(&self, r: TerrainTile) -> Option<&TileInfo> {
        self.templates.get(&r.ty)?.tiles.get(r.index as usize)?.as_ref()
    }

    pub fn terrain_index_of_tile(&self, r: TerrainTile) -> u8 {
        self.tile_info(r).map_or(self.default_walkable, |t| t.terrain_type)
    }

    pub fn default_tile(&self) -> TerrainTile {
        TerrainTile {
            ty: self.first_template,
            index: 0,
        }
    }
}
