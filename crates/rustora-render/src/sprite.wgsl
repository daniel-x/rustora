// SPDX-License-Identifier: GPL-3.0-or-later

struct Camera {
    scale: vec2<f32>,
    offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var sheet: texture_2d<u32>;
@group(0) @binding(2) var palettes: texture_2d<f32>;

// One quad: left, top, right and bottom; the texel rect; the palette row (-1 solid color, -2 a line); the color;
// the world tint of the four corners (or the line's four corners and, in `uv`, its end color).
struct InstanceIn {
    @location(0) pos: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) palette: f32,
    @location(3) color: vec4<f32>,
    @location(4) tint0: vec4<f32>,
    @location(5) tint1: vec4<f32>,
    @location(6) tint2: vec4<f32>,
};

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) palette: f32,
    @location(2) color: vec4<f32>,
    @location(3) tint: vec3<f32>,
    @location(4) @interpolate(flat) rect: vec4<f32>,
};

// The two triangles top left, top right, bottom right and top left, bottom right, bottom left.
const CORNERS = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, q: InstanceIn) -> VertexOut {
    let t = array<f32, 12>(q.tint0.x, q.tint0.y, q.tint0.z, q.tint0.w, q.tint1.x, q.tint1.y, q.tint1.z, q.tint1.w,
                           q.tint2.x, q.tint2.y, q.tint2.z, q.tint2.w);
    let c = CORNERS[vi];
    var o: VertexOut;
    var pos: vec2<f32>;
    if (q.palette < -1.5) {
        pos = vec2<f32>(t[2u * c], t[2u * c + 1u]);
        o.uv = vec2<f32>(0.0);
        o.palette = -1.0;
        o.color = select(q.color, q.uv, c >= 2u);
        o.tint = vec3<f32>(1.0);
        o.rect = vec4<f32>(0.0);
    } else {
        let right = c == 1u || c == 2u;
        let bottom = c >= 2u;
        pos = vec2<f32>(select(q.pos.x, q.pos.z, right), select(q.pos.y, q.pos.w, bottom));
        o.uv = vec2<f32>(select(q.uv.x, q.uv.z, right), select(q.uv.y, q.uv.w, bottom));
        o.palette = q.palette;
        o.color = q.color;
        o.tint = vec3<f32>(t[3u * c], t[3u * c + 1u], t[3u * c + 2u]);
        o.rect = q.uv;
    }
    o.pos = vec4<f32>(pos * camera.scale + camera.offset, 0.0, 1.0);
    return o;
}

// A negative palette row draws the vertex color; otherwise the sheet texel indexes the palette row, and the world
// tint multiplies the color like `c *= vTint` of combined.frag. The texel stays inside the sprite's rect: at a
// scaled user interface, quad edges fall on pixel centers, where the interpolated uv can end just outside.
@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
    if (v.palette < 0.0) {
        return v.color;
    }
    let texel_pos = clamp(floor(v.uv), v.rect.xy, max(v.rect.xy, v.rect.zw - vec2<f32>(1.0)));
    let index = textureLoad(sheet, vec2<i32>(texel_pos), 0).r;
    if (index == 0u) {
        discard;
    }
    let texel = textureLoad(palettes, vec2<i32>(i32(index), i32(v.palette)), 0);
    if (v.color.a < 0.0) {
        return vec4<f32>(v.color.rgb * v.tint, 1.0) * -v.color.a * texel.a;
    }
    return vec4<f32>(texel.rgb * v.tint, texel.a) * v.color.a;
}
