// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `OpenRA.Game/FieldLoader.cs`: typed values from MiniYaml.
//!
//! [`FieldValue`] parses one value (like the C# `TypeParsers`), [`FieldLoad`] loads a struct
//! (derive it with `rustora_derive::FieldLoad`).

use std::hash::Hash;

use indexmap::{IndexMap, IndexSet};

use crate::miniyaml::MiniYaml;
use crate::world_types::*;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct LoadError(pub String);

impl From<crate::miniyaml::YamlError> for LoadError {
    fn from(e: crate::miniyaml::YamlError) -> Self {
        LoadError(e.0)
    }
}

pub fn invalid<T>(value: Option<&str>, ty: &str, field: &str) -> Result<T, LoadError> {
    Err(LoadError(format!(
        "FieldLoader: Cannot parse `{}` into `{field}.{ty}` ",
        value.unwrap_or("")
    )))
}

/// Child nodes of a yaml block keyed by name.
pub type NodeMap<'a> = IndexMap<&'a str, &'a MiniYaml>;

/// A value that can be parsed from a yaml value string (and, for collections, the child nodes).
pub trait FieldValue: Sized {
    const TYPE_NAME: &'static str;

    /// `value` is already trimmed; `None` stands for the C# `null`.
    fn parse(field: &str, value: Option<&str>, yaml: Option<&MiniYaml>) -> Result<Self, LoadError>;

    /// Parses a comma separated list into an array, like the C# array handling. Types with
    /// special array parsers (`WVec[]`, `CPos[]`, ...) override this.
    fn parse_array(field: &str, value: Option<&str>, allow_empty: bool) -> Result<Vec<Self>, LoadError> {
        let Some(v) = value else { return Ok(Vec::new()) };
        v.split(',')
            .filter(|p| allow_empty || !p.is_empty())
            .map(|p| Self::parse(field, Some(p.trim()), None))
            .collect()
    }
}

/// `FieldLoader.GetValue(fieldName, type, MiniYaml)`.
pub fn get_value_yaml<T: FieldValue + ArrayAware>(field: &str, yaml: &MiniYaml, allow_empty: bool) -> Result<T, LoadError> {
    let v = yaml.value.as_deref().map(str::trim);
    T::parse_with(field, v, Some(yaml), allow_empty)
}

/// `FieldLoader.GetValue<T>(field, value)`.
pub fn get_value<T: FieldValue + ArrayAware>(field: &str, value: Option<&str>) -> Result<T, LoadError> {
    T::parse_with(field, value.map(str::trim), None, false)
}

/// Lets arrays see the `AllowEmptyEntries` flag; all other types ignore it.
pub trait ArrayAware: FieldValue {
    fn parse_with(field: &str, value: Option<&str>, yaml: Option<&MiniYaml>, _allow_empty: bool) -> Result<Self, LoadError> {
        Self::parse(field, value, yaml)
    }
}

/// A struct loaded from the child nodes of a yaml block.
pub trait FieldLoad {
    fn load_fields(&mut self, my: &MiniYaml, md: &NodeMap<'_>, missing: &mut Vec<String>) -> Result<(), LoadError>;

    /// YAML keys this struct reads, for lint checks.
    fn field_names() -> Vec<&'static str>;

    /// `FieldLoader.Load(self, my)`: loads all fields, errors on duplicate keys and missing required fields.
    fn load(&mut self, my: &MiniYaml) -> Result<(), LoadError> {
        let md: NodeMap<'_> = my.to_map()?;
        let mut missing = Vec::new();
        self.load_fields(my, &md, &mut missing)?;
        if missing.is_empty() {
            Ok(())
        } else {
            Err(LoadError(missing.join(", ")))
        }
    }
}

/// Loads a fresh `T::default()` from yaml.
pub fn load<T: FieldLoad + Default>(my: &MiniYaml) -> Result<T, LoadError> {
    let mut t = T::default();
    t.load(my)?;
    Ok(t)
}

macro_rules! simple {
    ($t:ty, $name:literal, |$v:ident| $body:expr) => {
        impl FieldValue for $t {
            const TYPE_NAME: &'static str = $name;
            fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
                let r: Option<$t> = (|| -> Option<$t> {
                    let $v = value?;
                    $body
                })();
                match r {
                    Some(x) => Ok(x),
                    None => invalid(value, $name, field),
                }
            }
        }
        impl ArrayAware for $t {}
    };
}

