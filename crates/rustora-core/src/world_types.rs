// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Fixed point world types: `WDist`, `WPos`, `WVec`, `WAngle`, `WRot` and the cell types
//! `CPos`, `CVec`, `MPos`, `PPos`, `Int2`. Integer semantics follow C# (`unchecked` wrapping,
//! truncating division). `hash` returns the C# `GetHashCode` value used by sync hashing.

use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

use crate::angle_tables::{COSINE_TABLE, TAN_TABLE};

/// Port of `Exts.ISqrt(uint)` (floor).
pub fn isqrt_u32(number: u32) -> u32 {
    let mut divisor: u32 = 1 << 30;
    let mut root: u32 = 0;
    let mut remainder = number;
    while divisor > number {
        divisor >>= 2;
    }
    while divisor != 0 {
        if root.wrapping_add(divisor) <= remainder {
            remainder -= root + divisor;
            root += 2 * divisor;
        }
        root >>= 1;
        divisor >>= 2;
    }
    root
}

/// Port of `Exts.ISqrt(ulong)` (floor).
pub fn isqrt_u64(number: u64) -> u64 {
    let mut divisor: u64 = 1 << 62;
    let mut root: u64 = 0;
    let mut remainder = number;
    while divisor > number {
        divisor >>= 2;
    }
    while divisor != 0 {
        if root.wrapping_add(divisor) <= remainder {
            remainder -= root + divisor;
            root += 2 * divisor;
        }
        root >>= 1;
        divisor >>= 2;
    }
    root
}

/// `Exts.ISqrt(int)`. Panics on negative input like the C# version throws.
pub fn isqrt_i32(number: i32) -> i32 {
    assert!(
        number >= 0,
        "Attempted to calculate the square root of a negative integer: {number}"
    );
    isqrt_u32(number as u32) as i32
}

/// `Exts.ISqrt(long)`.
pub fn isqrt_i64(number: i64) -> i64 {
    assert!(
        number >= 0,
        "Attempted to calculate the square root of a negative integer: {number}"
    );
    isqrt_u64(number as u64) as i64
}

/// Parses an integer like `Exts.TryParseInt32Invariant` (`NumberStyles.Integer`: surrounding
/// whitespace and a leading sign allowed).
pub fn parse_i32(s: &str) -> Option<i32> {
    let t = s.trim();
    let t = t.strip_prefix('+').unwrap_or(t);
    t.parse().ok()
}

/// 1d world distance: 1024 units = 1 cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WDist {
    pub length: i32,
}

impl WDist {
    pub const ZERO: WDist = WDist { length: 0 };
    pub const MAX: WDist = WDist { length: i32::MAX };

    pub const fn new(length: i32) -> Self {
        WDist { length }
    }

    pub fn from_cells(cells: i32) -> Self {
        WDist::new(cells.wrapping_mul(1024))
    }

    pub fn length_squared(self) -> i64 {
        self.length as i64 * self.length as i64
    }

    pub fn hash(self) -> i32 {
        self.length
    }

    /// Accepts `1c512`, `-1c512` and `512`.
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let s = s.to_lowercase();
        let parts: Vec<&str> = s.split('c').collect();
        let (cell, mut subcell) = match parts.as_slice() {
            [c, sc] => (parse_i32(c)?, parse_i32(sc)?),
            [sc] => (0, parse_i32(sc)?),
            _ => return None,
        };
        if cell < 0 {
            subcell = -subcell;
        }
        Some(WDist::new(cell.wrapping_mul(1024).wrapping_add(subcell)))
    }

    /// `WDist.FromPDF`: average of `samples` uniform samples in [-1024, 1024).
    pub fn from_pdf(r: &mut MersenneTwister, samples: i32) -> Self {
        let mut sum = 0i32;
        for _ in 0..samples {
            sum = sum.wrapping_add(r.next_range(-1024, 1024));
        }
        WDist::new(sum / samples)
    }
}

impl fmt::Display for WDist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.length.unsigned_abs();
        let sign = if self.length < 0 { "-" } else { "" };
        write!(f, "{sign}{}c{}", abs / 1024, abs % 1024)
    }
}

impl Add for WDist {
    type Output = WDist;
    fn add(self, o: WDist) -> WDist {
        WDist::new(self.length.wrapping_add(o.length))
    }
}

impl Sub for WDist {
    type Output = WDist;
    fn sub(self, o: WDist) -> WDist {
        WDist::new(self.length.wrapping_sub(o.length))
    }
}

impl Neg for WDist {
    type Output = WDist;
    fn neg(self) -> WDist {
        WDist::new(self.length.wrapping_neg())
    }
}

