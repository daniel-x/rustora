// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! [`WorldSprites`] of the client: sequence frames turned into user interface textures with a palette.
//! Palettes: `chrome` and everything unknown use `temperat.pal` with shadow index 3; names starting with `player`
//! followed by a player's internal name get that player's color remap.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rustora_core::world_types::WAngle;
use rustora_engine::sequence::{ReadFile, SequenceSet};
use rustora_engine::world::World;
use rustora_formats::palette::Palette;
use rustora_formats::sprite::SpriteFrame;
use rustora_render::Renderer;
use rustora_ui::chrome::Sprite as UiSprite;
use rustora_ui::world_sprites::{UiFrame, WorldSprites};

use crate::sprites::SpriteCache;

const PLAYER_REMAP: [usize; 16] = [80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95];

type FrameKey = (String, String, i32, String);

pub struct UiSprites {
    cache: RefCell<SpriteCache>,
    sequences: Rc<SequenceSet>,
    raw: RefCell<HashMap<u32, Rc<Vec<SpriteFrame>>>>,
    frames: RefCell<HashMap<FrameKey, Option<UiFrame>>>,
    chrome: Palette,
    player_palettes: HashMap<String, Palette>,
}

impl UiSprites {
    pub fn new(world: &World, read_file: ReadFile) -> UiSprites {
        let pal = read_file("temperat.pal").expect("temperat.pal");
        let chrome = rustora_formats::palette::load(&pal, &[0], &[3]).expect("chrome palette");
        let player_base = rustora_formats::palette::load(&pal, &[0], &[4]).expect("player palette");
        let player_palettes = world
            .players()
            .iter()
            .map(|p| {
                let c = p.color.argb;
                let rgb = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
                let remapped = rustora_formats::palette::player_remap(&player_base, &PLAYER_REMAP, rgb);
                (p.internal_name.clone(), remapped)
            })
            .collect();
        UiSprites {
            cache: RefCell::new(SpriteCache::new(read_file, &world.map().tileset)),
            sequences: world.map().sequences().clone(),
            raw: RefCell::default(),
            frames: RefCell::default(),
            chrome,
            player_palettes,
        }
    }

    fn palette(&self, name: &str) -> &Palette {
        name.strip_prefix("player")
            .and_then(|p| self.player_palettes.get(p))
            .unwrap_or(&self.chrome)
    }

    fn raw_frames(&self, file: u32) -> Rc<Vec<SpriteFrame>> {
        let mut raw = self.raw.borrow_mut();
        raw.entry(file)
            .or_insert_with(|| {
                let cache = self.cache.borrow();
                let data = (cache.read_file())(cache.file_name(file));
                Rc::new(data.and_then(|d| rustora_formats::sprite::decode(&d)).unwrap_or_default())
            })
            .clone()
    }

    fn make_frame(&self, renderer: &mut Renderer, image: &str, sequence: &str, frame: i32, palette: &str) -> Option<UiFrame> {
        if !self.sequences.has_sequence(image, sequence) {
            return None;
        }
        let seq = self.sequences.get_sequence(image, sequence);
        let resolved = self.cache.borrow_mut().sequence(renderer, &seq)?;
        let s = resolved.sprite(frame, WAngle::ZERO)?;
        let raw = self.raw_frames(s.source.0);
        let f = raw.get(s.source.1 as usize)?;
        let pal = self.palette(palette);
        let rgba: Vec<u8> = f.data.iter().flat_map(|&i| pal[i as usize]).collect();
        let texture = renderer.add_ui_texture(f.width as u32, f.height as u32, &rgba, false);
        Some(UiFrame {
            sprite: UiSprite {
                texture,
                x: 0,
                y: 0,
                w: f.width as i32,
                h: f.height as i32,
            },
            offset: (s.offset[0], s.offset[1]),
        })
    }
}

impl WorldSprites for UiSprites {
    fn frame(&self, renderer: &mut Renderer, image: &str, sequence: &str, frame: i32, palette: &str) -> Option<UiFrame> {
        let key = (image.to_string(), sequence.to_string(), frame, palette.to_string());
        if let Some(f) = self.frames.borrow().get(&key) {
            return *f;
        }
        let f = self.make_frame(renderer, image, sequence, frame, palette);
        self.frames.borrow_mut().insert(key, f);
        f
    }

    fn length(&self, renderer: &mut Renderer, image: &str, sequence: &str) -> i32 {
        if !self.sequences.has_sequence(image, sequence) {
            return 0;
        }
        let seq = self.sequences.get_sequence(image, sequence);
        self.cache.borrow_mut().sequence(renderer, &seq).map_or(0, |r| r.length)
    }
}
