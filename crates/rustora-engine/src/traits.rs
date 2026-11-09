// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Core trait plumbing and the engine level trait interfaces of `Traits/TraitsInterfaces.cs`.

use rustora_core::miniyaml::{MiniYaml, MiniYamlNode};
use std::any::Any;
use std::rc::Rc;

use rustora_core::world_types::{CPos, WAngle, WPos, WRot};

use crate::actor::{ActorRef, ConditionCache};
use crate::actor_init::ActorInitializer;
use crate::order::Order;
use crate::player::PlayerRef;
use crate::trait_dict::TraitDict;
use crate::world::World;

/// A trait instance. Implemented with [`register_traits!`], which lists the interfaces.
pub trait TraitObject: Any {
    /// Adds this trait to the dictionary under its concrete type and all its interfaces.
    fn register(self: Rc<Self>, dict: &TraitDict, actor: &ActorRef);
    fn type_name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    /// `Box<Rc<T>>` of this trait as `T`, its concrete type or one of its registered interfaces.
    fn cast_rc(self: Rc<Self>, t: std::any::TypeId) -> Option<Box<dyn Any>>;
    /// Writes the state of the trait to a save game.
    fn save_state(&self, w: &mut crate::persist::SaveWriter);
    /// Reads the state written by [`TraitObject::save_state`].
    fn load_state(&self, r: &mut crate::persist::SaveReader);
}

/// Implements [`TraitObject`] for a trait struct: `register_traits!(Health: IHealth, ISync, ITick);`
/// Interfaces after `; cast:` are not added to the trait dictionary, but save games can refer to the trait as
/// them.
#[macro_export]
macro_rules! register_traits {
    ($ty:ty $(: $($iface:path),* $(,)?)? $(; cast: $($cast:path),* $(,)?)?) => {
        impl $crate::traits::TraitObject for $ty {
            fn register(self: ::std::rc::Rc<Self>, dict: &$crate::trait_dict::TraitDict, actor: &$crate::actor::ActorRef) {
                dict.add::<$ty>(actor, self.clone());
                $($(dict.add::<dyn $iface>(actor, self.clone());)*)?
            }
            fn type_name(&self) -> &'static str {
                stringify!($ty)
            }
            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }
            $crate::trait_object_persist!(@methods $ty: $($($iface),*)? $(, $($cast),*)?);
        }
        $crate::persist_trait_refs!($ty);
    };
}

/// The save game methods of [`TraitObject`] for a hand written implementation:
/// `trait_object_persist!(Turreted: ITick, ISync);` inside the `impl TraitObject` block, with all interfaces the
/// trait may register.
#[macro_export]
macro_rules! trait_object_persist {
    (@methods $ty:ty: $($iface:path),* $(,)?) => {
        fn cast_rc(self: ::std::rc::Rc<Self>, t: ::std::any::TypeId) -> Option<Box<dyn ::std::any::Any>> {
            if t == ::std::any::TypeId::of::<$ty>() {
                let rc: ::std::rc::Rc<$ty> = self;
                return Some(Box::new(rc));
            }
            $(if t == ::std::any::TypeId::of::<dyn $iface>() {
                let rc: ::std::rc::Rc<dyn $iface> = self;
                return Some(Box::new(rc));
            })*
            None
        }
        fn save_state(&self, w: &mut $crate::persist::SaveWriter) {
            $crate::persist::PersistState::save_state(self, w)
        }
        fn load_state(&self, r: &mut $crate::persist::SaveReader) {
            $crate::persist::PersistState::load_state(self, r)
        }
    };
    ($ty:ty $(: $($iface:path),* $(,)?)?) => {
        $crate::trait_object_persist!(@methods $ty: $($($iface),*)?);
    };
}

/// Static trait definition loaded from rules (`TraitInfo`).
pub trait TraitInfo: Any {
    /// Creates the trait instance, or `None` for infos that create no trait.
    fn create(self: Rc<Self>, init: &ActorInitializer) -> Option<Rc<dyn TraitObject>>;
    /// Adds this info to the actor info's type index under all its interfaces.
    fn register_info(self: Rc<Self>, index: &crate::actor_info::InfoIndex);
    fn instance_name(&self) -> Option<&str>;
    fn info_type_name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn as_any_rc(self: Rc<Self>) -> Rc<dyn Any>;
}

