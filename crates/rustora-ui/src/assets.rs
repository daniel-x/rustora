// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The shared chrome assets of a mod.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_fs::ReadFs;
use rustora_game::GameError;
use rustora_game::moddata::ModData;

use crate::chrome::{Chrome, Metrics, Sprite};
use crate::fluent::Messages;
use crate::font::{GlyphSheet, SpriteFont};

pub struct Assets {
    pub fs: Rc<dyn ReadFs>,
    pub chrome: Chrome,
    pub metrics: Metrics,
    pub messages: Messages,
    pub fonts: HashMap<String, SpriteFont>,
    pub glyphs: GlyphSheet,
    pub minimaps: HashMap<String, Option<Sprite>>,
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
            minimaps: HashMap::new(),
        })
    }

    /// Port of `SpriteFont.Measure`.
    pub fn measure(&mut self, font: &str, text: &str) -> (i32, i32) {
        self.fonts.get_mut(font).map_or((0, 0), |f| f.measure(text))
    }

    /// Port of `WidgetUtils.TruncateText`.
    pub fn truncate_text(&mut self, font: &str, text: &str, width: i32) -> String {
        if self.measure(font, text).0 <= width {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut length = chars.len();
        let mut trimmed = text.to_string();
        while self.measure(font, &trimmed).0 > width && length > 3 {
            length -= 1;
            trimmed = chars[..length - 3].iter().collect::<String>() + "...";
        }
        trimmed
    }

    /// Port of `WidgetUtils.WrapText`.
    pub fn wrap_text(&mut self, font: &str, text: &str, width: i32) -> String {
        if self.measure(font, text).0 <= width {
            return text.to_string();
        }
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].clone();
            if self.measure(font, &line).0 > width {
                let mut start = 0;
                while let Some(space) = line[start..].find(' ').map(|p| p + start) {
                    if self.measure(font, &line[..space]).0 > width {
                        break;
                    }
                    start = space + 1;
                }
                if start > 0 {
                    lines[i] = line[..start - 1].to_string();
                    lines.insert(i + 1, line[start..].to_string());
                }
            }
            i += 1;
        }
        lines.join("\n")
    }

    /// Assets without any chrome or font, for tests of widget behavior.
    pub fn empty(fs: Rc<dyn ReadFs>) -> Assets {
        let messages = Messages::new(&[], &*fs);
        Assets {
            fs,
            chrome: Chrome::new(&[]),
            metrics: Metrics::new(&[]),
            messages,
            fonts: HashMap::new(),
            glyphs: GlyphSheet::default(),
            minimaps: HashMap::new(),
        }
    }
}
