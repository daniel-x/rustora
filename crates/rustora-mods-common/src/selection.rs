// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! `Traits/Interactable.cs`, `Traits/Selectable.cs`, `Traits/World/Selection.cs` and `SelectableExts.cs`.
//! Selection is client state: it never touches synced state, and the client calls [`Selection::tick`] after each
//! world tick with its render player instead of the trait ticking with the world.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_core::field_flags;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use rustora_core::world_types::WDist;
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_info::ActorInfo;
use rustora_engine::player::{PlayerRef, PlayerRelationship};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::IGameSaveTraitData;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("Interactable", |n| load_info!(InteractableInfo, n));
    r.register("Selectable", |n| load_info!(SelectableInfo, n));
    r.register("Selection", |n| load_info!(SelectionInfo, n));
}

/// `SelectionPriorityModifiers`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionPriorityModifiers(pub i32);

impl SelectionPriorityModifiers {
    pub const NONE: SelectionPriorityModifiers = SelectionPriorityModifiers(0);
    pub const CTRL: SelectionPriorityModifiers = SelectionPriorityModifiers(1);
    pub const ALT: SelectionPriorityModifiers = SelectionPriorityModifiers(2);
    #[allow(non_upper_case_globals)]
    const None: SelectionPriorityModifiers = Self::NONE;
    #[allow(non_upper_case_globals)]
    const Ctrl: SelectionPriorityModifiers = Self::CTRL;
    #[allow(non_upper_case_globals)]
    const Alt: SelectionPriorityModifiers = Self::ALT;
}

field_flags!(SelectionPriorityModifiers, [None, Ctrl, Alt]);

/// The mouse bounds fields shared by `InteractableInfo` and `SelectableInfo`.
#[derive(Clone, Debug, Default, FieldLoad)]
pub struct InteractableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub bounds: Option<Vec<WDist>>,
    pub decoration_bounds: Option<Vec<WDist>>,
    pub polygon: Option<Vec<rustora_core::world_types::Int2>>,
}

register_traits!(Interactable);
info_impl!(InteractableInfo => |_info, _init| Some(Rc::new(Interactable)));

pub struct Interactable;

#[derive(Clone, Debug, FieldLoad)]
pub struct SelectableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub interactable: InteractableInfo,
    pub priority: i32,
    pub priority_modifiers: SelectionPriorityModifiers,
    pub class: Option<String>,
    pub voice: String,
}

impl Default for SelectableInfo {
    fn default() -> Self {
        SelectableInfo {
            instance_name: None,
            interactable: InteractableInfo::default(),
            priority: 10,
            priority_modifiers: SelectionPriorityModifiers::NONE,
            class: None,
            voice: "Select".into(),
        }
    }
}

register_traits!(Selectable);
info_impl!(SelectableInfo => |_info, _init| Some(Rc::new(Selectable)));

pub struct Selectable;

/// `Actor.MouseBounds` trait info: the `Interactable` part of the first `Interactable` or `Selectable`.
pub fn mouse_bounds_info(info: &ActorInfo) -> Option<InteractableInfo> {
    info.trait_info::<SelectableInfo>()
        .map(|s| s.interactable.clone())
        .or_else(|| info.trait_info::<InteractableInfo>().map(|i| (*i).clone()))
}

/// `ISelectable.Class`.
pub fn selection_class(a: &ActorRef) -> Option<String> {
    let s = a.info.trait_info::<SelectableInfo>()?;
    Some(s.class.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| a.info.name.clone()))
}

/// Modifier keys as `SelectableExts` reads them.
#[derive(Clone, Copy, Debug, Default)]
pub struct SelectionModifiers {
    pub ctrl: bool,
    pub alt: bool,
}

const PRIORITY_RANGE: i32 = 30;

fn base_selection_priority(info: &SelectableInfo, m: SelectionModifiers) -> i32 {
    let has = |f: SelectionPriorityModifiers| info.priority_modifiers.0 & f.0 != 0;
    if (m.ctrl && !m.alt && has(SelectionPriorityModifiers::CTRL)) || (m.alt && !m.ctrl && has(SelectionPriorityModifiers::ALT)) {
        return i32::MAX;
    }
    info.priority
}

/// `ActorInfo.SelectionPriority`.
pub fn info_selection_priority(info: &ActorInfo, m: SelectionModifiers) -> i32 {
    info.trait_info::<SelectableInfo>()
        .map_or(i32::MIN, |s| base_selection_priority(&s, m))
}

