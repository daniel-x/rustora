// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Transforms.cs`, `Activities/Transform.cs`, `Traits/Sellable.cs` and `Activities/Sell.cs`.

use std::rc::{Rc, Weak};

use rustora_core::world_types::{CVec, WAngle};
use rustora_derive::FieldLoad;
use rustora_engine::activity::{Activity, ActivityBase, ActivityRef, for_each_activity};
use rustora_engine::actor::ActorRef;
use rustora_engine::actor_init::{ActorInits, FacingInit, FactionInit, LocationInit, OwnerInit};
use rustora_engine::conditional::*;
use rustora_engine::order::Order;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::target::Target;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{activity_common, conditional_simple, info_impl, load_info, register_traits};

use crate::actor_reference::{HealthInit, SkipMakeAnimsInit};
use crate::base::can_place_building;
use crate::building::BuildingInfo;
use crate::interfaces::*;
use crate::player::PlayerResources;
use crate::production::get_sell_value;
use crate::render::WithMakeAnimation;

pub fn register(r: &mut TraitRegistry) {
    r.register("Transforms", |n| load_info!(TransformsInfo, n));
    r.register("Sellable", |n| load_info!(SellableInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct TransformsInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: PausableConditionalTraitInfo,
    #[field(require)]
    pub into_actor: String,
    pub offset: CVec,
    pub facing: WAngle,
    pub deploy_cursor: String,
    pub deploy_blocked_cursor: String,
}

impl Default for TransformsInfo {
    fn default() -> Self {
        TransformsInfo {
            instance_name: None,
            base: PausableConditionalTraitInfo::default(),
            into_actor: String::new(),
            offset: CVec::ZERO,
            facing: WAngle::new(384),
            deploy_cursor: "deploy".into(),
            deploy_blocked_cursor: "deploy-blocked".into(),
        }
    }
}

pub struct Transforms {
    pub info: Rc<TransformsInfo>,
    state: ConditionalState,
    self_: ActorRef,
    building_info: Option<Rc<BuildingInfo>>,
    faction: String,
}

register_traits!(Transforms: IResolveOrder, crate::orders::IIssueOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(TransformsInfo => |info, init| {
    let w = init.world();
    let actor_info = w.rules().actor(&info.into_actor.to_lowercase()).unwrap_or_else(|| panic!("unknown actor {}", info.into_actor));
    let building_info = actor_info.trait_info::<BuildingInfo>();
    let faction = init.inits.get::<FactionInit>().map_or_else(|| init.self_.owner().faction.clone(), |f| f.value.clone());
    let state = ConditionalState::new_pausable(&info.base);
    Some(Rc::new(Transforms { info, state, self_: init.self_.clone(), building_info, faction }))
});

conditional_simple!(Transforms);

impl Transforms {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }

    pub fn is_trait_paused(&self) -> bool {
        self.state.is_trait_paused()
    }

    pub fn can_deploy(&self) -> bool {
        if self.is_trait_paused() || self.is_trait_disabled() {
            return false;
        }
        match &self.building_info {
            None => true,
            Some(b) => can_place_building(&self.self_.world(), self.self_.location() + self.info.offset, b, Some(&self.self_)),
        }
    }

    pub fn get_transform_activity(&self) -> ActivityRef {
        Transform::new(
            &self.info.into_actor,
            TransformArgs {
                offset: self.info.offset,
                facing: self.info.facing,
                force_health_percentage: 0,
                skip_make_anims: false,
                faction: Some(self.faction.clone()),
            },
        )
    }

    pub fn deploy_transform(&self, queued: bool) {
        // Clearing blockers is done by the issuing client through its own orders.
        if !queued && !self.can_deploy() {
            return;
        }
        self.self_.queue_activity_queued(queued, self.get_transform_activity());
    }
}

impl crate::orders::IIssueOrder for Transforms {
    /// The cursor is chosen when the targeter is made instead of when it targets; nothing changes in between.
    fn orders(&self, _self_: &ActorRef) -> Vec<Box<dyn crate::orders::IOrderTargeter>> {
        if self.is_trait_disabled() {
            return Vec::new();
        }
        let cursor = if self.can_deploy() {
            &self.info.deploy_cursor
        } else {
            &self.info.deploy_blocked_cursor
        }
        .clone();
        vec![Box::new(crate::orders::DeployOrderTargeter::new(
            "DeployTransform",
            5,
            Box::new(move || cursor.clone()),
        ))]
    }

    fn issue_order(&self, self_: &ActorRef, order: &dyn crate::orders::IOrderTargeter, _target: &Target, queued: bool) -> Option<Order> {
        (order.order_id() == "DeployTransform").then(|| Order::new("DeployTransform", Some(self_.clone()), Target::Invalid, queued))
    }
}

impl IResolveOrder for Transforms {
    fn resolve_order(&self, _self_: &ActorRef, order: &Order) {
        if order.order_string == "DeployTransform" && !self.is_trait_paused() && !self.is_trait_disabled() {
            self.deploy_transform(order.queued);
        }
    }
}

pub struct TransformArgs {
    pub offset: CVec,
    pub facing: WAngle,
    pub force_health_percentage: i32,
    pub skip_make_anims: bool,
    pub faction: Option<String>,
}

impl Default for TransformArgs {
    fn default() -> Self {
        TransformArgs {
            offset: CVec::ZERO,
            facing: WAngle::new(384),
            force_health_percentage: 0,
            skip_make_anims: false,
            faction: None,
        }
    }
}

/// `Activities/Transform.cs`.
pub struct Transform {
    base: ActivityBase,
    me: Weak<Transform>,
    to_actor: String,
    args: TransformArgs,
}

impl Transform {
    pub fn new(to_actor: &str, args: TransformArgs) -> ActivityRef {
        Rc::new_cyclic(|me| Transform {
            base: ActivityBase::default(),
            me: me.clone(),
            to_actor: to_actor.to_string(),
            args,
        })
    }

    fn do_transform(&self, self_: &ActorRef, transforms: Option<Rc<Transforms>>, make_animation: Option<Rc<WithMakeAnimation>>) {
        let current = self_.current_activity();
        let me = self.me.upgrade().expect("activity alive");
        let a = self_.clone();
        self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| {
            if a.is_dead() || a.will_dispose() {
                return;
            }
            if transforms.as_ref().is_some_and(|t| !t.can_deploy()) {
                match make_animation.as_ref().filter(|_| !me.args.skip_make_anims) {
                    Some(m) => {
                        let (me2, a2) = (me.clone(), a.clone());
                        m.forward(
                            &a,
                            Rc::new(move || {
                                me2.base.is_interruptible.set(true);
                                me2.cancel(&a2, true);
                            }),
                        );
                    }
                    None => {
                        me.base.is_interruptible.set(true);
                        me.cancel(&a, true);
                    }
                }
                return;
            }
            for nt in a.traits_implementing::<dyn INotifyTransform>() {
                nt.on_transform(&a);
            }
            a.dispose();
            let mut init = ActorInits::new()
                .with(LocationInit::new(a.location() + me.args.offset))
                .with(OwnerInit::from_player(&a.owner()))
                .with(FacingInit::new(me.args.facing));
            if me.args.skip_make_anims {
                init.push(Rc::new(SkipMakeAnimsInit));
            }
            if let Some(f) = &me.args.faction {
                init.push(Rc::new(FactionInit::new(f.clone())));
            }
            if let Some(h) = a.trait_or_default::<dyn IHealth>() {
                let new_hp = if me.args.force_health_percentage > 0 {
                    me.args.force_health_percentage
                } else {
                    (h.hp() as i64 * 100 / h.max_hp() as i64) as i32
                };
                init.push(Rc::new(HealthInit::new(new_hp)));
            }
            for m in a.traits_implementing::<dyn ITransformActorInitModifier>() {
                m.modify_transform_actor_init(&a, &mut init);
            }
            let n = w.create_actor(&me.to_actor, init);
            for nt in a.traits_implementing::<dyn INotifyTransform>() {
                nt.after_transform(&n);
            }
            let mut transfers = Vec::new();
            for_each_activity(current.clone(), false, &mut |act| {
                if act.as_any().downcast_ref::<IssueOrderAfterTransform>().is_some() {
                    transfers.push(act.clone());
                }
            });
            for t in transfers {
                let tr = t.as_any().downcast_ref::<IssueOrderAfterTransform>().expect("checked");
                if tr.base.is_canceling() {
                    continue;
                }
                let order = Order::new(&tr.order_string, Some(n.clone()), tr.target.clone(), true);
                for r in n.traits_implementing::<dyn IResolveOrder>() {
                    r.resolve_order(&n, &order);
                }
            }
            *a.replaced_by_actor.borrow_mut() = Some(n);
        }));
    }
}