impl Mul<i32> for WDist {
    type Output = WDist;
    fn mul(self, o: i32) -> WDist {
        WDist::new(self.length.wrapping_mul(o))
    }
}

impl Div<i32> for WDist {
    type Output = WDist;
    fn div(self, o: i32) -> WDist {
        WDist::new(self.length.wrapping_div(o))
    }
}

macro_rules! vec3 {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub struct $name {
            pub x: i32,
            pub y: i32,
            pub z: i32,
        }

        impl $name {
            pub const ZERO: $name = $name { x: 0, y: 0, z: 0 };

            pub const fn new(x: i32, y: i32, z: i32) -> Self {
                $name { x, y, z }
            }

            pub fn from_dists(x: WDist, y: WDist, z: WDist) -> Self {
                $name::new(x.length, y.length, z.length)
            }

            pub fn hash(self) -> i32 {
                self.x ^ self.y ^ self.z
            }

            /// Parses `x,y,z` where each component is a `WDist`.
            pub fn parse(s: &str) -> Option<Self> {
                let p: Vec<&str> = s.split(',').collect();
                if p.len() != 3 {
                    return None;
                }
                Some($name::new(
                    WDist::parse(p[0])?.length,
                    WDist::parse(p[1])?.length,
                    WDist::parse(p[2])?.length,
                ))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{},{},{}", self.x, self.y, self.z)
            }
        }
    };
}

vec3!(WPos);
vec3!(WVec);

impl Add<WVec> for WPos {
    type Output = WPos;
    fn add(self, b: WVec) -> WPos {
        WPos::new(self.x.wrapping_add(b.x), self.y.wrapping_add(b.y), self.z.wrapping_add(b.z))
    }
}

impl Sub<WVec> for WPos {
    type Output = WPos;
    fn sub(self, b: WVec) -> WPos {
        WPos::new(self.x.wrapping_sub(b.x), self.y.wrapping_sub(b.y), self.z.wrapping_sub(b.z))
    }
}

impl Sub for WPos {
    type Output = WVec;
    fn sub(self, b: WPos) -> WVec {
        WVec::new(self.x.wrapping_sub(b.x), self.y.wrapping_sub(b.y), self.z.wrapping_sub(b.z))
    }
}

/// Truncating division of an exact rational, like `(int)` of a C# `decimal` quotient.
fn trunc_div_i128(n: i128, d: i128) -> i128 {
    n / d
}

impl WPos {
    pub fn to_vec(self) -> WVec {
        WVec::new(self.x, self.y, self.z)
    }

    pub fn lerp(a: WPos, b: WPos, mul: i32, div: i32) -> WPos {
        a + (b - a) * mul / div
    }

    pub fn lerp_long(a: WPos, b: WPos, mul: i64, div: i64) -> WPos {
        let c = |a: i32, b: i32| (a as i64).wrapping_add((b as i64 - a as i64).wrapping_mul(mul) / div) as i32;
        WPos::new(c(a.x, b.x), c(a.y, b.y), c(a.z, b.z))
    }

    pub fn lerp_quadratic(a: WPos, b: WPos, pitch: WAngle, mul: i32, div: i32) -> WPos {
        let ret = WPos::lerp(a, b, mul, div);
        if pitch.angle == 0 {
            return ret;
        }
        let n = (b - a).length() as i128 * pitch.tan() as i128 * mul as i128 * (div as i128 - mul as i128);
        let d = 1024i128 * div as i128 * div as i128;
        let z = trunc_div_i128(n + ret.z as i128 * d, d).clamp(i32::MIN as i128, i32::MAX as i128);
        WPos::new(ret.x, ret.y, z as i32)
    }

    /// `IEnumerable<WPos>.Average()`.
    pub fn average(items: impl IntoIterator<Item = WPos>) -> WPos {
        let (mut n, mut x, mut y, mut z) = (0i64, 0i64, 0i64, 0i64);
        for p in items {
            n += 1;
            x += p.x as i64;
            y += p.y as i64;
            z += p.z as i64;
        }
        if n == 0 {
            return WPos::ZERO;
        }
        WPos::new((x / n) as i32, (y / n) as i32, (z / n) as i32)
    }
}

impl Add for WVec {
    type Output = WVec;
    fn add(self, b: WVec) -> WVec {
        WVec::new(self.x.wrapping_add(b.x), self.y.wrapping_add(b.y), self.z.wrapping_add(b.z))
    }
}

