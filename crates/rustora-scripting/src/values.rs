// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The conversions between Lua values and engine values of `ScriptTypes.cs` (`TryGetClrValue` and `ToLuaValue`)
//! and the Lua bindings of the coordinate types, actors and players.

use std::rc::Rc;

use mlua::{AnyUserData, Function, IntoLua, Lua, MetaMethod, MultiValue, Table, UserData, UserDataMethods, Value};
use rustora_core::fieldloader::Color;
use rustora_core::world_types::{CPos, CVec, WAngle, WDist, WPos, WVec};
use rustora_engine::actor::ActorRef;
use rustora_engine::player::PlayerRef;

pub type LuaResult<T> = mlua::Result<T>;

pub fn error<T>(message: impl Into<String>) -> LuaResult<T> {
    Err(mlua::Error::RuntimeError(message.into()))
}

/// A Lua value converted to an engine type, as `TryGetClrValue`. `None` when the value has another type.
pub trait FromScript: Sized {
    /// The C# type name used in error messages.
    const NAME: &'static str;
    fn from_script(v: &Value) -> Option<Self>;
}

impl FromScript for Value {
    const NAME: &'static str = "LuaValue";
    fn from_script(v: &Value) -> Option<Self> {
        Some(v.clone())
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

impl FromScript for f64 {
    const NAME: &'static str = "Double";
    fn from_script(v: &Value) -> Option<Self> {
        number(v)
    }
}

/// `(int)double`, the C# truncating conversion.
pub fn truncate(n: f64) -> i32 {
    n as i32
}

impl FromScript for i32 {
    const NAME: &'static str = "Int32";
    fn from_script(v: &Value) -> Option<Self> {
        number(v).map(truncate)
    }
}

impl FromScript for bool {
    const NAME: &'static str = "Boolean";
    fn from_script(v: &Value) -> Option<Self> {
        match v {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }
    }
}

impl FromScript for String {
    const NAME: &'static str = "String";
    fn from_script(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Some(s.to_string_lossy()),
            _ => None,
        }
    }
}

impl FromScript for Function {
    const NAME: &'static str = "LuaFunction";
    fn from_script(v: &Value) -> Option<Self> {
        match v {
            Value::Function(f) => Some(f.clone()),
            _ => None,
        }
    }
}

impl FromScript for Table {
    const NAME: &'static str = "LuaTable";
    fn from_script(v: &Value) -> Option<Self> {
        match v {
            Value::Table(t) => Some(t.clone()),
            _ => None,
        }
    }
}

/// A reference type: `nil` converts to `None`.
impl<T: FromScript> FromScript for Option<T> {
    const NAME: &'static str = T::NAME;
    fn from_script(v: &Value) -> Option<Self> {
        match v {
            Value::Nil => Some(None),
            v => T::from_script(v).map(Some),
        }
    }
}

/// An array, from the values of a table in `lua_next` order. A table value that does not convert makes the call
/// fail; this is reported as a conversion failure of the array.
impl<T: FromScript> FromScript for Vec<T> {
    const NAME: &'static str = "Array";
    fn from_script(v: &Value) -> Option<Self> {
        let Value::Table(t) = v else { return None };
        let mut out = Vec::new();
        for pair in t.clone().pairs::<Value, Value>() {
            let (_, value) = pair.ok()?;
            out.push(T::from_script(&value)?);
        }
        Some(out)
    }
}

macro_rules! userdata_value {
    ($wrapper:ident, $inner:ty, $name:literal) => {
        impl FromScript for $inner {
            const NAME: &'static str = $name;
            fn from_script(v: &Value) -> Option<Self> {
                match v {
                    Value::UserData(ud) => ud.borrow::<$wrapper>().ok().map(|w| w.0.clone()),
                    _ => None,
                }
            }
        }
    };
}

#[derive(Clone)]
pub struct LuaActor(pub ActorRef);
#[derive(Clone)]
pub struct LuaPlayer(pub PlayerRef);
#[derive(Clone, Copy)]
pub struct LuaCPos(pub CPos);
#[derive(Clone, Copy)]
pub struct LuaCVec(pub CVec);
#[derive(Clone, Copy)]
pub struct LuaWPos(pub WPos);
#[derive(Clone, Copy)]
pub struct LuaWVec(pub WVec);
#[derive(Clone, Copy)]
pub struct LuaWDist(pub WDist);
#[derive(Clone, Copy)]
pub struct LuaWAngle(pub WAngle);
#[derive(Clone, Copy)]
pub struct LuaColor(pub Color);

userdata_value!(LuaActor, ActorRef, "Actor");
userdata_value!(LuaPlayer, PlayerRef, "Player");
userdata_value!(LuaCPos, CPos, "CPos");
userdata_value!(LuaCVec, CVec, "CVec");
userdata_value!(LuaWPos, WPos, "WPos");
userdata_value!(LuaWVec, WVec, "WVec");
userdata_value!(LuaWDist, WDist, "WDist");
userdata_value!(LuaWAngle, WAngle, "WAngle");
userdata_value!(LuaColor, Color, "Color");

/// `WrappedClrType().Name` for error messages.
pub fn type_name(v: &Value) -> String {
    if let Value::UserData(ud) = v {
        for (ok, name) in [
            (ud.is::<LuaActor>(), "Actor"),
            (ud.is::<LuaPlayer>(), "Player"),
            (ud.is::<LuaCPos>(), "CPos"),
            (ud.is::<LuaCVec>(), "CVec"),
            (ud.is::<LuaWPos>(), "WPos"),
            (ud.is::<LuaWVec>(), "WVec"),
            (ud.is::<LuaWDist>(), "WDist"),
            (ud.is::<LuaWAngle>(), "WAngle"),
            (ud.is::<LuaColor>(), "Color"),
        ] {
            if ok {
                return name.to_string();
            }
        }
    }
    match v {
        Value::Nil => "LuaNil",
        Value::Boolean(_) => "LuaBoolean",
        Value::Number(_) | Value::Integer(_) => "LuaNumber",
        Value::String(_) => "LuaString",
        Value::Table(_) => "LuaTable",
        Value::Function(_) => "LuaFunction",
        _ => "LuaUserdata",
    }
    .to_string()
}

/// A value returned to Lua, as `ToLuaValue`.
pub fn actor_value(lua: &Lua, a: Option<&ActorRef>) -> LuaResult<Value> {
    match a {
        Some(a) => LuaActor(a.clone()).into_lua(lua),
        None => Ok(Value::Nil),
    }
}

pub fn player_value(lua: &Lua, p: Option<&PlayerRef>) -> LuaResult<Value> {
    match p {
        Some(p) => LuaPlayer(p.clone()).into_lua(lua),
        None => Ok(Value::Nil),
    }
}

/// An array as a table with the keys 1 to n.
pub fn actors_table(lua: &Lua, actors: &[ActorRef]) -> LuaResult<Value> {
    Ok(Value::Table(lua.create_sequence_from(actors.iter().map(|a| LuaActor(a.clone())))?))
}

pub fn players_table(lua: &Lua, players: &[PlayerRef]) -> LuaResult<Value> {
    Ok(Value::Table(
        lua.create_sequence_from(players.iter().map(|p| LuaPlayer(p.clone())))?,
    ))
}

/// The arguments of a call from Lua with the conversions of `ScriptMemberWrapper.Invoke`.
pub struct Args {
    values: Vec<Value>,
    member: &'static str,
}

impl Args {
    pub fn new(values: MultiValue, member: &'static str) -> Args {
        Args {
            values: values.into_vec(),
            member,
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn raw(&self, i: usize) -> Option<&Value> {
        self.values.get(i)
    }

    fn convert<T: FromScript>(&self, i: usize, v: &Value) -> LuaResult<T> {
        T::from_script(v).map_or_else(|| error(format!("Unable to convert parameter {i} to {}", T::NAME)), Ok)
    }

    /// A required parameter.
    pub fn get<T: FromScript>(&self, i: usize) -> LuaResult<T> {
        match self.values.get(i) {
            Some(v) => self.convert(i, v),
            None => error(format!("Argument {i} of '{}' is not optional.", self.member)),
        }
    }

    /// An optional parameter with its default value.
    pub fn opt<T: FromScript>(&self, i: usize, default: T) -> LuaResult<T> {
        match self.values.get(i) {
            Some(v) => self.convert(i, v),
            None => Ok(default),
        }
    }
}

fn read_only<T>(name: &str) -> LuaResult<T> {
    error(format!("{name} is read-only. Use {name}.New to create a new value"))
}

fn no_member<T>(name: &str, key: &Value) -> LuaResult<T> {
    error(format!("{name} does not define a member '{}'", key_string(key)))
}

/// `LuaValue.ToString()` of an index key.
pub fn key_string(key: &Value) -> String {
    match key {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Nil => "nil".to_string(),
        v => type_name(v),
    }
}

fn int_arg(v: &Value) -> Option<i32> {
    i32::from_script(v)
}

fn invalid<T>(op: &str, a: &Value, b: &Value) -> LuaResult<T> {
    error(format!(
        "Attempted to call {op} with invalid arguments ({}, {})",
        type_name(a),
        type_name(b)
    ))
}

fn divide(a: i32, b: i32) -> LuaResult<i32> {
    if b == 0 {
        return error("Attempted to divide by zero.");
    }
    Ok(a.wrapping_div(b))
}

impl UserData for LuaCPos {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| match key_string(&key).as_str() {
            "X" => this.0.x().into_lua(lua),
            "Y" => this.0.y().into_lua(lua),
            "Layer" => (this.0.layer() as i32).into_lua(lua),
            _ => no_member("CPos", &key),
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, _, _: MultiValue| read_only::<()>("CPos"));
        m.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            match (CPos::from_script(&a), CVec::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaCPos(x + y)),
                _ => invalid("CPos.Add(CPos, CVec)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Sub, |lua, (a, b): (Value, Value)| {
            let Some(x) = CPos::from_script(&a) else {
                return invalid("CPos.Subtract(CPos, (CPos|CVec))", &a, &b);
            };
            if let Some(y) = CPos::from_script(&b) {
                return LuaCVec(x - y).into_lua(lua);
            }
            if let Some(y) = CVec::from_script(&b) {
                return LuaCPos(x - y).into_lua(lua);
            }
            invalid("CPos.Subtract(CPos, (CPos|CVec))", &a, &b)
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(matches!((CPos::from_script(&a), CPos::from_script(&b)), (Some(x), Some(y)) if x == y))
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }
}

impl UserData for LuaCVec {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| match key_string(&key).as_str() {
            "X" => this.0.x.into_lua(lua),
            "Y" => this.0.y.into_lua(lua),
            "Length" => this.0.length().into_lua(lua),
            _ => no_member("CVec", &key),
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, _, _: MultiValue| read_only::<()>("CVec"));
        m.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            match (CVec::from_script(&a), CVec::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaCVec(x + y)),
                _ => invalid("CVec.Add(CVec, CVec)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            match (CVec::from_script(&a), CVec::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaCVec(x - y)),
                _ => invalid("CVec.Subtract(CVec, CVec)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(matches!((CVec::from_script(&a), CVec::from_script(&b)), (Some(x), Some(y)) if x == y))
        });
        m.add_meta_method(MetaMethod::Unm, |_, this, _: MultiValue| Ok(LuaCVec(-this.0)));
        m.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| {
            match (CVec::from_script(&a), int_arg(&b)) {
                (Some(x), Some(y)) => Ok(LuaCVec(CVec::new(x.x.wrapping_mul(y), x.y.wrapping_mul(y)))),
                _ => invalid("CVec.Multiply(CVec, integer)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| {
            match (CVec::from_script(&a), int_arg(&b)) {
                (Some(x), Some(y)) => Ok(LuaCVec(CVec::new(divide(x.x, y)?, divide(x.y, y)?))),
                _ => invalid("CVec.Multiply(CVec, integer)", &a, &b),
            }
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }
}

impl UserData for LuaWPos {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| match key_string(&key).as_str() {
            "X" => this.0.x.into_lua(lua),
            "Y" => this.0.y.into_lua(lua),
            "Z" => this.0.z.into_lua(lua),
            _ => no_member("WPos", &key),
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, _, _: MultiValue| read_only::<()>("WPos"));
        m.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            match (WPos::from_script(&a), WVec::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaWPos(x + y)),
                _ => invalid("WPos.Add(WPos, WVec)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Sub, |lua, (a, b): (Value, Value)| {
            let Some(x) = WPos::from_script(&a) else {
                return invalid("WPos.Subtract(WPos, (WPos|WVec))", &a, &b);
            };
            if let Some(y) = WPos::from_script(&b) {
                return LuaWVec(x - y).into_lua(lua);
            }
            if let Some(y) = WVec::from_script(&b) {
                return LuaWPos(x - y).into_lua(lua);
            }
            invalid("WPos.Subtract(WPos, (WPos|WVec))", &a, &b)
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(matches!((WPos::from_script(&a), WPos::from_script(&b)), (Some(x), Some(y)) if x == y))
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }
}

impl UserData for LuaWVec {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| match key_string(&key).as_str() {
            "X" => this.0.x.into_lua(lua),
            "Y" => this.0.y.into_lua(lua),
            "Z" => this.0.z.into_lua(lua),
            "Facing" => LuaWAngle(this.0.yaw()).into_lua(lua),
            _ => no_member("WVec", &key),
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, _, _: MultiValue| read_only::<()>("WVec"));
        m.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            match (WVec::from_script(&a), WVec::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaWVec(x + y)),
                _ => invalid("WVec.Add(WVec, WVec)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            match (WVec::from_script(&a), WVec::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaWVec(x - y)),
                _ => invalid("WVec.Subtract(WVec, WVec)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(matches!((WVec::from_script(&a), WVec::from_script(&b)), (Some(x), Some(y)) if x == y))
        });
        m.add_meta_method(MetaMethod::Unm, |_, this, _: MultiValue| {
            let v = this.0;
            Ok(LuaWVec(WVec::new(v.x.wrapping_neg(), v.y.wrapping_neg(), v.z.wrapping_neg())))
        });
        m.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| {
            match (WVec::from_script(&a), int_arg(&b)) {
                (Some(x), Some(y)) => Ok(LuaWVec(WVec::new(x.x.wrapping_mul(y), x.y.wrapping_mul(y), x.z.wrapping_mul(y)))),
                _ => invalid("WVec.Multiply(WVec, integer)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| {
            match (WVec::from_script(&a), int_arg(&b)) {
                (Some(x), Some(y)) => Ok(LuaWVec(WVec::new(divide(x.x, y)?, divide(x.y, y)?, divide(x.z, y)?))),
                _ => invalid("WVec.Divide(WVec, integer)", &a, &b),
            }
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }
}

impl UserData for LuaWDist {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| match key_string(&key).as_str() {
            "Length" => this.0.length.into_lua(lua),
            _ => no_member("WDist", &key),
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, _, _: MultiValue| read_only::<()>("WDist"));
        fn pair(a: &Value, b: &Value, op: &str) -> LuaResult<(WDist, WDist)> {
            match (WDist::from_script(a), WDist::from_script(b)) {
                (Some(x), Some(y)) => Ok((x, y)),
                _ => error(format!("Attempted to call WDist.{op}(WDist, WDist) with invalid arguments.")),
            }
        }
        fn scalar(a: &Value, b: &Value, op: &str) -> LuaResult<(WDist, i32)> {
            match (WDist::from_script(a), int_arg(b)) {
                (Some(x), Some(y)) => Ok((x, y)),
                _ => error(format!("Attempted to call WDist.{op}(WDist, integer) with invalid arguments.")),
            }
        }
        m.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            let (x, y) = pair(&a, &b, "Add")?;
            Ok(LuaWDist(WDist::new(x.length.wrapping_add(y.length))))
        });
        m.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            let (x, y) = pair(&a, &b, "Subtract")?;
            Ok(LuaWDist(WDist::new(x.length.wrapping_sub(y.length))))
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            let (x, y) = pair(&a, &b, "Equals")?;
            Ok(x == y)
        });
        m.add_meta_method(MetaMethod::Unm, |_, this, _: MultiValue| {
            Ok(LuaWDist(WDist::new(this.0.length.wrapping_neg())))
        });
        m.add_meta_function(MetaMethod::Mul, |_, (a, b): (Value, Value)| {
            let (x, y) = scalar(&a, &b, "Multiply")?;
            Ok(LuaWDist(WDist::new(x.length.wrapping_mul(y))))
        });
        m.add_meta_function(MetaMethod::Div, |_, (a, b): (Value, Value)| {
            let (x, y) = scalar(&a, &b, "Divide")?;
            Ok(LuaWDist(WDist::new(divide(x.length, y)?)))
        });
        m.add_meta_function(MetaMethod::Lt, |_, (a, b): (Value, Value)| {
            let (x, y) = pair(&a, &b, "LessThan")?;
            Ok(x < y)
        });
        m.add_meta_function(MetaMethod::Le, |_, (a, b): (Value, Value)| {
            let (x, y) = pair(&a, &b, "LessThanOrEqualTo")?;
            Ok(x <= y)
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }
}

impl UserData for LuaWAngle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| match key_string(&key).as_str() {
            "Angle" => this.0.angle.into_lua(lua),
            _ => no_member("WAngle", &key),
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, _, _: MultiValue| {
            error::<()>("WAngle is read-only. Use Angle.New to create a new value")
        });
        m.add_meta_function(MetaMethod::Add, |_, (a, b): (Value, Value)| {
            match (WAngle::from_script(&a), WAngle::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaWAngle(WAngle::new(x.angle + y.angle))),
                _ => invalid("WAngle.Add(WAngle, WAngle)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Sub, |_, (a, b): (Value, Value)| {
            match (WAngle::from_script(&a), WAngle::from_script(&b)) {
                (Some(x), Some(y)) => Ok(LuaWAngle(WAngle::new(x.angle - y.angle))),
                _ => invalid("WAngle.Subtract(WAngle, WAngle)", &a, &b),
            }
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(matches!((WAngle::from_script(&a), WAngle::from_script(&b)), (Some(x), Some(y)) if x == y))
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.angle.to_string()));
    }
}

impl UserData for LuaColor {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(color_string(this.0)));
    }
}

/// `Color.ToString`: upper case hex `RRGGBB`, with `AA` appended unless opaque.
pub fn color_string(c: Color) -> String {
    let mut s = format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
    if c.a() != 255 {
        s += &format!("{:02X}", c.a());
    }
    s
}

/// `ILuaEqualityBinding` of actors and players: equal when both wrap the same object.
fn same<T: 'static, U: PartialEq>(a: &AnyUserData, b: &AnyUserData, key: impl Fn(&T) -> U) -> bool {
    match (a.borrow::<T>(), b.borrow::<T>()) {
        (Ok(x), Ok(y)) => key(&x) == key(&y),
        _ => false,
    }
}

