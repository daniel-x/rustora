// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Mods.Cnc/FileFormats/VqaVideo.cs` and `VideoLoaders/VqaLoader.cs`: Westwood VQA videos.

use crate::audio::decode_ima_adpcm;
use crate::lcw::lcw_decode_into_mode;

const MAX_CBFZ_SIZE: usize = 256000;

/// A cursor over the whole file, with the `Stream` reads of the C# decoder.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let b = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or_else(|| "Invalid vqa (unexpected end of file)".to_string())?;
        self.pos += n;
        Ok(b)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `int2.Swap(ReadUInt32())`.
    fn u32_be(&mut self) -> Result<u32, String> {
        Ok(self.u32()?.swap_bytes())
    }

    fn ascii(&mut self, n: usize) -> Result<String, String> {
        Ok(self.bytes(n)?.iter().map(|&b| b as char).collect())
    }

    /// `Stream.Peek`: the next byte, or `None` at the end.
    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn skip_padding(&mut self) {
        if self.peek() == Some(0) {
            self.pos += 1;
        }
    }
}

/// `VqaLoader.IsWestwoodVqa`.
pub fn is_vqa(data: &[u8]) -> bool {
    data.len() >= 12 && &data[0..4] == b"FORM" && data[4..8] != [0, 0, 0, 0] && &data[8..12] == b"WVQA"
}

/// A decoded VQA video. The frames decode on demand; the audio decodes when the file opens.
pub struct VqaVideo {
    data: Vec<u8>,
    pub frame_count: u16,
    pub framerate: u8,
    pub width: u16,
    pub height: u16,
    /// The current frame as BGRA, `total_frame_width` pixels per row.
    pub current_frame_data: Vec<u8>,
    pub current_frame_index: i32,
    /// 16 bit PCM, interleaved when there are two channels.
    pub audio_data: Vec<u8>,
    pub audio_channels: u8,
    pub sample_bits: u8,
    pub sample_rate: u16,
    pub total_frame_width: u16,
    num_colors: u16,
    block_width: u16,
    block_height: u16,
    chunk_buffer_parts: u8,
    blocks: (i32, i32),
    offsets: Vec<u32>,
    palette_bytes: Vec<u8>,
    video_flags: u16,
    cbf: Vec<u8>,
    cbp: Vec<u8>,
    cbf_buffer: Vec<u8>,
    cbp_is_compressed: bool,
    file_buffer: Vec<u8>,
    vtpr_size: usize,
    current_chunk_buffer: u8,
    chunk_buffer_offset: usize,
    orig_data: Vec<u8>,
}

