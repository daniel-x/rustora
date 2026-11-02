// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sprite file formats. `frame_count` is enough for the simulation; decoding is added for the renderer.

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

/// `ShpTDLoader.IsShpTD`.
pub fn is_shp_td(d: &[u8]) -> bool {
    let Some(image_count) = u16_at(d, 0) else { return false };
    if image_count == 0 {
        return false;
    }
    let final_offset = 14 + 8 * image_count as usize;
    if final_offset > d.len() || u32_at(d, final_offset) != Some(d.len() as u32) {
        return false;
    }
    matches!(d.get(17), Some(0x20 | 0x40 | 0x80))
}

/// `TmpRALoader.IsTmpRA`.
pub fn is_tmp_ra(d: &[u8]) -> bool {
    u32_at(d, 20) == Some(0) && u16_at(d, 26) == Some(0x2c73)
}

/// Number of frames in a sprite file, trying the loaders in the order of `SpriteFormats`.
pub fn frame_count(d: &[u8]) -> Option<usize> {
    if is_shp_td(d) {
        return u16_at(d, 0).map(|c| c as usize);
    }
    if is_tmp_ra(d) {
        let index_end = u32_at(d, 28)? as usize;
        let index_start = u32_at(d, 36)? as usize;
        return Some(index_end.checked_sub(index_start)?);
    }
    None
}
