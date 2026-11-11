// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The shared chrome assets of a mod.

use std::collections::HashMap;
use std::rc::Rc;

use rustora_fs::ReadFs;
use rustora_game::GameError;
use rustora_game::moddata::ModData;
use rustora_render::ui::UiTexture;

use crate::chrome::{Chrome, Metrics, Sprite};
use crate::fluent::Messages;
use crate::font::{GlyphSheet, SpriteFont};
use crate::hotkeys::Hotkeys;

pub struct Assets {
    pub fs: Rc<dyn ReadFs>,
    pub chrome: Chrome,
    pub metrics: Metrics,
    pub messages: Messages,
    pub fonts: HashMap<String, SpriteFont>,
    pub glyphs: GlyphSheet,
    pub minimaps: HashMap<String, Option<Sprite>>,
    /// The texture of `HueSliderWidget`, shared by all choosers.
    pub hue_texture: Option<UiTexture>,
    /// The texture of `ColorMixerWidget` and the hue it shows.
    pub mixer_texture: Option<(UiTexture, f32)>,
    pub hotkeys: Hotkeys,
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
        let hotkeys = Hotkeys::load(&ModData::load_yaml(&*fs, &manifest.hotkeys, None)?);
        let messages = Messages::new(&manifest.fluent_messages, &*fs);
        Ok(Assets {
            fs,
            chrome: Chrome::new(&chrome),
            metrics: Metrics::new(&metrics),
            messages,
            fonts,
            glyphs: GlyphSheet::default(),
            minimaps: HashMap::new(),
            hue_texture: None,
            mixer_texture: None,
            hotkeys,
        })
    }

    /// `Renderer.SetUIScale` for the fonts: all glyphs are rasterized anew at `scale`.
    pub fn set_font_scale(&mut self, scale: f32) {
        if self.fonts.values().all(|f| f.scale() == scale) {
            return;
        }
        for f in self.fonts.values_mut() {
            f.set_scale(scale);
        }
        self.glyphs.reset();
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

    /// `wrap_text` that also breaks words wider than `width` between characters, instead of letting them run past
    /// the right edge. Each row ends at its last fitting space, or after its last fitting character when no space
    /// fits; a row keeps at least one character.
    pub fn wrap_text_breaking_words(&mut self, font: &str, text: &str, width: i32) -> String {
        let mut rows = Vec::new();
        for line in text.split('\n') {
            let mut rest = line;
            while self.measure(font, rest).0 > width {
                let mut fit = rest.chars().next().map_or(0, char::len_utf8);
                let mut space = None;
                for (i, c) in rest.char_indices() {
                    let end = i + c.len_utf8();
                    if c == ' ' && i > 0 && self.measure(font, &rest[..i]).0 <= width {
                        space = Some(i);
                    }
                    if self.measure(font, &rest[..end]).0 > width {
                        break;
                    }
                    fit = fit.max(end);
                }
                if let Some(s) = space {
                    rows.push(&rest[..s]);
                    rest = &rest[s + 1..];
                } else {
                    rows.push(&rest[..fit]);
                    rest = &rest[fit..];
                }
            }
            rows.push(rest);
        }
        rows.join("\n")
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
            hue_texture: None,
            mixer_texture: None,
            hotkeys: Hotkeys::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoFiles;

    impl ReadFs for NoFiles {
        fn try_open(&self, _filename: &str) -> Option<Vec<u8>> {
            None
        }

        fn exists(&self, _filename: &str) -> bool {
            false
        }
    }

    fn assets_with_font() -> Option<Assets> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../openra-ref/mods/common/FreeSans.ttf");
        let data = std::fs::read(path).ok()?;
        let mut assets = Assets::empty(Rc::new(NoFiles));
        assets.fonts.insert("Regular".into(), SpriteFont::new(data, 14, 14));
        Some(assets)
    }

    #[test]
    fn breaking_wrap_keeps_every_row_inside_the_width() {
        let Some(mut assets) = assets_with_font() else { return };
        let text = "Connecting to ##openra-ingame-general-and-a-much-longer-name... ok";
        for scale in [1.0, 1.5, 3.0] {
            assets.set_font_scale(scale);
            let width = assets.measure("Regular", "##openra-ingame").0;
            let wrapped = assets.wrap_text_breaking_words("Regular", text, width);
            for row in wrapped.split('\n') {
                assert!(assets.measure("Regular", row).0 <= width, "{row:?} at {scale}");
            }
            assert_eq!(wrapped.replace(['\n', ' '], ""), text.replace(' ', ""));
            assert!(wrapped.starts_with("Connecting to\n"));
        }
    }
}
