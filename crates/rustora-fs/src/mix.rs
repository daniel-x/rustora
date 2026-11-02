// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Westwood `.mix` archives (C&C and RA formats, with Blowfish encrypted headers).
//!
//! Port of `OpenRA.Mods.Cnc/FileSystem/MixFile.cs`, `PackageEntry.cs`, `BlowfishKeyProvider.cs`
//! and the XCC filename databases.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use base64::Engine;
use blowfish::cipher::{BlockCipherDecrypt, KeyInit};
use num_bigint::BigUint;

use crate::{FileSystem, Package, PackageLoader};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashType {
    Classic,
    Crc32,
}

/// Port of `PackageEntry.HashFilename`.
pub fn hash_filename(name: &str, ty: HashType) -> u32 {
    let len = name.chars().count();
    let padding = if len % 4 != 0 { 4 - len % 4 } else { 0 };
    let mut upper: Vec<u8> = name.chars().map(|c| ascii_byte(c.to_uppercase().next().unwrap_or(c))).collect();
    upper.resize(len + padding, 0);
    match ty {
        HashType::Classic => {
            let mut result = 0u32;
            for chunk in upper.chunks_exact(4) {
                let next = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                result = result.rotate_left(1).wrapping_add(next);
            }
            result
        }
        HashType::Crc32 => {
            let rounded = len / 4 * 4;
            if len != rounded {
                upper[len] = (len - rounded) as u8;
                for p in 1..padding {
                    upper[len + p] = upper[rounded];
                }
            }
            crc32fast::hash(&upper)
        }
    }
}

/// `Encoding.ASCII` maps characters outside 0..=127 to '?'.
fn ascii_byte(c: char) -> u8 {
    if (c as u32) < 128 { c as u8 } else { b'?' }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    hash: u32,
    offset: u32,
    length: u32,
}

fn read_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn read_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn parse_entries(h: &[u8], offset: usize) -> Option<(Vec<Entry>, usize)> {
    let num = read_u16(h, offset)? as usize;
    let mut entries = Vec::with_capacity(num);
    for i in 0..num {
        let o = offset + 6 + i * 12;
        entries.push(Entry {
            hash: read_u32(h, o)?,
            offset: read_u32(h, o + 4)?,
            length: read_u32(h, o + 8)?,
        });
    }
    Some((entries, offset + 6 + num * 12))
}

const PUBLIC_KEY: &str = "AihRvNoIbTn85FZRYNZRcT+i6KpU+maCsEqr3Q5q+LDB5tH7Tz2qQ38V";

/// Decrypts the 80 byte key block into the 56 byte Blowfish key (RSA with the Westwood public key).
fn decrypt_key(src: &[u8]) -> Vec<u8> {
    let der = base64::engine::general_purpose::STANDARD.decode(PUBLIC_KEY).expect("valid key");
    let (len, start) = if der[1] & 0x80 != 0 {
        let n = (der[1] & 0x7f) as usize;
        let mut l = 0usize;
        for i in 0..n {
            l = (l << 8) | der[2 + i] as usize;
        }
        (l, 2 + n)
    } else {
        (der[1] as usize, 2)
    };
    let modulus = BigUint::from_bytes_be(&der[start..start + len]);
    let exponent = BigUint::from(0x10001u32);
    let key_len_bits = modulus.bits() as usize - 1;

    let a = (key_len_bits - 1) / 8;
    let mut pre_len = (55 / a + 1) * (a + 1);
    let mut dest = Vec::new();
    let mut src_offset = 0;
    while a + 1 <= pre_len {
        let block = BigUint::from_bytes_le(&src[src_offset..src_offset + a + 1]);
        let mut out = block.modpow(&exponent, &modulus).to_bytes_le();
        out.resize(a, 0);
        dest.extend_from_slice(&out[..a]);
        pre_len -= a + 1;
        src_offset += a + 1;
    }
    dest.resize(56, 0);
    dest
}

fn decrypt_blocks(data: &[u8], cipher: &blowfish::Blowfish) -> Vec<u8> {
    let mut out = data.to_vec();
    for chunk in out.chunks_exact_mut(8) {
        let mut b = blowfish::cipher::Block::<blowfish::Blowfish>::default();
        b.copy_from_slice(chunk);
        cipher.decrypt_block(&mut b);
        chunk.copy_from_slice(&b);
    }
    out
}

/// A parsed mix archive. The archive bytes stay in memory.
pub struct MixFile {
    name: String,
    data: Arc<[u8]>,
    data_start: usize,
    index: HashMap<String, Entry>,
    order: Vec<String>,
}

