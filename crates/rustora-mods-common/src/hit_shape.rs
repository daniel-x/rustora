// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `HitShapes/*` and `Traits/HitShape.cs`.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use rustora_core::fieldloader::{LoadError, load};
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{Int2, WAngle, WDist, WPos, WRot, WVec, isqrt_i32};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_ext::ITargetablePositions;
use rustora_engine::bitset::BitSet;
use rustora_engine::conditional::*;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::render::BodyOrientation;
use crate::turreted::Turreted;

pub fn register(r: &mut TraitRegistry) {
    r.register("HitShape", |n| load_info!(HitShapeInfo, n));
}

/// Tag of `BitSet<ArmorType>`.
pub struct ArmorType;

#[derive(Clone, Debug, FieldLoad)]
pub struct CircleShape {
    #[field(require)]
    pub radius: WDist,
    pub vertical_top_offset: i32,
    pub vertical_bottom_offset: i32,
}

impl Default for CircleShape {
    fn default() -> Self {
        CircleShape {
            radius: WDist::new(426),
            vertical_top_offset: 0,
            vertical_bottom_offset: 0,
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct RectangleShape {
    #[field(require)]
    pub top_left: Int2,
    #[field(require)]
    pub bottom_right: Int2,
    pub vertical_top_offset: i32,
    pub vertical_bottom_offset: i32,
    pub local_yaw: WAngle,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct CapsuleShape {
    #[field(require)]
    pub point_a: Int2,
    #[field(require)]
    pub point_b: Int2,
    pub radius: WDist,
    pub vertical_top_offset: i32,
    pub vertical_bottom_offset: i32,
}

impl Default for CapsuleShape {
    fn default() -> Self {
        CapsuleShape {
            point_a: Int2::ZERO,
            point_b: Int2::ZERO,
            radius: WDist::new(426),
            vertical_top_offset: 0,
            vertical_bottom_offset: 0,
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct PolygonShape {
    #[field(require)]
    pub points: Vec<Int2>,
    pub vertical_top_offset: i32,
    pub vertical_bottom_offset: i32,
    pub local_yaw: WAngle,
}

#[derive(Clone, Debug)]
pub enum HitShapeType {
    Circle(CircleShape),
    Rectangle(RectangleShape),
    Capsule(CapsuleShape),
    Polygon(PolygonShape),
}

impl Default for HitShapeType {
    fn default() -> Self {
        HitShapeType::Circle(CircleShape::default())
    }
}

fn winding(v0: Int2, v1: Int2, p: Int2) -> i32 {
    ((v1.x - v0.x) * (p.y - v0.y) - (p.x - v0.x) * (v1.y - v0.y)).signum()
}

fn polygon_contains(polygon: &[Int2], p: Int2) -> bool {
    let mut n = 0;
    for i in 0..polygon.len() {
        let tv = polygon[i];
        let nv = polygon[(i + 1) % polygon.len()];
        if tv.y <= p.y && nv.y > p.y && winding(tv, nv, p) > 0 {
            n += 1;
        } else if tv.y > p.y && nv.y <= p.y && winding(tv, nv, p) < 0 {
            n -= 1;
        }
    }
    n != 0
}

fn dist2_from_segment(c: Int2, a: Int2, b: Int2, ab2: i32) -> i32 {
    let ac = c - a;
    let ac2 = ac.length_squared();
    let bc2 = (c - b).length_squared();
    if ac2 + ab2 <= bc2 {
        return ac2;
    }
    if bc2 + ab2 <= ac2 {
        return bc2;
    }
    let ab = b - a;
    let ap2 = ac.x * ab.x + ac.y * ab.y;
    let ap = Int2::new(
        ((ab.x as i64 * ap2 as i64) / ab2 as i64) as i32,
        ((ab.y as i64 * ap2 as i64) / ab2 as i64) as i32,
    );
    (ac - ap).length_squared()
}

impl HitShapeType {
    pub fn outer_radius(&self) -> WDist {
        match self {
            HitShapeType::Circle(s) => s.radius,
            HitShapeType::Rectangle(s) => {
                let corners = [
                    s.top_left,
                    s.bottom_right,
                    Int2::new(s.bottom_right.x, s.top_left.y),
                    Int2::new(s.top_left.x, s.bottom_right.y),
                ];
                WDist::new(corners.iter().map(|c| c.length()).max().unwrap())
            }
            HitShapeType::Capsule(s) => s.radius + WDist::new(s.point_a.length().max(s.point_b.length())),
            HitShapeType::Polygon(s) => WDist::new(s.points.iter().map(|p| p.length()).max().unwrap()),
        }
    }

    pub fn distance_from_edge_vec(&self, v: WVec) -> WDist {
        match self {
            HitShapeType::Circle(s) => WDist::new((v.length() - s.radius.length).max(0)),
            HitShapeType::Rectangle(s) => {
                let q = Int2::new((s.bottom_right.x - s.top_left.x) / 2, (s.bottom_right.y - s.top_left.y) / 2);
                let center = s.top_left + q;
                let r = WVec::new(((v.x - center.x).abs() - q.x).max(0), ((v.y - center.y).abs() - q.y).max(0), 0);
                WDist::new(r.horizontal_length())
            }
            HitShapeType::Capsule(s) => {
                let ab = s.point_b - s.point_a;
                let ab_len_sq = ab.length_squared() / 1024;
                let p = Int2::new(v.x, v.y);
                let d = p - s.point_a;
                let t = (d.x * ab.x + d.y * ab.y) / ab_len_sq;
                if t < 0 {
                    return WDist::new(((s.point_a - p).length() - s.radius.length).max(0));
                }
                if t > 1024 {
                    return WDist::new(((s.point_b - p).length() - s.radius.length).max(0));
                }
                let projection = s.point_a + Int2::new(ab.x * t / 1024, ab.y * t / 1024);
                WDist::new(((projection - p).length() - s.radius.length).max(0))
            }
            HitShapeType::Polygon(s) => {
                let p = Int2::new(v.x, v.y);
                let z = v.z.abs();
                if polygon_contains(&s.points, p) {
                    return WDist::new(z);
                }
                let pts = &s.points;
                let sq = |i: usize| -> i32 {
                    if i == 0 {
                        (pts[0] - pts[pts.len() - 1]).length_squared()
                    } else {
                        (pts[i] - pts[i - 1]).length_squared()
                    }
                };
                let mut min2 = dist2_from_segment(p, pts[pts.len() - 1], pts[0], sq(0));
                for i in 1..pts.len() {
                    let d2 = dist2_from_segment(p, pts[i - 1], pts[i], sq(i));
                    if d2 < min2 {
                        min2 = d2;
                    }
                }
                WDist::new(isqrt_i32(min2 + z * z))
            }
        }
    }

    pub fn distance_from_edge(&self, pos: WPos, origin: WPos, orientation: WRot) -> WDist {
        let (top, bottom, rot) = match self {
            HitShapeType::Circle(s) => (s.vertical_top_offset, s.vertical_bottom_offset, None),
            HitShapeType::Rectangle(s) => (
                s.vertical_top_offset,
                s.vertical_bottom_offset,
                Some(orientation + WRot::from_yaw(s.local_yaw)),
            ),
            HitShapeType::Capsule(s) => (s.vertical_top_offset, s.vertical_bottom_offset, Some(orientation)),
            HitShapeType::Polygon(s) => (
                s.vertical_top_offset,
                s.vertical_bottom_offset,
                Some(orientation + WRot::from_yaw(s.local_yaw)),
            ),
        };
        let d = if pos.z > origin.z + top {
            pos - (origin + WVec::new(0, 0, top))
        } else if pos.z < origin.z + bottom {
            pos - (origin + WVec::new(0, 0, bottom))
        } else {
            pos - WPos::new(origin.x, origin.y, pos.z)
        };
        match rot {
            Some(r) => self.distance_from_edge_vec(d.rotate(&(-r))),
            None => self.distance_from_edge_vec(d),
        }
    }
}

fn load_shape(yaml: &MiniYaml) -> Result<HitShapeType, LoadError> {
    let Some(node) = yaml.node_with_key_or_default("Type")? else {
        return Ok(HitShapeType::default());
    };
    Ok(match node.value.value_str() {
        "" => HitShapeType::default(),
        "Circle" => HitShapeType::Circle(load(&node.value)?),
        "Rectangle" => HitShapeType::Rectangle(load(&node.value)?),
        "Capsule" => HitShapeType::Capsule(load(&node.value)?),
        "Polygon" => HitShapeType::Polygon(load(&node.value)?),
        o => return Err(LoadError(format!("HitShape {o}: unknown shape"))),
    })
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HitShapeInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub turret: Option<String>,
    pub targetable_offsets: Vec<WVec>,
    pub use_targetable_cells_offsets: bool,
    pub armor_types: BitSet<ArmorType>,
    #[field(name = "Type", loader = "load_shape")]
    pub ty: HitShapeType,
}

impl Default for HitShapeInfo {
    fn default() -> Self {
        HitShapeInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            turret: None,
            targetable_offsets: vec![WVec::ZERO],
            use_targetable_cells_offsets: false,
            armor_types: BitSet::default(),
            ty: HitShapeType::default(),
        }
    }
}

pub struct HitShape {
    pub info: Rc<HitShapeInfo>,
    state: ConditionalState,
    orientation: Rc<BodyOrientation>,
    targetable_cells: OnceCell<Option<Rc<dyn crate::interfaces::ITargetableCells>>>,
    turret: OnceCell<Option<Rc<Turreted>>>,
    cache: RefCell<Option<(CacheKey, Vec<WPos>)>>,
}

type CacheKey = (
    Option<Vec<(rustora_core::world_types::CPos, u8)>>,
    Option<WPos>,
    Option<WRot>,
    Option<WRot>,
    Option<WVec>,
);

register_traits!(HitShape: ITargetablePositions, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(HitShapeInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    let orientation = init.self_.trait_::<BodyOrientation>();
    Some(Rc::new(HitShape {
        info,
        state,
        orientation,
        targetable_cells: OnceCell::new(),
        turret: OnceCell::new(),
        cache: RefCell::new(None),
    }))
});

impl ConditionalHooks for HitShape {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for HitShape {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl IObservesVariables for HitShape {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for HitShape {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl INotifyCreated for HitShape {
    fn created(&self, self_: &ActorRef) {
        let _ = self
            .targetable_cells
            .set(self_.trait_or_default::<dyn crate::interfaces::ITargetableCells>());
        let turret = self_
            .traits_implementing::<Turreted>()
            .into_iter()
            .find(|t| Some(t.name()) == self.info.turret.as_deref());
        let _ = self.turret.set(turret);
        conditional_created(self, self_);
    }
}

impl ITargetablePositions for HitShape {
    fn is_trait_enabled(&self) -> bool {
        !self.state.is_trait_disabled()
    }

    fn targetable_positions(&self, self_: &ActorRef) -> Vec<WPos> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let has_offsets = !self.info.targetable_offsets.is_empty();
        let tc = self.targetable_cells.get().and_then(|t| t.as_ref());
        let turret = self.turret.get().and_then(|t| t.as_ref());
        let key: CacheKey = (
            if self.info.use_targetable_cells_offsets {
                tc.map(|t| t.targetable_cells())
            } else {
                None
            },
            has_offsets.then(|| self_.center_position()),
            has_offsets.then(|| self_.orientation()),
            if has_offsets { turret.map(|t| t.local_orientation()) } else { None },
            if has_offsets { turret.map(|t| t.offset()) } else { None },
        );
        let mut cache = self.cache.borrow_mut();
        if let Some((k, v)) = cache.as_ref()
            && *k == key
        {
            return v.clone();
        }
        let mut out = Vec::new();
        if self.info.use_targetable_cells_offsets
            && let Some(t) = tc
        {
            let w = self_.world();
            for (c, _) in t.targetable_cells() {
                out.push(w.map().center_of_cell(c));
            }
        }
        for o in &self.info.targetable_offsets {
            out.push(self_.center_position() + self.calculate_targetable_offset(self_, *o));
        }
        *cache = Some((key, out.clone()));
        out
    }
}

impl HitShape {
    fn calculate_targetable_offset(&self, self_: &ActorRef, offset: WVec) -> WVec {
        let mut local = offset;
        let quantized = self.orientation.quantize_orientation(self_.orientation());
        if let Some(t) = self.turret.get().and_then(|t| t.as_ref()) {
            local = local.rotate(&t.local_orientation());
            local = local + t.offset();
        }
        self.orientation.local_to_world(local.rotate(&quantized))
    }

    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn distance_from_edge(&self, self_: &ActorRef, pos: WPos) -> WDist {
        let turret = self.turret.get().and_then(|t| t.as_ref());
        let origin = match turret {
            Some(t) => self_.center_position() + t.position(self_),
            None => self_.center_position(),
        };
        let orientation = match turret {
            Some(t) => t.world_orientation(),
            None => self_.orientation(),
        };
        self.info.ty.distance_from_edge(pos, origin, orientation)
    }
}

/// `LargestActorRadius` and `LargestBlockingActorRadius` of the actor map.
pub fn largest_outer_radius(w: &World, blocking_only: bool) -> WDist {
    let mut max = WDist::ZERO;
    for (_, a) in w.rules().actors() {
        if blocking_only && !a.has_trait_info_named("BlocksProjectiles") && !a.has_trait_info_named("Gate") {
            continue;
        }
        for h in a.trait_infos::<HitShapeInfo>() {
            let r = h.ty.outer_radius();
            if r.length > max.length {
                max = r;
            }
        }
    }
    max
}
