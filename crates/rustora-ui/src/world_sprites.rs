// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sequence frames of the world drawn in the user interface with a world palette, as `WidgetUtils.DrawSpriteCentered`
//! does with a `PaletteReference`. The client provides the frames because it owns the sprite sheets.

use rustora_render::Renderer;

use crate::chrome::Sprite;
use crate::draw::DrawContext;

/// A frame as a user interface sprite and its offset from the frame center.
#[derive(Clone, Copy, Debug)]
pub struct UiFrame {
    pub sprite: Sprite,
    pub offset: (f32, f32),
}

pub trait WorldSprites {
    /// Frame `frame` of `image.sequence` at facing zero drawn with `palette`, `None` if the sequence does not exist.
    fn frame(&self, renderer: &mut Renderer, image: &str, sequence: &str, frame: i32, palette: &str) -> Option<UiFrame>;

    /// The length of a sequence, 0 if it does not exist.
    fn length(&self, renderer: &mut Renderer, image: &str, sequence: &str) -> i32;
}

impl DrawContext<'_> {
    /// `WidgetUtils.DrawSpriteCentered`.
    pub fn draw_frame_centered(&mut self, f: UiFrame, pos: (f32, f32)) {
        let s = f.sprite;
        self.draw_sprite(s, pos.0 - s.w as f32 / 2.0 + f.offset.0, pos.1 - s.h as f32 / 2.0 + f.offset.1);
    }
}