impl Sub for WVec {
    type Output = WVec;
    fn sub(self, b: WVec) -> WVec {
        WVec::new(self.x.wrapping_sub(b.x), self.y.wrapping_sub(b.y), self.z.wrapping_sub(b.z))
    }
}

impl Neg for WVec {
    type Output = WVec;
    fn neg(self) -> WVec {
        WVec::new(self.x.wrapping_neg(), self.y.wrapping_neg(), self.z.wrapping_neg())
    }
}

impl Mul<i32> for WVec {
    type Output = WVec;
    fn mul(self, b: i32) -> WVec {
        WVec::new(self.x.wrapping_mul(b), self.y.wrapping_mul(b), self.z.wrapping_mul(b))
    }
}

impl Div<i32> for WVec {
    type Output = WVec;
    fn div(self, b: i32) -> WVec {
        WVec::new(self.x.wrapping_div(b), self.y.wrapping_div(b), self.z.wrapping_div(b))
    }
}

impl WVec {
    pub fn dot(a: WVec, b: WVec) -> i32 {
        a.x.wrapping_mul(b.x)
            .wrapping_add(a.y.wrapping_mul(b.y))
            .wrapping_add(a.z.wrapping_mul(b.z))
    }

    pub fn length_squared(self) -> i64 {
        let (x, y, z) = (self.x as i64, self.y as i64, self.z as i64);
        x.wrapping_mul(x).wrapping_add(y.wrapping_mul(y)).wrapping_add(z.wrapping_mul(z))
    }

    pub fn length(self) -> i32 {
        isqrt_i64(self.length_squared()) as i32
    }

    pub fn horizontal_length_squared(self) -> i64 {
        let (x, y) = (self.x as i64, self.y as i64);
        x * x + y * y
    }

    pub fn horizontal_length(self) -> i32 {
        isqrt_i64(self.horizontal_length_squared()) as i32
    }

    pub fn vertical_length_squared(self) -> i64 {
        self.z as i64 * self.z as i64
    }

    pub fn vertical_length(self) -> i32 {
        isqrt_i64(self.vertical_length_squared()) as i32
    }

    pub fn rotate(self, rot: &WRot) -> WVec {
        self.rotate_matrix(&rot.as_matrix())
    }

    pub fn rotate_matrix(self, m: &Int32Matrix4x4) -> WVec {
        let (lx, ly, lz) = (self.x as i64, self.y as i64, self.z as i64);
        let c = |a: i32, b: i32, c: i32| ((lx * a as i64 + ly * b as i64 + lz * c as i64) / m.m[3][3] as i64) as i32;
        WVec::new(
            c(m.m[0][0], m.m[1][0], m.m[2][0]),
            c(m.m[0][1], m.m[1][1], m.m[2][1]),
            c(m.m[0][2], m.m[1][2], m.m[2][2]),
        )
    }

    pub fn yaw(self) -> WAngle {
        if self.length_squared() == 0 {
            return WAngle::ZERO;
        }
        WAngle::arc_tan(self.y.wrapping_neg(), self.x) - WAngle::new(256)
    }

    pub fn lerp(a: WVec, b: WVec, mul: i32, div: i32) -> WVec {
        a + (b - a) * mul / div
    }

    pub fn lerp_quadratic(a: WVec, b: WVec, pitch: WAngle, mul: i32, div: i32) -> WVec {
        let ret = WVec::lerp(a, b, mul, div);
        if pitch.angle == 0 {
            return ret;
        }
        let n = (b - a).length() as i128 * pitch.tan() as i128 * mul as i128 * (div as i128 - mul as i128);
        let d = 1024i128 * div as i128 * div as i128;
        let offset = trunc_div_i128(n, d) as i32;
        WVec::new(ret.x, ret.y, ret.z.wrapping_add(offset))
    }

    pub fn from_pdf(r: &mut MersenneTwister, samples: i32) -> WVec {
        let x = WDist::from_pdf(r, samples);
        let y = WDist::from_pdf(r, samples);
        WVec::new(x.length, y.length, 0)
    }
}

/// 1d angle: 1024 units = 360 degrees, always normalized to [0, 1024).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WAngle {
    pub angle: i32,
}

impl WAngle {
    pub const ZERO: WAngle = WAngle { angle: 0 };

    pub const fn new(a: i32) -> Self {
        let mut angle = a % 1024;
        if angle < 0 {
            angle += 1024;
        }
        WAngle { angle }
    }

    pub fn from_facing(facing: i32) -> Self {
        WAngle::new(facing.wrapping_mul(4))
    }

    pub fn from_degrees(degrees: i32) -> Self {
        WAngle::new(degrees.wrapping_mul(1024) / 360)
    }

