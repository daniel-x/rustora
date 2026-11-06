// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `VideoPlayerWidget.cs`. The video loads synchronously, also for `LoadAndPlayAsync`, and the window
//! scale is 1.

use std::time::{Duration, Instant};

use rustora_formats::audio::Pcm;
use rustora_formats::vqa::{VqaVideo, is_vqa};
use rustora_render::ui::UiTexture;

use super::impl_as_any;
use crate::chrome::Sprite;
use crate::draw::DrawContext;
use crate::hotkeys::Hotkey;
use crate::input::{KeyEvent, KeyInput, MouseInput};
use crate::logic::services;
use crate::widget::{Ui, WidgetId, WidgetKind};

/// What runs when a video ends or is stopped.
pub type VideoAction = Box<dyn FnOnce(&mut Ui)>;

/// `Stopwatch` of the play time.
#[derive(Default)]
struct PlayTime {
    elapsed: Duration,
    started: Option<Instant>,
}

impl PlayTime {
    fn elapsed(&self) -> Duration {
        self.elapsed + self.started.map_or(Duration::ZERO, |s| s.elapsed())
    }

    fn running(&self) -> bool {
        self.started.is_some()
    }

    fn start(&mut self) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
    }

    fn stop(&mut self) {
        if let Some(s) = self.started.take() {
            self.elapsed += s.elapsed();
        }
    }

    fn reset(&mut self) {
        *self = PlayTime::default();
    }
}

pub struct VideoPlayerWidget {
    pub cancel_key: Option<Hotkey>,
    pub aspect_ratio: f32,
    pub draw_overlay: bool,
    pub skippable: bool,
    pub video: Option<VqaVideo>,
    cached_video_file_name: Option<String>,
    texture: Option<UiTexture>,
    texture_dirty: bool,
    texture_size: (u32, u32),
    inv_length: f32,
    video_origin: (f32, f32),
    video_size: (f32, f32),
    overlay: Option<(UiTexture, (f32, f32), (f32, f32), i32)>,
    play_time: PlayTime,
    on_complete: Option<VideoAction>,
    /// Set by `Stop`: the completion runs after the next UI tick, as `Game.RunAfterTick`.
    stopped: bool,
}

impl Default for VideoPlayerWidget {
    fn default() -> Self {
        VideoPlayerWidget {
            cancel_key: Hotkey::parse("ESCAPE"),
            aspect_ratio: 1.2,
            draw_overlay: true,
            skippable: true,
            video: None,
            cached_video_file_name: None,
            texture: None,
            texture_dirty: false,
            texture_size: (0, 0),
            inv_length: 0.0,
            video_origin: (0.0, 0.0),
            video_size: (0.0, 0.0),
            overlay: None,
            play_time: PlayTime::default(),
            on_complete: None,
            stopped: false,
        }
    }
}

/// The 16 bit samples of the video audio.
fn video_pcm(v: &VqaVideo) -> Pcm {
    let samples = if v.sample_bits == 8 {
        v.audio_data.iter().map(|&b| ((b as i16) - 128) << 8).collect()
    } else {
        v.audio_data.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect()
    };
    Pcm {
        channels: v.audio_channels.max(1) as u16,
        sample_rate: v.sample_rate as u32,
        samples,
    }
}

/// `Game.RunAfterTick` of a one-shot action.
pub fn run_after_tick(ui: &mut Ui, f: VideoAction) {
    let f = std::cell::RefCell::new(Some(f));
    ui.run_after_tick(move |ui| {
        if let Some(f) = f.borrow_mut().take() {
            f(ui);
        }
    });
}

/// Opens a video of the mod file system, `Game.ModData.DefaultFileSystem.Open` with `VideoLoader.GetVideo`.
/// Returns `None` for a missing file, the `FileNotFoundException` of C#.
pub fn open_video(ui: &Ui, filename: &str) -> Option<Result<VqaVideo, String>> {
    let data = ui.assets.borrow().fs.try_open(filename)?;
    if !is_vqa(&data) {
        return Some(Err(format!("{filename} is not a supported video")));
    }
    Some(VqaVideo::new(data, true))
}