simple!(i32, "Int32", |v| parse_i32(v));
simple!(u16, "UInt16", |v| v.trim().parse().ok());
simple!(u8, "Byte", |v| v.trim().parse().ok());
simple!(i64, "Int64", |v| v.trim().strip_prefix('+').unwrap_or(v.trim()).parse().ok());
simple!(f32, "Single", |v| parse_percent_f32(v));
simple!(f64, "Double", |v| parse_percent_f32(v).map(f64::from));
simple!(bool, "Boolean", |v| match v.to_lowercase().as_str() {
    "true" => Some(true),
    "false" => Some(false),
    _ => None,
});
simple!(WDist, "WDist", |v| WDist::parse(v));
simple!(WAngle, "WAngle", |v| parse_i32(v).map(WAngle::new));
simple!(Color, "Color", |v| Color::parse(v));

/// `float.Parse` with an optional `%` suffix meaning a factor of 0.01.
fn parse_percent_f32(v: &str) -> Option<f32> {
    let f: f32 = v.replace('%', "").trim().parse().ok()?;
    Some(if v.contains('%') { f * 0.01 } else { f })
}

impl FieldValue for String {
    const TYPE_NAME: &'static str = "String";
    fn parse(_field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        Ok(value.unwrap_or("").to_string())
    }
}
impl ArrayAware for String {}

/// `Nullable<T>`, and nullable references such as `string`: empty means `None`.
impl<T: FieldValue + ArrayAware> FieldValue for Option<T> {
    const TYPE_NAME: &'static str = T::TYPE_NAME;
    fn parse(field: &str, value: Option<&str>, yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        match value {
            None | Some("") => Ok(None),
            Some(v) => Ok(Some(T::parse(field, Some(v), yaml)?)),
        }
    }
}
impl<T: FieldValue + ArrayAware> ArrayAware for Option<T> {}

impl<T: FieldValue> FieldValue for Vec<T> {
    const TYPE_NAME: &'static str = "Array";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        T::parse_array(field, value, false)
    }
}
impl<T: FieldValue> ArrayAware for Vec<T> {
    fn parse_with(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>, allow_empty: bool) -> Result<Self, LoadError> {
        T::parse_array(field, value, allow_empty)
    }
}

/// `HashSet<T>` and `List<T>` share one parser; `IndexSet` keeps the C# insertion order.
impl<T: FieldValue + Hash + Eq> FieldValue for IndexSet<T> {
    const TYPE_NAME: &'static str = "HashSet";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let Some(v) = value else { return Ok(IndexSet::new()) };
        v.split(',')
            .filter(|p| !p.is_empty())
            .map(|p| T::parse(field, Some(p.trim()), None))
            .collect()
    }
}
impl<T: FieldValue + Hash + Eq> ArrayAware for IndexSet<T> {}

/// `Dictionary<K, V>` from child nodes; duplicate keys are an error like `Dictionary.Add`.
impl<K: FieldValue + Hash + Eq, V: FieldValue + ArrayAware> FieldValue for IndexMap<K, V> {
    const TYPE_NAME: &'static str = "Dictionary";
    fn parse(field: &str, _value: Option<&str>, yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let mut out = IndexMap::new();
        let Some(y) = yaml else { return Ok(out) };
        for n in &y.nodes {
            let k = K::parse(field, Some(n.key.trim()), None)?;
            let v = get_value_yaml::<V>(field, &n.value, false)?;
            if out.insert(k, v).is_some() {
                return Err(LoadError(format!(
                    "An item with the same key has already been added. Key: {}",
                    n.key
                )));
            }
        }
        Ok(out)
    }
}
impl<K: FieldValue + Hash + Eq, V: FieldValue + ArrayAware> ArrayAware for IndexMap<K, V> {}

impl FieldValue for MiniYaml {
    const TYPE_NAME: &'static str = "MiniYaml";
    fn parse(_field: &str, _value: Option<&str>, yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        Ok(yaml.cloned().unwrap_or_default())
    }
}
impl ArrayAware for MiniYaml {}

