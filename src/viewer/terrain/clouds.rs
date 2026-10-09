// src/viewer/terrain/clouds.rs
//! Viewer-side volumetric clouds (linear-HDR).
//!
//! Reuses the offline deterministic cloud raymarch
//! (`core::cloud_volume::CloudVolumeResources`), which composites
//! `beauty * transmittance + inscatter` in linear HDR. The interactive viewer
//! therefore renders its terrain into an `Rgba16Float` beauty when clouds are
//! enabled, runs the shared cloud pass, then tonemaps once into the display
//! target. See `src/shaders/viewer_tonemap.wgsl`.

use crate::core::resource_tracker::{
    tracked_create_buffer, TrackedBuffer,
};
use crate::core::cloud_volume::{CloudFrameParams, CloudRenderSettings, CloudVolumeResources};
use super::ViewerTerrainScene;
use std::sync::Mutex;
use wgpu::{
    BindGroupLayout, BindGroupLayoutEntry, BindingType, BufferDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, Device, FragmentState, MultisampleState,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, Queue, RenderPassDescriptor,
    RenderPipeline, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, TextureFormat, TextureSampleType, TextureView, TextureViewDimension, VertexState,
};

/// Visible-cloud settings for the interactive viewer.
#[derive(Debug, Clone)]
pub struct ViewerCloudConfig {
    pub enabled: bool,
    pub coverage: f32,
    pub density: f32,
    pub altitude_m: f32,
    pub thickness_m: f32,
    pub scatter_strength: f32,
    pub phase_g: f32,
    pub detail: f32,
    pub powder: f32,
    pub wind_dir_deg: f32,
    pub wind_speed: f32,
    pub shadow_strength: f32,
    pub size: f32,
    pub weather_strength: f32,
    pub weather_map: Option<String>,
}

impl Default for ViewerCloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            coverage: 0.5,
            density: 0.6,
            altitude_m: 900.0,
            thickness_m: 1200.0,
            scatter_strength: 8.0,
            phase_g: 0.6,
            detail: 0.5,
            powder: 0.5,
            wind_dir_deg: 30.0,
            wind_speed: 0.0,
            shadow_strength: 0.5,
            size: 2.0,
            weather_strength: 0.6,
            weather_map: None,
        }
    }
}

/// Per-frame camera/sun inputs for the viewer cloud raymarch.
pub(crate) struct ViewerCloudCamera {
    pub inv_view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub sun_direction: [f32; 3],
    pub sun_radiance: [f32; 3],
    pub up_axis: [f32; 3],
    /// `1 / terrain_span` — normalizes noise sampling to the scene scale.
    pub noise_scale: f32,
    /// Cloud-field centre in world space (ground projection of the eye).
    pub bounds_center: [f32; 3],
    /// Horizontal extent radius of the cloud field, in world units.
    pub extent_radius: f32,
    pub time: f32,
}

pub(crate) struct ViewerCloudRenderer {
    tonemap_format: TextureFormat,
    volume: CloudVolumeResources,
    tonemap_pipeline: RenderPipeline,
    tonemap_bind_group_layout: BindGroupLayout,
    tonemap_sampler: Sampler,
    _scratch: TrackedBuffer,
    /// Cached tonemap bind group keyed by the identity of the HDR view it
    /// binds; rebuilt only when the render target changes (resize), so
    /// routine frames allocate nothing.
    tonemap_bind_group: Mutex<Option<(usize, wgpu::BindGroup)>>,
}

