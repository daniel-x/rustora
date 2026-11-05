// SPDX-License-Identifier: GPL-3.0-or-later

struct Screen {
    scale: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) pixel_art: f32,
};

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) pixel_art: f32,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.pos = vec4<f32>(v.pos * screen.scale + screen.offset, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    o.pixel_art = v.pixel_art;
    return o;
}

fn texel(p: vec2<i32>, max_coord: vec2<i32>) -> vec4<f32> {
    return textureLoad(image, clamp(p, vec2<i32>(0), max_coord), 0);
}

// The pixel art scaling of `combined.frag`: offsets the sampling point to simulate bilinear interpolation in window
// coordinates, then interpolates the four nearest texels as `SamplePalettedBilinear` does.
fn sample_pixel_art(uv: vec2<f32>, size: vec2<f32>, pixels_per_texel: vec2<f32>) -> vec4<f32> {
    let uv_px = uv * size;
    let offset = fract(uv_px);
    let ik = 1.43;
    let interp = clamp(offset * ik * pixels_per_texel, vec2<f32>(0.0), vec2<f32>(0.5))
        + clamp((offset - 1.0) * ik * pixels_per_texel + 0.5, vec2<f32>(0.0), vec2<f32>(0.5));
    let tex_pos = floor(uv_px) + interp - 0.5;
    let f = fract(tex_pos);
    let tl = vec2<i32>(floor(tex_pos));
    let max_coord = vec2<i32>(size) - 1;
    let c1 = texel(tl, max_coord);
    let c2 = texel(tl + vec2<i32>(1, 0), max_coord);
    let c3 = texel(tl + vec2<i32>(0, 1), max_coord);
    let c4 = texel(tl + vec2<i32>(1, 1), max_coord);
    return mix(mix(c1, c2, f.x), mix(c3, c4, f.x), f.y);
}

@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(image));
    let pixels_per_texel = vec2<f32>(1.0 / dpdx(v.uv.x * size.x), 1.0 / dpdy(v.uv.y * size.y));
    let direct = textureSample(image, image_sampler, v.uv);
    if v.pixel_art < 0.5 {
        return direct * v.color;
    }
    return sample_pixel_art(v.uv, size, pixels_per_texel) * v.color;
}
