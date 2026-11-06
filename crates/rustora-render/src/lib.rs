// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! A 2D sprite renderer on wgpu. Sprites are 8-bit palette indices packed into `R8Uint` sheets; the fragment shader
//! looks the index up in a row of the palette texture, so player colors and palette effects cost no extra sprites.

use bytemuck::{Pod, Zeroable};
use rustora_formats::palette::Palette;

pub use wgpu;

pub mod post;
pub mod ui;

const SHEET_SIZE: u32 = 2048;
/// Number of palette rows of the palette texture.
pub const MAX_PALETTES: u32 = 128;

/// A sprite in a sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpriteRect {
    pub sheet: u16,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// One textured or solid quad in world pixels. `palette: None` draws `color` instead of the sprite.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub sprite: SpriteRect,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub palette: Option<u32>,
    /// Premultiplied color for solid quads; for sprites only the alpha is used, unless `replace_color` is set.
    pub color: [f32; 4],
    /// Sprite texels draw `color` scaled by the palette alpha, the shader's `ReplaceColor` mode.
    pub replace_color: bool,
    /// Solid lines: the four corners and the color of the last two, replacing `x`, `y`, `w`, `h`.
    pub line: Option<Line>,
}

/// The geometry of a solid line quad.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub corners: [[f32; 2]; 4],
    pub end_color: [f32; 4],
}

impl Quad {
    pub fn sprite(sprite: SpriteRect, x: f32, y: f32, palette: u32) -> Quad {
        Quad {
            sprite,
            x,
            y,
            w: sprite.w as f32,
            h: sprite.h as f32,
            palette: Some(palette),
            color: [1.0; 4],
            replace_color: false,
            line: None,
        }
    }

