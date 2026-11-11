// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Ports of `Effects/Beacon.cs`, `Effects/FloatingText.cs` and `Traits/Player/PlaceBeacon.cs`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use rustora_core::fieldloader::Color;
use rustora_core::world_types::{WPos, WVec};
use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::effect::{Effect, EffectRef};
use rustora_engine::order::Order;
use rustora_engine::player::PlayerRef;
use rustora_engine::render::{PaletteRef, Renderable, fog_obscures, render_animation};
use rustora_engine::rules::TraitRegistry;
use rustora_engine::sequence::Animation;
use rustora_engine::traits::*;
use rustora_engine::world::World;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::radar::{PingVisibility, RadarPing, RadarPings};
use crate::support_powers::SupportPowerInfo;

const MAX_ARROW_HEIGHT: i32 = 512;

pub fn register(r: &mut TraitRegistry) {
    r.register("PlaceBeacon", |n| load_info!(PlaceBeaconInfo, n));
}

/// `Effects/Beacon.cs`.
pub struct Beacon {
    me: Weak<Beacon>,
    owner: PlayerRef,
    position: WPos,
    palette: PaletteRef,
    arrow: Option<Rc<Animation>>,
    beacon: Option<Rc<Animation>>,
    circles: Option<Rc<Animation>>,
    poster: Option<Rc<Animation>>,
    clock: Option<Rc<Animation>>,
    poster_palette: PaletteRef,
    duration: i32,
    delay: Cell<i32>,
    arrow_height: Cell<i32>,
    arrow_speed: Cell<i32>,
    tick: Cell<i32>,
}

impl Beacon {
    /// The `Beacon` of a `PlaceBeaconInfo`.
    pub fn new(owner: &PlayerRef, position: WPos, duration: i32, info: &PlaceBeaconInfo) -> Rc<Beacon> {
        let world = owner.player_actor().world();
        let animation = |sequence: &Option<String>, repeat: bool| {
            sequence.as_deref().filter(|s| !s.is_empty()).map(|s| {
                let a = Animation::new(world.map().sequences().clone(), &info.beacon_image, None, None);
                if repeat {
                    a.play_repeating(s);
                } else {
                    a.play(s);
                }
                a
            })
        };
        let palette = if info.is_player_palette {
            PaletteRef::player(&info.palette, owner)
        } else {
            PaletteRef::named(&info.palette)
        };
        let beacon = animation(&info.beacon_sequence, true);
        let arrow = animation(&info.arrow_sequence, false);
        let circles = animation(&info.circle_sequence, false);
        Rc::new_cyclic(|me| Beacon {
            me: me.clone(),
            owner: owner.clone(),
            position,
            palette,
            arrow,
            beacon,
            circles,
            poster: None,
            clock: None,
            poster_palette: PaletteRef::named(&info.palette),
            duration,
            delay: Cell::new(0),
            arrow_height: Cell::new(MAX_ARROW_HEIGHT),
            arrow_speed: Cell::new(50),
            tick: Cell::new(0),
        })
    }

