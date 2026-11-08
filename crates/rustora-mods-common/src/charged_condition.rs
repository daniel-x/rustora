// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Conditions/GrantChargedConditionOnToggle.cs`: a condition that the player switches on with the
//! deploy order, which lasts while a charge runs down and recharges while off.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use rustora_core::fieldloader::Color;
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef};
use rustora_engine::actor::{ActorRef, INVALID_CONDITION_TOKEN};
use rustora_engine::conditional::*;
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sound::{IOrderVoice, SoundType, play_random_at};
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::{activity_common, info_impl, load_info, register_traits};

use crate::orders::{DeployOrderTargeter, IIssueDeployOrder, IIssueOrder, IOrderTargeter};

const ORDER: &str = "ActivateCondition";

pub fn register(r: &mut TraitRegistry) {
    r.register("GrantChargedConditionOnToggle", |n| {
        load_info!(GrantChargedConditionOnToggleInfo, n)
    });
}

#[derive(Clone, Debug, FieldLoad)]
pub struct GrantChargedConditionOnToggleInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    #[field(require)]
    pub activated_condition: String,
    pub charged_condition: Option<String>,
    pub initial_charge: i32,
    pub charge_duration: i32,
    /// The C# field name keeps its spelling.
    pub charge_threshhold: i32,
    pub condition_duration: i32,
    pub can_cancel_condition: bool,
    pub cancels_current_activity: bool,
    pub cursor: String,
    pub blocked_cursor: String,
    pub activation_sounds: Vec<String>,
    /// The C# field name keeps its spelling.
    pub deactivattion_sounds: Vec<String>,
    pub voice: String,
    pub deactivated_color: Color,
    pub activated_color: Color,
    pub display_bar_when_empty: bool,
}

impl Default for GrantChargedConditionOnToggleInfo {
    fn default() -> Self {
        GrantChargedConditionOnToggleInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            activated_condition: String::new(),
            charged_condition: None,
            initial_charge: -1,
            charge_duration: 500,
            charge_threshhold: -1,
            condition_duration: 1,
            can_cancel_condition: false,
            cancels_current_activity: false,
            cursor: "deploy".into(),
            blocked_cursor: "deploy-blocked".into(),
            activation_sounds: Vec::new(),
            deactivattion_sounds: Vec::new(),
            voice: "Action".into(),
            deactivated_color: Color::from_argb(255, 255, 0, 255),
            activated_color: Color::from_argb(255, 139, 0, 139),
            display_bar_when_empty: true,
        }
    }
}

pub struct GrantChargedConditionOnToggle {
    info: Rc<GrantChargedConditionOnToggleInfo>,
    state: ConditionalState,
    me: Weak<GrantChargedConditionOnToggle>,
    charge_tick: Cell<i32>,
    is_active: Cell<bool>,
    activated_token: Cell<i32>,
    charged_token: Cell<i32>,
    charge_threshold: i32,
    activated_charge_threshold: i32,
}