    pub fn hash(self) -> i32 {
        self.angle
    }

    pub fn facing(self) -> i32 {
        self.angle / 4
    }

    pub fn angle_squared(self) -> i32 {
        self.angle.wrapping_mul(self.angle)
    }

    pub fn sin(self) -> i32 {
        WAngle::new(self.angle - 256).cos()
    }

    pub fn cos(self) -> i32 {
        if self.angle <= 256 {
            return COSINE_TABLE[self.angle as usize];
        }
        if self.angle <= 512 {
            return -COSINE_TABLE[(512 - self.angle) as usize];
        }
        -WAngle::new(self.angle - 512).cos()
    }

    pub fn tan(self) -> i32 {
        if self.angle <= 256 {
            return TAN_TABLE[self.angle as usize];
        }
        if self.angle <= 512 {
            return TAN_TABLE[(512 - self.angle) as usize].wrapping_neg();
        }
        WAngle::new(self.angle - 512).tan()
    }

    pub fn lerp(a: WAngle, b: WAngle, mul: i32, div: i32) -> WAngle {
        let (mut aa, mut bb) = (a.angle, b.angle);
        if aa > bb && aa - bb > 512 {
            aa -= 1024;
        }
        if bb > aa && bb - aa > 512 {
            bb -= 1024;
        }
        WAngle::new(aa + (bb - aa).wrapping_mul(mul) / div)
    }

    pub fn arc_sin(d: i32) -> WAngle {
        assert!(
            (-1024..=1024).contains(&d),
            "ArcSin is only valid for values between -1024 and 1024. Received {d}"
        );
        let a = closest_cosine_index(d.abs());
        WAngle::new(if d < 0 { 768 + a } else { 256 - a })
    }

    pub fn arc_cos(d: i32) -> WAngle {
        assert!(
            (-1024..=1024).contains(&d),
            "ArcCos is only valid for values between -1024 and 1024. Received {d}"
        );
        let a = closest_cosine_index(d.abs());
        WAngle::new(if d < 0 { 512 - a } else { a })
    }

    pub fn arc_tan(y: i32, x: i32) -> WAngle {
        WAngle::arc_tan_stride(y, x, 1)
    }

    pub fn arc_tan_stride(y: i32, x: i32, stride: i32) -> WAngle {
        if y == 0 {
            return WAngle::new(if x >= 0 { 0 } else { 512 });
        }
        if x == 0 {
            return WAngle::new(y.signum() * 256);
        }
        let ay = y.wrapping_abs();
        let ax = x.wrapping_abs();
        let mut best_val = i64::MAX;
        let mut best_angle = 0;
        let mut i = 0;
        while i < 256 {
            let val = ((1024 * ay) as i64)
                .wrapping_sub((ax as i64).wrapping_mul(TAN_TABLE[i as usize] as i64))
                .wrapping_abs();
            if val < best_val {
                best_val = val;
                best_angle = i;
            }
            i += stride;
        }
        if x < 0 && y > 0 {
            best_angle = 512 - best_angle;
        } else if x < 0 && y < 0 {
            best_angle += 512;
        } else if x > 0 && y < 0 {
            best_angle = 1024 - best_angle;
        }
        WAngle::new(best_angle)
    }

    /// Only for rendering code.
    pub fn renderer_radians(self) -> f32 {
        (self.angle as f64 * std::f64::consts::PI / 512.0) as f32
    }
}

fn closest_cosine_index(value: i32) -> i32 {
    let mut above = 0usize;
    let mut below = 256usize;
    while above != below - 1 {
        let index = (above + below) / 2;
        let val = COSINE_TABLE[index];
        if val == value {
            return index as i32;
        }
        if val < value {
            below = index;
        } else {
            above = index;
        }
    }
    if COSINE_TABLE[above] - value > value - COSINE_TABLE[below] {
        below as i32
    } else {
        above as i32
    }
}

impl Add for WAngle {
    type Output = WAngle;
    fn add(self, b: WAngle) -> WAngle {
        WAngle::new(self.angle + b.angle)
    }
}

impl Sub for WAngle {
    type Output = WAngle;
    fn sub(self, b: WAngle) -> WAngle {
        WAngle::new(self.angle - b.angle)
    }
}

impl Neg for WAngle {
    type Output = WAngle;
    fn neg(self) -> WAngle {
        WAngle::new(-self.angle)
    }
}

impl fmt::Display for WAngle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.angle)
    }
}

/// A 4x4 integer matrix, `m[row][col]` corresponds to C# `M{row+1}{col+1}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Int32Matrix4x4 {
    pub m: [[i32; 4]; 4],
}