/// Implements [`TraitInfo`] for an info struct with an `instance_name: Option<String>` field.
/// `info_impl!(HealthInfo => Health::new, [IHealthInfo]);` or with `=> none` for no trait.
#[macro_export]
macro_rules! info_impl {
    ($ty:ty => $create:expr $(, [$($iface:path),* $(,)?])?) => {
        impl $crate::traits::TraitInfo for $ty {
            fn create(self: ::std::rc::Rc<Self>, init: &$crate::actor_init::ActorInitializer) -> Option<::std::rc::Rc<dyn $crate::traits::TraitObject>> {
                let f: fn(::std::rc::Rc<$ty>, &$crate::actor_init::ActorInitializer) -> Option<::std::rc::Rc<dyn $crate::traits::TraitObject>> = $create;
                f(self, init)
            }
            fn register_info(self: ::std::rc::Rc<Self>, index: &$crate::actor_info::InfoIndex) {
                index.add::<$ty>(self.clone());
                $($(index.add::<dyn $iface>(self.clone());)*)?
            }
            fn instance_name(&self) -> Option<&str> {
                self.instance_name.as_deref()
            }
            fn info_type_name(&self) -> &'static str {
                stringify!($ty)
            }
            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }
            fn as_any_rc(self: ::std::rc::Rc<Self>) -> ::std::rc::Rc<dyn ::std::any::Any> {
                self
            }
        }
        $crate::persist_rule_refs!($ty);
    };
}

/// `IGameSaveTraitData`: state that is not part of the replayed orders.
pub trait IGameSaveTraitData {
    fn issue_trait_data(&self, self_: &ActorRef) -> Option<Vec<MiniYamlNode>>;
    fn resolve_trait_data(&self, self_: &ActorRef, data: &MiniYaml);
}

pub trait ITick {
    fn tick(&self, self_: &ActorRef);
}

pub trait ISync {
    fn sync_hash(&self) -> i32;
}

pub trait INotifyCreated {
    fn created(&self, self_: &ActorRef);
}

pub trait INotifyAddedToWorld {
    fn added_to_world(&self, self_: &ActorRef);
}

pub trait INotifyRemovedFromWorld {
    fn removed_from_world(&self, self_: &ActorRef);
}

pub trait INotifyActorDisposing {
    fn disposing(&self, self_: &ActorRef);
}

pub trait INotifyOwnerChanged {
    fn on_owner_changed(&self, self_: &ActorRef, old_owner: &PlayerRef, new_owner: &PlayerRef);
}

pub trait INotifyBecomingIdle {
    fn on_becoming_idle(&self, self_: &ActorRef);
}

pub trait INotifyIdle {
    fn tick_idle(&self, self_: &ActorRef);
}

pub trait IResolveOrder {
    fn resolve_order(&self, self_: &ActorRef, order: &Order);
}

pub trait IValidateOrder {
    fn order_validation(&self, world: &World, client_id: i32, order: &Order) -> bool;
}

pub trait IDisabledTrait {
    fn is_trait_disabled(&self) -> bool;
}

/// `SubCell` values of OpenRA.
pub mod subcell {
    pub const INVALID: u8 = u8::MAX;
    pub const ANY: u8 = u8::MAX - 1;
    pub const FULL_CELL: u8 = 0;
    pub const FIRST: u8 = 1;
}

pub trait IOccupySpace: Any {
    fn center_position(&self) -> WPos;
    fn top_left(&self) -> CPos;
    fn occupied_cells(&self) -> Vec<(CPos, u8)>;
    fn as_any(&self) -> &dyn Any;
    /// `IPositionable.IsLeavingCell`; false for non-positionable traits.
    fn is_leaving_cell(&self, _location: CPos, _sub: u8) -> bool {
        false
    }
}

/// Tag of `BitSet<CrushClass>`.
pub struct CrushClass;