register_traits!(GrantChargedConditionOnToggle: IIssueOrder, IIssueDeployOrder, IResolveOrder, ITick, ISelectionBar, IOrderVoice, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(GrantChargedConditionOnToggleInfo => |info, _init| {
    assert!(info.charge_duration >= 1, "ChargeDuration cannot be lower than 1.");
    assert!(info.condition_duration >= 1, "ConditionDuration cannot be lower than 1.");
    let charge_tick = if info.initial_charge < 0 || info.initial_charge >= info.charge_duration {
        info.charge_duration
    } else {
        info.initial_charge
    };
    let charge_threshold = if info.charge_threshhold < 0 || info.charge_threshhold > info.charge_duration {
        info.charge_duration
    } else {
        info.charge_threshhold
    };
    let activated_charge_threshold = charge_threshold * info.condition_duration / info.charge_duration;
    Some(Rc::new_cyclic(|me| GrantChargedConditionOnToggle {
        state: ConditionalState::new_pausable(&info.base),
        me: me.clone(),
        charge_tick: Cell::new(charge_tick),
        is_active: Cell::new(false),
        activated_token: Cell::new(INVALID_CONDITION_TOKEN),
        charged_token: Cell::new(INVALID_CONDITION_TOKEN),
        charge_threshold,
        activated_charge_threshold,
        info,
    }))
});

impl ConditionalHooks for GrantChargedConditionOnToggle {
    fn conditional(&self) -> &ConditionalState {
        &self.state
    }

    fn trait_disabled(&self, self_: &ActorRef) {
        if self.is_active.get() {
            self.deactivate(self_);
        }
        let i = &self.info;
        self.charge_tick
            .set(if i.initial_charge < 0 || i.initial_charge > i.charge_duration {
                i.charge_duration
            } else {
                i.initial_charge
            });
        if self.charged_token.get() != INVALID_CONDITION_TOKEN {
            self.charged_token.set(self_.revoke_condition(self.charged_token.get()));
        }
    }
}
rustora_engine::conditional_observers_impl!(GrantChargedConditionOnToggle);

impl ISync for GrantChargedConditionOnToggle {
    fn sync_hash(&self) -> i32 {
        self.state.sync_hash() ^ self.charge_tick.get()
    }
}

impl INotifyCreated for GrantChargedConditionOnToggle {
    fn created(&self, self_: &ActorRef) {
        conditional_created(self, self_);
    }
}

impl GrantChargedConditionOnToggle {
    /// `CanToggle`.
    pub fn can_toggle(&self) -> bool {
        !self.state.is_trait_disabled()
            && !self.state.is_trait_paused()
            && ((!self.is_active.get() && self.charge_tick.get() >= self.charge_threshold)
                || (self.is_active.get() && self.info.can_cancel_condition))
    }

    /// `ToggleState`: keeps the share of the charge between the charge and the condition duration.
    pub fn toggle_state(&self, self_: &ActorRef) {
        let i = &self.info;
        if self.is_active.get() {
            self.charge_tick
                .set(self.charge_tick.get() * i.charge_duration / i.condition_duration);
            self.deactivate(self_);
        } else {
            self.charge_tick
                .set(self.charge_tick.get() * i.condition_duration / i.charge_duration);
            self.activate(self_);
        }
    }

    fn activate(&self, self_: &ActorRef) {
        if !self.info.activation_sounds.is_empty() {
            play_random_at(
                &self_.world(),
                SoundType::World,
                &self.info.activation_sounds,
                self_.center_position(),
                1.0,
            );
        }
        if self.activated_token.get() == INVALID_CONDITION_TOKEN {
            self.activated_token.set(self_.grant_condition(&self.info.activated_condition));
        }
        self.is_active.set(true);
    }

    fn deactivate(&self, self_: &ActorRef) {
        if !self.info.deactivattion_sounds.is_empty() {
            play_random_at(
                &self_.world(),
                SoundType::World,
                &self.info.deactivattion_sounds,
                self_.center_position(),
                1.0,
            );
        }
        if self.activated_token.get() != INVALID_CONDITION_TOKEN {
            self.activated_token.set(self_.revoke_condition(self.activated_token.get()));
        }
        self.is_active.set(false);
    }
}

impl IIssueOrder for GrantChargedConditionOnToggle {
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn IOrderTargeter>> {
        if self.state.is_trait_disabled() {
            return Vec::new();
        }
        let me = self.me.clone();
        let (cursor, blocked) = (self.info.cursor.clone(), self.info.blocked_cursor.clone());
        vec![Box::new(DeployOrderTargeter::new(
            ORDER,
            5,
            Box::new(move || {
                if me.upgrade().is_some_and(|t| t.can_toggle()) {
                    cursor.clone()
                } else {
                    blocked.clone()
                }
            }),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn IOrderTargeter, _target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == ORDER).then(|| Order::new(ORDER, Some(self_.clone()), Target::Invalid, queued))
    }
}

impl IIssueDeployOrder for GrantChargedConditionOnToggle {
    fn issue_deploy_order(&self, self_: &ActorRef, queued: bool) -> Option<Order> {
        Some(Order::new(ORDER, Some(self_.clone()), Target::Invalid, queued))
    }

    fn can_issue_deploy_order(&self, _self_: &ActorRef, queued: bool) -> bool {
        queued || self.can_toggle()
    }
}

impl IResolveOrder for GrantChargedConditionOnToggle {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != ORDER {
            return;
        }
        if order.queued || self.info.cancels_current_activity {
            let toggle = self.me.upgrade().expect("trait alive");
            self_.queue_activity_queued(order.queued, ToggleChargedCondition::new(toggle));
        } else if self.can_toggle() {
            self.toggle_state(self_);
        }
    }
}

impl IOrderVoice for GrantChargedConditionOnToggle {
    fn voice_phrase_for_order(&self, _self_: &ActorRef, order: &Order) -> Option<String> {
        (order.order_string == ORDER).then(|| self.info.voice.clone())
    }
}

impl ITick for GrantChargedConditionOnToggle {
    fn tick(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() || self.state.is_trait_paused() {
            return;
        }
        if self.is_active.get() {
            if self.charge_tick.get() > 0 {
                self.charge_tick.set(self.charge_tick.get() - 1);
            } else {
                self.deactivate(self_);
            }
        } else if self.charge_tick.get() < self.info.charge_duration {
            self.charge_tick.set(self.charge_tick.get() + 1);
        }
        if let Some(charged) = &self.info.charged_condition {
            let threshold = if self.is_active.get() {
                self.activated_charge_threshold
            } else {
                self.charge_threshold
            };
            if self.charge_tick.get() < threshold {
                if self.charged_token.get() != INVALID_CONDITION_TOKEN {
                    self.charged_token.set(self_.revoke_condition(self.charged_token.get()));
                }
            } else if self.charged_token.get() == INVALID_CONDITION_TOKEN {
                self.charged_token.set(self_.grant_condition(charged));
            }
        }
    }
}

impl ISelectionBar for GrantChargedConditionOnToggle {
    fn value(&self, _self_: &ActorRef, _viewer: Option<&PlayerRef>) -> f32 {
        if self.state.is_trait_disabled() {
            return 0.0;
        }
        let duration = if self.is_active.get() {
            self.info.condition_duration
        } else {
            self.info.charge_duration
        };
        self.charge_tick.get() as f32 / duration as f32
    }

    fn color(&self) -> Color {
        if self.is_active.get() {
            self.info.activated_color
        } else {
            self.info.deactivated_color
        }
    }

    fn display_when_empty(&self) -> bool {
        self.info.display_bar_when_empty
    }
}

/// `ToggleChargedCondition`: toggles on its first run if possible.
pub struct ToggleChargedCondition {
    base: ActivityBase,
    toggle: Rc<GrantChargedConditionOnToggle>,
}

impl ToggleChargedCondition {
    pub fn new(toggle: Rc<GrantChargedConditionOnToggle>) -> ActivityRef {
        Rc::new(ToggleChargedCondition {
            base: ActivityBase::default(),
            toggle,
        })
    }
}

impl Activity for ToggleChargedCondition {
    activity_common!("ToggleChargedCondition");

    fn on_first_run(&self, self_: &ActorRef) {
        if self.toggle.can_toggle() {
            self.toggle.toggle_state(self_);
        }
    }
}
