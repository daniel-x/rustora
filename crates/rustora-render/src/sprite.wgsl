// SPDX-License-Identifier: GPL-3.0-or-later

struct Camera {
    scale: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var sheet: texture_2d<u32>;
@group(0) @binding(2) var palettes: texture_2d<f32>;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) palette: f32,
    @location(3) color: vec4<f32>,
    @location(4) tint: vec3<f32>,
};

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) palette: f32,
    @location(2) color: vec4<f32>,
    @location(3) tint: vec3<f32>,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.pos = vec4<f32>(v.pos * camera.scale + camera.offset, 0.0, 1.0);
    o.uv = v.uv;
    o.palette = v.palette;
    o.color = v.color;
    o.tint = v.tint;
    return o;
}

// A negative palette row draws the vertex color; otherwise the sheet texel indexes the palette row, and the world
// tint multiplies the color like `c *= vTint` of combined.frag.
@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
    if (v.palette < 0.0) {
        return v.color;
    }
    let index = textureLoad(sheet, vec2<i32>(floor(v.uv)), 0).r;
    if (index == 0u) {
        discard;
    }
    let texel = textureLoad(palettes, vec2<i32>(i32(index), i32(v.palette)), 0);
    if (v.color.a < 0.0) {
        return vec4<f32>(v.color.rgb * v.tint, 1.0) * -v.color.a * texel.a;
    }
    return vec4<f32>(texel.rgb * v.tint, texel.a) * v.color.a;
}
