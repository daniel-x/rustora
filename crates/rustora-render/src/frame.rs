// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The front of the renderer, like the message queue of `ThreadedGraphicsContext`: the calls that return a value do
//! their CPU work at once, all others are recorded as commands into [`FrameCommands`], which a
//! [`crate::GpuBackend`] executes later in the same order.

use std::ops::Range;

use rustora_formats::palette::Palette;

use crate::post::PostPass;
use crate::ui::{UiList, UiTexture};
use crate::{Camera, MAX_PALETTES, Quad, SHEET_SIZE, SpriteRect};

/// One recorded renderer call. Pixel data and quads live in the flat buffers of [`FrameCommands`].
#[derive(Clone, Debug)]
pub(crate) enum Command {
    /// Creates the next sprite sheet.
    AddSheet,
    /// Writes 8-bit indexed pixels into a sheet.
    Sprite { rect: SpriteRect, data: Range<usize> },
    /// Writes one row of the palette texture.
    Palette { row: u32, data: Range<usize> },
    /// Creates the next user interface texture from premultiplied RGBA rows.
    AddUiTexture {
        width: u32,
        height: u32,
        linear: bool,
        data: Range<usize>,
    },
    /// Replaces a region of a user interface texture.
    UpdateUiTexture {
        texture: UiTexture,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        data: Range<usize>,
    },
    /// Clears the target to black and draws world quads with their post passes.
    World {
        camera: Camera,
        ui_scale: f32,
        quads: Range<usize>,
        passes: Range<usize>,
    },
    /// Draws a user interface list over the target.
    Ui { size: (u32, u32), scale: f32, list: usize },
}

impl Command {
    /// True for the commands that change GPU resources which later frames use, false for the draws.
    pub(crate) fn is_upload(&self) -> bool {
        !matches!(self, Command::World { .. } | Command::Ui { .. })
    }
}

/// The recorded renderer calls of one frame. The buffers keep their capacity when the frame is cleared, so a
/// recycled frame records the next one without allocating.
#[derive(Default)]
pub struct FrameCommands {
    pub(crate) commands: Vec<Command>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) passes: Vec<PostPass>,
    pub(crate) ui_lists: Vec<UiList>,
    /// The entries of `ui_lists` that this frame uses; the others wait for reuse.
    pub(crate) ui_lists_used: usize,
}

impl FrameCommands {
    /// Removes all commands and keeps the capacity of the buffers.
    pub fn clear(&mut self) {
        self.commands.clear();
        self.bytes.clear();
        self.quads.clear();
        self.passes.clear();
        self.ui_lists_used = 0;
    }

    /// True if nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    fn push_bytes(&mut self, data: &[u8]) -> Range<usize> {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(data);
        start..self.bytes.len()
    }
}

/// [`FrameCommands`] cross from the thread that builds a frame to the thread that executes it.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<FrameCommands>();
};

/// Shelf packer state of the current sheet.
struct Packer {
    x: u32,
    y: u32,
    row_height: u32,
}

/// The renderer API that the scene, the palettes, the fonts and the widgets call. It packs sprites, hands out user
/// interface texture ids and records everything else into the current [`FrameCommands`].
pub struct Renderer {
    /// `Graphics.UIScale`: user interface pixels per window pixel.
    pub ui_scale: f32,
    packer: Packer,
    sheets: usize,
    ui_textures: u32,
    frame: FrameCommands,
    spare: Vec<FrameCommands>,
}