/// Clones the configuration of a template widget; the playing state starts fresh.
impl Clone for VideoPlayerWidget {
    fn clone(&self) -> Self {
        VideoPlayerWidget {
            cancel_key: self.cancel_key.clone(),
            aspect_ratio: self.aspect_ratio,
            draw_overlay: self.draw_overlay,
            skippable: self.skippable,
            ..VideoPlayerWidget::default()
        }
    }
}

impl VideoPlayerWidget {
    pub fn paused(&self) -> bool {
        !self.play_time.running()
    }

    /// `LoadAndPlay`: returns false if the file does not exist.
    pub fn load_and_play(&mut self, ui: &mut Ui, id: WidgetId, filename: &str) -> bool {
        if self.cached_video_file_name.as_deref() == Some(filename) {
            return true;
        }
        self.cached_video_file_name = Some(filename.to_string());
        match open_video(ui, filename) {
            None => false,
            Some(video) => {
                match video {
                    Ok(v) => self.play(ui, id, Some(v)),
                    Err(e) => log::warn!("{e}"),
                }
                true
            }
        }
    }

    /// `LoadAndPlayAsync`: `after` runs when the video ends, or right away if the file does not exist.
    pub fn load_and_play_async(&mut self, ui: &mut Ui, id: WidgetId, filename: &str, after: VideoAction) {
        if self.cached_video_file_name.as_deref() == Some(filename) {
            return;
        }
        self.cached_video_file_name = Some(filename.to_string());
        if self.video.is_some() {
            self.close_video(ui);
        }
        match open_video(ui, filename) {
            None | Some(Err(_)) => run_after_tick(ui, after),
            Some(Ok(v)) => {
                self.play(ui, id, Some(v));
                self.play_then(ui, after);
            }
        }
    }

    /// `Play(IVideo)`.
    pub fn play(&mut self, ui: &mut Ui, id: WidgetId, video: Option<VqaVideo>) {
        self.video = video;
        let Some(v) = &self.video else { return };
        self.play_time.reset();
        services(ui).sound.stop_video();
        self.on_complete = Some(Box::new(|_| {}));
        self.stopped = false;
        self.inv_length = v.framerate as f32 / v.frame_count.max(1) as f32;
        self.texture_size = (v.total_frame_width as u32, v.total_frame_width as u32);
        self.texture = None;
        self.texture_dirty = true;
        let rb = ui.render_bounds(id);
        let (w, h) = (v.width as f32, v.height as f32);
        let scale = (rb.w as f32 / w).min(rb.h as f32 / (h * self.aspect_ratio));
        self.video_origin = (
            rb.x as f32 + (rb.w as f32 - scale * w) / 2.0,
            rb.y as f32 + (rb.h as f32 - scale * h * self.aspect_ratio) / 2.0,
        );
        self.video_size = ((w * scale).ceil(), (h * self.aspect_ratio * scale).ceil());
    }

    /// `PlayThen`.
    pub fn play_then(&mut self, ui: &mut Ui, after: VideoAction) {
        let Some(v) = &self.video else { return };
        self.on_complete = Some(after);
        let sound = services(ui).sound.clone();
        if self.play_time.elapsed().is_zero() && v.has_audio() {
            sound.play_video(video_pcm(v));
        } else {
            sound.set_video_paused(false);
        }
        self.play_time.start();
    }

    /// `Pause`.
    pub fn pause(&mut self, ui: &Ui) {
        if self.paused() || self.video.is_none() {
            return;
        }
        self.play_time.stop();
        services(ui).sound.set_video_paused(true);
    }

    /// `Stop`.
    pub fn stop(&mut self, ui: &Ui) {
        let Some(v) = &mut self.video else { return };
        self.play_time.reset();
        services(ui).sound.stop_video();
        if let Err(e) = v.reset() {
            log::warn!("{e}");
        }
        self.texture_dirty = true;
        self.stopped = true;
    }

    /// `CloseVideo`.
    pub fn close_video(&mut self, ui: &mut Ui) {
        self.stop(ui);
        self.video = None;
    }

    fn upload(&mut self, dc: &mut DrawContext) {
        let Some(v) = &self.video else { return };
        let rgba: Vec<u8> = v
            .current_frame_data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        let (w, h) = self.texture_size;
        match self.texture {
            None => self.texture = Some(dc.renderer.add_ui_texture(w, h, &rgba, true)),
            Some(t) => dc.renderer.update_ui_texture(t, 0, 0, w, h, &rgba),
        }
        self.texture_dirty = false;
    }