impl VqaVideo {
    /// Opens a video. `use_frame_padding` pads the frame to a power of two square, as for a texture.
    pub fn new(data: Vec<u8>, use_frame_padding: bool) -> Result<VqaVideo, String> {
        let mut s = Cursor { data: &data, pos: 0 };
        if s.ascii(4)? != "FORM" {
            return Err("Invalid vqa (invalid FORM section)".into());
        }
        s.u32()?;
        if s.ascii(8)? != "WVQAVQHD" {
            return Err("Invalid vqa (not WVQAVQHD)".into());
        }
        s.u32()?;
        s.u16()?;
        let video_flags = s.u16()?;
        let frame_count = s.u16()?;
        let width = s.u16()?;
        let height = s.u16()?;
        let block_width = s.u8()? as u16;
        let block_height = s.u8()? as u16;
        let framerate = s.u8()?;
        let chunk_buffer_parts = s.u8()?;
        if block_width == 0 || block_height == 0 {
            return Err("Invalid vqa (zero block size)".into());
        }
        let blocks = ((width / block_width) as i32, (height / block_height) as i32);
        let num_colors = s.u16()?;
        s.u16()?;
        s.u16()?;
        s.u32()?;
        let sample_rate = s.u16()?;
        let audio_channels = s.u8()?;
        let sample_bits = s.u8()?;
        s.u32()?;
        s.u16()?;
        s.u32()?;
        s.u32()?;
        let hq = video_flags & 0x10 == 0x10;
        let pixels = width as usize * height as usize;
        let (cbf_buffer, cbf, cbp, orig_data) = if hq {
            (
                vec![0; MAX_CBFZ_SIZE],
                vec![0; MAX_CBFZ_SIZE * 3],
                Vec::new(),
                vec![0; MAX_CBFZ_SIZE],
            )
        } else {
            (
                vec![0; pixels],
                vec![0; pixels],
                vec![0; pixels],
                vec![0; 2 * (blocks.0 * blocks.1) as usize],
            )
        };
        let mut ty = s.ascii(4)?;
        while ty != "FINF" {
            if ty.as_bytes()[3] == b'F' {
                let jmp = s.u32_be()? as usize;
                s.pos += jmp;
                ty = s.ascii(4)?;
            } else {
                return Err(format!("Vqa uses unknown Subtype: {ty}"));
            }
        }
        s.u16()?;
        s.u16()?;
        let mut offsets = Vec::with_capacity(frame_count as usize);
        for _ in 0..frame_count {
            let mut o = s.u32()?;
            if o > 0x4000_0000 {
                o -= 0x4000_0000;
            }
            offsets.push(o << 1);
        }
        let total_frame_width = if use_frame_padding {
            (width.max(height) as u32).next_power_of_two() as u16
        } else {
            width
        };
        let frame_bytes = if use_frame_padding {
            total_frame_width as usize * total_frame_width as usize * 4
        } else {
            pixels * 4
        };
        let mut video = VqaVideo {
            data: Vec::new(),
            frame_count,
            framerate,
            width,
            height,
            current_frame_data: vec![0; frame_bytes],
            current_frame_index: 0,
            audio_data: Vec::new(),
            audio_channels,
            sample_bits,
            sample_rate,
            total_frame_width,
            num_colors,
            block_width,
            block_height,
            chunk_buffer_parts,
            blocks,
            offsets,
            palette_bytes: vec![0; num_colors as usize * 4],
            video_flags,
            cbf,
            cbp,
            cbf_buffer,
            cbp_is_compressed: false,
            file_buffer: vec![0; MAX_CBFZ_SIZE],
            vtpr_size: 0,
            current_chunk_buffer: 0,
            chunk_buffer_offset: 0,
            orig_data,
        };
        video.data = data;
        video.collect_audio_data()?;
        video.reset()?;
        Ok(video)
    }

    pub fn has_audio(&self) -> bool {
        !self.audio_data.is_empty()
    }

    fn is_hq(&self) -> bool {
        self.video_flags & 0x10 == 0x10
    }

    fn frame_end(&self, index: usize, file_len: usize) -> usize {
        if index + 1 < self.offsets.len() {
            self.offsets[index + 1] as usize
        } else {
            file_len
        }
    }

    pub fn reset(&mut self) -> Result<(), String> {
        self.current_frame_index = 0;
        self.chunk_buffer_offset = 0;
        self.current_chunk_buffer = 0;
        self.load_frame()
    }

    pub fn advance_frame(&mut self) -> Result<(), String> {
        self.current_frame_index += 1;
        self.load_frame()
    }

    fn collect_audio_data(&mut self) -> Result<(), String> {
        let data = std::mem::take(&mut self.data);
        let result = self.collect_audio_from(&data);
        self.data = data;
        result
    }

    fn collect_audio_from(&mut self, data: &[u8]) -> Result<(), String> {
        let mut audio1 = Vec::new();
        let mut audio2 = Vec::new();
        let mut compressed = false;
        let mut s = Cursor { data, pos: 0 };
        for i in 0..self.offsets.len() {
            s.pos = self.offsets[i] as usize;
            let end = self.frame_end(i, data.len());
            while s.pos < end {
                let mut ty = s.ascii(4)?;
                if ty == "SN2J" {
                    let jmp = s.u32_be()? as usize;
                    s.pos += jmp;
                    ty = s.ascii(4)?;
                }
                let length = s.u32_be()? as usize;
                match ty.as_str() {
                    "SND0" | "SND2" => {
                        match self.audio_channels {
                            0 => return Err("Vqa audio without channels".into()),
                            1 => audio1.extend_from_slice(s.bytes(length)?),
                            _ => {
                                audio1.extend_from_slice(s.bytes(length / 2)?);
                                audio2.extend_from_slice(s.bytes(length / 2)?);
                                if length % 2 != 0 {
                                    s.pos += 2;
                                }
                            }
                        }
                        compressed = ty == "SND2";
                    }
                    _ => {
                        if length + s.pos > data.len() {
                            return Err(format!("Vqa uses unknown Subtype: {ty}"));
                        }
                        s.pos += length;
                    }
                }
                s.skip_padding();
            }
        }
        let decode = |raw: &[u8]| -> Vec<u8> {
            let (mut index, mut current) = (0, 0);
            decode_ima_adpcm(raw, &mut index, &mut current)
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect()
        };
        if self.audio_channels == 1 {
            self.audio_data = if compressed { decode(&audio1) } else { audio1 };
        } else {
            let (left, right) = if compressed {
                (decode(&audio1), decode(&audio2))
            } else {
                (audio1, audio2)
            };
            let mut out = Vec::with_capacity(left.len() + right.len());
            for (l, r) in left.chunks_exact(2).zip(right.chunks_exact(2)) {
                out.extend_from_slice(l);
                out.extend_from_slice(r);
            }
            self.audio_data = out;
        }
        Ok(())
    }