impl Default for Renderer {
    fn default() -> Renderer {
        Renderer::new()
    }
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            ui_scale: 1.0,
            packer: Packer { x: 0, y: 0, row_height: 0 },
            sheets: 0,
            ui_textures: 1,
            frame: FrameCommands::default(),
            spare: Vec::new(),
        }
    }

    /// Hands out the commands recorded so far and starts a new frame, on recycled buffers if there are any.
    pub fn take_frame(&mut self) -> FrameCommands {
        let next = self.spare.pop().unwrap_or_default();
        std::mem::replace(&mut self.frame, next)
    }

    /// Keeps the buffers of an executed frame for a later one.
    pub fn recycle(&mut self, mut frame: FrameCommands) {
        frame.clear();
        self.spare.push(frame);
    }

    fn add_sheet(&mut self) {
        self.sheets += 1;
        self.packer = Packer { x: 0, y: 0, row_height: 0 };
        self.frame.commands.push(Command::AddSheet);
    }

    /// Creates a user interface texture from premultiplied RGBA rows.
    pub fn add_ui_texture(&mut self, width: u32, height: u32, rgba: &[u8], linear: bool) -> UiTexture {
        let data = self.frame.push_bytes(rgba);
        self.frame.commands.push(Command::AddUiTexture {
            width,
            height,
            linear,
            data,
        });
        let id = UiTexture(self.ui_textures);
        self.ui_textures += 1;
        id
    }

    /// Replaces a region of a user interface texture.
    pub fn update_ui_texture(&mut self, texture: UiTexture, x: u32, y: u32, width: u32, height: u32, rgba: &[u8]) {
        if width == 0 || height == 0 {
            return;
        }
        let data = self.frame.push_bytes(rgba);
        self.frame.commands.push(Command::UpdateUiTexture {
            texture,
            x,
            y,
            width,
            height,
            data,
        });
    }

    /// Uploads an 8-bit indexed image of `w` x `h` pixels. A 1 px gap separates sprites.
    pub fn add_sprite(&mut self, w: u32, h: u32, data: &[u8]) -> SpriteRect {
        assert!(w < SHEET_SIZE && h < SHEET_SIZE, "sprite of {w}x{h} exceeds the sheet size");
        if self.sheets == 0 {
            self.add_sheet();
        }
        if self.packer.x + w > SHEET_SIZE {
            self.packer = Packer {
                x: 0,
                y: self.packer.y + self.packer.row_height + 1,
                row_height: 0,
            };
        }
        if self.packer.y + h > SHEET_SIZE {
            self.add_sheet();
        }
        let p = &mut self.packer;
        let rect = SpriteRect {
            sheet: (self.sheets - 1) as u16,
            x: p.x as u16,
            y: p.y as u16,
            w: w as u16,
            h: h as u16,
        };
        p.x += w + 1;
        p.row_height = p.row_height.max(h);
        if w > 0 && h > 0 {
            let data = self.frame.push_bytes(&data[..(w * h) as usize]);
            self.frame.commands.push(Command::Sprite { rect, data });
        }
        rect
    }

    pub fn set_palette(&mut self, row: u32, palette: &Palette) {
        assert!(row < MAX_PALETTES);
        let data = self.frame.push_bytes(palette.as_flattened());
        self.frame.commands.push(Command::Palette { row, data });
    }

    /// Clears the target to black and draws `quads` in order.
    pub fn render(&mut self, camera: Camera, quads: &[Quad]) {
        self.render_with_passes(camera, quads, &[]);
    }

    /// Draws `quads` like [`Renderer::render`], with the post-processing `passes` between them.
    ///
    /// Like `Renderer.BeginWorld` and `BeginUI`, the world draws into a buffer with one texel per world pixel, which
    /// the pixel art scaling then stretches onto the target by the zoom and the user interface scale. Without passes
    /// and at a total scale of 1, the quads draw directly onto the target.
    pub fn render_with_passes(&mut self, camera: Camera, quads: &[Quad], passes: &[PostPass]) {
        if self.sheets == 0 {
            self.add_sheet();
        }
        let f = &mut self.frame;
        let quad_start = f.quads.len();
        f.quads.extend_from_slice(quads);
        let pass_start = f.passes.len();
        f.passes.extend_from_slice(passes);
        f.commands.push(Command::World {
            camera,
            ui_scale: self.ui_scale,
            quads: quad_start..f.quads.len(),
            passes: pass_start..f.passes.len(),
        });
    }

    /// Draws the user interface over the current content of the target of `size` window pixels.
    pub fn render_ui(&mut self, size: (u32, u32), list: &UiList) {
        let f = &mut self.frame;
        if f.ui_lists_used == f.ui_lists.len() {
            f.ui_lists.push(UiList::default());
        }
        let index = f.ui_lists_used;
        f.ui_lists[index].copy_from(list);
        f.ui_lists_used += 1;
        f.commands.push(Command::Ui {
            size,
            scale: self.ui_scale,
            list: index,
        });
    }
}