/// `Actor.SelectionPriority`, where `viewer` is the local player, or the render player when spectating.
pub fn selection_priority(a: &ActorRef, viewer: Option<&PlayerRef>, m: SelectionModifiers) -> i32 {
    let Some(info) = a.info.trait_info::<SelectableInfo>() else {
        return i32::MIN;
    };
    let base = base_selection_priority(&info, m);
    let owner = a.owner();
    let Some(viewer) = viewer.filter(|v| !Rc::ptr_eq(v, &owner)) else {
        return base;
    };
    let r = viewer.relationship_with(Some(&owner));
    if r == PlayerRelationship::ALLY {
        base - PRIORITY_RANGE
    } else if r == PlayerRelationship::NEUTRAL {
        base - 2 * PRIORITY_RANGE
    } else {
        base - 3 * PRIORITY_RANGE
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct SelectionInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
}

/// The world's `Selection`.
#[derive(Default)]
pub struct Selection {
    actors: RefCell<Vec<ActorRef>>,
    rollover: RefCell<Vec<ActorRef>>,
    hash: Cell<i32>,
}

register_traits!(Selection: IGameSaveTraitData);
info_impl!(SelectionInfo => |_info, _init| Some(Rc::new(Selection::default())));

fn contains(list: &[ActorRef], a: &ActorRef) -> bool {
    list.iter().any(|b| Rc::ptr_eq(a, b))
}

impl Selection {
    /// The world's selection.
    pub fn of(world: &World) -> Option<Rc<Selection>> {
        world.world_actor().trait_or_default::<Selection>()
    }

    /// Changes whenever the selection changes.
    pub fn hash(&self) -> i32 {
        self.hash.get()
    }

    fn update_hash(&self) {
        self.hash.set(self.hash.get().wrapping_add(1));
    }

    pub fn actors(&self) -> Vec<ActorRef> {
        self.actors.borrow().clone()
    }

    pub fn contains(&self, a: &ActorRef) -> bool {
        contains(&self.actors.borrow(), a)
    }

    pub fn add(&self, a: &ActorRef) {
        if !self.contains(a) {
            self.actors.borrow_mut().push(a.clone());
        }
        self.update_hash();
    }

    pub fn remove(&self, a: &ActorRef) {
        let mut actors = self.actors.borrow_mut();
        if let Some(i) = actors.iter().position(|b| Rc::ptr_eq(a, b)) {
            actors.remove(i);
            drop(actors);
            self.update_hash();
        }
    }

    /// `Combine` without the selection voice.
    pub fn combine(&self, new_selection: &[ActorRef], is_combine: bool, is_click: bool) {
        let mut actors = self.actors.borrow_mut();
        let take = if is_click {
            &new_selection[..new_selection.len().min(1)]
        } else {
            new_selection
        };
        if is_click && is_combine {
            for a in take {
                if let Some(i) = actors.iter().position(|b| Rc::ptr_eq(a, b)) {
                    actors.remove(i);
                } else {
                    actors.push(a.clone());
                }
            }
        } else {
            if !is_combine {
                actors.clear();
            }
            for a in take {
                if !contains(&actors, a) {
                    actors.push(a.clone());
                }
            }
        }
        drop(actors);
        self.update_hash();
    }

    pub fn clear(&self) {
        self.actors.borrow_mut().clear();
        self.update_hash();
    }

    pub fn set_rollover(&self, rollover: Vec<ActorRef>) {
        *self.rollover.borrow_mut() = rollover;
    }

    pub fn rollover_contains(&self, a: &ActorRef) -> bool {
        contains(&self.rollover.borrow(), a)
    }

    /// `ITick.Tick`: drops actors that left the world or that the render player can no longer see.
    pub fn tick(&self, render_player: Option<&PlayerRef>) {
        let mut actors = self.actors.borrow_mut();
        let before = actors.len();
        actors.retain(|a| {
            a.is_in_world()
                && (render_player.is_none()
                    || a.owner().is_allied_with(render_player.map(|p| &**p))
                    || a.can_be_viewed_by_player(render_player))
        });
        let removed = actors.len() != before;
        drop(actors);
        if removed {
            self.update_hash();
        }
    }
}

impl IGameSaveTraitData for Selection {
    fn issue_trait_data(&self, _actor: &ActorRef) -> Option<Vec<MiniYamlNode>> {
        let ids: Vec<String> = self.actors.borrow().iter().map(|a| a.id.to_string()).collect();
        Some(vec![MiniYamlNode::with_value_str("Selection", ids.join(", "))])
    }

    fn resolve_trait_data(&self, self_: &ActorRef, data: &MiniYaml) {
        let Some(node) = data.nodes.iter().find(|n| n.key == "Selection") else {
            return;
        };
        let world = self_.world();
        let selected: Vec<ActorRef> = node
            .value
            .value
            .as_deref()
            .unwrap_or("")
            .split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .filter_map(|id| world.get_actor_by_id(id))
            .collect();
        self.combine(&selected, false, false);
    }
}