    pub fn solid(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Quad {
        Quad {
            sprite: SpriteRect::default(),
            x,
            y,
            w,
            h,
            palette: None,
            color,
            replace_color: false,
            line: None,
        }
    }

    /// A line from `from` to `to`, `width` pixels thick, as `RgbaColorRenderer.DrawLine`.
    pub fn line(from: [f32; 2], to: [f32; 2], width: f32, from_color: [f32; 4], to_color: [f32; 4]) -> Quad {
        let d = [to[0] - from[0], to[1] - from[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(f32::EPSILON);
        let n = [-d[1] / len * width / 2.0, d[0] / len * width / 2.0];
        let mut q = Quad::solid(from[0].min(to[0]), from[1].min(to[1]), d[0].abs(), d[1].abs(), from_color);
        q.line = Some(Line {
            corners: [
                [from[0] - n[0], from[1] - n[1]],
                [from[0] + n[0], from[1] + n[1]],
                [to[0] + n[0], to[1] + n[1]],
                [to[0] - n[0], to[1] - n[1]],
            ],
            end_color: to_color,
        });
        q
    }
}

/// The visible window into the world: the world pixel at the top left corner, the zoom and the target size.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    palette: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    scale: [f32; 2],
    offset: [f32; 2],
}

struct Sheet {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// Shelf packer state of the current sheet.
struct Packer {
    x: u32,
    y: u32,
    row_height: u32,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub ui: ui::UiPass,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    palette_texture: wgpu::Texture,
    palette_view: wgpu::TextureView,
    sheets: Vec<Sheet>,
    packer: Packer,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    format: wgpu::TextureFormat,
    post: Option<post::PostResources>,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Renderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite"),
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
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
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
            multiview_mask: None,
            cache: None,
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let palette_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("palettes"),
            size: wgpu::Extent3d {
                width: 256,
                height: MAX_PALETTES,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let palette_view = palette_texture.create_view(&Default::default());
        let vertex_capacity = 6 * 4096;
        let vertex_buffer = Self::create_vertex_buffer(&device, vertex_capacity);
        let ui = ui::UiPass::new(&device, &queue, format);
        Renderer {
            ui,
            device,
            queue,
            pipeline,
            layout,
            camera_buffer,
            palette_texture,
            palette_view,
            sheets: Vec::new(),
            packer: Packer { x: 0, y: 0, row_height: 0 },
            vertex_buffer,
            vertex_capacity,
            format,
            post: None,
        }
    }

    fn create_vertex_buffer(device: &wgpu::Device, vertices: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite vertices"),
            size: (vertices * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn add_sheet(&mut self) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sheet"),
            size: wgpu::Extent3d {
                width: SHEET_SIZE,
                height: SHEET_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sheet"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.palette_view),
                },
            ],
        });
        self.sheets.push(Sheet { texture, bind_group });
        self.packer = Packer { x: 0, y: 0, row_height: 0 };
    }

    /// Draws the user interface over the current content of `target`.
    pub fn render_ui(&mut self, target: &wgpu::TextureView, size: (u32, u32), list: &ui::UiList) {
        self.ui.render(&self.device, &self.queue, target, size, list);
    }

    /// Creates a user interface texture from premultiplied RGBA rows.
    pub fn add_ui_texture(&mut self, width: u32, height: u32, rgba: &[u8], linear: bool) -> ui::UiTexture {
        self.ui.add_texture(&self.device, &self.queue, width, height, rgba, linear)
    }

    /// Replaces a region of a user interface texture.
    pub fn update_ui_texture(&mut self, texture: ui::UiTexture, x: u32, y: u32, width: u32, height: u32, rgba: &[u8]) {
        self.ui.update_texture(&self.queue, texture, x, y, width, height, rgba);
    }

    /// Uploads an 8-bit indexed image of `w` x `h` pixels. A 1 px gap separates sprites.
    pub fn add_sprite(&mut self, w: u32, h: u32, data: &[u8]) -> SpriteRect {
        assert!(w < SHEET_SIZE && h < SHEET_SIZE, "sprite of {w}x{h} exceeds the sheet size");
        if self.sheets.is_empty() {
            self.add_sheet();
        }
        if self.packer.x + w > SHEET_SIZE {
            self.packer = Packer {
                x: 0,
                y: self.packer.y + self.packer.row_height + 1,
                row_height: 0,
            };
        }
        if self.packer.y + h > SHEET_SIZE {
            self.add_sheet();
        }
        let p = &mut self.packer;
        let rect = SpriteRect {
            sheet: (self.sheets.len() - 1) as u16,
            x: p.x as u16,
            y: p.y as u16,
            w: w as u16,
            h: h as u16,
        };
        p.x += w + 1;
        p.row_height = p.row_height.max(h);
        if w > 0 && h > 0 {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.sheets[rect.sheet as usize].texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: rect.x as u32,
                        y: rect.y as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        rect
    }

    pub fn set_palette(&self, row: u32, palette: &Palette) {
        assert!(row < MAX_PALETTES);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.palette_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            palette.as_flattened(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256 * 4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 256,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Clears `target` to black and draws `quads` in order.
    pub fn render(&mut self, target: &wgpu::TextureView, camera: Camera, quads: &[Quad]) {
        self.render_with_passes(target, camera, quads, &[]);
    }

    /// Draws `quads` like [`Renderer::render`], with the post-processing `passes` between them.
    pub fn render_with_passes(&mut self, target: &wgpu::TextureView, camera: Camera, quads: &[Quad], passes: &[post::PostPass]) {
        let cam = CameraUniform {
            scale: [2.0 * camera.zoom / camera.width as f32, -2.0 * camera.zoom / camera.height as f32],
            offset: [
                -1.0 - 2.0 * camera.zoom * camera.x / camera.width as f32,
                1.0 + 2.0 * camera.zoom * camera.y / camera.height as f32,
            ],
        };
        self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&cam));
        if self.sheets.is_empty() {
            self.add_sheet();
        }

        let mut vertices = Vec::with_capacity(quads.len() * 6);
        let mut batches: Vec<(u16, u32, u32)> = Vec::new();
        for q in quads {
            let start = vertices.len() as u32;
            let palette = q.palette.map_or(-1.0, |p| p as f32);
            let (u0, v0) = (q.sprite.x as f32, q.sprite.y as f32);
            let (u1, v1) = (u0 + q.sprite.w as f32, v0 + q.sprite.h as f32);
            let color = if q.replace_color {
                [q.color[0], q.color[1], q.color[2], -q.color[3]]
            } else {
                q.color
            };
            let v = |x: f32, y: f32, u: f32, w: f32| Vertex {
                pos: [x, y],
                uv: [u, w],
                palette,
                color,
            };
            if let Some(l) = &q.line {
                let [a, b, c, d] = l.corners;
                let p = |pos: [f32; 2], color: [f32; 4]| Vertex {
                    pos,
                    uv: [0.0, 0.0],
                    palette,
                    color,
                };
                vertices.extend_from_slice(&[
                    p(a, q.color),
                    p(b, q.color),
                    p(c, l.end_color),
                    p(a, q.color),
                    p(c, l.end_color),
                    p(d, l.end_color),
                ]);
            } else {
                let (x0, y0, x1, y1) = (q.x, q.y, q.x + q.w, q.y + q.h);
                vertices.extend_from_slice(&[
                    v(x0, y0, u0, v0),
                    v(x1, y0, u1, v0),
                    v(x1, y1, u1, v1),
                    v(x0, y0, u0, v0),
                    v(x1, y1, u1, v1),
                    v(x0, y1, u0, v1),
                ]);
            }
            let sheet = if q.palette.is_some() {
                q.sprite.sheet
            } else {
                batches.last().map_or(0, |b| b.0)
            };
            match batches.last_mut() {
                Some(b) if b.0 == sheet => b.2 = start + 6,
                _ => batches.push((sheet, start, start + 6)),
            }
        }
        if vertices.len() > self.vertex_capacity {
            self.vertex_capacity = vertices.len().next_power_of_two();
            self.vertex_buffer = Self::create_vertex_buffer(&self.device, self.vertex_capacity);
        }
        self.queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        if passes.is_empty() {
            self.draw_batches(&mut encoder, target, true, &batches, 0, vertices.len() as u32);
            self.queue.submit([encoder.finish()]);
            return;
        }
        let size = (camera.width, camera.height);
        if self.post.as_ref().is_none_or(|p| p.size != size) {
            self.post = Some(post::PostResources::new(&self.device, self.format, size));
        }
        let post = self.post.take().expect("post resources");
        let mut start = 0u32;
        let mut first = true;
        for pass in passes {
            let end = (pass.at.min(quads.len()) * 6) as u32;
            self.draw_batches(&mut encoder, &post.world_view, first, &batches, start, end.max(start));
            first = false;
            start = end.max(start);
            post.snapshot(&mut encoder);
            let bind_group = post.bind_group(&self.device, &self.queue, post::PostUniform::of(pass.effect));
            Self::draw_post(&mut encoder, &post, &post.world_view, &bind_group, false);
        }
        self.draw_batches(&mut encoder, &post.world_view, false, &batches, start, vertices.len() as u32);
        post.snapshot(&mut encoder);
        let bind_group = post.bind_group(&self.device, &self.queue, post::PostUniform::COPY);
        Self::draw_post(&mut encoder, &post, target, &bind_group, true);
        self.queue.submit([encoder.finish()]);
        self.post = Some(post);
    }

    /// Draws the parts of `batches` within the vertices `start..end` into `target`.
    fn draw_batches(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clear: bool,
        batches: &[(u16, u32, u32)],
        start: u32,
        end: u32,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sprites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for &(sheet, s, e) in batches {
            let (s, e) = (s.max(start), e.min(end));
            if s < e {
                pass.set_bind_group(0, &self.sheets[sheet as usize].bind_group, &[]);
                pass.draw(s..e, 0..1);
            }
        }
    }

    fn draw_post(
        encoder: &mut wgpu::CommandEncoder,
        post: &post::PostResources,
        target: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        clear: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&post.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Creates a device without a window, for screenshots and tests.
pub fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster_block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster_block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Renders `quads` into an offscreen RGBA texture and reads it back as tightly packed RGBA rows.
pub fn render_to_rgba(renderer: &mut Renderer, camera: Camera, quads: &[Quad]) -> Vec<u8> {
    render_offscreen(renderer, camera.width, camera.height, |renderer, view| {
        renderer.render(view, camera, quads)
    })
}

/// Runs `draw` on an offscreen RGBA texture of the given size and reads it back as tightly packed RGBA rows.
pub fn render_offscreen(renderer: &mut Renderer, w: u32, h: u32, draw: impl FnOnce(&mut Renderer, &wgpu::TextureView)) -> Vec<u8> {
    let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    draw(renderer, &texture.create_view(&Default::default()));
    let row = (4 * w).div_ceil(256) * 256;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
    renderer.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped range");
    let mut out = Vec::with_capacity((4 * w * h) as usize);
    for y in 0..h {
        out.extend_from_slice(&data[(y * row) as usize..(y * row + 4 * w) as usize]);
    }
    out
}

/// Runs a future to completion on the current thread. wgpu's native futures resolve immediately.
pub fn pollster_block_on<F: std::future::Future>(f: F) -> F::Output {
    pollster::block_on(f)
}
