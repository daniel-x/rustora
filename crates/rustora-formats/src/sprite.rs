// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sprite file formats: `frame_count` for the simulation, `decode` for the renderer. Port of `ShpD2Loader.cs`, `ShpTDLoader.cs`, `TmpRALoader.cs`
//! `TmpTDLoader.cs` and `ShpTSLoader.cs`.

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

/// `TmpTDLoader.IsTmpTD`.
pub fn is_tmp_td(d: &[u8]) -> bool {
    u32_at(d, 16) == Some(0) && u32_at(d, 20) == Some(0x0d1a_ffff)
}

/// `TmpTDLoader.ParseFrames`.
fn decode_tmp_td(d: &[u8]) -> Option<Vec<SpriteFrame>> {
    let (width, height) = (u16_at(d, 0)? as usize, u16_at(d, 2)? as usize);
    let img_start = u32_at(d, 12)? as usize;
    let index_end = u32_at(d, 24)? as usize;
    let index_start = u32_at(d, 28)? as usize;
    d.get(index_start..index_end)?
        .iter()
        .map(|&b| {
            if b == 255 {
                return Some(SpriteFrame {
                    frame_width: width,
                    frame_height: height,
                    ..SpriteFrame::default()
                });
            }
            let start = img_start + b as usize * width * height;
            Some(SpriteFrame {
                width,
                height,
                frame_width: width,
                frame_height: height,
                offset: (0.0, 0.0),
                data: d.get(start..start + width * height)?.to_vec(),
            })
        })
        .collect()
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
    if is_tmp_td(d) {
        let index_end = u32_at(d, 24)? as usize;
        let index_start = u32_at(d, 28)? as usize;
        return index_end.checked_sub(index_start);
    }
    if is_shp_ts(d) {
        return u16_at(d, 6).map(|c| c as usize);
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
    if is_tmp_td(d) {
        return decode_tmp_td(d);
    }
    if is_shp_ts(d) {
        return decode_shp_ts(d);
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

/// `ShpTSLoader.IsShpTS`.
pub fn is_shp_ts(d: &[u8]) -> bool {
    if u16_at(d, 0) != Some(0) {
        return false;
    }
    let Some(image_count) = u16_at(d, 6) else { return false };
    if 8 + 24 * image_count as usize > d.len() {
        return false;
    }
    let mut position = 12;
    let mut f = 0;
    loop {
        let (Some(w), Some(h), Some(&kind)) = (u16_at(d, position), u16_at(d, position + 2), d.get(position + 4)) else {
            return false;
        };
        if (w == 0 || h == 0) && kind == 0 {
            return false;
        }
        position += 24;
        f += 1;
        if !(w == 0 && h == 0 && f < image_count) {
            return f == image_count || kind < 4;
        }
    }
}

/// `ShpTSLoader.ParseFrames` with `ShpTSFrame`. Frames without data become empty frames.
fn decode_shp_ts(d: &[u8]) -> Option<Vec<SpriteFrame>> {
    let (fw, fh) = (u16_at(d, 2)? as usize, u16_at(d, 4)? as usize);
    let count = u16_at(d, 6)? as usize;
    (0..count)
        .map(|i| {
            let o = 8 + 24 * i;
            let (x, y) = (u16_at(d, o)? as i32, u16_at(d, o + 2)? as i32);
            let (width, height) = (u16_at(d, o + 4)? as usize, u16_at(d, o + 6)? as usize);
            let format = *d.get(o + 8)?;
            let file_offset = u32_at(d, o + 20)? as usize;
            let (dw, dh) = (width + width % 2, height + height % 2);
            if file_offset == 0 {
                return Some(SpriteFrame {
                    frame_width: fw,
                    frame_height: fh,
                    ..SpriteFrame::default()
                });
            }
            let mut data = vec![0u8; dw * dh];
            let mut s = file_offset;
            if format == 3 {
                for j in 0..height {
                    let length = (u16_at(d, s)? as usize).checked_sub(2)?;
                    rle_zeros_decode_into(d.get(s + 2..s + 2 + length)?, data.get_mut(dw * j..)?)?;
                    s += 2 + length;
                }
            } else {
                let length = if format == 2 {
                    let l = (u16_at(d, s)? as usize).checked_sub(2)?;
                    s += 2;
                    l
                } else {
                    width
                };
                for j in 0..height {
                    data.get_mut(dw * j..dw * j + length)?.copy_from_slice(d.get(s..s + length)?);
                    s += length;
                }
            }
            Some(SpriteFrame {
                width: dw,
                height: dh,
                frame_width: fw,
                frame_height: fh,
                offset: ((x + (dw as i32 - fw as i32) / 2) as f32, (y + (dh as i32 - fh as i32) / 2) as f32),
                data,
            })
        })
        .collect()
}

#[cfg(test)]
mod d2_td_tests {
    use super::*;

    fn tmp_td_file() -> Vec<u8> {
        let mut d = vec![0u8; 32];
        d[0..2].copy_from_slice(&2u16.to_le_bytes());
        d[2..4].copy_from_slice(&1u16.to_le_bytes());
        d[12..16].copy_from_slice(&34u32.to_le_bytes());
        d[20..24].copy_from_slice(&0x0d1a_ffffu32.to_le_bytes());
        d[24..28].copy_from_slice(&34u32.to_le_bytes());
        d[28..32].copy_from_slice(&32u32.to_le_bytes());
        d.extend([0, 255, 7, 9]);
        d
    }

    #[test]
    fn tmp_td_decodes_tiles_and_empty_frames() {
        let d = tmp_td_file();
        assert_eq!(frame_count(&d), Some(2));
        let frames = decode(&d).expect("frames");
        assert_eq!(frames[0].data, [7, 9]);
        assert_eq!((frames[0].width, frames[0].height), (2, 1));
        assert!(frames[1].data.is_empty());
        assert_eq!((frames[1].frame_width, frames[1].width), (2, 0));
    }

    fn shp_d2_file(flags: u16, body: &[u8], width: u16, height: u8) -> Vec<u8> {
        let size = 10 + body.len() as u16;
        let mut d = vec![1, 0];
        d.extend(8u32.to_le_bytes());
        d.extend((8 + size as u32).to_le_bytes());
        d.extend(flags.to_le_bytes());
        d.push(0);
        d.extend(width.to_le_bytes());
        d.push(height);
        d.extend(size.to_le_bytes());
        d.extend((body.len() as u16).to_le_bytes());
        d.extend(body);
        d
    }

    #[test]
    fn shp_d2_expands_zero_runs_and_remaps_low_indices() {
        let d = shp_d2_file(2, &[1, 0, 2, 9], 4, 1);
        assert!(is_shp_d2(&d));
        assert_eq!(frame_count(&d), Some(1));
        let frames = decode(&d).expect("frames");
        assert_eq!(frames[0].data, [0x7f, 0, 0, 9]);
        assert_eq!(frames[0].offset, (0.0, 0.0));
    }

    #[test]
    fn shp_ts_decodes_rle_rows_and_pads_to_even_sizes() {
        let mut d = vec![0u8; 8 + 24];
        d[2..4].copy_from_slice(&4u16.to_le_bytes());
        d[4..6].copy_from_slice(&2u16.to_le_bytes());
        d[6..8].copy_from_slice(&1u16.to_le_bytes());
        d[8..10].copy_from_slice(&1u16.to_le_bytes());
        d[12..14].copy_from_slice(&3u16.to_le_bytes());
        d[14..16].copy_from_slice(&1u16.to_le_bytes());
        d[16] = 3;
        d[28..32].copy_from_slice(&32u32.to_le_bytes());
        d.extend([5, 0, 7, 0, 1]);
        assert!(is_shp_ts(&d));
        assert_eq!(frame_count(&d), Some(1));
        let f = &decode(&d).expect("frames")[0];
        assert_eq!((f.width, f.height, f.frame_width, f.frame_height), (4, 2, 4, 2));
        assert_eq!(f.data, [7, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(f.offset, (1.0, 0.0));
    }

    #[test]
    fn shp_d2_rejects_a_wrong_end_offset() {
        let mut d = shp_d2_file(2, &[1, 2], 2, 1);
        d[6] = 0x7f;
        assert!(!is_shp_d2(&d));
    }
}