/// 3d world rotation, stored as Euler angles plus the integer quaternion used for calculations.
#[derive(Clone, Copy, Debug)]
pub struct WRot {
    pub roll: WAngle,
    pub pitch: WAngle,
    pub yaw: WAngle,
    x: i32,
    y: i32,
    z: i32,
    w: i32,
}

impl PartialEq for WRot {
    fn eq(&self, o: &WRot) -> bool {
        self.roll == o.roll && self.pitch == o.pitch && self.yaw == o.yaw
    }
}

impl Eq for WRot {}

impl Default for WRot {
    fn default() -> Self {
        WRot::NONE
    }
}

impl WRot {
    pub const NONE: WRot = WRot {
        roll: WAngle::ZERO,
        pitch: WAngle::ZERO,
        yaw: WAngle::ZERO,
        x: 0,
        y: 0,
        z: 0,
        w: 1024,
    };

    pub fn new(roll: WAngle, pitch: WAngle, yaw: WAngle) -> Self {
        let qr = WAngle::new(-roll.angle / 2);
        let qp = WAngle::new(-pitch.angle / 2);
        let qy = WAngle::new(-yaw.angle / 2);
        let (cr, sr) = (qr.cos() as i64, qr.sin() as i64);
        let (cp, sp) = (qp.cos() as i64, qp.sin() as i64);
        let (cy, sy) = (qy.cos() as i64, qy.sin() as i64);
        WRot {
            roll,
            pitch,
            yaw,
            x: ((sr * cp * cy - cr * sp * sy) / 1048576) as i32,
            y: ((cr * sp * cy + sr * cp * sy) / 1048576) as i32,
            z: ((cr * cp * sy - sr * sp * cy) / 1048576) as i32,
            w: ((cr * cp * cy + sr * sp * sy) / 1048576) as i32,
        }
    }

    pub fn from_axis_angle(axis: WVec, angle: WAngle) -> Self {
        let s = WAngle::new(-angle.angle / 2).sin();
        let x = axis.x.wrapping_mul(s) / 1024;
        let y = axis.y.wrapping_mul(s) / 1024;
        let z = axis.z.wrapping_mul(s) / 1024;
        let w = WAngle::new(-angle.angle / 2).cos();
        WRot::from_quaternion(x, y, z, w)
    }

    fn from_quaternion(x: i32, y: i32, z: i32, w: i32) -> Self {
        let (roll, pitch, yaw) = quaternion_to_euler(x, y, z, w);
        WRot {
            roll,
            pitch,
            yaw,
            x,
            y,
            z,
            w,
        }
    }

    pub fn from_facing(facing: i32) -> Self {
        WRot::new(WAngle::ZERO, WAngle::ZERO, WAngle::from_facing(facing))
    }

    pub fn from_yaw(yaw: WAngle) -> Self {
        WRot::new(WAngle::ZERO, WAngle::ZERO, yaw)
    }

    pub fn hash(self) -> i32 {
        self.roll.hash() ^ self.pitch.hash() ^ self.yaw.hash()
    }

    pub fn with_roll(self, roll: WAngle) -> Self {
        WRot::new(roll, self.pitch, self.yaw)
    }

    pub fn with_pitch(self, pitch: WAngle) -> Self {
        WRot::new(self.roll, pitch, self.yaw)
    }

    pub fn with_yaw(self, yaw: WAngle) -> Self {
        WRot::new(self.roll, self.pitch, yaw)
    }

    pub fn rotate(self, rot: &WRot) -> WRot {
        if self == WRot::NONE {
            return *rot;
        }
        if *rot == WRot::NONE {
            return self;
        }
        let (x, y, z, w) = (self.x as i64, self.y as i64, self.z as i64, self.w as i64);
        let (rx, ry, rz, rw) = (rot.x as i64, rot.y as i64, rot.z as i64, rot.w as i64);
        WRot::from_quaternion(
            ((rw * x + rx * w + ry * z - rz * y) / 1024) as i32,
            ((rw * y - rx * z + ry * w + rz * x) / 1024) as i32,
            ((rw * z + rx * y - ry * x + rz * w) / 1024) as i32,
            ((rw * w - rx * x - ry * y - rz * z) / 1024) as i32,
        )
    }

