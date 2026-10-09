// src/terrain/renderer/skybox.rs
//! HDRI skybox: loads a Radiance `.hdr` equirectangular image and renders it
//! as the scene background behind the terrain.
//!
//! The decode, the `-Y +X`/`+Y -X` orientation handling and the truncation
//! checks come from [`crate::formats::hdr::load_hdr`] (row 0 = image top for
//! the conventional `-Y +X` marker) — a second hand-rolled RGBE/RLE decoder
//! here is a second place for that contract to drift.
//!
//! The texture is `Rgba16Float`: it is filterable, so the pipeline can sample
//! it with the linear sampler. `Rgba32Float` is NOT filterable in wgpu, and
//! sampling it with a filtering sampler is a validation error — one the error
//! scope around pipeline creation only records as a degradation while the
//! surrounding submission is dropped, which blacks the whole frame.
//!
//! A missing or unreadable asset degrades explicitly (recorded degradation,
//! skybox becomes a no-op) instead of failing `TerrainScene` construction for
//! every caller.
use super::*;
use crate::core::resource_tracker::{
    tracked_create_buffer, tracked_create_texture, TrackedBuffer, TrackedTexture,
};

/// Default HDRI used for the scene background.
const DEFAULT_SKYBOX_HDR: &str = "assets/hdri/sky.hdr";
/// Environment variable that overrides [`DEFAULT_SKYBOX_HDR`].
const SKYBOX_HDR_ENV: &str = "FORGE3D_SKYBOX_HDR";
/// `write_texture` row pitch must be a multiple of 256 bytes.
const COPY_ROW_ALIGNMENT: u32 = 256;

/// Everything the skybox pass needs, built once and cached: the environment
/// texture, the static bind group and the pipeline.
struct SkyboxPipeline {
    _texture: TrackedTexture,
    _sampler: wgpu::Sampler,
    uniform_buffer: TrackedBuffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

pub(crate) struct SkyboxResources {
    /// `None` when the HDRI could not be loaded: `render` is then a no-op.
    environment: Option<SkyboxPipeline>,
}

/// Row-align an `Rgba16Float` (8 bytes/pixel) upload of `width` pixels.
fn skybox_bytes_per_row(width: u32) -> u32 {
    let row = width * 8;
    ((row + COPY_ROW_ALIGNMENT - 1) / COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT
}

/// Interleave linear-RGB `f32` into row-aligned `Rgba16Float` bytes (alpha 1).
fn hdri_to_rgba16_bytes(rgb: &[f32], width: u32, height: u32) -> Vec<u8> {
    let bytes_per_row = skybox_bytes_per_row(width);
    let mut padded = vec![0u8; (bytes_per_row * height) as usize];
    for y in 0..height as usize {
        let src = y * width as usize * 3;
        let dst = y * bytes_per_row as usize;
        for x in 0..width as usize {
            let px = &rgb[src + x * 3..src + x * 3 + 3];
            let base = dst + x * 8;
            for channel in 0..3 {
                let bits = half::f16::from_f32(px[channel]).to_bits();
                padded[base + channel * 2..base + channel * 2 + 2]
                    .copy_from_slice(&bits.to_ne_bytes());
            }
            padded[base + 6..base + 8].copy_from_slice(&half::f16::ONE.to_bits().to_ne_bytes());
        }
    }
    padded
}

impl SkyboxResources {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let path = std::env::var(SKYBOX_HDR_ENV).unwrap_or_else(|_| DEFAULT_SKYBOX_HDR.to_string());
        let img = match crate::formats::hdr::load_hdr(&path) {
            Ok(img) => img,
            Err(err) => {
                crate::core::degradation::record_degradation(
                    "rendering_fallback",
                    "terrain_skybox_asset_unavailable",
                    &format!(
                        "skybox background disabled: cannot decode '{path}' ({err}); \
                         set {SKYBOX_HDR_ENV} to point at an HDRI"
                    ),
                );
                return Self { environment: None };
            }
        };

        let data = hdri_to_rgba16_bytes(&img.data, img.width, img.height);
        let texture = match tracked_create_texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("terrain.skybox.texture"),
                size: wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        ) {
            Ok(texture) => texture,
            Err(err) => {
                crate::core::degradation::record_degradation(
                    "rendering_fallback",
                    "terrain_skybox_texture_unavailable",
                    &format!("skybox texture allocation failed ({err})"),
                );
                return Self { environment: None };
            }
        };
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(skybox_bytes_per_row(img.width)),
                rows_per_image: Some(img.height),
            },
            wgpu::Extent3d {
                width: img.width,
                height: img.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain.skybox.sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buffer = match tracked_create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("terrain.skybox.uniform_buffer"),
                size: std::mem::size_of::<[[f32; 4]; 4]>() as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        ) {
            Ok(buffer) => buffer,
            Err(err) => {
                crate::core::degradation::record_degradation(
                    "rendering_fallback",
                    "terrain_skybox_uniform_unavailable",
                    &format!("skybox uniform buffer allocation failed ({err})"),
                );
                return Self { environment: None };
            }
        };

        // Binding order MUST match src/shaders/skybox.wgsl: 0 = uniform
        // buffer, 1 = environment texture, 2 = filtering sampler. A swapped
        // order fails pipeline validation as a recorded degradation and
        // takes the whole submission (the frame) with it.
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain.skybox.bgl"),
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

        let shader = crate::core::shader_registry::create_labeled_shader_module(
            device,
            "terrain.skybox.shader",
            &crate::shader_sources::skybox(),
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain.skybox.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = crate::core::shader_registry::create_render_pipeline_scoped(
            device,
            &wgpu::RenderPipelineDescriptor {
                label: Some("terrain.skybox.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
            },
        );

        // All resources are static, so the bind group is built once here —
        // routine frames allocate nothing.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain.skybox.bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            environment: Some(SkyboxPipeline {
                _texture: texture,
                _sampler: sampler,
                uniform_buffer,
                bind_group,
                pipeline,
            }),
        }
    }

    /// Draw the skybox into `color_view` (a fullscreen pass) as the scene
    /// background. No-op (and no recorded degradation here) when the HDRI
    /// failed to load — that was recorded at construction.
    pub(crate) fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        inv_view_proj: [[f32; 4]; 4],
    ) -> Result<()> {
        let Some(environment) = self.environment.as_ref() else {
            return Ok(());
        };
        queue.write_buffer(
            &environment.uniform_buffer,
            0,
            bytemuck::cast_slice(&inv_view_proj),
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain.skybox.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&environment.pipeline);
            pass.set_bind_group(0, &environment.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        crate::core::shader_registry::record_shader_use("terrain.skybox.shader");
        Ok(())
    }
}
