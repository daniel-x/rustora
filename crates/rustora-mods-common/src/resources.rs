// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/World/ResourceLayer.cs` and `Traits/SeedsResource.cs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use rustora_core::fieldloader::{LoadError, load};
use rustora_core::miniyaml::MiniYaml;
use rustora_core::world_types::{CPos, CVec, Int2};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::conditional::*;
use rustora_engine::map::CellLayer;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::building::BuildingInfluence;
use crate::util;

pub fn register(r: &mut TraitRegistry) {
    r.register("ResourceLayer", |n| load_info!(ResourceLayerInfo, n));
    r.register("SeedsResource", |n| load_info!(SeedsResourceInfo, n));
}

#[derive(Clone, Debug, Default)]
pub struct ResourceLayerContents {
    pub ty: Option<Rc<str>>,
    pub density: i32,
}

#[derive(Clone, Debug, FieldLoad)]
pub struct ResourceTypeInfo {
    #[field(require)]
    pub resource_index: u8,
    #[field(require)]
    pub terrain_type: String,
    #[field(require)]
    pub allowed_terrain_types: IndexSet<String>,
    pub max_density: i32,
}

impl Default for ResourceTypeInfo {
    fn default() -> Self {
        ResourceTypeInfo {
            resource_index: 0,
            terrain_type: String::new(),
            allowed_terrain_types: IndexSet::new(),
            max_density: 10,
        }
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct ResourceLayerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(loader = "load_resource_types")]
    pub resource_types: IndexMap<String, ResourceTypeInfo>,
    pub recalculate_resource_density: bool,
}

fn load_resource_types(yaml: &MiniYaml) -> Result<IndexMap<String, ResourceTypeInfo>, LoadError> {
    let mut ret = IndexMap::new();
    if let Some(r) = yaml.node_with_key_or_default("ResourceTypes")? {
        for n in &r.value.nodes {
            ret.insert(n.key.clone(), load::<ResourceTypeInfo>(&n.value)?);
        }
    }
    Ok(ret)
}

pub type ResourceCellChanged = Rc<dyn Fn(CPos, Option<&str>)>;

pub struct ResourceLayer {
    pub info: Rc<ResourceLayerInfo>,
    world: std::rc::Weak<World>,
    building_influence: Rc<BuildingInfluence>,
    content: RefCell<CellLayer<ResourceLayerContents>>,
    types_by_index: HashMap<u8, Rc<str>>,
    res_cells: Cell<i32>,
    cell_changed: RefCell<Vec<ResourceCellChanged>>,
}

register_traits!(ResourceLayer: IWorldLoaded);
info_impl!(ResourceLayerInfo => |info, init| {
    let w = init.world();
    let (mw, mh) = w.map().map_size;
    let types_by_index = info.resource_types.iter().map(|(k, v)| (v.resource_index, Rc::from(k.as_str()))).collect();
    Some(Rc::new(ResourceLayer {
        info,
        world: Rc::downgrade(&w),
        building_influence: init.self_.trait_::<BuildingInfluence>(),
        content: RefCell::new(CellLayer::new(mw, mh, ResourceLayerContents::default())),
        types_by_index,
        res_cells: Cell::new(0),
        cell_changed: RefCell::new(Vec::new()),
    }))
});

impl IWorldLoaded for ResourceLayer {
    fn world_loaded(&self, w: &Rc<World>) {
        let map = w.map();
        for cell in map.all_cells() {
            let resource = *map.resources.at(cell);
            let Some(ty) = self.types_by_index.get(&resource.ty) else { continue };
            if !self.allow_resource_at(ty, cell) {
                continue;
            }
            let c = self.create_resource_cell(ty, cell, resource.index as i32);
            self.content.borrow_mut().set_at(cell, c);
        }
        if !self.info.recalculate_resource_density {
            return;
        }
        for cell in map.all_cells() {
            let resource = self.content.borrow().at(cell).clone();
            let Some(ty) = resource.ty else { continue };
            let Some(ri) = self.info.resource_types.get(&*ty) else { continue };
            let mut adjacent = 0;
            {
                let content = self.content.borrow();
                for d in CVec::DIRECTIONS {
                    let c = cell + d;
                    if content.contains_cell(c) && content.at(c).ty.as_deref() == Some(&*ty) {
                        adjacent += 1;
                    }
                }
            }
            let density = Int2::lerp_i32(0, ri.max_density, adjacent, 9).max(1);
            self.content.borrow_mut().set_at(cell, ResourceLayerContents { ty: Some(ty), density });
        }
    }
}

impl ResourceLayer {
    fn world(&self) -> Rc<World> {
        self.world.upgrade().expect("world alive")
    }

    fn allow_resource_at(&self, ty: &str, cell: CPos) -> bool {
        let w = self.world();
        if !w.map().contains(cell) {
            return false;
        }
        let Some(ri) = self.info.resource_types.get(ty) else { return false };
        if !ri.allowed_terrain_types.contains(&w.map().terrain_type_info(cell).ty) {
            return false;
        }
        !self.building_influence.any_building_at(cell)
    }

    fn create_resource_cell(&self, ty: &Rc<str>, cell: CPos, density: i32) -> ResourceLayerContents {
        let w = self.world();
        let Some(ri) = self.info.resource_types.get(&**ty) else {
            w.map().set_custom_terrain(cell, u8::MAX);
            return ResourceLayerContents::default();
        };
        w.map().set_custom_terrain(cell, w.map().terrain.terrain_index(&ri.terrain_type));
        self.res_cells.set(self.res_cells.get() + 1);
        ResourceLayerContents {
            ty: Some(ty.clone()),
            density: density.clamp(1, ri.max_density),
        }
    }