    /// The support-power `Beacon` with an optional poster and a countdown clock, from the second `Beacon`
    /// constructor. `clock_fraction` is the share of the flight that is over, which fills the clock.
    pub fn for_support_power(
        owner: &PlayerRef,
        position: WPos,
        info: &SupportPowerInfo,
        clock_fraction: Option<Box<dyn Fn() -> f32>>,
        delay: i32,
        duration: i32,
    ) -> Rc<Beacon> {
        let world = owner.player_actor().world();
        let anim = |sequence: &Option<String>, repeat: bool| {
            sequence.as_deref().filter(|s| !s.is_empty()).map(|s| {
                let a = Animation::new(world.map().sequences().clone(), &info.beacon_image, None, None);
                if repeat {
                    a.play_repeating(s);
                } else {
                    a.play(s);
                }
                a
            })
        };
        let palette = if info.beacon_palette_is_player_palette {
            PaletteRef::player(&info.beacon_palette, owner)
        } else {
            PaletteRef::named(&info.beacon_palette)
        };
        let poster = anim(&info.beacon_poster, false);
        let clock = poster
            .as_ref()
            .and(info.clock_sequence.as_deref())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let a = Animation::new(world.map().sequences().clone(), &info.beacon_image, None, None);
                let fraction = clock_fraction.unwrap_or_else(|| Box::new(|| 0.0));
                let weak = Rc::downgrade(&a);
                a.play_fetch_index(
                    s,
                    Rc::new(move || {
                        let len = weak.upgrade().and_then(|a| a.current_sequence()).map_or(1, |s| s.length());
                        ((fraction() * (len - 1) as f32) as i32).clamp(0, len - 1)
                    }),
                );
                a
            });
        Rc::new_cyclic(|me| Beacon {
            me: me.clone(),
            owner: owner.clone(),
            position,
            palette,
            arrow: anim(&info.arrow_sequence, false),
            beacon: anim(&info.beacon_sequence, true),
            circles: anim(&info.circle_sequence, false),
            poster,
            clock,
            poster_palette: PaletteRef::named(&info.beacon_poster_palette),
            duration,
            delay: Cell::new(delay),
            arrow_height: Cell::new(MAX_ARROW_HEIGHT),
            arrow_speed: Cell::new(50),
            tick: Cell::new(0),
        })
    }
}

impl Effect for Beacon {
    fn is_cosmetic(&self) -> bool {
        true
    }

