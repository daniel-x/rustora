// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Port of `Squads/AttackOrFleeFuzzy.cs` with the Mamdani inference of the FuzzyLogicLibrary it uses:
//! min for `and`, max for `or`, min implication, max aggregation and a 50 step Simpson centroid.
//! The rules are written out as conjunctions of disjunctions of `variable is term`.

use rustora_engine::actor::ActorRef;
use rustora_engine::traits::IHealth;

use crate::attack::{Armament, AttackBaseInfo};
use crate::mobile::MobileInfo;

#[derive(Clone, Copy)]
struct Trapezoid(f64, f64, f64, f64);

impl Trapezoid {
    /// `TrapezoidMembershipFunction.GetValue`.
    fn value(&self, x: f64) -> f64 {
        let Trapezoid(x1, x2, x3, x4) = *self;
        if x == x1 && x == x2 {
            return 1.0;
        }
        if x == x3 && x == x4 {
            return 1.0;
        }
        if x <= x1 || x >= x4 {
            return 0.0;
        }
        if x >= x2 && x <= x3 {
            return 1.0;
        }
        if x > x1 && x < x2 {
            return x / (x2 - x1) - x1 / (x2 - x1);
        }
        (0.0 - x) / (x4 - x3) + x4 / (x4 - x3)
    }
}

/// Terms of `OwnHealth` and `EnemyHealth`: NearDead, Injured, Normal.
const HEALTH: [Trapezoid; 3] = [
    Trapezoid(0.0, 0.0, 20.0, 40.0),
    Trapezoid(30.0, 50.0, 50.0, 70.0),
    Trapezoid(50.0, 80.0, 100.0, 100.0),
];
/// Terms of `RelativeAttackPower` (Weak, Equal, Strong) and `RelativeSpeed` (Slow, Equal, Fast).
const RELATIVE: [Trapezoid; 3] = [
    Trapezoid(0.0, 0.0, 70.0, 90.0),
    Trapezoid(85.0, 100.0, 100.0, 115.0),
    Trapezoid(110.0, 150.0, 150.0, 1000.0),
];
const ATTACK: Trapezoid = Trapezoid(0.0, 15.0, 15.0, 30.0);
const FLEE: Trapezoid = Trapezoid(25.0, 35.0, 35.0, 50.0);

const ND: u8 = 1;
const INJ: u8 = 2;
const NORM: u8 = 4;
const LOW: u8 = 1;
const EQ: u8 = 2;
const HIGH: u8 = 4;
const ANY: u8 = 7;

/// One rule: term sets of OwnHealth, EnemyHealth, RelativeAttackPower, RelativeSpeed, and whether it concludes Attack.
type Rule = ([u8; 4], bool);

const DEFAULT_RULES: [Rule; 11] = [
    ([NORM, ANY, ANY, ANY], true),
    ([INJ, ND, ANY, ANY], true),
    ([INJ, INJ | NORM, EQ | HIGH, ANY], true),
    ([INJ, INJ | NORM, LOW, LOW], true),
    ([INJ, INJ | NORM, LOW, EQ | HIGH], false),
    ([INJ, ANY, ANY, LOW], true),
    ([ND, ND | INJ, EQ | HIGH, LOW | EQ], true),
    ([ND, ND | INJ, LOW, EQ | HIGH], false),
    ([ND, NORM, LOW, EQ | HIGH], false),
    ([ND, NORM, EQ | HIGH, HIGH], false),
    ([ND, INJ, EQ, HIGH], false),
];

const RUSH_RULES: [Rule; 12] = [
    ([NORM, ANY, HIGH, ANY], true),
    ([NORM, ANY, LOW | EQ, ANY], false),
    DEFAULT_RULES[1],
    DEFAULT_RULES[2],
    DEFAULT_RULES[3],
    DEFAULT_RULES[4],
    DEFAULT_RULES[5],
    DEFAULT_RULES[6],
    DEFAULT_RULES[7],
    DEFAULT_RULES[8],
    DEFAULT_RULES[9],
    DEFAULT_RULES[10],
];

#[derive(Clone, Copy)]
pub enum AttackOrFleeFuzzy {
    Default,
    Rush,
}

/// The rule strength: `and` of the variables, `or` of the terms of each variable, in the order of the rule text.
fn strength(rule: &[u8; 4], memberships: &[[f64; 3]; 4]) -> f64 {
    let mut out = 0.0;
    for (v, &terms) in rule.iter().enumerate() {
        let mut or: Option<f64> = None;
        for (t, m) in memberships[v].iter().enumerate() {
            if terms & (1 << t) != 0 {
                or = Some(or.map_or(*m, |o: f64| o.max(*m)));
            }
        }
        let or = or.expect("term");
        out = if v == 0 { or } else { f64::min(out, or) };
    }
    out
}

