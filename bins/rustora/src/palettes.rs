// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `HardwarePalette` and the `ILoadsPalettes` traits: builds the palette rows from the world actor's traits, and
//! applies the palette modifiers each world tick.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::fieldloader::Color;
use rustora_engine::player::PlayerRef;
use rustora_engine::render::PaletteRef;
use rustora_engine::sequence::ReadFile;
use rustora_engine::world::World;
use rustora_formats::palette::{self, Palette};
use rustora_mods_common::palettes::*;
use rustora_render::{MAX_PALETTES, Renderer};

struct Row {
    name: String,
    base: Palette,
    allow_modifiers: bool,
}

/// The palette rows of a world.
pub struct Palettes {
    rows: Vec<Row>,
    named: HashMap<String, u32>,
    player: HashMap<String, HashMap<u32, u32>>,
    rotations: Vec<Rc<RotationPaletteEffectInfo>>,
    lights: Vec<Rc<LightPaletteRotatorInfo>>,
    last_tick: i32,
    /// The players with the color that their palettes have.
    player_colors: Vec<(PlayerRef, Color)>,
}

impl Palettes {
    pub fn new(renderer: &Renderer, world: &World, read_file: &ReadFile) -> Palettes {
        let mut p = Palettes {
            rows: Vec::new(),
            named: HashMap::new(),
            player: HashMap::new(),
            rotations: Vec::new(),
            lights: Vec::new(),
            last_tick: -1,
            player_colors: Vec::new(),
        };
        let info = &world.world_actor().info;
        let tileset = world.map().tileset.as_str();
        let matches = |t: &Option<String>| t.as_deref().is_none_or(|t| t.eq_ignore_ascii_case(tileset));
        for f in info.trait_infos::<PaletteFromFileInfo>() {
            if !matches(&f.tileset) {
                continue;
            }
            let indices = |v: &[i32]| v.iter().map(|&i| i as usize).collect::<Vec<_>>();
            let d = read_file(&f.filename).unwrap_or_else(|| panic!("palette {} not found", f.filename));
            let pal = palette::load(&d, &indices(&f.transparent_index), &indices(&f.shadow_index)).expect("palette");
            p.add(&f.name, pal, f.allow_modifiers);
        }
        for f in info.trait_infos::<PaletteFromRgbaInfo>() {
            if matches(&f.tileset) {
                p.add(
                    &f.name,
                    palette::from_rgba(f.r as u8, f.g as u8, f.b as u8, f.a as u8),
                    f.allow_modifiers,
                );
            }
        }
        for s in info.trait_infos::<ShroudPaletteInfo>() {
            p.add(&s.name, shroud_palette(s.fog), true);
        }
        for f in info.trait_infos::<IndexedPaletteInfo>() {
            let base = p.base(&f.base_palette);
            let mut pal = base;
            for (&i, &r) in f.index.iter().zip(&f.replace_index) {
                pal[i as usize] = base[r as usize];
            }
            p.add(&f.name, pal, true);
        }
        for f in info.trait_infos::<FixedColorPaletteInfo>() {
            let Some(base) = p.try_base(&f.base_palette) else { continue };
            let remap: Vec<usize> = if f.remap_index.is_empty() {
                (0..256).collect()
            } else {
                f.remap_index.iter().map(|&i| i as usize).collect()
            };
            let c = f.color.argb;
            let pal = palette::player_remap(&base, &remap, [(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            p.add(&f.name, pal, f.allow_modifiers);
        }
        for f in info.trait_infos::<PaletteFromPaletteWithAlphaInfo>() {
            if let Some(base) = p.try_base(&f.base_palette) {
                p.add(&f.name, palette::alpha_remap(&base, f.alpha, f.premultiply), f.allow_modifiers);
            }
        }
        for pl in world.players() {
            p.load_player_palettes(world, &pl);
            p.player_colors.push((pl.clone(), pl.color()));
        }
        p.rotations = info.trait_infos::<RotationPaletteEffectInfo>();
        p.filter_tileset(tileset);
        p.lights = info.trait_infos::<LightPaletteRotatorInfo>();
        p.last_tick = 0;
        p.update_all(renderer, 0);
        p
    }

    /// `ILoadsPlayerPalettes.LoadPlayerPalettes` of the world actor's traits for one player, which replaces the
    /// palettes of that player if they exist.
    fn load_player_palettes(&mut self, world: &World, pl: &PlayerRef) {
        let info = &world.world_actor().info;
        let id = pl.player_actor().id;
        for f in info.trait_infos::<IndexedPlayerPaletteInfo>() {
            let base = self.base(&f.base_palette);
            let mut pal = base;
            if let Some(remap) = f.player_index.get(&pl.internal_name) {
                let ramp: Vec<i32> = if f.remap_index.is_empty() {
                    (0..256).collect()
                } else {
                    f.remap_index.clone()
                };
                for (&i, &r) in ramp.iter().zip(remap) {
                    pal[i as usize] = base[r as usize];
                }
            }
            self.add_player(&f.base_name, id, &pl.internal_name, pal);
        }
        for f in info.trait_infos::<PlayerColorPaletteInfo>() {
            let base = self.base(&f.base_palette);
            let c = pl.color().argb;
            let remap: Vec<usize> = f.remap_index.iter().map(|&i| i as usize).collect();
            let pal = palette::player_remap(&base, &remap, [(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            self.add_player(&f.base_name, id, &pl.internal_name, pal);
        }
        for f in info.trait_infos::<PaletteFromPlayerPaletteWithAlphaInfo>() {
            if let Some(base) = self.try_base(&format!("{}{}", f.base_palette, pl.internal_name)) {
                let pal = palette::alpha_remap(&base, f.alpha, f.premultiply);
                self.add_player(&f.base_name, id, &pl.internal_name, pal);
            }
        }
    }

    /// `WorldRenderer.UpdatePalettesForPlayer` for the players whose `Player.Color` changed.
    fn update_player_colors(&mut self, world: &World) -> bool {
        let changed: Vec<PlayerRef> = self
            .player_colors
            .iter_mut()
            .filter(|(pl, c)| pl.color() != *c)
            .map(|(pl, c)| {
                *c = pl.color();
                pl.clone()
            })
            .collect();
        for pl in &changed {
            self.load_player_palettes(world, pl);
        }
        !changed.is_empty()
    }

    fn base(&self, name: &str) -> Palette {
        let row = *self.named.get(name).unwrap_or_else(|| panic!("palette {name} is not defined"));
        self.rows[row as usize].base
    }

    fn try_base(&self, name: &str) -> Option<Palette> {
        self.named.get(name).map(|&row| self.rows[row as usize].base)
    }

    fn add(&mut self, name: &str, pal: Palette, allow_modifiers: bool) -> u32 {
        if let Some(&row) = self.named.get(name) {
            self.rows[row as usize].base = pal;
            return row;
        }
        let row = self.rows.len() as u32;
        assert!(row < MAX_PALETTES, "more than {MAX_PALETTES} palettes");
        self.rows.push(Row {
            name: name.to_string(),
            base: pal,
            allow_modifiers,
        });
        self.named.insert(name.to_string(), row);
        row
    }

    fn add_player(&mut self, base: &str, player_id: u32, internal_name: &str, pal: Palette) {
        let row = self.add(&format!("{base}{internal_name}"), pal, true);
        self.player.entry(base.to_string()).or_default().insert(player_id, row);
    }

    /// The row of a palette name, which must exist.
    pub fn named(&self, name: &str) -> u32 {
        self.named.get(name).copied().unwrap_or(0)
    }

    pub fn row(&self, p: &PaletteRef) -> u32 {
        if let Some(pl) = &p.player
            && let Some(&row) = self.player.get(&*p.name).and_then(|m| m.get(&pl.player_actor().id))
        {
            return row;
        }
        self.named(&p.name)
    }

    /// `RotationPaletteEffect.AdjustPalette` and `LightPaletteRotator.AdjustPalette`, once per world tick.
    pub fn update(&mut self, renderer: &Renderer, world: &World) {
        let world_tick = world.world_tick();
        if !self.update_player_colors(world) && world_tick == self.last_tick {
            return;
        }
        self.last_tick = world_tick;
        self.update_all(renderer, world_tick);
    }

    fn update_all(&self, renderer: &Renderer, world_tick: i32) {
        for (i, row) in self.rows.iter().enumerate() {
            let mut pal = row.base;
            if row.allow_modifiers {
                for r in &self.rotations {
                    apply_rotation(r, &row.name, &mut pal, world_tick);
                }
                for l in &self.lights {
                    if !l.exclude_palettes.iter().any(|e| e == &row.name) {
                        let t = (world_tick as f32 * l.time_step) as usize;
                        let index = l.rotation_indices[t % l.rotation_indices.len()];
                        pal[l.modify_index as usize] = pal[index as usize];
                    }
                }
            }
            renderer.set_palette(i as u32, &pal);
        }
    }

    fn filter_tileset(&mut self, tileset: &str) {
        self.rotations.retain(|r| {
            let contains = |l: &[String]| l.iter().any(|t| t.eq_ignore_ascii_case(tileset));
            if r.tilesets.is_empty() {
                !contains(&r.exclude_tilesets)
            } else {
                contains(&r.tilesets) && !contains(&r.exclude_tilesets)
            }
        });
    }
}

fn apply_rotation(info: &RotationPaletteEffectInfo, name: &str, pal: &mut Palette, world_tick: i32) {
    let starts = |l: &[String]| l.iter().any(|p| name.starts_with(p.as_str()));
    if (!info.palettes.is_empty() && !starts(&info.palettes)) || (!info.exclude_palettes.is_empty() && starts(&info.exclude_palettes)) {
        return;
    }
    let range = info.rotation_range as usize;
    let rotate = (world_tick as f32 * info.rotation_step) as usize % range;
    if rotate == 0 {
        return;
    }
    let base = info.rotation_base as usize;
    let mut buffer = vec![[0u8; 4]; range];
    for i in 0..range {
        buffer[(rotate + i) % range] = pal[base + i];
    }
    pal[base..base + range].copy_from_slice(&buffer);
}

/// `ShroudPalette.LoadPalettes` with premultiplied colors.
fn shroud_palette(fog: bool) -> Palette {
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];
    let black = |a: u8| [0, 0, 0, a];
    let c: [[u8; 4]; 8] = if fog {
        [black(0), GREEN, BLUE, YELLOW, black(128), black(96), black(64), black(32)]
    } else {
        [black(0), GREEN, BLUE, YELLOW, black(255), black(160), black(128), black(64)]
    };
    std::array::from_fn(|i| c[i % 8])
}
