// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Screen space drawing for the user interface: premultiplied RGBA textures, tint colors and clip rectangles.

use bytemuck::{Pod, Zeroable};

use crate::wgpu;

/// A premultiplied RGBA texture. Texture 0 is one white pixel, used for solid rectangles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UiTexture(pub u32);

/// One screen space quad. `src` is in texels, `color` is a premultiplied tint.
#[derive(Clone, Copy, Debug)]
pub struct UiQuad {
    pub texture: UiTexture,
    pub dst: [f32; 4],
    pub src: [f32; 4],
    pub color: [f32; 4],
    /// Premultiplied colors of the top left, top right, bottom right and bottom left corners, replacing `color`.
    pub gradient: Option<[[f32; 4]; 4]>,
    /// Corners of a solid rotated quad, replacing `dst`.
    pub corners: Option<[[f32; 2]; 4]>,
    /// Samples with the pixel art scaling filter, which `Renderer.EnableAntialiasingFilter` turns on.
    pub pixel_art: bool,
}

/// Quads in draw order with the clip rectangles that apply from a quad index on.
#[derive(Default)]
pub struct UiList {
    pub quads: Vec<UiQuad>,
    clips: Vec<(usize, Option<[i32; 4]>)>,
}

impl UiList {
    /// Sets the clip rectangle (x, y, width, height) for the quads that follow.
    pub fn set_clip(&mut self, clip: Option<[i32; 4]>) {
        self.clips.push((self.quads.len(), clip));
    }

    pub fn clear(&mut self) {
        self.quads.clear();
        self.clips.clear();
    }

    /// Adds a solid rectangle with a premultiplied color.
    pub fn solid(&mut self, dst: [f32; 4], color: [f32; 4]) {
        self.quads.push(UiQuad {
            texture: UiTexture(0),
            dst,
            src: [0.0, 0.0, 1.0, 1.0],
            color,
            gradient: None,
            corners: None,
            pixel_art: false,
        });
    }

    /// Adds a rectangle filled with a gradient between premultiplied corner colors, as `FillRectWithColor` with four
    /// colors. The corner order is top left, top right, bottom right, bottom left.
    pub fn gradient(&mut self, dst: [f32; 4], colors: [[f32; 4]; 4]) {
        self.quads.push(UiQuad {
            texture: UiTexture(0),
            dst,
            src: [0.0, 0.0, 1.0, 1.0],
            color: colors[0],
            gradient: Some(colors),
            corners: None,
            pixel_art: false,
        });
    }