fn centroid(strengths: &[(f64, bool)]) -> f64 {
    let mf = |x: f64| {
        let mut v: Option<f64> = None;
        for &(s, attack) in strengths {
            let c = f64::min(s, if attack { ATTACK.value(x) } else { FLEE.value(x) });
            v = Some(v.map_or(c, |v: f64| v.max(c)));
        }
        v.unwrap_or(0.0)
    };
    let (min, max, n) = (0.0f64, 50.0f64, 50);
    let step = (max - min) / n as f64;
    let (mut x2, mut y2, mut xy2) = (0.0, 0.0, 0.0);
    let (mut num, mut den) = (0.0, 0.0);
    for i in 0..n {
        if i == 0 {
            x2 = min;
            y2 = mf(x2);
            xy2 = x2 * y2;
        }
        let xm = min + step * (i as f64 + 0.5);
        let x_next = min + step * (i + 1) as f64;
        let y0 = y2;
        let ym = mf(xm);
        y2 = mf(x_next);
        let xy0 = xy2;
        let xym = xm * ym;
        xy2 = x_next * y2;
        num += step * (xy0 + 4.0 * xym + xy2) / 3.0;
        den += step * (y0 + 4.0 * ym + y2) / 3.0;
        x2 = x_next;
    }
    let _ = x2;
    num / den
}

/// `Exts.Clamp` on floats: a NaN compares below everything and becomes `min`.
fn clamp_f32(v: f32, min: f32, max: f32) -> f32 {
    if v.is_nan() || v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

fn normalized_health(actors: &[ActorRef]) -> f32 {
    let (mut max, mut hp) = (0i32, 0i32);
    for a in actors {
        if let Some(h) = a.trait_or_default::<dyn IHealth>() {
            max = max.wrapping_add(h.max_hp());
            hp = hp.wrapping_add(h.hp());
        }
    }
    if max == 0 {
        return 0.0;
    }
    (hp as i64 * 100 / max as i64) as i32 as f32
}

fn relative_value(own: &[ActorRef], enemy: &[ActorRef], f: impl Fn(&[ActorRef]) -> f32) -> f32 {
    if enemy.is_empty() {
        return 999.0;
    }
    if own.is_empty() {
        return 0.0;
    }
    clamp_f32(f(own) / f(enemy) * 100.0, 0.0, 999.0)
}

fn attack_power(a: &ActorRef) -> i32 {
    let mut sum = 0i32;
    for arm in a.traits_implementing::<Armament>() {
        let w = &arm.weapon;
        let burst = w.burst;
        let total_reload = w.reload_delay + (w.burst_delays[0].wrapping_mul(burst - 1)).clamp(1, 200);
        for wh in &w.warheads {
            if let Some(d) = wh.damage_value() {
                sum = sum.wrapping_add(d.wrapping_mul(burst) / total_reload * 100);
            }
        }
    }
    sum
}

fn relative_power(own: &[ActorRef], enemy: &[ActorRef]) -> f32 {
    relative_value(own, enemy, |actors| {
        let mut sum = 0i32;
        for a in actors {
            if a.info.has_trait_info::<AttackBaseInfo>() {
                sum = sum.wrapping_add(attack_power(a));
            }
        }
        sum as f32
    })
}

fn relative_speed(own: &[ActorRef], enemy: &[ActorRef]) -> f32 {
    relative_value(own, enemy, |actors| {
        let (mut sum, mut count) = (0i32, 0i32);
        for a in actors {
            if let Some(m) = a.info.trait_info::<MobileInfo>() {
                sum = sum.wrapping_add(m.speed);
                count += 1;
            }
        }
        if count == 0 { 0.0 } else { (sum / count) as f32 }
    })
}

impl AttackOrFleeFuzzy {
    pub fn can_attack(self, own: &[ActorRef], enemy: &[ActorRef]) -> bool {
        let inputs = [
            normalized_health(own) as f64,
            normalized_health(enemy) as f64,
            relative_power(own, enemy) as f64,
            relative_speed(own, enemy) as f64,
        ];
        let memberships = [
            HEALTH.map(|t| t.value(inputs[0])),
            HEALTH.map(|t| t.value(inputs[1])),
            RELATIVE.map(|t| t.value(inputs[2])),
            RELATIVE.map(|t| t.value(inputs[3])),
        ];
        let rules: &[Rule] = match self {
            AttackOrFleeFuzzy::Default => &DEFAULT_RULES,
            AttackOrFleeFuzzy::Rush => &RUSH_RULES,
        };
        let strengths: Vec<(f64, bool)> = rules.iter().map(|(r, attack)| (strength(r, &memberships), *attack)).collect();
        let chance = centroid(&strengths);
        !chance.is_nan() && chance < 30.0
    }
}
