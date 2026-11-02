// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Player actor traits.

use std::cell::Cell;
use std::rc::Rc;

use indexmap::IndexMap;
use rustora_derive::FieldLoad;
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

pub fn register(r: &mut TraitRegistry) {
    r.register("PlayerResources", |n| load_info!(PlayerResourcesInfo, n));
    r.register("GrantConditionOnBotOwner", |n| load_info!(GrantConditionOnBotOwnerInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PlayerResourcesInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub selectable_cash: Vec<i32>,
    pub default_cash: i32,
    pub insufficient_funds_notification: Option<String>,
    pub insufficient_funds_text_notification: Option<String>,
    pub insufficient_funds_notification_interval: i32,
    pub cash_tick_up_notification: Option<String>,
    pub cash_tick_down_notification: Option<String>,
    pub resource_values: IndexMap<String, i32>,
}

impl Default for PlayerResourcesInfo {
    fn default() -> Self {
        PlayerResourcesInfo {
            instance_name: None,
            selectable_cash: vec![2500, 5000, 10000, 20000],
            default_cash: 5000,
            insufficient_funds_notification: None,
            insufficient_funds_text_notification: None,
            insufficient_funds_notification_interval: 30000,
            cash_tick_up_notification: None,
            cash_tick_down_notification: None,
            resource_values: IndexMap::new(),
        }
    }
}

pub struct PlayerResources {
    pub info: Rc<PlayerResourcesInfo>,
    pub cash: Cell<i32>,
    pub resources: Cell<i32>,
    pub resource_capacity: Cell<i32>,
    pub earned: Cell<i32>,
    pub spent: Cell<i32>,
}

register_traits!(PlayerResources: ISync);
info_impl!(PlayerResourcesInfo => |info, init| {
    let w = init.world();
    let s = w.lobby_info.global_settings.option_str("startingcash", "");
    let cash = if s.is_empty() { info.default_cash } else { s.parse().unwrap_or(info.default_cash) };
    Some(Rc::new(PlayerResources {
        info,
        cash: Cell::new(cash),
        resources: Cell::new(0),
        resource_capacity: Cell::new(0),
        earned: Cell::new(0),
        spent: Cell::new(0),
    }))
});

impl ISync for PlayerResources {
    fn sync_hash(&self) -> i32 {
        self.cash.get() ^ self.resources.get() ^ self.resource_capacity.get()
    }
}

impl PlayerResources {
    pub fn change_cash(&self, amount: i32) -> i32 {
        if amount >= 0 {
            self.give_cash(amount);
            amount
        } else {
            let amount = amount.max(-self.get_cash_and_resources());
            self.take_cash(-amount, false);
            amount
        }
    }

    pub fn can_give_resources(&self, amount: i32) -> bool {
        self.resources.get().wrapping_add(amount) <= self.resource_capacity.get()
    }

    pub fn give_resources(&self, num: i32) {
        self.resources.set(self.resources.get().wrapping_add(num));
        self.earned.set(self.earned.get().wrapping_add(num));
        if self.resources.get() > self.resource_capacity.get() {
            self.earned
                .set(self.earned.get() - (self.resources.get() - self.resource_capacity.get()));
            self.resources.set(self.resource_capacity.get());
        }
    }

    pub fn take_resources(&self, num: i32) -> bool {
        if self.resources.get() < num {
            return false;
        }
        self.resources.set(self.resources.get() - num);
        self.spent.set(self.spent.get().wrapping_add(num));
        true
    }

    /// Adds cash, saturating at `int.MaxValue` like the checked C# arithmetic.
    pub fn give_cash(&self, num: i32) {
        if self.cash.get() < i32::MAX {
            self.cash.set(self.cash.get().checked_add(num).unwrap_or(i32::MAX));
        }
        if self.earned.get() < i32::MAX {
            self.earned.set(self.earned.get().checked_add(num).unwrap_or(i32::MAX));
        }
    }

    /// Spends ore before cash. Low funds notifications are a client feature and not ported here.
    pub fn take_cash(&self, num: i32, _notify_low_funds: bool) -> bool {
        if self.get_cash_and_resources() < num {
            return false;
        }
        self.resources.set(self.resources.get().wrapping_sub(num));
        self.spent.set(self.spent.get().wrapping_add(num));
        if self.resources.get() < 0 {
            self.cash.set(self.cash.get().wrapping_add(self.resources.get()));
            self.resources.set(0);
        }
        true
    }

    pub fn add_storage_capacity(&self, capacity: i32) {
        self.resource_capacity.set(self.resource_capacity.get().wrapping_add(capacity));
    }

    pub fn remove_storage_capacity(&self, capacity: i32) {
        self.resource_capacity.set(self.resource_capacity.get().wrapping_sub(capacity));
        if self.resources.get() > self.resource_capacity.get() {
            self.resources.set(self.resource_capacity.get());
        }
    }

    pub fn get_cash_and_resources(&self) -> i32 {
        self.cash.get().wrapping_add(self.resources.get())
    }
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct GrantConditionOnBotOwnerInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(require)]
    pub condition: String,
    #[field(require)]
    pub bots: Vec<String>,
}

pub struct GrantConditionOnBotOwner {
    info: Rc<GrantConditionOnBotOwnerInfo>,
    token: Cell<i32>,
}

register_traits!(GrantConditionOnBotOwner: INotifyCreated, INotifyOwnerChanged);
info_impl!(GrantConditionOnBotOwnerInfo => |info, _| Some(Rc::new(GrantConditionOnBotOwner { info, token: Cell::new(INVALID_CONDITION_TOKEN) })));

impl INotifyCreated for GrantConditionOnBotOwner {
    fn created(&self, self_: &ActorRef) {
        let o = self_.owner();
        if o.is_bot && o.bot_type.as_ref().is_some_and(|b| self.info.bots.contains(b)) {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }
}

impl INotifyOwnerChanged for GrantConditionOnBotOwner {
    fn on_owner_changed(&self, self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        if self.token.get() != INVALID_CONDITION_TOKEN {
            self.token.set(self_.revoke_condition(self.token.get()));
        }
        if new_owner.bot_type.as_ref().is_some_and(|b| self.info.bots.contains(b)) {
            self.token.set(self_.grant_condition(&self.info.condition));
        }
    }
}