    /// Adds a solid line, `width` pixels thick, with a premultiplied color, as `RgbaColorRenderer.DrawLine`.
    pub fn line(&mut self, from: [f32; 2], to: [f32; 2], width: f32, color: [f32; 4]) {
        let d = [to[0] - from[0], to[1] - from[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(f32::EPSILON);
        let n = [-d[1] / len * width / 2.0, d[0] / len * width / 2.0];
        self.quads.push(UiQuad {
            texture: UiTexture(0),
            dst: [0.0; 4],
            src: [0.0, 0.0, 1.0, 1.0],
            color,
            gradient: None,
            pixel_art: false,
            corners: Some([
                [from[0] - n[0], from[1] - n[1]],
                [from[0] + n[0], from[1] + n[1]],
                [to[0] + n[0], to[1] + n[1]],
                [to[0] - n[0], to[1] - n[1]],
            ]),
        });
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UiVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    pixel_art: f32,
}

struct Texture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// GPU state of the user interface pass.
pub struct UiPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    textures: Vec<Texture>,
    vertices: wgpu::Buffer,
    capacity: usize,
}

fn vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui vertices"),
        size: (capacity * std::mem::size_of::<UiVertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl UiPass {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> UiPass {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<UiVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let mut pass = UiPass {
            pipeline,
            layout,
            uniform,
            nearest: sampler(wgpu::FilterMode::Nearest),
            linear: sampler(wgpu::FilterMode::Linear),
            textures: Vec::new(),
            vertices: vertex_buffer(device, 6 * 1024),
            capacity: 6 * 1024,
        };
        pass.add_texture(device, queue, 1, 1, &[255; 4], false);
        pass
    }

    /// Creates a texture from premultiplied RGBA rows.
    pub fn add_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
        linear: bool,
    ) -> UiTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui image"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(if linear { &self.linear } else { &self.nearest }),
                },
            ],
        });
        self.textures.push(Texture {
            texture,
            bind_group,
            size: (width, height),
        });
        let id = UiTexture(self.textures.len() as u32 - 1);
        self.update_texture(queue, id, 0, 0, width, height, rgba);
        id
    }

    /// Replaces a region of a texture with premultiplied RGBA rows.
    pub fn update_texture(&self, queue: &wgpu::Queue, id: UiTexture, x: u32, y: u32, width: u32, height: u32, rgba: &[u8]) {
        if width == 0 || height == 0 {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.textures[id.0 as usize].texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draws `list` over the current content of `target`.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, size: (u32, u32), list: &UiList) {
        let (width, height) = size;
        let screen: [f32; 4] = [2.0 / width as f32, -2.0 / height as f32, -1.0, 1.0];
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&screen));

        let mut vertices = Vec::with_capacity(list.quads.len() * 6);
        for q in &list.quads {
            let (tw, th) = self.textures[q.texture.0 as usize].size;
            let (tw, th) = (tw as f32, th as f32);
            let [x, y, w, h] = q.dst;
            let [sx, sy, sw, sh] = q.src;
            let (u0, v0, u1, v1) = (sx / tw, sy / th, (sx + sw) / tw, (sy + sh) / th);
            let corner = |i: usize| q.gradient.map_or(q.color, |g| g[i]);
            let v = |px: f32, py: f32, u: f32, t: f32| UiVertex {
                pos: [px, py],
                uv: [u, t],
                color: corner(match (u == u1, t == v1) {
                    (false, false) => 0,
                    (true, false) => 1,
                    (true, true) => 2,
                    (false, true) => 3,
                }),
                pixel_art: if q.pixel_art { 1.0 } else { 0.0 },
            };
            if let Some([a, b, c, d]) = q.corners {
                vertices.extend_from_slice(&[
                    v(a[0], a[1], u0, v0),
                    v(b[0], b[1], u1, v0),
                    v(c[0], c[1], u1, v1),
                    v(a[0], a[1], u0, v0),
                    v(c[0], c[1], u1, v1),
                    v(d[0], d[1], u0, v1),
                ]);
                continue;
            }
            vertices.extend_from_slice(&[
                v(x, y, u0, v0),
                v(x + w, y, u1, v0),
                v(x + w, y + h, u1, v1),
                v(x, y, u0, v0),
                v(x + w, y + h, u1, v1),
                v(x, y + h, u0, v1),
            ]);
        }
        if vertices.len() > self.capacity {
            self.capacity = vertices.len().next_power_of_two();
            self.vertices = vertex_buffer(device, self.capacity);
        }
        queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ui") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));

            let mut next_clip = 0;
            let mut start = 0;
            while start < list.quads.len() {
                while next_clip < list.clips.len() && list.clips[next_clip].0 <= start {
                    let clip = list.clips[next_clip].1;
                    let (x, y, w, h) = clip_rect(clip, width, height);
                    pass.set_scissor_rect(x, y, w, h);
                    next_clip += 1;
                }
                let limit = list.clips.get(next_clip).map_or(list.quads.len(), |c| c.0);
                let texture = list.quads[start].texture;
                let mut end = start + 1;
                while end < limit && list.quads[end].texture == texture {
                    end += 1;
                }
                pass.set_bind_group(0, &self.textures[texture.0 as usize].bind_group, &[]);
                pass.draw(start as u32 * 6..end as u32 * 6, 0..1);
                start = end;
            }
        }
        queue.submit([encoder.finish()]);
    }
}

fn clip_rect(clip: Option<[i32; 4]>, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let Some([x, y, w, h]) = clip else {
        return (0, 0, width, height);
    };
    let x0 = x.clamp(0, width as i32);
    let y0 = y.clamp(0, height as i32);
    let x1 = (x + w).clamp(x0, width as i32);
    let y1 = (y + h).clamp(y0, height as i32);
    (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
}
