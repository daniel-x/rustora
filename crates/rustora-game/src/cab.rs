// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `InstallShieldCABCompression.cs`.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

const MAX_FILE_GROUP_COUNT: usize = 71;
const SIGNATURE: u32 = 0x2863_5349;
const FLAG_SPLIT: u16 = 0x1;
const FLAG_OBFUSCATED: u16 = 0x2;
const FLAG_COMPRESSED: u16 = 0x4;
const FLAG_INVALID: u16 = 0x8;
const LINK_PREV: u8 = 0x1;
const COMMON_HEADER_SIZE: u64 = 16;

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

fn io_error(e: std::io::Error) -> String {
    e.to_string()
}

fn read_u8(s: &mut dyn ReadSeek) -> Result<u8, String> {
    let mut b = [0u8; 1];
    s.read_exact(&mut b).map_err(io_error)?;
    Ok(b[0])
}

fn read_u16(s: &mut dyn ReadSeek) -> Result<u16, String> {
    let mut b = [0u8; 2];
    s.read_exact(&mut b).map_err(io_error)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32(s: &mut dyn ReadSeek) -> Result<u32, String> {
    let mut b = [0u8; 4];
    s.read_exact(&mut b).map_err(io_error)?;
    Ok(u32::from_le_bytes(b))
}

fn skip(s: &mut dyn ReadSeek, n: i64) -> Result<(), String> {
    s.seek(SeekFrom::Current(n)).map(|_| ()).map_err(io_error)
}

fn seek(s: &mut dyn ReadSeek, position: u64) -> Result<(), String> {
    s.seek(SeekFrom::Start(position)).map(|_| ()).map_err(io_error)
}

fn read_asciiz(s: &mut dyn ReadSeek) -> Result<String, String> {
    let mut bytes = Vec::new();
    loop {
        match read_u8(s)? {
            0 => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            b => bytes.push(b),
        }
    }
}

fn read_asciiz_at(s: &mut dyn ReadSeek, position: u64) -> Result<String, String> {
    let back = s.stream_position().map_err(io_error)?;
    seek(s, position)?;
    let text = read_asciiz(s)?;
    seek(s, back)?;
    Ok(text)
}

#[derive(Clone, Debug, Default)]
struct FileDescriptor {
    index: u32,
    flags: u16,
    expanded_size: u32,
    compressed_size: u32,
    data_offset: u32,
    directory_index: u32,
    link_to_previous: u32,
    link_flags: u8,
    volume: u16,
    filename: String,
}

impl FileDescriptor {
    fn read(s: &mut dyn ReadSeek, index: u32, table_offset: u64, version: u32) -> Result<FileDescriptor, String> {
        let mut file = FileDescriptor {
            index,
            ..FileDescriptor::default()
        };
        if version <= 5 {
            let name_offset = read_u32(s)?;
            file.directory_index = read_u32(s)?;
            file.flags = read_u16(s)?;
            file.expanded_size = read_u32(s)?;
            file.compressed_size = read_u32(s)?;
            skip(s, 20)?;
            file.data_offset = read_u32(s)?;
            if file.flags & FLAG_INVALID == 0 {
                file.filename = read_asciiz_at(s, table_offset + u64::from(name_offset))?;
            }
        } else {
            file.flags = read_u16(s)?;
            file.expanded_size = read_u32(s)?;
            skip(s, 4)?;
            file.compressed_size = read_u32(s)?;
            skip(s, 4)?;
            file.data_offset = read_u32(s)?;
            skip(s, 4 + 16 + 16)?;
            let name_offset = read_u32(s)?;
            file.directory_index = u32::from(read_u16(s)?);
            skip(s, 12)?;
            file.link_to_previous = read_u32(s)?;
            skip(s, 4)?;
            file.link_flags = read_u8(s)?;
            file.volume = read_u16(s)?;
            file.filename = read_asciiz_at(s, table_offset + u64::from(name_offset))?;
        }
        Ok(file)
    }
}

/// The parts of `VolumeHeader` that the extraction reads.
struct VolumeHeader {
    first_file_index: u32,
    last_file_index: u32,
    first_file_offset: u32,
    first_file_size_expanded: u32,
    first_file_size_compressed: u32,
    last_file_offset: u32,
    last_file_size_expanded: u32,
    last_file_size_compressed: u32,
}

impl VolumeHeader {
    fn read(s: &mut dyn ReadSeek) -> Result<VolumeHeader, String> {
        let mut v = [0u32; 16];
        for value in &mut v {
            *value = read_u32(s)?;
        }
        Ok(VolumeHeader {
            first_file_index: v[2],
            last_file_index: v[3],
            first_file_offset: v[4],
            first_file_size_expanded: v[6],
            first_file_size_compressed: v[8],
            last_file_offset: v[10],
            last_file_size_expanded: v[12],
            last_file_size_compressed: v[14],
        })
    }
}

/// `CabExtracter`: reads the data of one file across volume boundaries.
struct Extracter<'a> {
    file: &'a FileDescriptor,
    volumes: &'a mut HashMap<i32, File>,
    remaining_in_archive: u32,
    to_extract: u32,
    volume_id: i32,
}