    fn draw_overlay_lines(&mut self, ui: &Ui, id: WidgetId, dc: &mut DrawContext) {
        let Some(v) = &self.video else { return };
        if self.overlay.is_none() {
            let rb = ui.render_bounds(id);
            let video_scale = (rb.w as f32 / v.width as f32).min(rb.h as f32 / (v.height as f32 * self.aspect_ratio));
            let half_row_height = (video_scale / 2.0 + 0.5) as i32;
            if half_row_height == 0 {
                self.draw_overlay = false;
                return;
            }
            let overlay_height = rb.h / half_row_height;
            let sheet_height = (overlay_height.max(1) as u32).next_power_of_two();
            let mut overlay = vec![0u8; 4 * sheet_height as usize];
            let mut i = 3;
            while i < 4 * overlay_height as usize {
                overlay[i] = 128;
                i += 8;
            }
            let texture = dc.renderer.add_ui_texture(1, sheet_height, &overlay, false);
            let origin = (rb.x as f32, rb.y as f32);
            let size = (rb.w as f32, (overlay_height * half_row_height) as f32);
            self.overlay = Some((texture, origin, size, overlay_height));
        }
        if let Some((texture, origin, size, height)) = self.overlay {
            let sprite = Sprite {
                texture,
                x: 0,
                y: 0,
                w: 1,
                h: height,
            };
            dc.draw_sprite_sized(sprite, origin.0, origin.1, size.0, size.1);
        }
    }
}

impl WidgetKind for VideoPlayerWidget {
    impl_as_any!();

    fn set_field(&mut self, key: &str, value: &str) -> bool {
        match key {
            "CancelKey" => self.cancel_key = Hotkey::parse(value),
            "AspectRatio" => self.aspect_ratio = value.trim().parse().unwrap_or(1.2),
            "DrawOverlay" => self.draw_overlay = value.trim().eq_ignore_ascii_case("true"),
            "Skippable" => self.skippable = value.trim().eq_ignore_ascii_case("true"),
            _ => return false,
        }
        true
    }

    fn tick(&mut self, ui: &mut Ui, _id: WidgetId) {
        if self.stopped {
            self.stopped = false;
            if let Some(f) = self.on_complete.take() {
                run_after_tick(ui, f);
            }
        }
    }

    fn draw(&mut self, ui: &mut Ui, id: WidgetId, dc: &mut DrawContext) {
        let Some(v) = &mut self.video else { return };
        if self.play_time.running() {
            let sound = services(ui).sound.clone();
            let position = if v.has_audio() && sound.video_playing() {
                sound.video_seek_position()
            } else {
                self.play_time.elapsed().as_secs_f32()
            };
            let next_frame = (v.frame_count as f32 * position * self.inv_length) as i32;
            if next_frame > v.frame_count as i32 || next_frame < v.current_frame_index {
                self.stop(ui);
                return;
            }
            let mut skipped = 0;
            while next_frame > v.current_frame_index {
                if let Err(e) = v.advance_frame() {
                    log::warn!("{e}");
                    break;
                }
                skipped += 1;
            }
            if skipped > 0 {
                self.texture_dirty = true;
            }
        }
        if self.texture_dirty || self.texture.is_none() {
            self.upload(dc);
        }
        let Some(v) = &self.video else { return };
        if let Some(texture) = self.texture {
            let sprite = Sprite {
                texture,
                x: 0,
                y: 0,
                w: v.width as i32,
                h: v.height as i32,
            };
            dc.draw_sprite_sized(
                sprite,
                self.video_origin.0,
                self.video_origin.1,
                self.video_size.0,
                self.video_size.1,
            );
        }
        if self.draw_overlay {
            self.draw_overlay_lines(ui, id, dc);
        }
    }

    fn handle_key(&mut self, ui: &mut Ui, _id: WidgetId, e: &KeyInput) -> bool {
        if !self.skippable || e.event != KeyEvent::Down || self.cancel_key.is_none() {
            return false;
        }
        if Some(Hotkey::from_key_input(e)) != self.cancel_key {
            return false;
        }
        self.stop(ui);
        true
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: WidgetId, mi: &MouseInput) -> bool {
        ui.render_bounds(id).contains(mi.x, mi.y) && self.skippable
    }
}
