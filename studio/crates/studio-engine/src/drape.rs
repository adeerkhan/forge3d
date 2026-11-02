//! Draped imagery in the live viewport.
//!
//! The engine's viewer draws the terrain; this pass paints the dataset's raster
//! on top of it *as if it were the surface*, using the depth buffer the viewer
//! just wrote. It reconstructs world position per pixel from the depth and the
//! camera's inverse view-projection, converts that to terrain UV, samples the
//! imagery, and blends it in. Nothing is re-drawn, so the geometry, silhouette
//! and depth stay the engine's — the raster cannot drift away from the surface
//! it is draped on, which is the failure mode of a screen-space overlay.
//!
//! Heights are relative to the terrain minimum with `z_scale` applied (see
//! `TerrainScene::load_terrain_for_viewer`), and the ground plane is `z = 0`, so
//! `uv = world.xy / span + 0.5` is the same mapping the terrain mesh uses.

use wgpu::util::DeviceExt;

use crate::grid::view_projection;
use studio_project::CameraParams;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrapeUniforms {
    /// Inverse of projection * view, for the depth → world reconstruction.
    inv_view_proj: [[f32; 4]; 4],
    /// Sun direction (unit, pointing from the surface toward the light).
    sun: [f32; 3],
    world_span: f32,
    /// Height exaggeration, matching the terrain mesh.
    z_scale: f32,
    /// How strongly the imagery replaces the terrain's own colour.
    strength: f32,
    _pad: [f32; 2],
}

const SHADER: &str = r#"
struct Uniforms {
    inv_view_proj: mat4x4<f32>,
    sun: vec3<f32>,
    world_span: f32,
    z_scale: f32,
    strength: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var imagery: texture_2d<f32>;
@group(0) @binding(2) var imagery_sampler: sampler;
@group(0) @binding(3) var<uniform> uniforms: Uniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.position = vec4<f32>(corners[index], 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(input.position.xy);
    let d = textureLoad(depth, pixel, 0);
    // Sky: nothing to drape.
    if (d >= 0.9999) {
        discard;
    }
    let size = vec2<f32>(textureDimensions(depth));
    let ndc = vec2<f32>(
        (f32(pixel.x) + 0.5) / size.x * 2.0 - 1.0,
        1.0 - (f32(pixel.y) + 0.5) / size.y * 2.0,
    );
    // World position of this surface sample, with the 1/w divide.
    let clip = vec4<f32>(ndc, d, 1.0);
    let world4 = uniforms.inv_view_proj * clip;
    if (abs(world4.w) < 1e-6) {
        discard;
    }
    let world = world4.xyz / world4.w;
    let uv = world.xy / uniforms.world_span + vec2<f32>(0.5, 0.5);

    let texel = textureSample(imagery, imagery_sampler, vec2<f32>(uv.x, 1.0 - uv.y));
    // Real surface normal from the reconstructed position's screen derivatives,
    // so draped imagery keeps the relief shading the geometry implies instead
    // of looking like a flat decal.
    let normal = normalize(cross(dpdx(world), dpdy(world)));
    let lambert = clamp(abs(dot(normal, normalize(uniforms.sun))) * 0.55 + 0.62, 0.35, 1.25);
    let draped = texel.rgb * lambert * 1.1;
    return vec4<f32>(draped, uniforms.strength);
}
"#;

/// Imagery plus the pass that drapes it over the terrain frame.
pub struct DrapePass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    texture: Option<wgpu::Texture>,
    view: Option<wgpu::TextureView>,
    bind_group: Option<wgpu::BindGroup>,
}

impl DrapePass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("studio.drape.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio.drape.uniforms"),
            contents: bytemuck::bytes_of(&DrapeUniforms {
                inv_view_proj: [[0.0; 4]; 4],
                sun: [0.0, 0.0, 1.0],
                world_span: 1.0,
                z_scale: 1.0,
                strength: 1.0,
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("studio.drape.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio.drape.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("studio.drape.layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("studio.drape.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            // No depth state: this pass only *reads* depth, it must not fight
            // the geometry it was built from.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        Self {
            pipeline,
            layout,
            uniforms,
            sampler,
            texture: None,
            view: None,
            bind_group: None,
        }
    }

    /// Upload RGBA8 imagery; the texture is what gets draped.
    pub fn set_imagery(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        let size = wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("studio.drape.imagery"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        self.view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.texture = Some(texture);
        self.bind_group = None;
    }

    pub fn has_imagery(&self) -> bool {
        self.view.is_some()
    }

    /// Bind the frame's depth view and this pass's imagery.
    pub fn bind(&mut self, device: &wgpu::Device, depth_view: &wgpu::TextureView) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("studio.drape.bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniforms.as_entire_binding(),
                },
            ],
        }));
    }

    /// Drape into the frame's colour target.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        camera: &CameraParams,
        size: (u32, u32),
        world_span: f32,
        z_scale: f32,
        sun_azimuth_deg: f32,
        sun_elevation_deg: f32,
        strength: f32,
    ) {
        let view_proj = glam::Mat4::from_cols_array_2d(&view_projection(camera, size.0, size.1));
        let inv = view_proj.inverse();
        let azimuth = sun_azimuth_deg.to_radians();
        let elevation = sun_elevation_deg.to_radians();
        let sun = [
            elevation.cos() * azimuth.cos(),
            elevation.cos() * azimuth.sin(),
            elevation.sin(),
        ];
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&DrapeUniforms {
                inv_view_proj: inv.to_cols_array_2d(),
                sun,
                world_span: world_span.max(1.0),
                z_scale,
                strength: strength.clamp(0.0, 1.0),
                _pad: [0.0; 2],
            }),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("studio.drape.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        if let Some(bind_group) = self.bind_group.as_ref() {
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// Decode a GeoTIFF or PNG raster into RGBA8 for draping.
///
/// Rasters in this tool are already registered to the DEM's footprint by the
/// time they get here (the fetch script writes them with the DEM's bounds), so
/// the pixels are used as a straight UV lookup; only the alpha channel from a
/// PNG is honoured.
pub fn decode_imagery(path: &std::path::Path) -> Result<(u32, u32, Vec<u8>), String> {
    let image = image::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let rgba = image.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}