impl UserData for LuaActor {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            crate::members::actor_index(lua, &this.0, &key_string(&key))
        });
        m.add_meta_method(MetaMethod::NewIndex, |lua, this, (key, value): (Value, Value)| {
            crate::members::actor_new_index(lua, &this.0, &key_string(&key), value)
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(same::<LuaActor, u32>(&a, &b, |x| x.0.id))
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(format!("Actor ({})", actor_string(&this.0))));
    }
}

/// `Actor.ToString`.
pub fn actor_string(a: &ActorRef) -> String {
    let mut name = format!("{} {}", a.info.name, a.id);
    if !a.is_in_world() {
        name += " (not in world)";
    }
    name
}

impl UserData for LuaPlayer {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            crate::members::player_index(lua, &this.0, &key_string(&key))
        });
        m.add_meta_method(MetaMethod::NewIndex, |lua, this, (key, value): (Value, Value)| {
            crate::members::player_new_index(lua, &this.0, &key_string(&key), value)
        });
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(same::<LuaPlayer, *const ()>(&a, &b, |x| Rc::as_ptr(&x.0) as *const ()))
        });
        m.add_meta_method(MetaMethod::ToString, |lua, this, ()| {
            Ok(format!("Player ({})", crate::host::resolved_player_name(lua, &this.0)))
        });
    }
}