    fn load_frame(&mut self) -> Result<(), String> {
        let index = self.current_frame_index as usize;
        if index >= self.offsets.len() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.data);
        let result = self.load_frame_from(&data, index);
        self.data = data;
        result?;
        self.decode_frame_data()
    }

    fn load_frame_from(&mut self, data: &[u8], index: usize) -> Result<(), String> {
        let mut s = Cursor {
            data,
            pos: self.offsets[index] as usize,
        };
        let end = self.frame_end(index, data.len());
        while s.pos < end {
            let mut ty = s.ascii(4)?;
            if ty == "SN2J" {
                let jmp = s.u32_be()? as usize;
                s.pos += jmp;
                ty = s.ascii(4)?;
                if ty != "SND2" {
                    return Err("Vqa SN2J without SND2".into());
                }
                let length = s.u32_be()? as usize;
                s.pos += length;
                ty = s.ascii(4)?;
            }
            let length = s.u32_be()? as usize;
            match ty.as_str() {
                "VQFR" => self.decode_vqfr(&mut s, false)?,
                "\0VQF" => {
                    s.u8()?;
                    self.decode_vqfr(&mut s, false)?;
                }
                "VQFL" => self.decode_vqfr(&mut s, true)?,
                _ => s.pos += length,
            }
            s.skip_padding();
        }
        Ok(())
    }

    /// `DecodeVQFR`; `vqfl` stands for the parent type `VQFL`.
    fn decode_vqfr(&mut self, s: &mut Cursor, vqfl: bool) -> Result<(), String> {
        if self.current_chunk_buffer == self.chunk_buffer_parts && self.chunk_buffer_parts != 0 {
            if !self.cbp_is_compressed {
                self.cbf = self.cbp.clone();
            } else {
                lcw_decode_into_mode(&self.cbp, &mut self.cbf, 0, false).ok_or("Invalid vqa (CBPZ)")?;
            }
            self.chunk_buffer_offset = 0;
            self.current_chunk_buffer = 0;
        }
        loop {
            s.skip_padding();
            let ty = s.ascii(4)?;
            let length = s.u32_be()? as usize;
            match ty.as_str() {
                "CBFZ" => {
                    let reverse = s.peek() == Some(0);
                    let chunk = s.bytes(length)?;
                    self.file_buffer
                        .get_mut(..length)
                        .ok_or("Invalid vqa (CBFZ)")?
                        .copy_from_slice(chunk);
                    self.cbf.fill(0);
                    self.cbf_buffer.fill(0);
                    let count = lcw_decode_into_mode(&self.file_buffer, &mut self.cbf_buffer, reverse as usize, reverse)
                        .ok_or("Invalid vqa (CBFZ)")?;
                    if self.is_hq() {
                        let mut p = 0;
                        let mut i = 0;
                        while i < count {
                            let packed = (self.cbf_buffer[i + 1] as u16) << 8 | self.cbf_buffer[i] as u16;
                            self.cbf[p] = ((packed & 0x7c00) >> 7) as u8;
                            self.cbf[p + 1] = ((packed & 0x3e0) >> 2) as u8;
                            self.cbf[p + 2] = ((packed & 0x1f) << 3) as u8;
                            p += 3;
                            i += 2;
                        }
                    } else {
                        self.cbf = self.cbf_buffer.clone();
                    }
                    if vqfl {
                        return Ok(());
                    }
                }
                "CBF0" => self.cbf = s.bytes(length)?.to_vec(),
                "CBP0" | "CBPZ" => {
                    let chunk = s.bytes(length)?;
                    let at = self.chunk_buffer_offset;
                    self.cbp.get_mut(at..at + length).ok_or("Invalid vqa (CBP)")?.copy_from_slice(chunk);
                    self.chunk_buffer_offset += length;
                    self.current_chunk_buffer += 1;
                    self.cbp_is_compressed = ty == "CBPZ";
                }
                "CPL0" => {
                    for i in 0..self.num_colors as usize {
                        let r = s.u8()? << 2;
                        let g = s.u8()? << 2;
                        let b = s.u8()? << 2;
                        self.palette_bytes[i * 4..i * 4 + 4].copy_from_slice(&[b, g, r, 255]);
                    }
                }
                "VPTZ" => {
                    let chunk = s.bytes(length)?;
                    lcw_decode_into_mode(chunk, &mut self.orig_data, 0, false).ok_or("Invalid vqa (VPTZ)")?;
                    return Ok(());
                }
                "VPRZ" => {
                    self.orig_data.fill(0);
                    let chunk = s.bytes(length)?;
                    self.file_buffer
                        .get_mut(..length)
                        .ok_or("Invalid vqa (VPRZ)")?
                        .copy_from_slice(chunk);
                    if self.file_buffer[0] != 0 {
                        self.vtpr_size =
                            lcw_decode_into_mode(&self.file_buffer, &mut self.orig_data, 0, false).ok_or("Invalid vqa (VPRZ)")?;
                    } else {
                        lcw_decode_into_mode(&self.file_buffer, &mut self.orig_data, 1, true).ok_or("Invalid vqa (VPRZ)")?;
                    }
                    return Ok(());
                }
                "VPTR" => {
                    self.orig_data.fill(0);
                    let chunk = s.bytes(length)?;
                    self.orig_data.get_mut(..length).ok_or("Invalid vqa (VPTR)")?.copy_from_slice(chunk);
                    self.vtpr_size = length;
                    return Ok(());
                }
                _ => return Err(format!("Unknown sub-chunk {ty}")),
            }
        }
    }

    fn decode_frame_data(&mut self) -> Result<(), String> {
        if self.is_hq() {
            return self.decode_hq_frame_data();
        }
        let (bw, bh) = (self.block_width as usize, self.block_height as usize);
        let (bx, by) = (self.blocks.0 as usize, self.blocks.1 as usize);
        let total = self.total_frame_width as usize;
        for y in 0..by {
            for x in 0..bx {
                let px = self.orig_data[x + y * bx] as usize;
                let m = self.orig_data[x + (y + by) * bx] as usize;
                for j in 0..bh {
                    for i in 0..bw {
                        let color = if m == 0x0f {
                            px
                        } else {
                            let cbfi = (m * 256 + px) * 8 + j * bw + i;
                            *self.cbf.get(cbfi).ok_or("Invalid vqa (codebook index)")? as usize
                        };
                        let pos = (y * bh + j) * total + x * bw + i;
                        let c = self
                            .palette_bytes
                            .get(color * 4..color * 4 + 4)
                            .ok_or("Invalid vqa (palette index)")?;
                        self.current_frame_data[pos * 4..pos * 4 + 4].copy_from_slice(c);
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_hq_frame_data(&mut self) -> Result<(), String> {
        let mut p = 0;
        let mut y = 0i32;
        while y < self.blocks.1 {
            let mut x = 0i32;
            while x < self.blocks.0 {
                if y >= self.blocks.1 {
                    break;
                }
                let val = self.orig_data[p] as i32 | (self.orig_data[p + 1] as i32) << 8;
                p += 2;
                let a = val & 0x1fff;
                let b1 = val & 0xff;
                let b2 = (((val / 256) & 0x1f) + 1) * 2;
                match val >> 13 {
                    0 => x += a,
                    1 => self.write_block(b1, b2, &mut x, &mut y)?,
                    2 => {
                        self.write_block(b1, 1, &mut x, &mut y)?;
                        for _ in 0..b2 {
                            let n = self.orig_data[p] as i32;
                            p += 1;
                            self.write_block(n, 1, &mut x, &mut y)?;
                        }
                    }
                    3 => self.write_block(a, 1, &mut x, &mut y)?,
                    5 => {
                        let n = self.orig_data[p] as i32;
                        p += 1;
                        self.write_block(a, n, &mut x, &mut y)?;
                    }
                    _ => return Err("Invalid vqa (block command)".into()),
                }
            }
            y += 1;
        }
        if p != self.vtpr_size {
            return Err("Invalid vqa (block commands length)".into());
        }
        Ok(())
    }

    fn write_block(&mut self, block: i32, count: i32, x: &mut i32, y: &mut i32) -> Result<(), String> {
        let (bw, bh) = (self.block_width as usize, self.block_height as usize);
        let total = self.total_frame_width as usize;
        for i in 0..count {
            let offset = block as usize * bh * bw * 3;
            for by in 0..bh {
                for bx in 0..bw {
                    let p = offset + (bx + by * bw) * 3;
                    let rgb = self.cbf.get(p..p + 3).ok_or("Invalid vqa (codebook index)")?;
                    let pos = (*y as usize * bh + by) * total + *x as usize * bw + bx;
                    let px = [rgb[2], rgb[1], rgb[0], 255];
                    self.current_frame_data
                        .get_mut(pos * 4..pos * 4 + 4)
                        .ok_or("Invalid vqa (block position)")?
                        .copy_from_slice(&px);
                }
            }
            *x += 1;
            if *x >= self.blocks.0 {
                *x = 0;
                *y += 1;
                if *y >= self.blocks.1 && i != count - 1 {
                    return Err("Invalid vqa (block overflow)".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(ty: &[u8], body: &[u8]) -> Vec<u8> {
        let mut c = ty.to_vec();
        c.extend_from_slice(&(body.len() as u32).to_be_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    /// A 4x2 video of one frame with 4x2 blocks: one codebook entry of eight colors, mono 8 bit audio.
    fn sample() -> Vec<u8> {
        let mut header = Vec::new();
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&4u16.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&[4, 2, 15, 0]);
        header.extend_from_slice(&8u16.to_le_bytes());
        header.extend_from_slice(&[0; 2 + 2 + 4]);
        header.extend_from_slice(&22050u16.to_le_bytes());
        header.extend_from_slice(&[1, 16]);
        header.extend_from_slice(&[0; 4 + 2 + 4 + 4]);
        let mut cbf = vec![0u8; 8 * 256 * 8];
        cbf[..8].copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
        let palette: Vec<u8> = (0..8u8).flat_map(|i| [i, i * 2, i * 3]).collect();
        let mut vqfr = chunk(b"CBF0", &cbf);
        vqfr.extend(chunk(b"CPL0", &palette));
        vqfr.extend(chunk(b"VPTR", &[0, 0]));
        let mut frame = chunk(b"SND0", &[1, 2, 3, 4]);
        frame.extend(chunk(b"VQFR", &vqfr));
        let mut body = b"WVQA".to_vec();
        body.extend(chunk(b"VQHD", &header));
        let frame_at = (8 + body.len() + 12) as u32;
        let mut finf = Vec::new();
        finf.extend_from_slice(&[0; 4]);
        finf.extend_from_slice(&(frame_at >> 1).to_le_bytes());
        body.extend_from_slice(b"FINF");
        body.extend_from_slice(&finf[..4]);
        body.extend_from_slice(&finf[4..]);
        body.extend(frame);
        let mut file = b"FORM".to_vec();
        file.extend_from_slice(&(body.len() as u32).to_be_bytes());
        file.extend(body);
        file
    }

    #[test]
    fn decodes_a_vq_frame_and_pcm_audio() {
        let data = sample();
        assert!(is_vqa(&data));
        let v = VqaVideo::new(data, false).expect("valid video");
        assert_eq!((v.width, v.height, v.frame_count, v.framerate), (4, 2, 1, 15));
        assert_eq!(v.audio_data, vec![1, 2, 3, 4]);
        let blue: Vec<u8> = v.current_frame_data.chunks(4).map(|p| p[0]).collect();
        assert_eq!(blue, vec![0, 12, 24, 36, 48, 60, 72, 84]);
        assert!(v.current_frame_data.chunks(4).all(|p| p[3] == 255));
    }
}