    pub fn as_matrix(&self) -> Int32Matrix4x4 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let lsq = x * x + y * y + z * z + w * w;
        Int32Matrix4x4 {
            m: [
                [lsq - 2 * (y * y + z * z), 2 * (x * y + z * w), 2 * (x * z - y * w), 0],
                [2 * (x * y - z * w), lsq - 2 * (x * x + z * z), 2 * (y * z + x * w), 0],
                [2 * (x * z + y * w), 2 * (y * z - x * w), lsq - 2 * (x * x + y * y), 0],
                [0, 0, 0, lsq],
            ],
        }
    }

    pub fn slerp(a: &WRot, b: &WRot, mul: i32, div: i32) -> WRot {
        let dot = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
        let flip: i32 = if dot >= 0 { 1 } else { -1 };
        if flip * dot >= 1024 * 1024 {
            return *a;
        }
        let theta = WAngle::arc_cos(dot / 1024);
        let s1 = WAngle::new((div - mul) * theta.angle / div).sin() as i64;
        let s2 = WAngle::new(mul * theta.angle / div).sin() as i64;
        let s3 = theta.sin() as i64;
        let f = flip as i64;
        let x = (a.x as i64 * s1 + f * b.x as i64 * s2) / s3;
        let y = (a.y as i64 * s1 + f * b.y as i64 * s2) / s3;
        let z = (a.z as i64 * s1 + f * b.z as i64 * s2) / s3;
        let w = (a.w as i64 * s1 + f * b.w as i64 * s2) / s3;
        let l = isqrt_i64(x * x + y * y + z * z + w * w);
        WRot::from_quaternion(
            (1024 * x / l) as i32,
            (1024 * y / l) as i32,
            (1024 * z / l) as i32,
            (1024 * w / l) as i32,
        )
    }
}

impl Neg for WRot {
    type Output = WRot;
    fn neg(self) -> WRot {
        WRot {
            roll: -self.roll,
            pitch: -self.pitch,
            yaw: -self.yaw,
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }
}

impl Add for WRot {
    type Output = WRot;
    fn add(self, b: WRot) -> WRot {
        WRot::new(self.roll + b.roll, self.pitch + b.pitch, self.yaw + b.yaw)
    }
}

impl Sub for WRot {
    type Output = WRot;
    fn sub(self, b: WRot) -> WRot {
        WRot::new(self.roll - b.roll, self.pitch - b.pitch, self.yaw - b.yaw)
    }
}

impl fmt::Display for WRot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{}", self.roll, self.pitch, self.yaw)
    }
}

fn quaternion_to_euler(x: i32, y: i32, z: i32, w: i32) -> (WAngle, WAngle, WAngle) {
    let lsq = x
        .wrapping_mul(x)
        .wrapping_add(y.wrapping_mul(y))
        .wrapping_add(z.wrapping_mul(z))
        .wrapping_add(w.wrapping_mul(w));
    let srcp = 2 * (w * x + y * z);
    let crcp = lsq - 2 * (x * x + y * y);
    let sp = (w * y - z * x) / 512;
    let sycp = 2 * (w * z + x * y);
    let cycp = lsq - 2 * (y * y + z * z);
    let roll = -WAngle::arc_tan(srcp, crcp);
    let pitch = -(if sp.abs() >= 1024 {
        WAngle::new(sp.signum() * 256)
    } else {
        WAngle::arc_sin(sp)
    });
    let yaw = -WAngle::arc_tan(sycp, cycp);
    (roll, pitch, yaw)
}

/// Cell position packed into 32 bits: 12 bit signed X, 12 bit signed Y, 8 bit layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CPos {
    pub bits: i32,
}

impl CPos {
    pub const ZERO: CPos = CPos { bits: 0 };

    pub const fn from_bits(bits: i32) -> Self {
        CPos { bits }
    }

    pub const fn new(x: i32, y: i32) -> Self {
        CPos::with_layer(x, y, 0)
    }

    pub const fn with_layer(x: i32, y: i32, layer: u8) -> Self {
        CPos {
            bits: ((x & 0xFFF) << 20) | ((y & 0xFFF) << 8) | layer as i32,
        }
    }

    pub const fn x(self) -> i32 {
        self.bits >> 20
    }

    pub const fn y(self) -> i32 {
        ((self.bits >> 4) as i16 >> 4) as i32
    }

    pub const fn layer(self) -> u8 {
        self.bits as u8
    }

    pub fn hash(self) -> i32 {
        self.bits
    }

    pub fn to_mpos(self, grid: MapGridType) -> MPos {
        match grid {
            MapGridType::Rectangular => MPos::new(self.x(), self.y()),
            MapGridType::RectangularIsometric => MPos::new((self.x() - self.y()) / 2, self.x() + self.y()),
        }
    }
}