    fn fire(&self, cell: CPos, ty: Option<&str>) {
        let l = self.cell_changed.borrow().clone();
        for f in l {
            f(cell, ty);
        }
    }

    pub fn on_cell_changed(&self, f: ResourceCellChanged) {
        self.cell_changed.borrow_mut().push(f);
    }

    pub fn get_resource(&self, cell: CPos) -> ResourceLayerContents {
        let c = self.content.borrow();
        if c.contains_cell(cell) { c.at(cell).clone() } else { ResourceLayerContents::default() }
    }

    pub fn get_max_density(&self, ty: &str) -> i32 {
        self.info.resource_types.get(ty).map_or(0, |r| r.max_density)
    }

    pub fn can_add_resource(&self, ty: &str, cell: CPos, amount: i32) -> bool {
        if !self.world().map().contains(cell) {
            return false;
        }
        let Some(ri) = self.info.resource_types.get(ty) else { return false };
        let content = self.content.borrow().at(cell).clone();
        match content.ty {
            None => amount <= ri.max_density && self.allow_resource_at(ty, cell),
            Some(t) if &*t != ty => false,
            Some(_) => content.density + amount <= ri.max_density,
        }
    }

    pub fn add_resource(&self, ty: &str, cell: CPos, amount: i32) -> i32 {
        if !self.content.borrow().contains_cell(cell) {
            return 0;
        }
        let Some(ri) = self.info.resource_types.get(ty) else { return 0 };
        let mut content = self.content.borrow().at(cell).clone();
        if content.ty.is_none() {
            content = self.create_resource_cell(&Rc::from(ty), cell, 0);
        }
        if content.ty.as_deref() != Some(ty) {
            return 0;
        }
        let old = content.density;
        let density = ri.max_density.min(old + amount);
        self.content.borrow_mut().set_at(cell, ResourceLayerContents { ty: content.ty.clone(), density });
        self.fire(cell, content.ty.as_deref());
        density - old
    }

    pub fn remove_resource(&self, ty: &str, cell: CPos, amount: i32) -> i32 {
        if !self.content.borrow().contains_cell(cell) {
            return 0;
        }
        let content = self.content.borrow().at(cell).clone();
        if content.ty.as_deref() != Some(ty) {
            return 0;
        }
        let old = content.density;
        let density = (old - amount).max(0);
        if density == 0 {
            self.content.borrow_mut().set_at(cell, ResourceLayerContents::default());
            self.world().map().set_custom_terrain(cell, u8::MAX);
            self.res_cells.set(self.res_cells.get() - 1);
            self.fire(cell, None);
        } else {
            self.content.borrow_mut().set_at(cell, ResourceLayerContents { ty: content.ty.clone(), density });
            self.fire(cell, content.ty.as_deref());
        }
        old - density
    }

    pub fn clear_resources(&self, cell: CPos) {
        if !self.content.borrow().contains_cell(cell) {
            return;
        }
        if self.content.borrow().at(cell).ty.is_none() {
            return;
        }
        self.content.borrow_mut().set_at(cell, ResourceLayerContents::default());
        self.world().map().set_custom_terrain(cell, u8::MAX);
        self.res_cells.set(self.res_cells.get() - 1);
        self.fire(cell, None);
    }

    pub fn is_empty(&self) -> bool {
        self.res_cells.get() < 1
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SeedsResourceInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub interval: i32,
    pub resource_type: String,
    pub max_range: i32,
}

impl Default for SeedsResourceInfo {
    fn default() -> Self {
        SeedsResourceInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            interval: 75,
            resource_type: "Ore".into(),
            max_range: 100,
        }
    }
}

pub struct SeedsResource {
    info: Rc<SeedsResourceInfo>,
    state: ConditionalState,
    resource_layer: Rc<ResourceLayer>,
    ticks: Cell<i32>,
}

register_traits!(SeedsResource: ITick, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SeedsResourceInfo => |info, init| {
    let state = ConditionalState::new(&info.base);
    let resource_layer = init.world().world_actor().trait_::<ResourceLayer>();
    Some(Rc::new(SeedsResource { info, state, resource_layer, ticks: Cell::new(0) }))
});

impl ConditionalHooks for SeedsResource {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }
}

impl ISync for SeedsResource {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash()
    }
}

impl IObservesVariables for SeedsResource {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver> {
        conditional_observers(&self)
    }
}

impl IDisabledTrait for SeedsResource {
    fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

impl INotifyCreated for SeedsResource {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl ITick for SeedsResource {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        self.ticks.set(self.ticks.get() - 1);
        if self.ticks.get() <= 0 {
            self.seed(self_);
            self.ticks.set(self.info.interval);
        }
    }
}

impl SeedsResource {
    pub fn seed(&self, self_: &ActorRef) {
        let w = self_.world();
        let ty = self.info.resource_type.as_str();
        let mut p = self_.location();
        let mut cell = None;
        for _ in 0..self.info.max_range {
            p = util::random_walk_step(p, &mut w.shared_random.borrow_mut());
            if !(self.resource_layer.get_resource(p).ty.as_deref() == Some(ty) && !self.resource_layer.can_add_resource(ty, p, 1)) {
                cell = Some(p);
                break;
            }
        }
        if let Some(c) = cell {
            if self.resource_layer.can_add_resource(ty, c, 1) {
                self.resource_layer.add_resource(ty, c, 1);
            }
        }
    }
}
