// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The shared chrome assets of a mod.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_fs::ReadFs;
use rustora_game::GameError;
use rustora_game::moddata::ModData;

use crate::chrome::{Chrome, Metrics};
use crate::fluent::Messages;
use crate::font::{GlyphSheet, SpriteFont};

pub struct Assets {
    pub fs: Rc<dyn ReadFs>,
    pub chrome: Chrome,
    pub metrics: Metrics,
    pub messages: Messages,
    pub fonts: HashMap<String, SpriteFont>,
    pub glyphs: GlyphSheet,
}

impl Assets {
    pub fn load(mod_data: &ModData, fs: Rc<dyn ReadFs>) -> Result<Assets, GameError> {
        let manifest = &mod_data.manifest;
        let chrome = ModData::load_yaml(&*fs, &manifest.chrome, None)?;
        let metrics = ModData::load_yaml(&*fs, &manifest.chrome_metrics, None)?;
        let mut fonts = HashMap::new();
        if let Some(section) = manifest.section("Fonts") {
            for font in &section.nodes {
                let field = |key: &str| {
                    font.value
                        .nodes
                        .iter()
                        .find(|n| n.key == key)
                        .map(|n| n.value.value_str().to_string())
                        .unwrap_or_default()
                };
                let data = fs.open(&field("Font")).map_err(|e| GameError::Data(e.to_string()))?;
                let size = field("Size").parse().unwrap_or(12);
                let ascender = field("Ascender").parse().unwrap_or(size);
                fonts.insert(font.key.clone(), SpriteFont::new(data, size, ascender));
            }
        }
        let messages = Messages::new(&manifest.fluent_messages, &*fs);
        Ok(Assets {
            fs,
            chrome: Chrome::new(&chrome),
            metrics: Metrics::new(&metrics),
            messages,
            fonts,
            glyphs: GlyphSheet::default(),
        })
    }
}