impl ViewerCloudRenderer {
    pub(crate) fn new(
        device: &Device,
        queue: &Queue,
        tonemap_format: TextureFormat,
    ) -> Result<Self, String> {
        let volume = CloudVolumeResources::new(device, queue).map_err(|e| e.to_string())?;

        let tonemap_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("viewer.clouds.tonemap_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let tonemap_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("viewer.clouds.tonemap_bgl"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Float { filterable: true },
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let shader = crate::core::shader_registry::create_labeled_shader_module(
            device,
            "viewer.clouds.tonemap.shader",
            include_str!("../../shaders/viewer_tonemap.wgsl"),
        );
        let tonemap_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("viewer.clouds.tonemap.pipeline_layout"),
            bind_group_layouts: &[&tonemap_bind_group_layout],
            push_constant_ranges: &[],
        });
        let tonemap_pipeline = crate::core::shader_registry::create_render_pipeline_scoped(
            device,
            &RenderPipelineDescriptor {
                label: Some("viewer.clouds.tonemap.pipeline"),
                layout: Some(&tonemap_pipeline_layout),
                vertex: VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                },
                fragment: Some(FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(ColorTargetState {
                        format: tonemap_format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview: None,
            },
        );
        let _scratch = tracked_create_buffer(
            device,
            &BufferDescriptor {
                label: Some("viewer.clouds.scratch"),
                size: 16,
                usage: BufferUsages::UNIFORM,
                mapped_at_creation: false,
            },
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            tonemap_format,
            volume,
            tonemap_pipeline,
            tonemap_bind_group_layout,
            tonemap_sampler,
            _scratch,
            tonemap_bind_group: Mutex::new(None),
        })
    }

    pub(crate) fn matches_format(&self, format: TextureFormat) -> bool {
        self.tonemap_format == format
    }

    /// Composite clouds into `hdr_texture` (linear HDR) and tonemap the result
    /// into `color_view`. `depth_view` must be a single-sample bindable depth.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &self,
        device: &Device,
        queue: &Queue,
        encoder: &mut wgpu::CommandEncoder,
        hdr_texture: &wgpu::Texture,
        hdr_view: &TextureView,
        depth_view: &TextureView,
        color_view: &TextureView,
        width: u32,
        height: u32,
        config: &ViewerCloudConfig,
        camera: ViewerCloudCamera,
    ) -> Result<bool, String> {
        let settings = CloudRenderSettings {
            enabled: true,
            coverage: config.coverage,
            density: config.density,
            altitude_m: config.altitude_m,
            thickness_m: config.thickness_m,
            scatter_strength: config.scatter_strength,
            phase_g: config.phase_g,
            detail: config.detail,
            powder: config.powder,
            wind_dir_deg: config.wind_dir_deg,
            wind_speed: config.wind_speed,
            time_seconds: camera.time,
            shadow_strength: config.shadow_strength,
            size: config.size,
            weather_strength: config.weather_strength,
            weather_map: config.weather_map.clone(),
        };
        let frame = CloudFrameParams {
            inv_view_proj: camera.inv_view_proj,
            camera_pos: camera.eye,
            sun_direction: camera.sun_direction,
            sun_radiance: camera.sun_radiance,
            up_axis: camera.up_axis,
            noise_scale: camera.noise_scale,
            bounds_center: camera.bounds_center,
            extent_radius: camera.extent_radius,
            sample_index: 0,
        };
        let rendered = self
            .volume
            .render(
                device,
                queue,
                encoder,
                hdr_view,
                hdr_texture,
                depth_view,
                width,
                height,
                &settings,
                frame,
            )
            .map_err(|e| e.to_string())?;
        if rendered {
            crate::core::shader_registry::record_shader_use("terrain.clouds.shader");
        }

        // Tonemap the composited linear-HDR beauty into the display target.
        // The bind group's only varying input is the (stable, stored) HDR view,
        // so it is cached by that view's identity and rebuilt on resize.
        let hdr_view_key = std::ptr::from_ref(hdr_view) as usize;
        let mut tonemap_guard = self
            .tonemap_bind_group
            .lock()
            .map_err(|_| "viewer cloud tonemap bind group mutex poisoned".to_string())?;
        let tonemap_stale = tonemap_guard
            .as_ref()
            .map(|(key, _)| *key != hdr_view_key)
            .unwrap_or(true);
        if tonemap_stale {
            *tonemap_guard = Some((
                hdr_view_key,
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("viewer.clouds.tonemap.bg"),
                    layout: &self.tonemap_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(hdr_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.tonemap_sampler),
                        },
                    ],
                }),
            ));
        }
        let tonemap_bind_group = &tonemap_guard
            .as_ref()
            .expect("viewer cloud tonemap bind group present after cache fill")
            .1;
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("viewer.clouds.tonemap.pass"),
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
            crate::core::shader_registry::record_shader_use("viewer.clouds.tonemap.shader");
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, tonemap_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok(true)
    }
}