impl<'a> Extracter<'a> {
    fn new(file: &'a FileDescriptor, volumes: &'a mut HashMap<i32, File>) -> Result<Extracter<'a>, String> {
        let compressed = file.flags & FLAG_COMPRESSED != 0;
        let mut extracter = Extracter {
            file,
            volumes,
            remaining_in_archive: 0,
            to_extract: if compressed { file.compressed_size } else { file.expanded_size },
            volume_id: 0,
        };
        extracter.set_volume(i32::from(file.volume))?;
        Ok(extracter)
    }

    fn current(&mut self) -> Result<&mut File, String> {
        let id = self.volume_id;
        self.volumes.get_mut(&id).ok_or_else(|| format!("Volume {id} is not available"))
    }

    fn set_volume(&mut self, volume: i32) -> Result<(), String> {
        self.volume_id = volume;
        let file = self.file;
        let compressed = file.flags & FLAG_COMPRESSED != 0;
        let stream = self.current()?;
        seek(stream, 0)?;
        if read_u32(stream)? != SIGNATURE {
            return Err("Not an Installshield CAB package".to_string());
        }
        let (remaining, offset) = if file.flags & FLAG_SPLIT != 0 {
            skip(stream, COMMON_HEADER_SIZE as i64)?;
            let head = VolumeHeader::read(stream)?;
            if file.index == head.last_file_index {
                let size = if compressed {
                    head.last_file_size_compressed
                } else {
                    head.last_file_size_expanded
                };
                (size, head.last_file_offset)
            } else if file.index == head.first_file_index {
                let size = if compressed {
                    head.first_file_size_compressed
                } else {
                    head.first_file_size_expanded
                };
                (size, head.first_file_offset)
            } else {
                return Err("Cannot Resolve Remaining Stream".to_string());
            }
        } else {
            (if compressed { file.compressed_size } else { file.expanded_size }, file.data_offset)
        };
        seek(stream, u64::from(offset))?;
        self.remaining_in_archive = remaining;
        Ok(())
    }

    /// `GetBytes`, but reads short counts in a loop, which the C# stream reads leave to chance.
    fn get_bytes(&mut self, count: u32) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; count as usize];
        if count < self.remaining_in_archive {
            self.remaining_in_archive -= count;
            self.current()?.read_exact(&mut out).map_err(io_error)?;
            return Ok(out);
        }
        let first = self.remaining_in_archive as usize;
        self.current()?.read_exact(&mut out[..first]).map_err(io_error)?;
        self.remaining_in_archive = 0;
        if self.to_extract > first as u32 {
            self.set_volume(self.volume_id + 1)?;
            let rest = count as usize - first;
            self.current()?.read_exact(&mut out[first..]).map_err(io_error)?;
            self.remaining_in_archive = self.remaining_in_archive.saturating_sub(rest as u32);
        }
        Ok(out)
    }

    fn copy_to(&mut self, output: &mut File, on_progress: &dyn Fn(i64)) -> Result<(), String> {
        let expanded = i64::from(self.file.expanded_size.max(1));
        let mut written = 0i64;
        if self.file.flags & FLAG_COMPRESSED != 0 {
            let mut buffer = vec![0u8; 165_535];
            loop {
                let count = self.block_length()?;
                let block = self.get_bytes(u32::from(count))?;
                self.to_extract = self.to_extract.saturating_sub(u32::from(count));
                let mut inflater = flate2::Decompress::new(false);
                let mut consumed = 0usize;
                while consumed < block.len() {
                    on_progress(100 * written / expanded);
                    let before_in = inflater.total_in();
                    let before_out = inflater.total_out();
                    let status = inflater
                        .decompress(&block[consumed..], &mut buffer, flate2::FlushDecompress::None)
                        .map_err(|e| e.to_string())?;
                    consumed += (inflater.total_in() - before_in) as usize;
                    let produced = (inflater.total_out() - before_out) as usize;
                    output.write_all(&buffer[..produced]).map_err(io_error)?;
                    written += produced as i64;
                    if status == flate2::Status::StreamEnd || (produced == 0 && inflater.total_in() == before_in) {
                        break;
                    }
                }
                if self.to_extract == 0 {
                    return Ok(());
                }
            }
        }
        while self.to_extract > 0 {
            on_progress(100 * written / expanded);
            let count = self.remaining_in_archive;
            if count == 0 {
                return Err("the file data ended early".to_string());
            }
            let bytes = self.get_bytes(count)?;
            self.to_extract = self.to_extract.saturating_sub(count);
            output.write_all(&bytes).map_err(io_error)?;
            written += i64::from(count);
        }
        Ok(())
    }

    fn block_length(&mut self) -> Result<u16, String> {
        let count = read_u16(self.current()?)?;
        self.remaining_in_archive = self.remaining_in_archive.saturating_sub(2);
        self.to_extract = self.to_extract.saturating_sub(2);
        Ok(count)
    }
}

