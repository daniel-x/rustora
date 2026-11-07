// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `LoadScreens/LogoStripeLoadScreen.cs` and its base `SheetLoadScreen.cs`: the logo on a stripe with a
//! random "Reticulating Splines..." line, shown while a world loads.

use rustora_core::fieldloader::Color;
use rustora_core::miniyaml::MiniYaml;

use crate::chrome::{Sprite, decode_png};
use crate::draw::{DrawContext, Rect, TextStyle};

const LOADING: &str = "loadscreen-loading";

/// `LogoStripeLoadScreen`. Only the 1x image is used.
pub struct LoadScreen {
    image: Option<String>,
    sheet: Option<Option<Sprite>>,
}

impl LoadScreen {
    /// `Init`: the `LoadScreen` node of the mod manifest.
    pub fn new(info: &MiniYaml) -> LoadScreen {
        let image = info
            .nodes
            .iter()
            .find(|n| n.key == "Image")
            .map(|n| n.value.value_str().to_string());
        LoadScreen { image, sheet: None }
    }

    /// `DisplayInner`: draws the stripe, the logo and one random loading message for the screen size.
    pub fn draw(&mut self, dc: &mut DrawContext, resolution: (i32, i32)) {
        let sheet = *self.sheet.get_or_insert_with(|| {
            let data = dc.assets.fs.try_open(self.image.as_deref()?)?;
            let (w, h, rgba) = decode_png(&data).ok()?;
            Some(Sprite {
                texture: dc.renderer.add_ui_texture(w, h, &rgba, true),
                x: 0,
                y: 0,
                w: w as i32,
                h: h as i32,
            })
        });
        let (width, height) = resolution;
        if let Some(s) = sheet {
            let stripe = Sprite {
                x: 258,
                y: 0,
                w: 253,
                h: 256,
                ..s
            };
            let logo = Sprite {
                x: 0,
                y: 0,
                w: 256,
                h: 256,
                ..s
            };
            dc.fill_rect_with_sprite(Rect::new(0, height / 2 - 128, width, 256), stripe);
            dc.draw_sprite(logo, (width / 2 - 128) as f32, (height / 2 - 128) as f32);
        }
        let all = dc.assets.messages.get(LOADING);
        let messages: Vec<&str> = all.split(',').map(str::trim).filter(|m| !m.is_empty()).collect();
        if messages.is_empty() {
            return;
        }
        let text = messages[crate::logic::cosmetic_random(messages.len())];
        let (tw, th) = dc.measure("Bold", text);
        dc.draw_text(
            "Bold",
            text,
            width - tw - 20,
            height - th - 20,
            Color::from_argb(255, 255, 255, 255),
            TextStyle::Plain,
        );
    }
}
