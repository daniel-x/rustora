// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sprite file formats: `frame_count` for the simulation, `decode` for the renderer. Port of `ShpD2Loader.cs`, `ShpTDLoader.cs` and `TmpRALoader.cs`.

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

/// `ShpD2Loader.IsShpD2`.
pub fn is_shp_d2(d: &[u8]) -> bool {
    let Some(image_count) = u16_at(d, 0).filter(|&c| c != 0) else {
        return false;
    };
    let Some(test_offset) = u32_at(d, 2) else { return false };
    let two_bytes = test_offset & 0xff0000 > 0;
    let final_offset = 2 + if two_bytes { 2 } else { 4 } * image_count as usize;
    if final_offset > d.len() {
        return false;
    }
    let eof = if two_bytes {
        u16_at(d, final_offset).map(u32::from)
    } else {
        u32_at(d, final_offset)
    };
    if eof.map(|e| e as usize + 2) != Some(d.len()) {
        return false;
    }
    u16_at(d, final_offset + if two_bytes { 2 } else { 4 }).is_some_and(|b| b == 5 || b <= 3)
}

/// `RLEZerosCompression.DecodeInto`.
fn rle_zeros_decode_into(src: &[u8], dest: &mut [u8]) -> Option<()> {
    let (mut s, mut o) = (0, 0);
    while s < src.len() {
        let cmd = src[s];
        s += 1;
        if cmd == 0 {
            let count = *src.get(s)? as usize;
            s += 1;
            dest.get_mut(o..o + count)?.fill(0);
            o += count;
        } else {
            *dest.get_mut(o)? = cmd;
            o += 1;
        }
    }
    Some(())
}

/// `ShpD2Frame`.
fn decode_shp_d2_frame(d: &[u8], start: usize) -> Option<SpriteFrame> {
    const PALETTE_TABLE: u16 = 1;
    const NOT_LCW_COMPRESSED: u16 = 2;
    const VARIABLE_LENGTH_TABLE: u16 = 4;
    let flags = u16_at(d, start)?;
    let width = u16_at(d, start + 3)? as usize;
    let height = *d.get(start + 5)? as usize;
    let mut data_left = (u16_at(d, start + 6)? as usize).checked_sub(10)?;
    let data_size = u16_at(d, start + 8)? as usize;
    let mut position = start + 10;
    let table: Vec<u8> = if flags & PALETTE_TABLE != 0 {
        let n = if flags & VARIABLE_LENGTH_TABLE != 0 {
            let n = *d.get(position)? as usize;
            position += 1;
            n
        } else {
            16
        };
        let table = d.get(position..position + n)?.to_vec();
        position += n;
        data_left = data_left.checked_sub(n)?;
        table
    } else {
        let mut table: Vec<u8> = (0..=255).collect();
        table[1..5].copy_from_slice(&[0x7f, 0x7e, 0x7d, 0x7c]);
        table
    };
    let compressed = d.get(position..position + data_left)?;
    let mut data = vec![0; width * height];
    if flags & NOT_LCW_COMPRESSED == 0 {
        let mut expanded = vec![0; data_size];
        crate::lcw::lcw_decode_into(compressed, &mut expanded, 0)?;
        rle_zeros_decode_into(&expanded, &mut data)?;
    } else {
        rle_zeros_decode_into(compressed, &mut data)?;
    }
    for v in &mut data {
        *v = *table.get(*v as usize)?;
    }
    Some(SpriteFrame {
        width,
        height,
        frame_width: width,
        frame_height: height,
        offset: (0.0, 0.0),
        data,
    })
}

/// `ShpD2Loader.ParseFrames`.
fn decode_shp_d2(d: &[u8]) -> Option<Vec<SpriteFrame>> {
    let count = u16_at(d, 0)? as usize;
    let two_bytes = u32_at(d, 2)? & 0xff0000 > 0;
    (0..count)
        .map(|i| {
            let offset = if two_bytes {
                u16_at(d, 2 + 2 * i)? as usize
            } else {
                u32_at(d, 2 + 4 * i)? as usize
            };
            decode_shp_d2_frame(d, offset + 2)
        })
        .collect()
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
    if is_shp_d2(d) || is_shp_td(d) {
        return u16_at(d, 0).map(|c| c as usize);
    }
    if is_tmp_ra(d) {
        let index_end = u32_at(d, 28)? as usize;
        let index_start = u32_at(d, 36)? as usize;
        return index_end.checked_sub(index_start);
    }
    None
}

/// An 8-bit indexed sprite frame. `offset` is the center of `data` relative to the center of the untrimmed frame.
#[derive(Clone, Debug, Default)]
pub struct SpriteFrame {
    pub width: usize,
    pub height: usize,
    pub frame_width: usize,
    pub frame_height: usize,
    pub offset: (f32, f32),
    pub data: Vec<u8>,
}

/// Decodes all frames of a sprite file, trying the loaders in the order of `SpriteFormats`.
pub fn decode(d: &[u8]) -> Option<Vec<SpriteFrame>> {
    if is_shp_d2(d) {
        return decode_shp_d2(d);
    }
    if is_shp_td(d) {
        return decode_shp_td(d);
    }
    if is_tmp_ra(d) {
        return decode_tmp_ra(d);
    }
    None
}