/// An InstallShield cabinet: a header file with the file index and volumes with the data.
pub struct InstallShieldCab {
    index: HashMap<String, FileDescriptor>,
    volumes: HashMap<i32, File>,
}

impl InstallShieldCab {
    pub fn new(header: File, volumes: HashMap<i32, File>) -> Result<InstallShieldCab, String> {
        let mut header = header;
        Ok(InstallShieldCab {
            index: read_index(&mut header)?,
            volumes,
        })
    }

    /// `ExtractFile`. `on_progress` receives percentages.
    pub fn extract_file(&mut self, filename: &str, output: &mut File, on_progress: &dyn Fn(i64)) -> Result<(), String> {
        let file = self
            .index
            .get(filename)
            .cloned()
            .ok_or_else(|| format!("{filename} is not in the cabinet"))?;
        self.extract_descriptor(&file, output, on_progress)
    }

    fn extract_descriptor(&mut self, file: &FileDescriptor, output: &mut File, on_progress: &dyn Fn(i64)) -> Result<(), String> {
        if file.flags & FLAG_INVALID != 0 {
            return Err("File Invalid".to_string());
        }
        if file.link_flags & LINK_PREV != 0 {
            let previous = self
                .index
                .values()
                .find(|f| f.index == file.link_to_previous)
                .cloned()
                .ok_or("the link to the previous file is broken")?;
            return self.extract_descriptor(&previous, output, on_progress);
        }
        if file.flags & FLAG_OBFUSCATED != 0 {
            return Err("Obfuscated files are not supported".to_string());
        }
        Extracter::new(file, &mut self.volumes)?.copy_to(output, on_progress)?;
        let length = output.metadata().map_err(io_error)?.len();
        if length != u64::from(file.expanded_size) {
            return Err(format!(
                "File expanded to wrong length. Expected = {}, Got = {length}",
                file.expanded_size
            ));
        }
        Ok(())
    }
}