impl Activity for Transform {
    activity_common!("Transform");

    fn on_first_run(&self, self_: &ActorRef) {
        if self_.info.has_trait_info::<crate::mobile::MobileInfo>()
            || self_.info.has_trait_info_named("Husk")
            || self_.info.has_trait_info_named("Aircraft")
        {
            self.base.queue_child(crate::activities::Turn::new(self_, self.args.facing));
        }
        if self_.info.has_trait_info_named("Aircraft") {
            self.base.queue_child(crate::aircraft::Land::new(self_));
        }
    }

    fn tick(&self, self_: &ActorRef) -> bool {
        if self.base.is_canceling() {
            return true;
        }
        let transforms = self_.trait_or_default::<Transforms>();
        if transforms.as_ref().is_some_and(|t| !t.can_deploy()) {
            return true;
        }
        for nt in self_.traits_implementing::<dyn INotifyTransform>() {
            nt.before_transform(self_);
        }
        let make_animation = self_.trait_or_default::<WithMakeAnimation>();
        if let Some(m) = make_animation.as_ref().filter(|_| !self.args.skip_make_anims) {
            self.base.is_interruptible.set(false);
            self.base.queue_child(crate::activities::WaitFor::new(Box::new(|| false), true));
            let (me, a, t, m2) = (
                self.me.upgrade().expect("activity alive"),
                self_.clone(),
                transforms.clone(),
                m.clone(),
            );
            m.reverse(self_, Rc::new(move || me.do_transform(&a, t.clone(), Some(m2.clone()))));
            return false;
        }
        self.do_transform(self_, transforms, None);
        true
    }
}