impl ViewerTerrainScene {
    /// Apply `set_terrain_clouds` from IPC. Any `None` field is left unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_terrain_clouds(
        &mut self,
        enabled: Option<bool>,
        coverage: Option<f32>,
        density: Option<f32>,
        altitude_m: Option<f32>,
        thickness_m: Option<f32>,
        scatter_strength: Option<f32>,
        phase_g: Option<f32>,
        detail: Option<f32>,
        powder: Option<f32>,
        wind_dir_deg: Option<f32>,
        wind_speed: Option<f32>,
    ) {
        let clouds = &mut self.pbr_config.clouds;
        if let Some(value) = enabled {
            clouds.enabled = value;
        }
        if let Some(value) = coverage {
            clouds.coverage = value.clamp(0.0, 1.0);
        }
        if let Some(value) = density {
            clouds.density = value.clamp(0.0, 2.0);
        }
        if let Some(value) = altitude_m {
            clouds.altitude_m = value.max(0.0);
        }
        if let Some(value) = thickness_m {
            clouds.thickness_m = value.max(1.0);
        }
        if let Some(value) = scatter_strength {
            clouds.scatter_strength = value.max(0.0);
        }
        if let Some(value) = phase_g {
            clouds.phase_g = value.clamp(-0.99, 0.99);
        }
        if let Some(value) = detail {
            clouds.detail = value.clamp(0.0, 1.0);
        }
        if let Some(value) = powder {
            clouds.powder = value.clamp(0.0, 2.0);
        }
        if let Some(value) = wind_dir_deg {
            clouds.wind_dir_deg = value;
        }
        if let Some(value) = wind_speed {
            clouds.wind_speed = value.max(0.0);
        }
    }

    fn ensure_cloud_renderer(&mut self, format: TextureFormat) -> bool {
        let needs_recreate = self
            .cloud_renderer
            .as_ref()
            .map(|renderer| !renderer.matches_format(format))
            .unwrap_or(true);
        if needs_recreate {
            match ViewerCloudRenderer::new(self.device.as_ref(), self.queue.as_ref(), format) {
                Ok(renderer) => self.cloud_renderer = Some(renderer),
                Err(error) => {
                    eprintln!("[terrain] viewer cloud resource init failed: {error}");
                    return false;
                }
            }
        }
        self.cloud_renderer.is_some()
    }

    /// Composite the viewer cloud layer over the linear-HDR snapshot beauty.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_viewer_clouds(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        hdr_texture: &wgpu::Texture,
        hdr_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        color_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        format: TextureFormat,
        view_proj: glam::Mat4,
        eye: glam::Vec3,
        sun_dir: glam::Vec3,
        sun_radiance: [f32; 3],
        terrain_span: f32,
        time: f32,
    ) -> bool {
        if !self.pbr_config.clouds.enabled {
            return false;
        }
        if !self.ensure_cloud_renderer(format) {
            return false;
        }
        let Some(renderer) = self.cloud_renderer.as_ref() else {
            return false;
        };
        let up = glam::Vec3::Z;
        let camera = ViewerCloudCamera {
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            eye: eye.to_array(),
            sun_direction: sun_dir.to_array(),
            sun_radiance,
            // The viewer terrain is geographic with +Z up, so the cloud slab is
            // measured along +Z (matching the offline `mesh:zup` path).
            up_axis: up.to_array(),
            noise_scale: if terrain_span > 1.0 {
                1.0 / terrain_span
            } else {
                1.0
            },
            bounds_center: (eye - up * eye.dot(up)).to_array(),
            extent_radius: terrain_span.max(1.0),
            time,
        };
        match renderer.render(
            self.device.as_ref(),
            self.queue.as_ref(),
            encoder,
            hdr_texture,
            hdr_view,
            depth_view,
            color_view,
            width,
            height,
            &self.pbr_config.clouds,
            camera,
        ) {
            Ok(rendered) => rendered,
            Err(error) => {
                eprintln!("[terrain] viewer cloud pass failed: {error}");
                false
            }
        }
    }
}