fn read_index(header: &mut dyn ReadSeek) -> Result<HashMap<String, FileDescriptor>, String> {
    if read_u32(header)? != SIGNATURE {
        return Err("Not an Installshield CAB package".to_string());
    }
    let raw_version = read_u32(header)?;
    let version = match raw_version >> 24 {
        1 => (raw_version >> 12) & 0xf,
        2 | 4 => (raw_version & 0xffff) / 100,
        _ => 0,
    };
    skip(header, 4)?;
    let descriptor = u64::from(read_u32(header)?);

    seek(header, descriptor + 12)?;
    let file_table_offset = u64::from(read_u32(header)?);
    skip(header, 4)?;
    let _file_table_size = read_u32(header)?;
    let _file_table_size2 = read_u32(header)?;
    let directory_count = read_u32(header)?;
    skip(header, 8)?;
    let _file_count = read_u32(header)?;
    let file_table_offset2 = u64::from(read_u32(header)?);
    skip(header, 14)?;
    let mut group_offsets = [0u32; MAX_FILE_GROUP_COUNT];
    for offset in &mut group_offsets {
        *offset = read_u32(header)?;
    }

    let table = descriptor + file_table_offset;
    seek(header, table)?;
    let mut directories = Vec::new();
    for _ in 0..directory_count {
        let name_offset = u64::from(read_u32(header)?);
        directories.push(read_asciiz_at(header, table + name_offset)?);
    }

    let mut groups = Vec::new();
    let mut visited = HashSet::new();
    for offset in group_offsets {
        let mut next = offset;
        while next != 0 && visited.insert(next) {
            seek(header, descriptor + u64::from(next) + 4)?;
            let group_descriptor = u64::from(read_u32(header)?);
            next = read_u32(header)?;
            seek(header, descriptor + group_descriptor)?;
            let name_offset = u64::from(read_u32(header)?);
            skip(header, 18)?;
            if version <= 5 {
                skip(header, 54)?;
            }
            let first = read_u32(header)?;
            let last = read_u32(header)?;
            groups.push((read_asciiz_at(header, descriptor + name_offset)?, first, last));
        }
    }

    let mut index = HashMap::new();
    for (name, first, last) in groups {
        for i in first..=last {
            if version <= 5 {
                seek(header, table + file_table_offset2 + u64::from(i) * 4)?;
                let entry = u64::from(read_u32(header)?);
                seek(header, table + entry)?;
            } else {
                seek(header, table + file_table_offset2 + u64::from(i) * 0x57)?;
            }
            let file = FileDescriptor::read(header, i, table, version)?;
            let directory = directories
                .get(file.directory_index as usize)
                .ok_or_else(|| format!("bad directory index {}", file.directory_index))?;
            index.insert(format!("{name}\\{directory}\\{}", file.filename), file);
        }
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn put_u32(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u16(data: &mut [u8], at: usize, value: u16) {
        data[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_str(data: &mut [u8], at: usize, value: &str) {
        data[at..at + value.len()].copy_from_slice(value.as_bytes());
    }

    /// A header of format version 5 with one group "Grp" and the files `Dir` / `A.TXT` (compressed) and `Dir` / `B.TXT`
    /// (stored). The layout mirrors what the reader expects: all offsets are relative to `descriptor` or `table`.
    fn build_header(a: (u32, u32, u32), b: (u32, u32, u32)) -> Vec<u8> {
        let mut data = vec![0u8; 4096];
        let descriptor = 0x100usize;
        let table_rel = 0x200usize;
        let table = descriptor + table_rel;
        put_u32(&mut data, 0, SIGNATURE);
        put_u32(&mut data, 4, 0x0100_5000);
        put_u32(&mut data, 12, descriptor as u32);

        let d = descriptor + 12;
        put_u32(&mut data, d, table_rel as u32);
        put_u32(&mut data, d + 16, 1);
        let table2 = 0x40u32;
        put_u32(&mut data, d + 32, table2);
        let groups = d + 36 + 14;
        put_u32(&mut data, groups, 0x180);

        put_u32(&mut data, descriptor + 0x180 + 4, 0x1c0);
        let group = descriptor + 0x1c0;
        put_u32(&mut data, group, 0x1a0);
        put_str(&mut data, descriptor + 0x1a0, "Grp");
        put_u32(&mut data, group + 4 + 18 + 54, 0);
        put_u32(&mut data, group + 4 + 18 + 54 + 4, 1);

        put_u32(&mut data, table, 0x20);
        put_str(&mut data, table + 0x20, "Dir");
        put_u32(&mut data, table + table2 as usize, 0x80);
        put_u32(&mut data, table + table2 as usize + 4, 0xc0);
        for (entry, name_offset, name, flags, (expanded, compressed, offset)) in [
            (0x80usize, 0x30u32, "A.TXT", FLAG_COMPRESSED, a),
            (0xc0usize, 0x38u32, "B.TXT", 0, b),
        ] {
            let e = table + entry;
            put_u32(&mut data, e, name_offset);
            put_u32(&mut data, e + 4, 0);
            put_u16(&mut data, e + 8, flags);
            put_u32(&mut data, e + 10, expanded);
            put_u32(&mut data, e + 14, compressed);
            put_u32(&mut data, e + 38, offset);
            put_str(&mut data, table + name_offset as usize, name);
        }
        data
    }

    #[test]
    fn extracts_compressed_and_stored_files() {
        let dir = std::env::temp_dir().join(format!("rustora-cab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let text = b"compressed content, compressed content, compressed content".as_slice();
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text).unwrap();
        let deflated = encoder.finish().unwrap();
        let stored = b"stored".as_slice();

        let mut volume = SIGNATURE.to_le_bytes().to_vec();
        volume.resize(0x20, 0);
        let a_offset = volume.len() as u32;
        volume.extend_from_slice(&(deflated.len() as u16).to_le_bytes());
        volume.extend_from_slice(&deflated);
        let b_offset = volume.len() as u32;
        volume.extend_from_slice(stored);

        let header = build_header(
            (text.len() as u32, deflated.len() as u32 + 2, a_offset),
            (stored.len() as u32, stored.len() as u32, b_offset),
        );
        std::fs::write(dir.join("data.hdr"), &header).unwrap();
        std::fs::write(dir.join("data.cab"), &volume).unwrap();

        let volumes = HashMap::from([(0, File::open(dir.join("data.cab")).unwrap())]);
        let mut cab = InstallShieldCab::new(File::open(dir.join("data.hdr")).unwrap(), volumes).unwrap();
        let extract = |cab: &mut InstallShieldCab, name: &str| {
            let path = dir.join("out");
            let mut out = File::options()
                .create(true)
                .write(true)
                .read(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            cab.extract_file(name, &mut out, &|_| {}).unwrap();
            std::fs::read(&path).unwrap()
        };
        assert_eq!(extract(&mut cab, "Grp\\Dir\\A.TXT"), text);
        assert_eq!(extract(&mut cab, "Grp\\Dir\\B.TXT"), stored);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
