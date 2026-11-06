// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Mods.Cnc/FileFormats/LCWCompression.cs` and `XORDeltaCompression.cs` (decoding only).

struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.src.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn word(&mut self) -> Option<usize> {
        Some(self.byte()? as usize | (self.byte()? as usize) << 8)
    }
}

/// `LCWCompression.DecodeInto` without the `reverse` mode. Returns the number of bytes written.
pub fn lcw_decode_into(src: &[u8], dest: &mut [u8], src_offset: usize) -> Option<usize> {
    lcw_decode_into_mode(src, dest, src_offset, false)
}

/// `LCWCompression.DecodeInto`: with `reverse`, the long copy commands address bytes relative to the end of
/// the output. Returns the number of bytes written.
pub fn lcw_decode_into_mode(src: &[u8], dest: &mut [u8], src_offset: usize, reverse: bool) -> Option<usize> {
    let mut r = Reader { src, pos: src_offset };
    let mut d = 0usize;
    loop {
        let i = r.byte()?;
        if i & 0x80 == 0 {
            let second = r.byte()? as usize;
            let count = (((i & 0x70) >> 4) + 3) as usize;
            let rpos = (((i & 0xf) as usize) << 8) + second;
            if d + count > dest.len() {
                return Some(d);
            }
            let s = d.checked_sub(rpos)?;
            for k in 0..count {
                dest[d + k] = if d - s == 1 { dest[d - 1] } else { dest[s + k] };
            }
            d += count;
        } else if i & 0x40 == 0 {
            let count = (i & 0x3f) as usize;
            if count == 0 {
                return Some(d);
            }
            dest.get_mut(d..d + count)?.copy_from_slice(src.get(r.pos..r.pos + count)?);
            r.pos += count;
            d += count;
        } else {
            let count3 = (i & 0x3f) as usize;
            if count3 == 0x3e {
                let count = r.word()?;
                let color = r.byte()?;
                dest.get_mut(d..d + count)?.fill(color);
                d += count;
            } else {
                let count = if count3 == 0x3f { r.word()? } else { count3 + 3 };
                let s = if reverse { d.checked_sub(r.word()?)? } else { r.word()? };
                if s >= d {
                    return None;
                }
                for k in 0..count {
                    *dest.get_mut(d + k)? = dest[s + k];
                }
                d += count;
            }
        }
    }
}

/// `XORDeltaCompression.DecodeInto`. Returns the number of bytes covered.
pub fn xor_delta_decode_into(src: &[u8], dest: &mut [u8], src_offset: usize) -> Option<usize> {
    let mut r = Reader { src, pos: src_offset };
    let mut d = 0usize;
    loop {
        let i = r.byte()?;
        let count = (i & 0x7f) as usize;
        if i & 0x80 == 0 {
            if count == 0 {
                let count = r.byte()? as usize;
                let value = r.byte()?;
                for x in dest.get_mut(d..d + count)? {
                    *x ^= value;
                }
                d += count;
            } else {
                for _ in 0..count {
                    *dest.get_mut(d)? ^= r.byte()?;
                    d += 1;
                }
            }
        } else if count == 0 {
            let count = r.word()?;
            if count == 0 {
                return Some(d);
            }
            if count & 0x8000 == 0 {
                d += count & 0x7fff;
            } else if count & 0x4000 == 0 {
                for _ in 0..count & 0x3fff {
                    *dest.get_mut(d)? ^= r.byte()?;
                    d += 1;
                }
            } else {
                let value = r.byte()?;
                for x in dest.get_mut(d..d + (count & 0x3fff))? {
                    *x ^= value;
                }
                d += count & 0x3fff;
            }
        } else {
            d += count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcw_copy_fill_and_back_reference() {
        let src = [0x83, 1, 2, 3, 0xfe, 4, 0, 9, 0x00, 7, 0x80];
        let mut dest = [0u8; 10];
        assert_eq!(lcw_decode_into(&src, &mut dest, 0), Some(10));
        assert_eq!(dest, [1, 2, 3, 9, 9, 9, 9, 1, 2, 3]);
    }

    #[test]
    fn xor_delta_skip_and_xor() {
        let src = [0x82, 0x01, 0xff, 0x00, 0x02, 0x10, 0x80, 0x00, 0x00];
        let mut dest = [1u8, 1, 1, 1, 1, 1];
        assert_eq!(xor_delta_decode_into(&src, &mut dest, 0), Some(5));
        assert_eq!(dest, [1, 1, 0xfe, 0x11, 0x11, 1]);
    }
}
