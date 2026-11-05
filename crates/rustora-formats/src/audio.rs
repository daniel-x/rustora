// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Sound file decoders: ports of `AudReader.cs`, `ImaAdpcmReader.cs`, `WestwoodCompressedReader.cs` and `WavReader.cs`.

/// Decoded sound data with interleaved 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

impl Pcm {
    /// `ISoundFormat.LengthInSeconds`, computed from the decoded samples.
    pub fn length_in_seconds(&self) -> f32 {
        self.samples.len() as f32 / (self.channels as f32 * self.sample_rate as f32)
    }
}

const FORMAT_WESTWOOD_COMPRESSED: u8 = 1;
const FORMAT_IMA_ADPCM: u8 = 99;

/// Runs the sound loaders on a whole file: `AudLoader` first, then `WavLoader`, the order of `SoundFormats` in the mod.
pub fn decode_sound(data: &[u8]) -> Option<Pcm> {
    decode_aud(data).or_else(|| decode_wav(data))
}

/// `ISoundFormat.LengthInSeconds` of a whole file, read from its header without decoding the samples.
pub fn sound_length_in_seconds(data: &[u8]) -> Option<f32> {
    let format = *data.get(11)?;
    if format == FORMAT_WESTWOOD_COMPRESSED || format == FORMAT_IMA_ADPCM {
        let mut r = Reader::new(data);
        let rate = r.u16()? as f32;
        r.i32()?;
        let output_size = r.i32()? as f32;
        let flags = r.u8()?;
        let channels = if flags & 1 == 0 { 1.0 } else { 2.0 };
        let bits = if flags & 2 == 0 { 8.0 } else { 16.0 };
        return Some(output_size * 8.0 / (channels * bits * rate));
    }
    let h = read_wav_header(data)?;
    let bits = if h.audio_type == WAVE_PCM { h.sample_bits as f32 } else { 16.0 };
    Some(data.len() as f32 * 8.0 / (h.channels as f32 * h.sample_rate as f32 * bits))
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let b = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(b)
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }

    fn i32(&mut self) -> Option<i32> {
        self.bytes(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.i32().map(|v| v as u32)
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn skip(&mut self, n: i64) {
        self.pos = (self.pos as i64 + n).clamp(0, self.data.len() as i64) as usize;
    }
}

/// `AudReader.LoadSound`: `None` when the file is not a supported AUD. A truncated stream keeps the samples decoded so far.
pub fn decode_aud(data: &[u8]) -> Option<Pcm> {
    let format = *data.get(11)?;
    if format != FORMAT_WESTWOOD_COMPRESSED && format != FORMAT_IMA_ADPCM {
        return None;
    }
    let mut r = Reader::new(data);
    let sample_rate = r.u16()? as u32;
    let data_size = r.i32()?;
    let output_size = r.i32()?;
    let flags = r.u8()?;
    r.u8()?;
    if sample_rate == 0 {
        return None;
    }
    let channels: u16 = if flags & 1 == 0 { 1 } else { 2 };
    let is_16_bit = flags & 2 != 0;
    let samples = if format == FORMAT_IMA_ADPCM {
        read_aud_ima(&mut r, data_size, output_size)
    } else {
        let bytes = read_aud_westwood(&mut r, data_size, output_size);
        if is_16_bit { bytes_to_i16(&bytes) } else { u8_to_i16(&bytes) }
    };
    Some(Pcm {
        channels,
        sample_rate,
        samples,
    })
}

fn read_aud_chunk_header(r: &mut Reader) -> Option<(usize, usize)> {
    let compressed = r.u16()? as usize;
    let output = r.u16()? as usize;
    (r.u32()? == 0xdeaf).then_some((compressed, output))
}

fn read_aud_ima(r: &mut Reader, mut data_size: i32, output_size: i32) -> Vec<i16> {
    let mut out = Vec::with_capacity(output_size.max(0) as usize / 2);
    let (mut index, mut current) = (0, 0);
    let mut base_offset = 0;
    while data_size > 0 {
        let Some((compressed, _)) = read_aud_chunk_header(r) else {
            break;
        };
        let Some(chunk) = r.bytes(compressed) else {
            break;
        };
        for &b in chunk {
            out.push(decode_ima_adpcm_sample(b, &mut index, &mut current));
            base_offset += 2;
            if base_offset < output_size {
                out.push(decode_ima_adpcm_sample(b >> 4, &mut index, &mut current));
                base_offset += 2;
            }
        }
        data_size -= 8 + compressed as i32;
    }
    out
}

fn read_aud_westwood(r: &mut Reader, mut data_size: i32, output_size: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(output_size.max(0) as usize);
    while data_size > 0 {
        let Some((compressed, output)) = read_aud_chunk_header(r) else {
            break;
        };
        let Some(chunk) = r.bytes(compressed) else {
            break;
        };
        let start = out.len();
        out.resize(start + output, 0);
        decode_westwood_compressed_sample(chunk, &mut out[start..]);
        data_size -= 8 + compressed as i32;
    }
    out
}

fn u8_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes.iter().map(|&b| ((b as i16) - 128) << 8).collect()
}

