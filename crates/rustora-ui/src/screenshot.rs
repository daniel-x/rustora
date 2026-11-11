// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Game.TakeScreenshotInner`.

use std::path::Path;

use crate::fluent::MessageArg;
use crate::logic::chat_commands::add_debug_line;
use crate::logic::{message_with, services};
use crate::widget::Ui;

/// Saves the RGBA rows of a frame as `Screenshots/<mod>/<version>/<mod>-<UTC time>.png` in the support directory and
/// reports the file name in the chat.
pub fn save(ui: &Ui, support_dir: &Path, rgba: &[u8], width: u32, height: u32) {
    let service = services(ui);
    let directory = support_dir.join("Screenshots").join(&service.mod_id).join(&service.mod_version);
    let filename = format!("{}-{}", service.mod_id, chrono::Utc::now().format("%Y-%m-%dT%H%M%S%3fZ"));
    let path = directory.join(format!("{filename}.png"));
    log::debug!("Taking screenshot {}", path.display());
    if let Err(e) = std::fs::create_dir_all(&directory).and_then(|_| write_png(&path, rgba, width, height)) {
        log::warn!("Cannot save screenshot {}: {e}", path.display());
        return;
    }
    add_debug_line(
        ui,
        message_with(ui, "notification-saved-screenshot", &[("filename", MessageArg::Text(&filename))]),
    );
}

fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(rgba).map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)
}