impl fmt::Display for CPos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.layer() == 0 {
            write!(f, "{},{}", self.x(), self.y())
        } else {
            write!(f, "{},{},{}", self.x(), self.y(), self.layer())
        }
    }
}

impl Add<CVec> for CPos {
    type Output = CPos;
    fn add(self, b: CVec) -> CPos {
        CPos::with_layer(self.x() + b.x, self.y() + b.y, self.layer())
    }
}

impl Sub<CVec> for CPos {
    type Output = CPos;
    fn sub(self, b: CVec) -> CPos {
        CPos::with_layer(self.x() - b.x, self.y() - b.y, self.layer())
    }
}

impl Sub for CPos {
    type Output = CVec;
    fn sub(self, b: CPos) -> CVec {
        CVec::new(self.x() - b.x(), self.y() - b.y())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MapGridType {
    #[default]
    Rectangular,
    RectangularIsometric,
}

macro_rules! vec2 {
    ($name:ident, $a:ident, $b:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name {
            pub $a: i32,
            pub $b: i32,
        }

        impl $name {
            pub const ZERO: $name = $name { $a: 0, $b: 0 };

            pub const fn new($a: i32, $b: i32) -> Self {
                $name { $a, $b }
            }

            pub fn hash(self) -> i32 {
                self.$a ^ self.$b
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{},{}", self.$a, self.$b)
            }
        }
    };
}

vec2!(CVec, x, y);
vec2!(Int2, x, y);
vec2!(MPos, u, v);
vec2!(PPos, u, v);

impl CVec {
    pub const DIRECTIONS: [CVec; 8] = [
        CVec::new(-1, -1),
        CVec::new(-1, 0),
        CVec::new(-1, 1),
        CVec::new(0, -1),
        CVec::new(0, 1),
        CVec::new(1, -1),
        CVec::new(1, 0),
        CVec::new(1, 1),
    ];

    pub fn length_squared(self) -> i32 {
        self.x * self.x + self.y * self.y
    }

    pub fn length(self) -> i32 {
        isqrt_i32(self.length_squared())
    }

    pub fn sign(self) -> CVec {
        CVec::new(self.x.signum(), self.y.signum())
    }

    pub fn abs(self) -> CVec {
        CVec::new(self.x.abs(), self.y.abs())
    }
}

impl Add for CVec {
    type Output = CVec;
    fn add(self, b: CVec) -> CVec {
        CVec::new(self.x + b.x, self.y + b.y)
    }
}

impl Sub for CVec {
    type Output = CVec;
    fn sub(self, b: CVec) -> CVec {
        CVec::new(self.x - b.x, self.y - b.y)
    }
}

impl Neg for CVec {
    type Output = CVec;
    fn neg(self) -> CVec {
        CVec::new(-self.x, -self.y)
    }
}

impl Mul<i32> for CVec {
    type Output = CVec;
    fn mul(self, a: i32) -> CVec {
        CVec::new(a * self.x, a * self.y)
    }
}

impl Div<i32> for CVec {
    type Output = CVec;
    fn div(self, a: i32) -> CVec {
        CVec::new(self.x / a, self.y / a)
    }
}

impl Int2 {
    pub fn length_squared(self) -> i32 {
        self.x * self.x + self.y * self.y
    }

    pub fn length(self) -> i32 {
        isqrt_i32(self.length_squared())
    }

    pub fn lerp_i32(a: i32, b: i32, mul: i32, div: i32) -> i32 {
        a + (b - a) * mul / div
    }
}

impl Add for Int2 {
    type Output = Int2;
    fn add(self, b: Int2) -> Int2 {
        Int2::new(self.x + b.x, self.y + b.y)
    }
}

impl Sub for Int2 {
    type Output = Int2;
    fn sub(self, b: Int2) -> Int2 {
        Int2::new(self.x - b.x, self.y - b.y)
    }
}

impl MPos {
    pub fn to_cpos(self, grid: MapGridType) -> CPos {
        match grid {
            MapGridType::Rectangular => CPos::new(self.u, self.v),
            MapGridType::RectangularIsometric => {
                let offset = if (self.v & 1) == 1 { 1 } else { 0 };
                let y = (self.v - offset) / 2 - self.u;
                let x = self.v - y;
                CPos::new(x, y)
            }
        }
    }
}

/// Fisher-Yates shuffle consuming the random numbers like the C# iterator when fully enumerated.
pub fn shuffle<T: Clone>(ts: &[T], random: &mut MersenneTwister) -> Vec<T> {
    let mut items = ts.to_vec();
    if items.is_empty() {
        return items;
    }
    for i in 0..items.len() - 1 {
        let j = random.next_max((items.len() - i) as i32) as usize;
        items.swap(i, i + j);
    }
    items
}

thread_local! {
    /// The actor whose activities are ticking, printed with traced random draws.
    pub static TRACE_ACTOR_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Port of OpenRA's `MersenneTwister` (MT19937 with OpenRA's output reduction).
#[derive(Clone)]
pub struct MersenneTwister {
    mt: [u32; 624],
    index: usize,
    pub last: i32,
    pub total_count: i32,
    /// Prints a backtrace for each draw whose count is in this inclusive range, for desync debugging.
    pub trace: Option<(i32, i32)>,
}

impl MersenneTwister {
    pub fn new(seed: i32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = seed as u32;
        for i in 1..624u32 {
            let p = mt[i as usize - 1];
            mt[i as usize] = 1812433253u32.wrapping_mul(p ^ (p >> 30)).wrapping_add(i);
        }
        MersenneTwister {
            mt,
            index: 0,
            last: 0,
            total_count: 0,
            trace: None,
        }
    }

    /// `MersenneTwister.Next()`; named like the C# method, it is not an iterator.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> i32 {
        if self.index == 0 {
            self.generate();
        }
        let mut y = self.mt[self.index];
        y ^= y >> 11;
        y ^= (y << 7) & 2636928640;
        y ^= (y << 15) & 4022730752;
        y ^= y >> 18;
        self.index = (self.index + 1) % 624;
        self.total_count = self.total_count.wrapping_add(1);
        self.last = (y % i32::MAX as u32) as i32;
        if let Some((from, to)) = self.trace
            && (from..=to).contains(&self.total_count)
        {
            let actor = TRACE_ACTOR_ID.with(|a| a.get());
            eprintln!(
                "RANDOM {} actor {actor}\n{}",
                self.total_count,
                std::backtrace::Backtrace::force_capture()
            );
        }
        self.last
    }

    /// Value in [low, high); returns `low` when the range is at most 1 wide.
    pub fn next_range(&mut self, low: i32, high: i32) -> i32 {
        assert!(high >= low, "Maximum value is less than the minimum value.");
        let diff = high.wrapping_sub(low);
        if diff <= 1 {
            return low;
        }
        low.wrapping_add(self.next() % diff)
    }

    pub fn next_max(&mut self, high: i32) -> i32 {
        self.next_range(0, high)
    }

    pub fn next_float(&mut self) -> f32 {
        (self.next() as f32 / 0x7fffffff as f32).abs()
    }

    fn generate(&mut self) {
        for i in 0..624usize {
            let y = (self.mt[i] & 0x80000000) | (self.mt[(i + 1) % 624] & 0x7fffffff);
            self.mt[i] = self.mt[(i + 397) % 624] ^ (y >> 1);
            if (y & 1) == 1 {
                self.mt[i] ^= 2567483615;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wdist_parse_and_display() {
        assert_eq!(WDist::parse("1c512"), Some(WDist::new(1536)));
        assert_eq!(WDist::parse("-1c512"), Some(WDist::new(-1536)));
        assert_eq!(WDist::parse("300"), Some(WDist::new(300)));
        assert_eq!(WDist::parse("a"), None);
        assert_eq!(WDist::new(-1536).to_string(), "-1c512");
    }

    #[test]
    fn angles_are_normalized_and_trig_matches_tables() {
        assert_eq!(WAngle::new(-1).angle, 1023);
        assert_eq!(WAngle::new(0).cos(), 1024);
        assert_eq!(WAngle::new(256).sin(), 1024);
        assert_eq!(WAngle::new(768).sin(), -1024);
        assert_eq!(WAngle::arc_tan(1, 1).angle, 128);
        assert_eq!(WAngle::arc_tan(-5, -5).angle, 640);
    }

    #[test]
    fn cpos_packing_round_trips_negative_coordinates() {
        let c = CPos::with_layer(-3, -7, 2);
        assert_eq!((c.x(), c.y(), c.layer()), (-3, -7, 2));
    }

    #[test]
    fn yaw_rotation_of_vector() {
        let v = WVec::new(0, -1024, 0).rotate(&WRot::from_yaw(WAngle::new(256)));
        assert_eq!(v, WVec::new(-1024, 0, 0));
    }

    #[test]
    fn mersenne_twister_first_values() {
        let mut r = MersenneTwister::new(5489);
        // MT19937 reference output for seed 5489 is 3499211612; OpenRA reduces modulo int.MaxValue.
        assert_eq!(r.next(), (3499211612u32 % i32::MAX as u32) as i32);
    }
}