fn split3(v: &str) -> Option<[&str; 3]> {
    let p: Vec<&str> = v.split(',').collect();
    (p.len() == 3).then(|| [p[0], p[1], p[2]])
}

macro_rules! vec3_value {
    ($t:ty, $name:literal) => {
        impl FieldValue for $t {
            const TYPE_NAME: &'static str = $name;
            fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
                let r = value.and_then(split3).and_then(|[a, b, c]| {
                    Some(<$t>::new(
                        WDist::parse(a)?.length,
                        WDist::parse(b)?.length,
                        WDist::parse(c)?.length,
                    ))
                });
                r.map_or_else(|| invalid(value, $name, field), Ok)
            }
        }
        impl ArrayAware for $t {}
    };
}

vec3_value!(WPos, "WPos");

impl FieldValue for WVec {
    const TYPE_NAME: &'static str = "WVec";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let r = value
            .and_then(split3)
            .and_then(|[a, b, c]| Some(WVec::new(WDist::parse(a)?.length, WDist::parse(b)?.length, WDist::parse(c)?.length)));
        r.map_or_else(|| invalid(value, "WVec", field), Ok)
    }

    /// `ParseWVecArray`: no trimming of parts, invalid triples stay zero.
    fn parse_array(field: &str, value: Option<&str>, _allow_empty: bool) -> Result<Vec<Self>, LoadError> {
        let Some(v) = value else { return invalid(value, "WVec[]", field) };
        let parts: Vec<&str> = v.split(',').collect();
        if parts.len() % 3 != 0 {
            return invalid(value, "WVec[]", field);
        }
        Ok(parts
            .chunks(3)
            .map(|c| match (WDist::parse(c[0]), WDist::parse(c[1]), WDist::parse(c[2])) {
                (Some(x), Some(y), Some(z)) => WVec::new(x.length, y.length, z.length),
                _ => WVec::ZERO,
            })
            .collect())
    }
}
impl ArrayAware for WVec {}

impl FieldValue for WRot {
    const TYPE_NAME: &'static str = "WRot";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let r = value.and_then(split3).and_then(|[a, b, c]| {
            Some(WRot::new(
                WAngle::new(parse_i32(a)?),
                WAngle::new(parse_i32(b)?),
                WAngle::new(parse_i32(c)?),
            ))
        });
        r.map_or_else(|| invalid(value, "WRot", field), Ok)
    }
}
impl ArrayAware for WRot {}

fn int_parts(v: &str) -> Vec<&str> {
    v.split(',').filter(|p| !p.is_empty()).collect()
}

/// `int.TryParse` with the current (invariant) culture, used by the C# `CPos[]`/`CVec[]` parsers.
fn try_int(s: &str) -> Option<i32> {
    parse_i32(s)
}

impl FieldValue for CPos {
    const TYPE_NAME: &'static str = "CPos";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let Some(v) = value else { return invalid(value, "CPos", field) };
        let p = int_parts(v);
        let int = |s: &str| parse_i32(s).ok_or_else(|| LoadError(format!("Cannot parse `{s}` as integer")));
        if p.len() == 3 {
            let layer: u8 = p[2]
                .trim()
                .parse()
                .map_err(|_| LoadError(format!("Cannot parse `{}` as byte", p[2])))?;
            return Ok(CPos::with_layer(int(p[0])?, int(p[1])?, layer));
        }
        if p.len() < 2 {
            return invalid(value, "CPos", field);
        }
        Ok(CPos::new(int(p[0])?, int(p[1])?))
    }

    fn parse_array(field: &str, value: Option<&str>, _allow_empty: bool) -> Result<Vec<Self>, LoadError> {
        let Some(v) = value else { return invalid(value, "CPos[]", field) };
        let parts: Vec<&str> = v.split(',').collect();
        if parts.len() % 2 != 0 {
            return invalid(value, "CPos[]", field);
        }
        Ok(parts
            .chunks(2)
            .map(|c| match (try_int(c[0]), try_int(c[1])) {
                (Some(x), Some(y)) => CPos::new(x, y),
                _ => CPos::ZERO,
            })
            .collect())
    }
}
impl ArrayAware for CPos {}