    fn tick(&self, world: &Rc<World>) {
        let delay = self.delay.get();
        self.delay.set(delay - 1);
        if delay > 0 {
            return;
        }
        let height = self.arrow_height.get() + self.arrow_speed.get();
        let clamped = height.clamp(0, MAX_ARROW_HEIGHT);
        self.arrow_height.set(clamped);
        if height != clamped {
            self.arrow_speed.set(-self.arrow_speed.get());
        }
        for a in [&self.arrow, &self.beacon, &self.circles, &self.clock].into_iter().flatten() {
            a.tick();
        }
        let tick = self.tick.get();
        self.tick.set(tick + 1);
        if self.duration > 0 && self.duration <= tick {
            let me: EffectRef = self.me.upgrade().expect("beacon");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
    }

    fn render_above_shroud(&self, viewer: Option<&PlayerRef>, out: &mut Vec<Renderable>) {
        if self.delay.get() > 0 || !self.owner.is_allied_with(viewer.map(|v| &**v)) {
            return;
        }
        let zero = WVec::new(0, 0, 0);
        for a in [&self.beacon, &self.circles].into_iter().flatten() {
            render_animation(a, self.position, zero, 0, &self.palette, out);
        }
        if let Some(a) = &self.arrow {
            render_animation(
                a,
                self.position + WVec::new(0, 0, self.arrow_height.get()),
                zero,
                0,
                &self.palette,
                out,
            );
        }
        if let Some(a) = &self.poster {
            render_animation(a, self.position, zero, 0, &self.poster_palette, out);
            if let Some(clock) = &self.clock {
                render_animation(clock, self.position, zero, 0, &self.poster_palette, out);
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `Effects/FloatingText.cs`.
pub struct FloatingText {
    me: Weak<FloatingText>,
    text: String,
    color: Color,
    remaining: Cell<i32>,
    pos: Cell<WPos>,
}

impl FloatingText {
    const VELOCITY: WVec = WVec::new(0, 0, 86);

    pub fn new(pos: WPos, color: Color, text: &str, duration: i32) -> Rc<FloatingText> {
        Rc::new_cyclic(|me| FloatingText {
            me: me.clone(),
            text: text.to_string(),
            color,
            remaining: Cell::new(duration),
            pos: Cell::new(pos),
        })
    }

    /// `FloatingText.FormatCashTick`.
    pub fn format_cash_tick(cash_amount: i32) -> String {
        format!("{}${}", if cash_amount < 0 { "-" } else { "+" }, cash_amount.unsigned_abs())
    }
}

impl Effect for FloatingText {
    fn is_cosmetic(&self) -> bool {
        true
    }

    fn tick(&self, world: &Rc<World>) {
        let remaining = self.remaining.get() - 1;
        self.remaining.set(remaining);
        if remaining <= 0 {
            let me: EffectRef = self.me.upgrade().expect("floating text");
            world.add_frame_end_task(Box::new(move |w| w.remove_effect(&me)));
        }
        self.pos.set(self.pos.get() + Self::VELOCITY);
    }

    fn annotation_text(&self, viewer: Option<&PlayerRef>) -> Option<(WPos, Color, String)> {
        let pos = self.pos.get();
        (!fog_obscures(viewer, pos)).then(|| (pos, self.color, self.text.clone()))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct PlaceBeaconInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    pub duration: i32,
    pub notification_type: String,
    pub notification: String,
    pub is_player_palette: bool,
    pub palette: String,
    pub beacon_image: String,
    pub beacon_sequence: Option<String>,
    pub arrow_sequence: Option<String>,
    pub circle_sequence: Option<String>,
}

impl Default for PlaceBeaconInfo {
    fn default() -> Self {
        PlaceBeaconInfo {
            instance_name: None,
            duration: 750,
            notification_type: "Sounds".into(),
            notification: "Beacon".into(),
            is_player_palette: true,
            palette: "player".into(),
            beacon_image: "beacon".into(),
            beacon_sequence: None,
            arrow_sequence: Some("arrow".into()),
            circle_sequence: Some("circles".into()),
        }
    }
}

/// `PlaceBeacon`: cosmetic, so its state stays out of the sync hash.
#[derive(rustora_engine::persist::Persist)]
pub struct PlaceBeacon {
    info: Rc<PlaceBeaconInfo>,
    #[persist(skip)]
    player_beacon: Rc<RefCell<Option<EffectRef>>>,
    #[persist(skip)]
    player_radar_ping: Rc<RefCell<Option<Rc<RadarPing>>>>,
}

register_traits!(PlaceBeacon: IResolveOrder);
info_impl!(PlaceBeaconInfo => |info, _init| Some(Rc::new(PlaceBeacon {
    info,
    player_beacon: Rc::default(),
    player_radar_ping: Rc::default(),
})));

impl IResolveOrder for PlaceBeacon {
    fn resolve_order(&self, self_: &ActorRef, order: &Order) {
        if order.order_string != "PlaceBeacon" {
            return;
        }
        let (info, me) = (self.info.clone(), self_.clone());
        let (beacon_slot, ping_slot) = (self.player_beacon.clone(), self.player_radar_ping.clone());
        let position = order.target.center_position();
        self_.world().add_frame_end_task(Box::new(move |w: &Rc<World>| {
            if let Some(old) = beacon_slot.borrow_mut().take() {
                w.remove_effect(&old);
            }
            let owner = me.owner();
            let beacon: EffectRef = Beacon::new(&owner, position, info.duration, &info);
            w.add_effect(beacon.clone());
            *beacon_slot.borrow_mut() = Some(beacon);
            let render_player = w.sound.local_player();
            if owner.is_allied_with(render_player.as_deref()) {
                let faction = render_player.as_ref().map(|p| p.faction.clone());
                rustora_engine::sound::play_notification(w, None, &info.notification_type, &info.notification, faction.as_deref());
            }
            if let Some(pings) = w.world_actor().trait_or_default::<RadarPings>() {
                if let Some(old) = ping_slot.borrow_mut().take() {
                    pings.remove(&old);
                }
                let ping = pings.add(
                    PingVisibility::AlliesOf(owner.clone()),
                    render_player.as_ref(),
                    position,
                    owner.color(),
                    info.duration,
                );
                *ping_slot.borrow_mut() = Some(ping);
            }
        }));
    }
}

rustora_engine::persist_never!(Beacon);

rustora_engine::persist_never!(FloatingText);
