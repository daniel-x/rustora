// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The post-processing passes of `RenderPostProcessPassBase`: the world draws into an offscreen buffer, each pass
//! reads a snapshot of it (`Renderer.WorldBufferSnapshot`) and writes the whole buffer, and a final copy puts the
//! buffer on the target.

use bytemuck::{Pod, Zeroable};

/// A post-processing pass of the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PostEffect {
    /// `TintPostProcessEffect`: multiplies the colors.
    Tint([f32; 3]),
    /// `FlashPostProcessEffect`: blends toward a color.
    Flash { blend: f32, color: [f32; 3] },
    /// `ChronoshiftPostProcessEffect`: blends toward gray.
    Chronoshift { blend: f32 },
    /// `MenuPostProcessEffect`: `from` and `to` are 0 for none, 1 for black and 2 for desaturated.
    MenuFade { from: u32, to: u32, blend: f32 },
}

/// A pass that runs after the first `at` quads of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostPass {
    pub at: usize,
    pub effect: PostEffect,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct PostUniform {
    mode: u32,
    blend: f32,
    from_effect: f32,
    to_effect: f32,
    color: [f32; 4],
    /// The part of the buffer that the final copy stretches over the target, as a fraction of its size.
    region: [f32; 2],
    padding: [f32; 2],
}

impl PostUniform {
    /// The final copy of the buffer onto the target, with the pixel art scaling.
    pub(crate) fn copy(region: [f32; 2]) -> PostUniform {
        PostUniform {
            mode: 4,
            blend: 0.0,
            from_effect: 0.0,
            to_effect: 0.0,
            color: [0.0; 4],
            region,
            padding: [0.0; 2],
        }
    }

    pub(crate) fn of(effect: PostEffect) -> PostUniform {
        let mut u = PostUniform::copy([1.0; 2]);
        match effect {
            PostEffect::Tint(c) => {
                u.mode = 0;
                u.color = [c[0], c[1], c[2], 1.0];
            }
            PostEffect::Flash { blend, color } => {
                u.mode = 1;
                u.blend = blend;
                u.color = [color[0], color[1], color[2], 1.0];
            }
            PostEffect::Chronoshift { blend } => {
                u.mode = 2;
                u.blend = blend;
            }
            PostEffect::MenuFade { from, to, blend } => {
                u.mode = 3;
                u.blend = blend;
                u.from_effect = from as f32;
                u.to_effect = to as f32;
            }
        }
        u
    }
}

/// The world buffer, its snapshot and the pass pipeline, for one target size.
pub(crate) struct PostResources {
    pub size: (u32, u32),
    pub world: wgpu::Texture,
    pub world_view: wgpu::TextureView,
    pub snapshot: wgpu::Texture,
    snapshot_view: wgpu::TextureView,
    pub pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl PostResources {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> PostResources {
        let texture = |label, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let world = texture(
            "world buffer",
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let snapshot = texture(
            "world snapshot",
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        PostResources {
            size,
            world_view: world.create_view(&Default::default()),
            world,
            snapshot_view: snapshot.create_view(&Default::default()),
            snapshot,
            pipeline,
            layout,
        }
    }

    /// A bind group of the snapshot with its own uniform buffer holding `uniform`.
    pub fn bind_group(&self, device: &wgpu::Device, queue: &wgpu::Queue, uniform: PostUniform) -> wgpu::BindGroup {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post uniform"),
            size: std::mem::size_of::<PostUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&uniform));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.snapshot_view),
                },
            ],
        })
    }

    /// `Renderer.WorldBufferSnapshot`.
    pub fn snapshot(&self, encoder: &mut wgpu::CommandEncoder) {
        let copy = |t| wgpu::TexelCopyTextureInfo {
            texture: t,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        };
        encoder.copy_texture_to_texture(
            copy(&self.world),
            copy(&self.snapshot),
            wgpu::Extent3d {
                width: self.size.0.max(1),
                height: self.size.1.max(1),
                depth_or_array_layers: 1,
            },
        );
    }
}