impl MixFile {
    pub fn parse(data: Arc<[u8]>, name: &str, global_filenames: &[String]) -> Option<Self> {
        let is_cnc = read_u16(&data, 0)? != 0;
        let encrypted = !is_cnc && read_u16(&data, 2)? & 0x2 != 0;
        let (entries, data_start) = if encrypted {
            let key = decrypt_key(data.get(4..84)?);
            let cipher = blowfish::Blowfish::new_from_slice(&key).ok()?;
            let first = decrypt_blocks(data.get(84..92)?, &cipher);
            let num_files = read_u16(&first, 0)? as usize;
            let block_count = (13 + num_files * 12) / 8;
            let header = decrypt_blocks(data.get(84..84 + block_count * 8)?, &cipher);
            let (entries, _) = parse_entries(&header, 0)?;
            (entries, 84 + block_count * 8)
        } else {
            parse_entries(&data, if is_cnc { 0 } else { 4 })?
        };

        let mut by_hash: HashMap<u32, Entry> = HashMap::with_capacity(entries.len());
        for e in &entries {
            if by_hash.insert(e.hash, *e).is_some() {
                log::warn!("{name}: duplicate hash {:08x}", e.hash);
                return None;
            }
        }

        let mut mix = MixFile {
            name: name.to_string(),
            data,
            data_start,
            index: HashMap::new(),
            order: Vec::new(),
        };
        mix.build_index(&by_hash, &entries, global_filenames);
        Some(mix)
    }

    fn build_index(&mut self, by_hash: &HashMap<u32, Entry>, entries: &[Entry], global: &[String]) {
        let mut names: Vec<String> = global.to_vec();
        let mut seen: HashSet<String> = names.iter().cloned().collect();
        let db_classic = hash_filename("local mix database.dat", HashType::Classic);
        let db_crc = hash_filename("local mix database.dat", HashType::Crc32);
        if let Some(e) = entries.iter().find(|e| e.hash == db_classic || e.hash == db_crc) {
            for n in parse_local_database(self.content(e)) {
                if seen.insert(n.clone()) {
                    names.push(n);
                }
            }
        }

        let mut classic: Vec<(String, Entry)> = Vec::new();
        let mut crc: Vec<(String, Entry)> = Vec::new();
        for n in names {
            if let Some(e) = by_hash.get(&hash_filename(&n, HashType::Classic)) {
                classic.push((n.clone(), *e));
            }
            if let Some(e) = by_hash.get(&hash_filename(&n, HashType::Crc32)) {
                crc.push((n, *e));
            }
        }
        let best = if crc.len() > classic.len() { crc } else { classic };
        let unknown = entries.len() - best.len();
        if unknown > 0 {
            log::debug!("{}: failed to resolve filenames for {unknown} unknown hashes", self.name);
        }
        self.order = best.iter().map(|(n, _)| n.clone()).collect();
        self.index = best.into_iter().collect();
    }

    fn content(&self, e: &Entry) -> &[u8] {
        let start = self.data_start + e.offset as usize;
        let end = (start + e.length as usize).min(self.data.len());
        &self.data[start.min(end)..end]
    }
}

impl Package for MixFile {
    fn name(&self) -> &str {
        &self.name
    }

    fn contents(&self) -> Vec<String> {
        self.order.clone()
    }

    fn get(&self, filename: &str) -> Option<Vec<u8>> {
        self.index.get(filename).map(|e| self.content(e).to_vec())
    }

    fn contains(&self, filename: &str) -> bool {
        self.index.contains_key(filename)
    }

    fn open_package(&self, filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        let data = self.get(filename)?;
        fs.try_parse_package(data, filename)
    }
}

fn parse_local_database(d: &[u8]) -> Vec<String> {
    let Some(count) = read_u32(d, 48) else { return Vec::new() };
    let mut out = Vec::new();
    let mut pos = 52;
    for _ in 0..count {
        let end = d[pos.min(d.len())..].iter().position(|&b| b == 0).map_or(d.len(), |p| pos + p);
        out.push(String::from_utf8_lossy(&d[pos.min(end)..end]).into_owned());
        pos = end + 1;
    }
    out
}

/// Port of `XccGlobalDatabase`: groups of `count` followed by `name\0comment\0` pairs.
pub fn parse_global_database(d: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut pos = 0;
    while pos + 4 <= d.len() {
        let count = read_u32(d, pos).unwrap_or(0);
        pos += 4;
        for _ in 0..count {
            let Some(n) = d[pos.min(d.len())..].iter().position(|&b| b == 0) else {
                return out;
            };
            let name: String = d[pos..pos + n].iter().map(|&b| b as char).collect();
            pos += n + 1;
            let Some(c) = d[pos.min(d.len())..].iter().position(|&b| b == 0) else {
                return out;
            };
            pos += c + 1;
            if seen.insert(name.clone()) {
                out.push(name);
            }
        }
    }
    out
}

/// Loads `.mix` files; reads `global mix database.dat` from the file system on first use.
#[derive(Default)]
pub struct MixLoader {
    global_filenames: OnceLock<Vec<String>>,
}

impl PackageLoader for MixLoader {
    fn try_parse(&self, data: &[u8], filename: &str, fs: &FileSystem) -> Option<Arc<dyn Package>> {
        if !filename.to_ascii_lowercase().ends_with(".mix") {
            return None;
        }
        let global = match self.global_filenames.get() {
            Some(g) => g.as_slice(),
            None => match fs.try_open("global mix database.dat") {
                Some(db) => self.global_filenames.get_or_init(|| parse_global_database(&db)).as_slice(),
                None => &[],
            },
        };
        MixFile::parse(data.into(), filename, global).map(|m| Arc::new(m) as Arc<dyn Package>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_hash_is_rotate_add() {
        assert_eq!(hash_filename("A", HashType::Classic), 0x41);
        assert_eq!(
            hash_filename("abcde", HashType::Classic),
            0x4443_4241u32.rotate_left(1).wrapping_add(0x45)
        );
    }
}
