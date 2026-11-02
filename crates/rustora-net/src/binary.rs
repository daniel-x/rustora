// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Little endian readers and writers compatible with .NET `BinaryReader`/`BinaryWriter`.

use crate::NetError;

pub struct Reader<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], NetError> {
        if self.remaining() < n {
            return Err(NetError::Invalid("unexpected end of data".into()));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, NetError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn i16(&mut self) -> Result<i16, NetError> {
        Ok(i16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32, NetError> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32, NetError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64, NetError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Result<i64, NetError> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32, NetError> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    /// `BinaryReader.ReadString`: 7-bit encoded byte length, then UTF-8.
    pub fn string(&mut self) -> Result<String, NetError> {
        let mut len = 0usize;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            len |= ((b & 0x7f) as usize) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 35 {
                return Err(NetError::Invalid("bad string length".into()));
            }
        }
        Ok(String::from_utf8_lossy(self.bytes(len)?).into_owned())
    }
}

#[derive(Default)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.data.push(v);
    }

    pub fn i16(&mut self, v: i16) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn string(&mut self, s: &str) {
        let mut len = s.len();
        loop {
            let mut b = (len & 0x7f) as u8;
            len >>= 7;
            if len != 0 {
                b |= 0x80;
            }
            self.data.push(b);
            if len == 0 {
                break;
            }
        }
        self.data.extend_from_slice(s.as_bytes());
    }
}