fn bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

const IMA_INDEX_ADJUST: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157,
    173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707,
    1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// `ImaAdpcmReader.DecodeImaAdpcmSample`: decodes the low nibble of `b`.
pub fn decode_ima_adpcm_sample(b: u8, index: &mut i32, current: &mut i32) -> i16 {
    let negative = b & 8 != 0;
    let b = (b & 7) as i32;
    let step = IMA_STEP_TABLE[*index as usize];
    let mut delta = step * b / 4 + step / 8;
    if negative {
        delta = -delta;
    }
    *current = (*current + delta).clamp(i16::MIN as i32, i16::MAX as i32);
    *index = (*index + IMA_INDEX_ADJUST[b as usize]).clamp(0, 88);
    *current as i16
}

/// `ImaAdpcmReader.LoadImaAdpcmSound`: decodes both nibbles of each byte, low nibble first.
pub fn decode_ima_adpcm(raw: &[u8], index: &mut i32, current: &mut i32) -> Vec<i16> {
    let mut out = Vec::with_capacity(raw.len() * 2);
    for &b in raw {
        out.push(decode_ima_adpcm_sample(b, index, current));
        out.push(decode_ima_adpcm_sample(b >> 4, index, current));
    }
    out
}

const WS_STEP_2: [i32; 4] = [-2, -1, 0, 1];
const WS_STEP_4: [i32; 16] = [-9, -8, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 8];

/// `WestwoodCompressedReader.DecodeWestwoodCompressedSample`. Corrupt input ends the decoding early.
pub fn decode_westwood_compressed_sample(input: &[u8], output: &mut [u8]) {
    if input.len() == output.len() {
        output.copy_from_slice(input);
        return;
    }
    let mut sample = 0x80i32;
    let (mut r, mut w) = (0, 0);
    let mut put = |w: &mut usize, v: u8| -> bool {
        match output.get_mut(*w) {
            Some(o) => {
                *o = v;
                *w += 1;
                true
            }
            None => false,
        }
    };
    while r < input.len() {
        let head = input[r];
        r += 1;
        let count = (head & 0x3f) as i32;
        match head >> 6 {
            0 => {
                for _ in 0..=count {
                    let Some(&code) = input.get(r) else { return };
                    r += 1;
                    for shift in [0, 2, 4, 6] {
                        sample = (sample + WS_STEP_2[((code >> shift) & 3) as usize]).clamp(0, 255);
                        if !put(&mut w, sample as u8) {
                            return;
                        }
                    }
                }
            }
            1 => {
                for _ in 0..=count {
                    let Some(&code) = input.get(r) else { return };
                    r += 1;
                    for shift in [0, 4] {
                        sample = (sample + WS_STEP_4[((code >> shift) & 15) as usize]).clamp(0, 255);
                        if !put(&mut w, sample as u8) {
                            return;
                        }
                    }
                }
            }
            2 if count & 0x20 != 0 => {
                sample += (((count as i8) << 3) >> 3) as i32;
                if !put(&mut w, sample as u8) {
                    return;
                }
            }
            2 => {
                for _ in 0..=count {
                    let Some(&v) = input.get(r) else { return };
                    r += 1;
                    if !put(&mut w, v) {
                        return;
                    }
                    sample = v as i32;
                }
            }
            _ => {
                for _ in 0..=count {
                    if !put(&mut w, sample as u8) {
                        return;
                    }
                }
            }
        }
    }
}

const WAVE_PCM: i16 = 1;
const WAVE_MS_ADPCM: i16 = 2;
const WAVE_IMA_ADPCM: i16 = 0x11;

