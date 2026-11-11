// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

use rustora_render::post::{PostEffect, PostPass};
use rustora_render::{Camera, Quad, Renderer, headless_device, render_offscreen};

/// Renders a white 4x4 quad, then `passes`, and returns the first pixel. `None` without a graphics adapter.
fn first_pixel(passes: &[PostPass]) -> Option<[u8; 4]> {
    let (device, queue) = headless_device()?;
    let mut renderer = Renderer::new(device, queue, rustora_render::wgpu::TextureFormat::Rgba8Unorm);
    let camera = Camera {
        x: 0.0,
        y: 0.0,
        zoom: 1.0,
        width: 4,
        height: 4,
    };
    let quads = [Quad::solid(0.0, 0.0, 4.0, 4.0, [1.0, 1.0, 1.0, 1.0])];
    let rgba = render_offscreen(&mut renderer, 4, 4, |r, view| r.render_with_passes(view, camera, &quads, passes));
    Some([rgba[0], rgba[1], rgba[2], rgba[3]])
}

#[test]
fn passes_run_in_order_after_their_quads() {
    let Some(plain) = first_pixel(&[]) else { return };
    assert_eq!(plain, [255, 255, 255, 255]);
    let tint = PostPass {
        at: 1,
        effect: PostEffect::Tint([0.5, 1.0, 0.25]),
    };
    let p = first_pixel(&[tint]).expect("adapter");
    assert!(
        p[0].abs_diff(128) <= 1 && p[1] == 255 && p[2].abs_diff(64) <= 1 && p[3] == 255,
        "{p:?}"
    );
    let black = PostPass {
        at: 1,
        effect: PostEffect::MenuFade {
            from: 1,
            to: 1,
            blend: 0.0,
        },
    };
    assert_eq!(first_pixel(&[tint, black]).expect("adapter"), [0, 0, 0, 255]);
    let flash = PostPass {
        at: 1,
        effect: PostEffect::Flash {
            blend: 1.0,
            color: [1.0, 0.0, 0.0],
        },
    };
    assert_eq!(first_pixel(&[black, flash]).expect("adapter"), [255, 0, 0, 255]);
}

/// The brightness of a white world pixel column on a window at a user interface scale of 1.5, for camera positions
/// 0 to 3. `None` without a graphics adapter.
fn column_brightness() -> Option<Vec<u32>> {
    let (device, queue) = headless_device()?;
    let mut renderer = Renderer::new(device, queue, rustora_render::wgpu::TextureFormat::Rgba8Unorm);
    renderer.ui_scale = 1.5;
    let quads = [Quad::solid(10.0, 0.0, 1.0, 4.0, [1.0, 1.0, 1.0, 1.0])];
    let mut sums = Vec::new();
    for x in 0..4 {
        let camera = Camera {
            x: x as f32,
            y: 0.0,
            zoom: 1.0,
            width: 20,
            height: 4,
        };
        let rgba = render_offscreen(&mut renderer, 30, 6, |r, view| r.render(view, camera, &quads));
        let row = &rgba[3 * 30 * 4..4 * 30 * 4];
        sums.push(row.chunks(4).map(|p| u32::from(p[0])).sum());
    }
    Some(sums)
}

#[test]
fn scaled_world_pixels_keep_their_width_while_scrolling() {
    let Some(sums) = column_brightness() else { return };
    for s in &sums {
        assert!(s.abs_diff(sums[0]) <= 8 && s.abs_diff(382) <= 8, "{sums:?}");
    }
}
