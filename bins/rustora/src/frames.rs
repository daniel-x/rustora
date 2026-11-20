// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The frames that the client builds and the window presents, the queue between the two threads, and the report
//! that returns a presented frame's buffers for reuse.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustora_render::{FrameCommands, GpuBackend, wgpu};

/// The recorded commands of one frame for a window of `size` pixels.
pub struct Frame {
    pub commands: FrameCommands,
    pub size: (u32, u32),
    /// `Game.TakeScreenshot`: the frame is also read back.
    pub screenshot: bool,
}

/// The frames that the simulation thread built and the main thread has not taken out yet, oldest first. The
/// simulation thread puts frames in and, with `Graphics.FrameSkip`, merges them; the main thread only takes them
/// out one by one.
#[derive(Clone, Default)]
pub struct FrameQueue(Arc<Mutex<VecDeque<Frame>>>);

impl FrameQueue {
    /// The most frames that wait without `Graphics.FrameSkip`.
    const CAPACITY: usize = 1;
    /// The most frames that wait with `Graphics.FrameSkip`.
    const SKIP_CAPACITY: usize = 2;

    fn frames(&self) -> std::sync::MutexGuard<'_, VecDeque<Frame>> {
        self.0.lock().expect("frame queue")
    }

    pub fn is_empty(&self) -> bool {
        self.frames().is_empty()
    }

    /// Takes out the oldest frame.
    pub fn pop(&self) -> Option<Frame> {
        self.frames().pop_front()
    }

    /// Moves the frame of `waiting` in if there is one and the queue has room. Returns true if it moved.
    pub fn push_waiting(&self, waiting: &mut Option<Frame>) -> bool {
        let mut frames = self.frames();
        if frames.len() >= Self::CAPACITY {
            return false;
        }
        let Some(frame) = waiting.take() else { return false };
        frames.push_back(frame);
        true
    }

    /// Puts `frame` in. If the queue is full, `frame` replaces the newest waiting frame, which keeps only its uploads,
    /// and the commands that are left over come back for reuse.
    pub fn push_merging(&self, mut frame: Frame) -> Option<FrameCommands> {
        let mut older = {
            let mut frames = self.frames();
            if frames.len() < Self::SKIP_CAPACITY {
                frames.push_back(frame);
                return None;
            }
            frames.pop_back().expect("full queue")
        };
        older.commands.merge_newer(&mut frame.commands);
        older.size = frame.size;
        older.screenshot |= frame.screenshot;
        self.frames().push_back(older);
        Some(frame.commands)
    }
}

/// Why the window ran only the uploads of a frame and did not draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The surface had no texture, and it is configured anew.
    NoTexture,
    /// A requested redraw did not come in time.
    Stalled,
    /// The window is hidden or has no surface.
    Hidden,
}

/// What the window did with a [`Frame`].
pub struct Presented {
    /// The commands of the frame, cleared by the receiver for the next frame.
    pub commands: FrameCommands,
    /// The draw calls of the frame, as `PerfHistory` "batches" counts them.
    pub draw_calls: u32,
    /// The time to execute the commands.
    pub execute: Duration,
    /// The time of the present.
    pub present: Duration,
    /// Why only the uploads of the frame ran, or `None` if it was drawn.
    pub skipped: Option<Skip>,
    /// The time that the main thread spent in its event handlers since its previous report.
    pub main_busy: Duration,
    /// The window pixels of the frame.
    pub size: (u32, u32),
    /// The RGBA rows of a frame with [`Frame::screenshot`], opaque.
    pub screenshot: Option<Vec<u8>>,
}

/// The window surface and the GPU backend that executes the frames.
pub struct Surface {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub backend: GpuBackend,
}

impl Surface {
    /// Executes only the uploads of `frame`, for a frame that is not drawn for `reason`. A screenshot frame is still
    /// read back.
    pub fn skip(&mut self, frame: Frame, reason: Skip) -> Presented {
        let start = Instant::now();
        let screenshot = self.read_back(&frame);
        if screenshot.is_none() {
            self.backend.upload(&frame.commands);
        }
        Presented {
            commands: frame.commands,
            draw_calls: self.backend.take_draw_calls(),
            execute: start.elapsed(),
            present: Duration::ZERO,
            main_busy: Duration::ZERO,
            skipped: Some(reason),
            size: frame.size,
            screenshot,
        }
    }

    /// Executes all commands of a screenshot frame offscreen and reads the pixels back as opaque RGBA rows.
    fn read_back(&mut self, frame: &Frame) -> Option<Vec<u8>> {
        if !frame.screenshot {
            return None;
        }
        let (w, h) = frame.size;
        let format = self.config.format;
        let mut pixels = rustora_render::render_offscreen_as(&mut self.backend, &frame.commands, w, h, format);
        if matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
        }
        pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
        Some(pixels)
    }

    /// Executes `frame` on the next surface texture and presents it. `pre_present` runs right before the present.
    /// Without a surface texture only the uploads run, and the surface is configured anew.
    pub fn present(&mut self, frame: Frame, pre_present: impl FnOnce()) -> Presented {
        let start = Instant::now();
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                let report = self.skip(frame, Skip::NoTexture);
                self.surface.configure(&self.backend.device, &self.config);
                return report;
            }
        };
        let view = texture.texture.create_view(&Default::default());
        let screenshot = self.read_back(&frame);
        if screenshot.is_some() {
            self.backend.draw(&frame.commands, &view);
        } else {
            self.backend.execute(&frame.commands, &view);
        }
        let execute = start.elapsed();
        let start = Instant::now();
        pre_present();
        self.backend.queue.present(texture);
        Presented {
            commands: frame.commands,
            draw_calls: self.backend.take_draw_calls(),
            execute,
            present: start.elapsed(),
            main_busy: Duration::ZERO,
            skipped: None,
            size: frame.size,
            screenshot,
        }
    }
}