struct WavHeader<'a> {
    audio_type: i16,
    channels: u16,
    sample_rate: u32,
    sample_bits: u16,
    block_align: usize,
    uncompressed_size: i32,
    data: &'a [u8],
}

fn read_wav_header(file: &[u8]) -> Option<WavHeader<'_>> {
    let mut r = Reader::new(file);
    if r.bytes(4)? != b"RIFF" {
        return None;
    }
    r.i32()?;
    if r.bytes(4)? != b"WAVE" {
        return None;
    }
    let mut h = WavHeader {
        audio_type: 0,
        channels: 0,
        sample_rate: 0,
        sample_bits: 0,
        block_align: 0,
        uncompressed_size: -1,
        data: &[],
    };
    while r.remaining() > 0 {
        if r.pos & 1 == 1 {
            r.u8()?;
        }
        if r.remaining() == 0 {
            break;
        }
        match r.bytes(4)? {
            b"fmt " => {
                let size = r.i32()? as i64;
                h.audio_type = r.i16()?;
                if ![WAVE_PCM, WAVE_MS_ADPCM, WAVE_IMA_ADPCM].contains(&h.audio_type) {
                    return None;
                }
                h.channels = r.u16()?;
                h.sample_rate = r.u32()?;
                r.i32()?;
                h.block_align = r.u16()? as usize;
                h.sample_bits = r.u16()?;
                r.skip(size - 16);
            }
            b"fact" => {
                let size = r.i32()? as i64;
                h.uncompressed_size = r.i32()?;
                r.skip(size - 4);
            }
            b"data" => {
                let size = r.i32()?.max(0) as usize;
                let end = (r.pos + size).min(file.len());
                h.data = &file[r.pos..end];
                r.pos = end;
            }
            b"LIST" | b"cue " => {
                let size = r.i32()? as i64;
                r.skip(size);
            }
            _ => break,
        }
    }
    (h.channels > 0 && h.sample_rate > 0).then_some(h)
}

/// `WavReader.LoadSound`: `None` when the file is not a supported WAV.
pub fn decode_wav(file: &[u8]) -> Option<Pcm> {
    let h = read_wav_header(file)?;
    let samples = match h.audio_type {
        WAVE_IMA_ADPCM => decode_wav_ima(&h),
        WAVE_MS_ADPCM => decode_wav_ms(&h),
        _ if h.sample_bits == 8 => u8_to_i16(h.data),
        _ if h.sample_bits == 16 => bytes_to_i16(h.data),
        _ => return None,
    };
    Some(Pcm {
        channels: h.channels,
        sample_rate: h.sample_rate,
        samples,
    })
}

fn decode_wav_ima(h: &WavHeader) -> Vec<i16> {
    let channels = h.channels as usize;
    let mut out = Vec::new();
    let limit = (h.uncompressed_size >= 0).then(|| h.uncompressed_size as usize * channels);
    let header_size = channels * 4;
    if h.block_align <= header_size {
        return out;
    }
    for block in h.data.chunks_exact(h.block_align) {
        let mut predictor = vec![0i32; channels];
        let mut index = vec![0i32; channels];
        for c in 0..channels {
            let head = &block[c * 4..c * 4 + 4];
            predictor[c] = i16::from_le_bytes([head[0], head[1]]) as i32;
            index[c] = (head[2] as i32).clamp(0, 88);
            out.push(predictor[c] as i16);
        }
        for group in block[header_size..].chunks_exact(4 * channels) {
            let decoded: Vec<Vec<i16>> = (0..channels)
                .map(|c| decode_ima_adpcm(&group[c * 4..c * 4 + 4], &mut index[c], &mut predictor[c]))
                .collect();
            for i in 0..8 {
                out.extend(decoded.iter().map(|d| d[i]));
            }
        }
    }
    if let Some(limit) = limit {
        out.truncate(limit);
    }
    out
}

const MS_ADAPTATION: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];
const MS_COEFF_1: [i32; 7] = [256, 512, 0, 192, 240, 460, 392];
const MS_COEFF_2: [i32; 7] = [0, -256, 0, 64, 0, -208, -232];

