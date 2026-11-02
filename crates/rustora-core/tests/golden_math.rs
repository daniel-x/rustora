// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Compares fixed point math and the RNG against `testdata/golden/math.txt` from the C# reference.

use std::path::PathBuf;

use rustora_core::world_types::*;

fn golden() -> Option<String> {
    let home = std::env::var_os("RUSTORA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
    std::fs::read_to_string(home.join("testdata/golden/math.txt")).ok()
}

#[test]
fn math_matches_reference() {
    let Some(expected) = golden() else { return };
    let mut out = Vec::new();
    let mut r = MersenneTwister::new(12345);
    for _ in 0..2000 {
        let a = r.next();
        let b = r.next_range(-1024, 1024);
        let c = r.next_max(7);
        out.push(format!("mt {a} {b} {c}"));
    }

    let mut g = MersenneTwister::new(777);
    for _ in 0..3000 {
        let a = WAngle::new(g.next_range(-2048, 2048));
        let b = WAngle::new(g.next_range(-2048, 2048));
        let c = WAngle::new(g.next_range(-2048, 2048));
        let rot = WRot::new(a, b, c);
        let v = WVec::new(
            g.next_range(-100000, 100000),
            g.next_range(-100000, 100000),
            g.next_range(-100000, 100000),
        );
        let r1 = WAngle::new(g.next_max(1024));
        let r2 = WAngle::new(g.next_max(1024));
        let r3 = WAngle::new(g.next_max(1024));
        let rot2 = WRot::new(r1, r2, r3);
        let mul = g.next_range(0, 100);
        let x = g.next_range(-50000, 50000);
        let y = g.next_range(-50000, 50000);
        let p1 = WPos::new(g.next_range(-100000, 100000), g.next_range(-100000, 100000), g.next_range(0, 10000));
        let p2 = WPos::new(g.next_range(-100000, 100000), g.next_range(-100000, 100000), g.next_range(0, 10000));
        let pitch = WAngle::new(g.next_range(0, 256));
        let axis = WVec::new(0, 0, 1024);
        out.push(format!(
            "angle {} {} {} {} {} {} {} {}",
            a.angle,
            a.sin(),
            a.cos(),
            a.tan(),
            WAngle::arc_tan(y, x).angle,
            WAngle::arc_sin(x % 1025).angle,
            WAngle::arc_cos(y % 1025).angle,
            WAngle::lerp(a, b, mul, 100).angle
        ));
        out.push(format!(
            "rot {} {} {} {} {} {} {}",
            rot,
            rot.rotate(&rot2),
            v.rotate(&rot),
            WRot::slerp(&rot, &rot2, mul, 100),
            -rot,
            rot + rot2,
            WRot::from_axis_angle(axis, a)
        ));
        let lq = WPos::lerp_quadratic(p1, p2, pitch, mul, 100);
        let lqv = WVec::lerp_quadratic(p1 - WPos::ZERO, p2 - WPos::ZERO, pitch, mul, 100);
        let pdf = WDist::from_pdf(&mut g, 3).length;
        out.push(format!(
            "vec {} {} {} {} {} {}",
            v.length(),
            v.horizontal_length(),
            v.yaw().angle,
            lq,
            lqv,
            pdf
        ));
    }

    let mut mismatches = 0;
    for (i, (e, a)) in expected.lines().zip(out.iter()).enumerate() {
        if e != a {
            if mismatches < 5 {
                eprintln!("line {}:\n  expected {e}\n  actual   {a}", i + 1);
            }
            mismatches += 1;
        }
    }
    assert_eq!(expected.lines().count(), out.len());
    assert_eq!(mismatches, 0);
}
