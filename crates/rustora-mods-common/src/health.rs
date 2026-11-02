// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Traits/Health.cs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rustora_derive::FieldLoad;
use rustora_engine::actor::ActorRef;
use rustora_engine::damage::{AttackInfo, Damage, DamageState, DamageTypes};
use rustora_engine::player::PlayerRef;
use rustora_engine::rules::TraitRegistry;
use rustora_engine::traits::*;
use rustora_engine::{info_impl, load_info, register_traits};

use crate::actor_reference::HealthInit;
use crate::interfaces::*;

pub fn register(r: &mut TraitRegistry) {
    r.register("Health", |n| load_info!(HealthInfo, n));
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HealthInfo {
    #[field(ignore)]
    pub instance_name: Option<String>,
    #[field(name = "HP")]
    pub hp: i32,
    pub notify_applied_damage: bool,
}

impl Default for HealthInfo {
    fn default() -> Self {
        HealthInfo {
            instance_name: None,
            hp: 0,
            notify_applied_damage: true,
        }
    }
}

#[derive(Default)]
struct Notifiers {
    damage_state_changed: Vec<Rc<dyn INotifyDamageStateChanged>>,
    damage: Vec<Rc<dyn INotifyDamage>>,
    damage_player: Vec<Rc<dyn INotifyDamage>>,
    modifiers: Vec<Rc<dyn IDamageModifier>>,
    modifiers_player: Vec<Rc<dyn IDamageModifier>>,
    killed: Vec<Rc<dyn INotifyKilled>>,
    killed_player: Vec<Rc<dyn INotifyKilled>>,
}

pub struct Health {
    pub info: Rc<HealthInfo>,
    hp: Cell<i32>,
    max_hp: i32,
    display_hp: Cell<i32>,
    pub remove_on_death: Cell<bool>,
    n: RefCell<Notifiers>,
}

register_traits!(Health: IHealth, ISync, ITick, INotifyCreated, INotifyOwnerChanged);
info_impl!(HealthInfo => |info, init| {
    let max_hp = if info.hp > 0 { info.hp } else { 1 };
    let mut hp = max_hp;
    if let Some(h) = init.inits.get::<HealthInit>() {
        let v = if h.value <= 0 { 1 } else { h.value };
        hp = (v as i64 * max_hp as i64 / 100) as i32;
    }
    Some(Rc::new(Health {
        info,
        hp: Cell::new(hp),
        max_hp,
        display_hp: Cell::new(hp),
        remove_on_death: Cell::new(true),
        n: RefCell::new(Notifiers::default()),
    }))
});

impl ISync for Health {
    fn sync_hash(&self) -> i32 {
        self.hp.get()
    }
}

impl ITick for Health {
    fn tick(&self, _self_: &ActorRef) {
        let (hp, d) = (self.hp.get(), self.display_hp.get());
        self.display_hp.set(if hp >= d { hp } else { (2 * d + hp) / 3 });
    }
}

impl INotifyCreated for Health {
    fn created(&self, self_: &ActorRef) {
        let pa = self_.owner().player_actor().clone();
        *self.n.borrow_mut() = Notifiers {
            damage_state_changed: self_.traits_implementing::<dyn INotifyDamageStateChanged>(),
            damage: self_.traits_implementing::<dyn INotifyDamage>(),
            damage_player: pa.traits_implementing::<dyn INotifyDamage>(),
            modifiers: self_.traits_implementing::<dyn IDamageModifier>(),
            modifiers_player: pa.traits_implementing::<dyn IDamageModifier>(),
            killed: self_.traits_implementing::<dyn INotifyKilled>(),
            killed_player: pa.traits_implementing::<dyn INotifyKilled>(),
        };
    }
}

impl INotifyOwnerChanged for Health {
    fn on_owner_changed(&self, _self_: &ActorRef, _old: &PlayerRef, new_owner: &PlayerRef) {
        let pa = new_owner.player_actor().clone();
        let mut n = self.n.borrow_mut();
        n.damage_player = pa.traits_implementing::<dyn INotifyDamage>();
        n.modifiers_player = pa.traits_implementing::<dyn IDamageModifier>();
        n.killed_player = pa.traits_implementing::<dyn INotifyKilled>();
    }
}

impl Health {
    fn state(&self) -> DamageState {
        let (hp, max) = (self.hp.get() as i64, self.max_hp as i64);
        if hp == max {
            DamageState::UNDAMAGED
        } else if hp <= 0 {
            DamageState::DEAD
        } else if hp * 100 < max * 25 {
            DamageState::CRITICAL
        } else if hp * 100 < max * 50 {
            DamageState::HEAVY
        } else if hp * 100 < max * 75 {
            DamageState::MEDIUM
        } else {
            DamageState::LIGHT
        }
    }

    fn notify_applied(&self, self_: &ActorRef, attacker: Option<&ActorRef>, ai: &AttackInfo) {
        if !self.info.notify_applied_damage {
            return;
        }
        let Some(a) = attacker.filter(|a| a.is_in_world() && !a.is_dead()) else {
            return;
        };
        for nd in a.traits_implementing::<dyn INotifyAppliedDamage>() {
            nd.applied_damage(a, self_, ai);
        }
        for nd in a.owner().player_actor().traits_implementing::<dyn INotifyAppliedDamage>() {
            nd.applied_damage(a, self_, ai);
        }
    }

    pub fn resurrect(&self, self_: &ActorRef, repairer: Option<&ActorRef>) {
        if !self.is_dead() {
            return;
        }
        self.hp.set(self.max_hp);
        let ai = AttackInfo {
            attacker: repairer.cloned(),
            damage: Damage::new(-self.max_hp, DamageTypes::default()),
            damage_state: self.state(),
            previous_damage_state: DamageState::DEAD,
        };
        let (d, dp, dsc) = {
            let n = self.n.borrow();
            (n.damage.clone(), n.damage_player.clone(), n.damage_state_changed.clone())
        };
        for nd in d.iter().chain(dp.iter()) {
            nd.damaged(self_, &ai);
        }
        for nd in dsc {
            nd.damage_state_changed(self_, &ai);
        }
        self.notify_applied(self_, repairer, &ai);
    }
}

impl IHealth for Health {
    fn damage_state(&self) -> DamageState {
        self.state()
    }

    fn hp(&self) -> i32 {
        self.hp.get()
    }

    fn max_hp(&self) -> i32 {
        self.max_hp
    }

    fn display_hp(&self) -> i32 {
        self.display_hp.get()
    }

    fn is_dead(&self) -> bool {
        self.hp.get() <= 0
    }

    fn inflict_damage(&self, self_: &ActorRef, attacker: Option<&ActorRef>, damage: &Damage, ignore_modifiers: bool) {
        if self.is_dead() {
            return;
        }
        let old_state = self.state();
        let mut damage = damage.clone();
        if !ignore_modifiers && damage.value > 0 {
            let (m, mp) = {
                let n = self.n.borrow();
                (n.modifiers.clone(), n.modifiers_player.clone())
            };
            // Exact rational product, truncated like the C# decimal arithmetic.
            let mut num = damage.value as i128;
            let mut den = 1i128;
            for dm in m.iter().chain(mp.iter()) {
                let modifier = dm.get_damage_modifier(attacker, &damage);
                if modifier != 100 {
                    num *= modifier as i128;
                    den *= 100;
                }
            }
            damage = Damage::new((num / den) as i32, damage.damage_types);
        }
        self.hp.set(self.hp.get().wrapping_sub(damage.value).clamp(0, self.max_hp));
        let ai = AttackInfo {
            attacker: attacker.cloned(),
            damage,
            damage_state: self.state(),
            previous_damage_state: old_state,
        };
        let (d, dp, dsc, k, kp) = {
            let n = self.n.borrow();
            (
                n.damage.clone(),
                n.damage_player.clone(),
                n.damage_state_changed.clone(),
                n.killed.clone(),
                n.killed_player.clone(),
            )
        };
        for nd in d.iter().chain(dp.iter()) {
            nd.damaged(self_, &ai);
        }
        if self.state() != old_state {
            for nd in dsc {
                nd.damage_state_changed(self_, &ai);
            }
        }
        self.notify_applied(self_, attacker, &ai);
        if self.hp.get() == 0 {
            for nd in k.iter().chain(kp.iter()) {
                nd.killed(self_, &ai);
            }
            if self.remove_on_death.get() {
                self_.dispose();
            }
        }
    }

    fn kill(&self, self_: &ActorRef, attacker: Option<&ActorRef>, damage_types: &DamageTypes) {
        self.inflict_damage(self_, attacker, &Damage::new(self.max_hp, *damage_types), true);
    }
}
