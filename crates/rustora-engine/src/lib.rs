// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Simulation engine: port of the synced parts of `OpenRA.Game` plus the conditional trait base.

pub mod activity;
pub mod actor;
pub mod actor_ext;
pub mod actor_info;
pub mod actor_init;
pub mod bitset;
pub mod conditional;
pub mod damage;
pub mod effect;
pub mod frozen;
pub mod generated;
pub mod map;
pub mod net_collections;
pub mod order;
pub mod order_manager;
pub mod player;
pub mod rules;
pub mod shroud;
pub mod spatial;
pub mod stub;
pub mod target;
pub mod terrain;
pub mod trait_dict;
pub mod traits;
pub mod world;
