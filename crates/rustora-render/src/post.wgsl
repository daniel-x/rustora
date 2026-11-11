// SPDX-License-Identifier: GPL-3.0-or-later
// Ports of glsl/postprocess_tint.frag, postprocess_flash.frag, postprocess_chronoshift.frag and
// postprocess_menufade.frag of OpenRA, plus the final copy onto the target (mode 4) with the pixel art scaling of
// combined.frag.

struct Post {
    mode: u32,
    blend: f32,
    from_effect: f32,
    to_effect: f32,
    color: vec4<f32>,
    region: vec2<f32>,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var world_texture: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // The position on the world texture, 0 to 1 from the top left, so that a target of another size, like a window
    // with a user interface scale, gets the whole texture.
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: VertexOut;
    out.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, 1.0 - y);
    return out;
}

fn luminance(c: vec4<f32>) -> f32 {
    return 0.5 * (min(c.r, min(c.g, c.b)) + max(c.r, max(c.g, c.b)));
}

fn color_for_effect(effect: f32, c: vec4<f32>) -> vec4<f32> {
    if (effect > 1.5) {
        let lum = luminance(c);
        return vec4<f32>(lum, lum, lum, c.a);
    }
    if (effect > 0.5) {
        return vec4<f32>(0.0, 0.0, 0.0, c.a);
    }
    return c;
}

fn texel(p: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    return textureLoad(world_texture, clamp(p, vec2<i32>(0), size - 1), 0);
}

// `EnablePixelArtScaling` of combined.frag: offsets the sampling point to simulate bilinear interpolation in window
// coordinates, so that every texel edge blends over about one window pixel, then interpolates the four nearest
// texels.
fn sample_pixel_art(uv: vec2<f32>, size: vec2<i32>) -> vec4<f32> {
    let uv_px = uv * post.region * vec2<f32>(size);
    let pixels_per_texel = abs(vec2<f32>(1.0 / dpdx(uv_px.x), 1.0 / dpdy(uv_px.y)));
    let offset = fract(uv_px);
    let ik = 1.43;
    let interp = clamp(offset * ik * pixels_per_texel, vec2<f32>(0.0), vec2<f32>(0.5))
        + clamp((offset - 1.0) * ik * pixels_per_texel + 0.5, vec2<f32>(0.0), vec2<f32>(0.5));
    let tex_pos = floor(uv_px) + interp - 0.5;
    let f = fract(tex_pos);
    let tl = vec2<i32>(floor(tex_pos));
    let c1 = texel(tl, size);
    let c2 = texel(tl + vec2<i32>(1, 0), size);
    let c3 = texel(tl + vec2<i32>(0, 1), size);
    let c4 = texel(tl + vec2<i32>(1, 1), size);
    return mix(mix(c1, c2, f.x), mix(c3, c4, f.x), f.y);
}

@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(world_texture));
    if post.mode == 4u {
        return sample_pixel_art(v.uv, size);
    }
    let c = textureLoad(world_texture, clamp(vec2<i32>(floor(v.uv * vec2<f32>(size))), vec2<i32>(0), size - 1), 0);
    switch post.mode {
        case 0u: {
            return vec4<f32>(post.color.rgb, c.a) * c;
        }
        case 1u: {
            return mix(c, vec4<f32>(post.color.rgb, c.a), post.blend);
        }
        case 2u: {
            let lum = luminance(c);
            return mix(c, vec4<f32>(lum, lum, lum, c.a), post.blend);
        }
        case 3u: {
            return mix(color_for_effect(post.to_effect, c), color_for_effect(post.from_effect, c), post.blend);
        }
        default: {
            return c;
        }
    }
}