/// `IssueOrderAfterTransform`.
pub struct IssueOrderAfterTransform {
    base: ActivityBase,
    order_string: String,
    target: Target,
}

impl IssueOrderAfterTransform {
    pub fn new(order_string: &str, target: &Target) -> ActivityRef {
        Rc::new(IssueOrderAfterTransform {
            base: ActivityBase::default(),
            order_string: order_string.to_string(),
            target: target.clone(),
        })
    }
}

impl Activity for IssueOrderAfterTransform {
    activity_common!("IssueOrderAfterTransform");
}

#[derive(Clone, Debug, FieldLoad)]
pub struct SellableInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(flatten)]
    pub base: ConditionalTraitInfo,
    pub refund_percent: i32,
    pub show_ticks: bool,
    pub skip_make_animation: bool,
    pub cursor: String,
}

impl Default for SellableInfo {
    fn default() -> Self {
        SellableInfo {
            instance_name: None,
            base: ConditionalTraitInfo::default(),
            refund_percent: 50,
            show_ticks: true,
            skip_make_animation: false,
            cursor: "sell".into(),
        }
    }
}

pub struct Sellable {
    pub info: Rc<SellableInfo>,
    state: ConditionalState,
}

impl Sellable {
    pub fn is_trait_disabled(&self) -> bool {
        self.state.is_trait_disabled()
    }
}

register_traits!(Sellable: IResolveOrder, INotifyCreated, IObservesVariables, IDisabledTrait, ISync);
info_impl!(SellableInfo => |info, _| {
    let state = ConditionalState::new(&info.base);
    Some(Rc::new(Sellable { info, state }))
});

conditional_simple!(Sellable);

impl IResolveOrder for Sellable {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string == "Sell" {
            self.sell(self_);
        }
    }
}

impl Sellable {
    pub fn sell(&self, self_: &ActorRef) {
        if self.state.is_trait_disabled() {
            return;
        }
        self_.cancel_activity();
        for ns in self_.traits_implementing::<dyn INotifySold>() {
            ns.selling(self_);
        }
        if !self.info.skip_make_animation
            && let Some(m) = self_.trait_or_default::<WithMakeAnimation>()
        {
            m.reverse_then_activity(self_, Sell::new(self_), false);
            return;
        }
        self_.queue_activity_queued(false, Sell::new(self_));
    }
}

/// `Activities/Sell.cs`.
pub struct Sell {
    base: ActivityBase,
    health: Option<Rc<dyn IHealth>>,
    sellable_info: Rc<SellableInfo>,
    player_resources: Rc<PlayerResources>,
}

impl Sell {
    pub fn new(self_: &ActorRef) -> ActivityRef {
        let base = ActivityBase::default();
        base.is_interruptible.set(false);
        Rc::new(Sell {
            base,
            health: self_.trait_or_default::<dyn IHealth>(),
            sellable_info: self_.info.trait_info::<SellableInfo>().expect("Sellable"),
            player_resources: self_.owner().player_actor().trait_::<PlayerResources>(),
        })
    }
}

impl Activity for Sell {
    activity_common!("Sell");

    fn tick(&self, self_: &ActorRef) -> bool {
        let sell_value = get_sell_value(self_) as i64;
        let (hp, max_hp) = self.health.as_ref().map_or((1i64, 1i64), |h| (h.hp() as i64, h.max_hp() as i64));
        let refund = (sell_value * self.sellable_info.refund_percent as i64 * hp / (100 * max_hp)) as i32;
        self.player_resources.change_cash(refund);
        for ns in self_.traits_implementing::<dyn INotifySold>() {
            ns.sold(self_);
        }
        self_.dispose();
        false
    }
}