/// `ShpTDSprite`: LCW key frames and XOR delta frames, trimmed like `TrimmedFrame`.
fn decode_shp_td(d: &[u8]) -> Option<Vec<SpriteFrame>> {
    let count = u16_at(d, 0)? as usize;
    let (w, h) = (u16_at(d, 6)? as usize, u16_at(d, 8)? as usize);
    let headers: Vec<(usize, u8, usize)> = (0..count)
        .map(|i| {
            let o = 14 + 8 * i;
            let v = u32_at(d, o)?;
            Some(((v & 0xff_ffff) as usize, (v >> 24) as u8, u16_at(d, o + 4)? as usize))
        })
        .collect::<Option<_>>()?;
    let mut images: Vec<Option<Vec<u8>>> = vec![None; count];
    for i in 0..count {
        decode_shp_frame(d, &headers, &mut images, i, w * h, 0)?;
    }
    Some(images.into_iter().map(|img| trim(img.unwrap_or_default(), w, h)).collect())
}

fn decode_shp_frame(
    d: &[u8],
    headers: &[(usize, u8, usize)],
    images: &mut [Option<Vec<u8>>],
    i: usize,
    size: usize,
    depth: usize,
) -> Option<()> {
    if images[i].is_some() || size == 0 {
        return Some(());
    }
    if depth > headers.len() {
        return None;
    }
    let (offset, format, ref_offset) = headers[i];
    let img = match format {
        0x20 | 0x40 => {
            let r = if format == 0x20 {
                i.checked_sub(1)?
            } else {
                headers.iter().position(|h| h.0 == ref_offset)?
            };
            decode_shp_frame(d, headers, images, r, size, depth + 1)?;
            let mut img = images[r].clone()?;
            crate::lcw::xor_delta_decode_into(d, &mut img, offset)?;
            img
        }
        0x80 => {
            let mut img = vec![0u8; size];
            crate::lcw::lcw_decode_into(d, &mut img, offset)?;
            img
        }
        _ => return None,
    };
    images[i] = Some(img);
    Some(())
}

/// `ShpTDSprite.TrimmedFrame`.
fn trim(data: Vec<u8>, w: usize, h: usize) -> SpriteFrame {
    let full = SpriteFrame {
        width: w,
        height: h,
        frame_width: w,
        frame_height: h,
        offset: (0.0, 0.0),
        data: data.clone(),
    };
    if data.len() < w * h || w == 0 || h == 0 {
        return SpriteFrame {
            width: 0,
            height: 0,
            frame_width: w,
            frame_height: h,
            offset: (0.0, 0.0),
            data: Vec::new(),
        };
    }
    let (mut top, mut bottom, mut left, mut right) = (h as i64 - 1, 0i64, w as i64 - 1, 0i64);
    for y in 0..h {
        for x in 0..w {
            if data[y * w + x] != 0 {
                top = top.min(y as i64);
                bottom = bottom.max(y as i64);
                left = left.min(x as i64);
                right = right.max(x as i64);
            }
        }
    }
    if left > 0 {
        left -= 1;
    }
    if top > 0 {
        top -= 1;
    }
    if right < w as i64 - 1 {
        right += 1;
    }
    if bottom < h as i64 - 1 {
        bottom += 1;
    }
    let (tw, th) = (right - left + 1, bottom - top + 1);
    if tw == w as i64 && th == h as i64 {
        return full;
    }
    if tw <= 0 || th <= 0 {
        return SpriteFrame {
            width: 0,
            height: 0,
            frame_width: w,
            frame_height: h,
            offset: (0.0, 0.0),
            data: Vec::new(),
        };
    }
    let (wf, hf) = (tw % 2, th % 2);
    let (dw, dh) = ((tw + wf) as usize, (th + hf) as usize);
    let mut out = vec![0u8; dw * dh];
    for y in 0..th as usize {
        let s = (y + top as usize) * w + left as usize;
        out[y * dw..y * dw + tw as usize].copy_from_slice(&data[s..s + tw as usize]);
    }
    SpriteFrame {
        width: dw,
        height: dh,
        frame_width: w,
        frame_height: h,
        offset: (
            0.5 * (left + right + wf - w as i64 + 1) as f32,
            0.5 * (top + bottom + hf - h as i64 + 1) as f32,
        ),
        data: out,
    }
}

/// `TmpRALoader.ParseFrames`. Missing tiles become empty frames.
fn decode_tmp_ra(d: &[u8]) -> Option<Vec<SpriteFrame>> {
    let (w, h) = (u16_at(d, 0)? as usize, u16_at(d, 2)? as usize);
    let img_start = u32_at(d, 16)? as usize;
    let index_end = u32_at(d, 28)? as usize;
    let index_start = u32_at(d, 36)? as usize;
    d.get(index_start..index_end)?
        .iter()
        .map(|&b| {
            if b == 255 {
                return Some(SpriteFrame {
                    width: 0,
                    height: 0,
                    frame_width: w,
                    frame_height: h,
                    offset: (0.0, 0.0),
                    data: Vec::new(),
                });
            }
            let s = img_start + b as usize * w * h;
            Some(SpriteFrame {
                width: w,
                height: h,
                frame_width: w,
                frame_height: h,
                offset: (0.0, 0.0),
                data: d.get(s..s + w * h)?.to_vec(),
            })
        })
        .collect()
}