pub trait ICrushable {
    fn crushable_by_actor(&self, self_: &ActorRef, crusher: &ActorRef, classes: crate::bitset::BitSet<CrushClass>) -> bool;
    /// Mask of the players whose units can crush this actor.
    fn crushable_by(&self, self_: &ActorRef, classes: crate::bitset::BitSet<CrushClass>) -> u64;
}

pub trait IFacing {
    fn turn_speed(&self) -> WAngle;
    fn facing(&self) -> WAngle;
    fn set_facing(&self, f: WAngle);
    fn orientation(&self) -> WRot;
}

pub trait IHealth {
    fn damage_state(&self) -> crate::damage::DamageState;
    fn hp(&self) -> i32;
    fn max_hp(&self) -> i32;
    fn display_hp(&self) -> i32;
    fn is_dead(&self) -> bool;
    fn inflict_damage(&self, self_: &ActorRef, attacker: Option<&ActorRef>, damage: &crate::damage::Damage, ignore_modifiers: bool);
    fn kill(&self, self_: &ActorRef, attacker: Option<&ActorRef>, damage_types: &crate::damage::DamageTypes);
}

pub trait IEffectiveOwner {
    fn disguised(&self) -> bool;
    fn owner(&self) -> Option<PlayerRef>;
}

pub trait IDefaultVisibility {
    fn is_visible(&self, self_: &ActorRef, by_player: Option<&PlayerRef>) -> bool;
}

/// `ISelectionBar`. `viewer` is the render player.
pub trait ISelectionBar {
    fn value(&self, self_: &ActorRef, viewer: Option<&PlayerRef>) -> f32;
    fn color(&self) -> rustora_core::fieldloader::Color;
    fn display_when_empty(&self) -> bool {
        false
    }
}

pub trait IVisibilityModifier {
    fn is_visible(&self, self_: &ActorRef, by_player: Option<&PlayerRef>) -> bool;
}

/// `IRadarColorModifier`; `local_player` stands for `World.LocalPlayer`.
pub trait IRadarColorModifier {
    fn radar_color_override(
        &self,
        self_: &ActorRef,
        color: rustora_core::fieldloader::Color,
        local_player: Option<&PlayerRef>,
    ) -> rustora_core::fieldloader::Color;
}

pub trait ICreationActivity {
    fn is_trait_enabled(&self) -> bool;
    fn get_creation_activity(&self) -> Option<crate::activity::ActivityRef>;
}

pub trait IWorldLoaded {
    fn world_loaded(&self, world: &Rc<World>);
}

pub trait IPostWorldLoaded {
    fn post_world_loaded(&self, world: &Rc<World>);
}

pub trait ICreatePlayers {
    fn create_players(&self, world: &Rc<World>, player_random: &mut rustora_core::world_types::MersenneTwister);
}

pub trait IAssignSpawnPoints {
    fn assign_home_location(
        &self,
        world: &World,
        client: &rustora_net::session::Client,
        player_random: &mut rustora_core::world_types::MersenneTwister,
    ) -> CPos;
    fn spawn_point_for_client(&self, client_index: i32) -> i32;
}

pub trait IGameOver {
    fn game_over(&self, world: &Rc<World>);
}

pub trait INotifyPlayerDisconnected {
    fn player_disconnected(&self, self_: &ActorRef, p: &PlayerRef);
}

pub trait IUnlocksRenderPlayer {
    fn render_player_unlocked(&self) -> bool;
}

/// Delegate notified when an observed variable (condition) changes.
pub type VariableObserverNotifier = Rc<dyn Fn(&ActorRef, &ConditionCache)>;

pub struct VariableObserver {
    pub notifier: VariableObserverNotifier,
    pub variables: Vec<String>,
}

pub trait IObservesVariables {
    fn get_variable_observers(self: Rc<Self>) -> Vec<VariableObserver>;
}

/// Type erased access used by the actor to call `IObservesVariables` on `Rc<dyn ...>`.
pub trait ObservesVariablesDyn {
    fn observers(&self) -> Vec<VariableObserver>;
}

crate::persist_trait_refs!(
    dyn IDisabledTrait,
    dyn IFacing,
    dyn IHealth,
    dyn INotifyIdle,
    dyn IVisibilityModifier
);