impl FieldValue for CVec {
    const TYPE_NAME: &'static str = "CVec";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let Some(v) = value else { return invalid(value, "CVec", field) };
        let p = int_parts(v);
        match (p.first().and_then(|s| parse_i32(s)), p.get(1).and_then(|s| parse_i32(s))) {
            (Some(x), Some(y)) => Ok(CVec::new(x, y)),
            _ => invalid(value, "CVec", field),
        }
    }

    fn parse_array(field: &str, value: Option<&str>, _allow_empty: bool) -> Result<Vec<Self>, LoadError> {
        let Some(v) = value else { return invalid(value, "CVec[]", field) };
        let parts: Vec<&str> = v.split(',').collect();
        if parts.len() % 2 != 0 {
            return invalid(value, "CVec[]", field);
        }
        Ok(parts
            .chunks(2)
            .map(|c| match (try_int(c[0]), try_int(c[1])) {
                (Some(x), Some(y)) => CVec::new(x, y),
                _ => CVec::ZERO,
            })
            .collect())
    }
}
impl ArrayAware for CVec {}

impl FieldValue for Int2 {
    const TYPE_NAME: &'static str = "int2";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let Some(v) = value else { return invalid(value, "int2", field) };
        let p = int_parts(v);
        if p.len() != 2 {
            return invalid(value, "int2", field);
        }
        match (parse_i32(p[0]), parse_i32(p[1])) {
            (Some(x), Some(y)) => Ok(Int2::new(x, y)),
            _ => invalid(value, "int2", field),
        }
    }

    fn parse_array(field: &str, value: Option<&str>, _allow_empty: bool) -> Result<Vec<Self>, LoadError> {
        let Some(v) = value else { return invalid(value, "int2[]", field) };
        let p = int_parts(v);
        if p.len() % 2 != 0 {
            return invalid(value, "int2[]", field);
        }
        p.chunks(2)
            .map(|c| match (parse_i32(c[0]), parse_i32(c[1])) {
                (Some(x), Some(y)) => Ok(Int2::new(x, y)),
                _ => invalid(value, "int2[]", field),
            })
            .collect()
    }
}
impl ArrayAware for Int2 {}

/// `System.Drawing`-like size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl FieldValue for Size {
    const TYPE_NAME: &'static str = "Size";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let p = value.map(int_parts).unwrap_or_default();
        match (p.first().and_then(|s| parse_i32(s)), p.get(1).and_then(|s| parse_i32(s))) {
            (Some(width), Some(height)) => Ok(Size { width, height }),
            _ => invalid(value, "Size", field),
        }
    }
}
impl ArrayAware for Size {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rectangle {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Rectangle { x, y, width, height }
    }

    pub fn left(&self) -> i32 {
        self.x
    }

    pub fn top(&self) -> i32 {
        self.y
    }

    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left() && x < self.right() && y >= self.top() && y < self.bottom()
    }
}

impl FieldValue for Rectangle {
    const TYPE_NAME: &'static str = "Rectangle";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let p: Vec<i32> = value
            .map(int_parts)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| parse_i32(s))
            .collect();
        if p.len() < 4 {
            return invalid(value, "Rectangle", field);
        }
        Ok(Rectangle::new(p[0], p[1], p[2], p[3]))
    }
}
impl ArrayAware for Rectangle {}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Float2 {
    pub x: f32,
    pub y: f32,
}

impl FieldValue for Float2 {
    const TYPE_NAME: &'static str = "float2";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let Some(v) = value else { return invalid(value, "float2", field) };
        let p = int_parts(v);
        if p.len() < 2 {
            return invalid(value, "float2", field);
        }
        Ok(Float2 {
            x: parse_percent_f32(p[0]).unwrap_or(0.0),
            y: parse_percent_f32(p[1]).unwrap_or(0.0),
        })
    }
}
impl ArrayAware for Float2 {}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Float3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl FieldValue for Float3 {
    const TYPE_NAME: &'static str = "float3";
    fn parse(field: &str, value: Option<&str>, _yaml: Option<&MiniYaml>) -> Result<Self, LoadError> {
        let Some(v) = value else { return invalid(value, "float3", field) };
        let p = int_parts(v);
        let f = |i: usize| p.get(i).and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0);
        Ok(Float3 { x: f(0), y: f(1), z: f(2) })
    }
}
impl ArrayAware for Float3 {}