fn decode_wav_ms(h: &WavHeader) -> Vec<i16> {
    let channels = h.channels as usize;
    let mut out = Vec::new();
    let header_size = channels * 7;
    if h.block_align <= header_size || channels > 2 {
        return out;
    }
    for block in h.data.chunks_exact(h.block_align) {
        let word = |offset: usize| i16::from_le_bytes([block[offset], block[offset + 1]]);
        let mut pred = [0usize; 2];
        let mut delta = [0i32; 2];
        let mut s1 = [0i32; 2];
        let mut s2 = [0i32; 2];
        for c in 0..channels {
            pred[c] = (block[c] as usize).min(MS_COEFF_1.len() - 1);
            delta[c] = word(channels + c * 2) as i32;
            s1[c] = word(channels * 3 + c * 2) as i32;
            s2[c] = word(channels * 5 + c * 2) as i32;
        }
        out.extend(s2[..channels].iter().map(|&v| v as i16));
        out.extend(s1[..channels].iter().map(|&v| v as i16));
        let second = if channels > 1 { 1 } else { 0 };
        for &code in &block[header_size..] {
            for (nibble, c) in [((code >> 4) as i32, 0), ((code & 15) as i32, second)] {
                let predict = (s1[c] * MS_COEFF_1[pred[c]] + s2[c] * MS_COEFF_2[pred[c]]) >> 8;
                let signed = if nibble & 8 != 0 { nibble - 16 } else { nibble };
                let sample = (predict + signed * delta[c]).clamp(i16::MIN as i32, i16::MAX as i32);
                s2[c] = s1[c];
                s1[c] = sample;
                delta[c] = ((MS_ADAPTATION[nibble as usize] * delta[c]) >> 8).max(16);
                out.push(sample as i16);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aud(format: u8, flags: u8, output_size: i32, chunk: &[u8], chunk_output: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend(22050u16.to_le_bytes());
        f.extend((chunk.len() as i32 + 8).to_le_bytes());
        f.extend(output_size.to_le_bytes());
        f.push(flags);
        f.push(format);
        f.extend((chunk.len() as u16).to_le_bytes());
        f.extend(chunk_output.to_le_bytes());
        f.extend(0xdeafu32.to_le_bytes());
        f.extend(chunk);
        f
    }

    #[test]
    fn ima_aud_drops_the_unused_final_nibble() {
        let pcm = decode_aud(&aud(FORMAT_IMA_ADPCM, 2, 6, &[0x12, 0x34], 6)).unwrap();
        assert_eq!(pcm.sample_rate, 22050);
        assert_eq!(pcm.channels, 1);
        let (mut i, mut c) = (0, 0);
        let expected: Vec<i16> = [0x12u8, 0x01, 0x34]
            .iter()
            .map(|&b| decode_ima_adpcm_sample(b, &mut i, &mut c))
            .collect();
        assert_eq!(pcm.samples, expected);
    }

    #[test]
    fn westwood_aud_modes() {
        let mut out = [0u8; 4];
        decode_westwood_compressed_sample(&[0xC3], &mut out);
        assert_eq!(out, [0x80; 4]);

        let mut out = [0u8; 3];
        decode_westwood_compressed_sample(&[0x81, 7, 9, 0xBF], &mut out);
        assert_eq!(out, [7, 9, 8]);

        let mut out = [0u8; 4];
        decode_westwood_compressed_sample(&[0x00, 0b11_10_01_00], &mut out);
        assert_eq!(out, [126, 125, 125, 126]);
    }

    #[test]
    fn westwood_equal_sizes_copy() {
        let pcm = decode_aud(&aud(FORMAT_WESTWOOD_COMPRESSED, 0, 2, &[128, 255], 2)).unwrap();
        assert_eq!(pcm.samples, vec![0, 127 << 8]);
    }

    #[test]
    fn wav_pcm() {
        let mut f = Vec::new();
        f.extend(b"RIFF\0\0\0\0WAVEfmt ");
        f.extend(16i32.to_le_bytes());
        f.extend(1i16.to_le_bytes());
        f.extend(2i16.to_le_bytes());
        f.extend(44100i32.to_le_bytes());
        f.extend(0i32.to_le_bytes());
        f.extend(4i16.to_le_bytes());
        f.extend(16i16.to_le_bytes());
        f.extend(b"data");
        f.extend(4i32.to_le_bytes());
        f.extend([1, 0, 2, 0]);
        let pcm = decode_sound(&f).unwrap();
        assert_eq!((pcm.channels, pcm.sample_rate), (2, 44100));
        assert_eq!(pcm.samples, vec![1, 2]);
    }

    #[test]
    fn rejects_other_files() {
        assert!(decode_sound(b"not a sound file at all").is_none());
    }
}