/// 32 bit ARGB color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub argb: u32,
}

impl Color {
    pub const fn from_argb(a: u8, r: u8, g: u8, b: u8) -> Self {
        Color {
            argb: (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32,
        }
    }

    /// `RRGGBB` or `RRGGBBAA` hex.
    pub fn parse(v: &str) -> Option<Self> {
        let v = v.trim();
        if v.len() != 6 && v.len() != 8 {
            return None;
        }
        let h = |i: usize| u8::from_str_radix(v.get(i..i + 2)?, 16).ok();
        let a = if v.len() == 8 { h(6)? } else { 255 };
        Some(Color::from_argb(a, h(0)?, h(2)?, h(4)?))
    }

    pub fn a(self) -> u8 {
        (self.argb >> 24) as u8
    }

    pub fn r(self) -> u8 {
        (self.argb >> 16) as u8
    }

    pub fn g(self) -> u8 {
        (self.argb >> 8) as u8
    }

    pub fn b(self) -> u8 {
        self.argb as u8
    }
}

/// Implements [`FieldValue`] for a C# style enum: case-insensitive names, and for `[Flags]`
/// enums comma separated combinations.
#[macro_export]
macro_rules! field_enum {
    ($t:ty, [$($v:ident),* $(,)?]) => {
        impl $crate::fieldloader::FieldValue for $t {
            const TYPE_NAME: &'static str = stringify!($t);
            fn parse(field: &str, value: Option<&str>, _yaml: Option<&$crate::miniyaml::MiniYaml>) -> Result<Self, $crate::fieldloader::LoadError> {
                let s = value.unwrap_or("").trim();
                $(if s.eq_ignore_ascii_case(stringify!($v)) { return Ok(<$t>::$v); })*
                $crate::fieldloader::invalid(value, stringify!($t), field)
            }
        }
        impl $crate::fieldloader::ArrayAware for $t {}
    };
}

/// Implements [`FieldValue`] for a bitflags-like newtype over an integer with named constants.
#[macro_export]
macro_rules! field_flags {
    ($t:ident, [$($v:ident),* $(,)?]) => {
        impl $crate::fieldloader::FieldValue for $t {
            const TYPE_NAME: &'static str = stringify!($t);
            fn parse(field: &str, value: Option<&str>, _yaml: Option<&$crate::miniyaml::MiniYaml>) -> Result<Self, $crate::fieldloader::LoadError> {
                let mut bits: i32 = 0;
                for part in value.unwrap_or("").split(',') {
                    let s = part.trim();
                    let mut found = false;
                    $(if s.eq_ignore_ascii_case(stringify!($v)) { bits |= $t::$v.0; found = true; })*
                    if !found {
                        if let Ok(n) = s.parse::<i32>() { bits |= n; } else { return $crate::fieldloader::invalid(value, stringify!($t), field); }
                    }
                }
                Ok($t(bits))
            }
        }
        impl $crate::fieldloader::ArrayAware for $t {}
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miniyaml::MiniYaml;

    #[test]
    fn arrays_drop_empty_entries_and_trim() {
        let v: Vec<i32> = get_value("f", Some("1, 2,,3")).unwrap();
        assert_eq!(v, vec![1, 2, 3]);
        let s: Vec<String> = get_value("f", None).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn dictionary_from_nodes() {
        let n = MiniYaml::from_str("D:\n\ta: 1\n\tb: 2\n", "t", true).unwrap();
        let d: IndexMap<String, i32> = get_value_yaml("D", &n[0].value, false).unwrap();
        assert_eq!(d.get("b"), Some(&2));
    }

    #[test]
    fn percent_floats_and_colors() {
        let f: f32 = get_value("f", Some("50%")).unwrap();
        assert!((f - 0.5).abs() < 1e-6);
        let c: Color = get_value("c", Some("FF000080")).unwrap();
        assert_eq!((c.r(), c.a()), (255, 0x80));
    }
}
