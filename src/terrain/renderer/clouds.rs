// src/terrain/renderer/clouds.rs
//! Depth-aware volumetric cloud layer.
//!
//! Owns the deterministic 3D noise volumes and the raymarch compute pass that
//! composites clouds into the terrain linear-HDR beauty after the terrain pass
//! and before accumulation/tonemap. When `decoded.clouds.enabled` is false the
//! pass is never dispatched, so output is byte-identical to a cloudless build.
use super::*;
use crate::core::resource_tracker::{
    tracked_create_buffer, tracked_create_texture, TrackedBuffer, TrackedTexture,
};

pub(crate) const CLOUD_BASE_NOISE_RESOLUTION: u32 = 128;
pub(crate) const CLOUD_DETAIL_NOISE_RESOLUTION: u32 = 32;
pub(crate) const CLOUD_WORLEY_NOISE_RESOLUTION: u32 = 64;
const COPY_ROW_ALIGNMENT: u32 = 256;

#[repr(C, align(16))]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CloudUniforms {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    sun_direction: [f32; 4],
    sun_radiance: [f32; 4],
    layer: [f32; 4],
    optics: [f32; 4],
    wind: [f32; 4],
    screen: [f32; 4],
    up_axis: [f32; 4],
}

/// Per-frame camera/sun inputs for the cloud raymarch.
pub(crate) struct CloudFrameParams {
    pub inv_view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub sun_direction: [f32; 3],
    pub sun_radiance: [f32; 3],
    /// World up axis of the camera frame: `Y` for screen/north, `Z` for Z-up mesh.
    pub up_axis: [f32; 3],
    /// `1 / terrain_span`: normalizes noise sampling to the scene scale.
    pub noise_scale: f32,
    pub sample_index: u32,
}

/// Shape/optics inputs for the cloud raymarch, independent of the offline
/// `DecodedTerrainSettings` so the interactive viewer can drive the same pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CloudRenderSettings {
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
    pub time_seconds: f32,
}

struct CloudOutput {
    texture: TrackedTexture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

pub(crate) struct CloudVolumeResources {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    uniform_buffer: TrackedBuffer,
    _base_noise: TrackedTexture,
    _detail_noise: TrackedTexture,
    _worley_noise: TrackedTexture,
    base_view: wgpu::TextureView,
    detail_view: wgpu::TextureView,
    worley_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    output: Mutex<Option<CloudOutput>>,
}

/// Deterministic 3D value hash in `[0, 1)`.
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(0x1f1f_1f1f)
        ^ (y as u32).wrapping_mul(0x8da6_b343)
        ^ (z as u32).wrapping_mul(0xd816_3841)
        ^ seed;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h as f32) * (1.0 / 4_294_967_296.0)
}

fn smoothstep_unit(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Seamlessly tiling trilinear value noise over a lattice of `period` cells.
fn value_noise(x: f32, y: f32, z: f32, period: i32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let zi = z.floor() as i32;
    let xf = smoothstep_unit(x - xi as f32);
    let yf = smoothstep_unit(y - yi as f32);
    let zf = smoothstep_unit(z - zi as f32);
    let corner = |dx: i32, dy: i32, dz: i32| {
        hash3(
            (xi + dx).rem_euclid(period),
            (yi + dy).rem_euclid(period),
            (zi + dz).rem_euclid(period),
            seed,
        )
    };
    let c00 = corner(0, 0, 0) + (corner(1, 0, 0) - corner(0, 0, 0)) * xf;
    let c10 = corner(0, 1, 0) + (corner(1, 1, 0) - corner(0, 1, 0)) * xf;
    let c01 = corner(0, 0, 1) + (corner(1, 0, 1) - corner(0, 0, 1)) * xf;
    let c11 = corner(0, 1, 1) + (corner(1, 1, 1) - corner(0, 1, 1)) * xf;
    let c0 = c00 + (c10 - c00) * yf;
    let c1 = c01 + (c11 - c01) * yf;
    c0 + (c1 - c0) * zf
}

/// Tiling FBM volume in `[0, 1]`. `ridge` selects the wispy erosion shape used
/// for the detail volume; otherwise it is a plain FBM shape.
pub(crate) fn build_noise(
    resolution: u32,
    octaves: u32,
    freq0: f32,
    seed: u32,
    ridge: bool,
) -> Vec<u8> {
    let res = resolution.max(1);
    let inv_res = 1.0 / res as f32;
    let mut data = vec![0u8; (res * res * res) as usize];
    for z in 0..res {
        for y in 0..res {
            for x in 0..res {
                let mut amp = 0.5f32;
                let mut freq = freq0;
                let mut sum = 0.0f32;
                let mut norm = 0.0f32;
                for octave in 0..octaves {
                    let n = value_noise(
                        x as f32 * inv_res * freq,
                        y as f32 * inv_res * freq,
                        z as f32 * inv_res * freq,
                        freq as i32,
                        seed.wrapping_add(octave.wrapping_mul(0x9e37_79b9)),
                    );
                    let shaped = if ridge { 1.0 - (2.0 * n - 1.0).abs() } else { n };
                    sum += shaped * amp;
                    norm += amp;
                    amp *= 0.5;
                    freq *= 2.0;
                }
                let value = (sum / norm.max(1e-6)).clamp(0.0, 1.0);
                data[(z * res * res + y * res + x) as usize] = (value * 255.0 + 0.5) as u8;
            }
        }
    }
    data
}

/// Seamlessly tiling Worley (F1 cellular) volume in `[0, 1]`; 1 at cell centres.
/// The feature lattice is periodic (`cells` per axis) so the volume repeats.
pub(crate) fn build_worley(resolution: u32, cells: i32, seed: u32) -> Vec<u8> {
    let res = resolution.max(1);
    let inv = 1.0 / res as f32;
    let cells = cells.max(1);
    let mut data = vec![0u8; (res * res * res) as usize];
    for z in 0..res {
        for y in 0..res {
            for x in 0..res {
                let p = glam::Vec3::new(x as f32 * inv, y as f32 * inv, z as f32 * inv)
                    * cells as f32;
                let ip = p.floor();
                let mut f1 = f32::MAX;
                for dz in -1..=1 {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let cx = ip.x as i32 + dx;
                            let cy = ip.y as i32 + dy;
                            let cz = ip.z as i32 + dz;
                            let wx = cx.rem_euclid(cells);
                            let wy = cy.rem_euclid(cells);
                            let wz = cz.rem_euclid(cells);
                            let hx = hash3(wx, wy, wz, seed);
                            let hy = hash3(wx, wy, wz, seed ^ 0x9e37_79b9);
                            let hz = hash3(wx, wy, wz, seed ^ 0x85eb_ca6b);
                            let feature = glam::Vec3::new(
                                cx as f32 + hx,
                                cy as f32 + hy,
                                cz as f32 + hz,
                            );
                            f1 = f1.min((feature - p).length());
                        }
                    }
                }
                let value = (1.0 - f1).clamp(0.0, 1.0);
                data[(z * res * res + y * res + x) as usize] = (value * 255.0 + 0.5) as u8;
            }
        }
    }
    data
}

/// Pad each volumetric row to `COPY_ROW_ALIGNMENT`, returning `(data, bytes_per_row)`.
pub(crate) fn pad_volume_rows(data: &[u8], resolution: u32) -> (Vec<u8>, u32) {
    let res = resolution.max(1);
    let bytes_per_row = ((res + COPY_ROW_ALIGNMENT - 1) / COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT;
    let mut padded = vec![0u8; (bytes_per_row * res * res) as usize];
    for z in 0..res {
        for y in 0..res {
            let src = ((z * res + y) * res) as usize;
            let dst = ((z * res + y) * bytes_per_row) as usize;
            padded[dst..dst + res as usize]
                .copy_from_slice(&data[src..src + res as usize]);
        }
    }
    (padded, bytes_per_row)
}

pub(crate) fn create_noise_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    resolution: u32,
    data: &[u8],
) -> Result<(TrackedTexture, wgpu::TextureView)> {
    let texture = tracked_create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: resolution,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    )?;
    let (padded, bytes_per_row) = pad_volume_rows(data, resolution);
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(resolution),
        },
        wgpu::Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: resolution,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok((texture, view))
}

impl CloudVolumeResources {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        let base_data = build_noise(CLOUD_BASE_NOISE_RESOLUTION, 4, 4.0, 0x0, false);
        let detail_data = build_noise(CLOUD_DETAIL_NOISE_RESOLUTION, 3, 6.0, 0x1234_5678, true);
        let worley_data = build_worley(CLOUD_WORLEY_NOISE_RESOLUTION, 4, 0x51ed_2701);
        let (_base_noise, base_view) = create_noise_texture(
            device,
            queue,
            "terrain.clouds.base_noise",
            CLOUD_BASE_NOISE_RESOLUTION,
            &base_data,
        )?;
        let (_detail_noise, detail_view) = create_noise_texture(
            device,
            queue,
            "terrain.clouds.detail_noise",
            CLOUD_DETAIL_NOISE_RESOLUTION,
            &detail_data,
        )?;
        let (_worley_noise, worley_view) = create_noise_texture(
            device,
            queue,
            "terrain.clouds.worley_noise",
            CLOUD_WORLEY_NOISE_RESOLUTION,
            &worley_data,
        )?;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain.clouds.sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buffer = tracked_create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("terrain.clouds.uniform_buffer"),
                size: std::mem::size_of::<CloudUniforms>() as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )?;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain.clouds.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_3d_entry(1),
                sampler_entry(2),
                texture_3d_entry(3),
                sampler_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                texture_3d_entry(8),
            ],
        });

        let shader = crate::core::shader_registry::create_labeled_shader_module(
            device,
            "terrain.clouds.shader",
            &crate::shader_sources::atmosphere_clouds(),
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain.clouds.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = crate::core::shader_registry::try_create_compute_pipeline_scoped(
            device,
            &wgpu::ComputePipelineDescriptor {
                label: Some("terrain.clouds.pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "clouds_main",
            },
        )
        .map_err(|message| anyhow!("terrain.clouds.pipeline: {message}"))?;

        Ok(Self {
            bind_group_layout,
            pipeline,
            uniform_buffer,
            _base_noise,
            _detail_noise,
            _worley_noise,
            base_view,
            detail_view,
            worley_view,
            sampler,
            output: Mutex::new(None),
        })
    }

    /// Dispatch the cloud composite into `beauty_texture`. Returns `true` when a
    /// pass ran, so the caller can account shader use and GPU timing.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        beauty_view: &wgpu::TextureView,
        beauty_texture: &wgpu::Texture,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        clouds: &CloudRenderSettings,
        frame: CloudFrameParams,
    ) -> Result<bool> {
        if !clouds.enabled || width == 0 || height == 0 {
            return Ok(false);
        }
        // The composite copies an Rgba16Float storage texture back into the
        // beauty target, so a non-HDR target cannot carry clouds.
        if beauty_texture.format() != wgpu::TextureFormat::Rgba16Float {
            crate::core::degradation::record_degradation(
                "rendering_fallback",
                "terrain_clouds_require_hdr_beauty",
                "volumetric clouds composite into the linear-HDR beauty target; a non-HDR target renders no clouds",
            );
            return Ok(false);
        }

        let mut output_guard = self
            .output
            .lock()
            .map_err(|_| anyhow!("terrain.clouds.output mutex poisoned"))?;
        if output_guard
            .as_ref()
            .map(|output| output.width != width || output.height != height)
            .unwrap_or(true)
        {
            let texture = tracked_create_texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some("terrain.clouds.output"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
            )?;
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            *output_guard = Some(CloudOutput {
                texture,
                view,
                width,
                height,
            });
        }
        let output = output_guard.as_ref().expect("cloud output present");

        let sun_dir = glam::Vec3::from_array(frame.sun_direction).normalize_or_zero();
        let uniforms = CloudUniforms {
            inv_view_proj: frame.inv_view_proj,
            camera_pos: [frame.camera_pos[0], frame.camera_pos[1], frame.camera_pos[2], 0.0],
            sun_direction: [sun_dir.x, sun_dir.y, sun_dir.z, 0.0],
            sun_radiance: [
                frame.sun_radiance[0],
                frame.sun_radiance[1],
                frame.sun_radiance[2],
                0.0,
            ],
            layer: [clouds.altitude_m, clouds.thickness_m, clouds.coverage, clouds.density],
            optics: [
                clouds.scatter_strength,
                clouds.phase_g,
                clouds.detail,
                clouds.powder,
            ],
            wind: [
                clouds.wind_dir_deg.to_radians(),
                clouds.wind_speed,
                clouds.time_seconds,
                frame.sample_index as f32,
            ],
            screen: [width as f32, height as f32, 1.0, 1.0],
            up_axis: [
                frame.up_axis[0],
                frame.up_axis[1],
                frame.up_axis[2],
                frame.noise_scale,
            ],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain.clouds.bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.base_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.detail_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(beauty_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&output.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&self.worley_view),
                },
            ],
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("terrain.clouds.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups((width + 7) / 8, (height + 7) / 8, 1);
        }

        encoder.copy_texture_to_texture(
            wgpu::ImageCopyTexture {
                texture: &output.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyTexture {
                texture: beauty_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        Ok(true)
    }
}

fn texture_3d_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_in_range() {
        let first = build_noise(CLOUD_BASE_NOISE_RESOLUTION, 4, 1.0, 0x0, false);
        let second = build_noise(CLOUD_BASE_NOISE_RESOLUTION, 4, 1.0, 0x0, false);
        assert_eq!(first, second);
        assert_eq!(first.len(), (CLOUD_BASE_NOISE_RESOLUTION.pow(3)) as usize);
        assert!(first.iter().any(|&value| value > 0));

        let ridged = build_noise(CLOUD_DETAIL_NOISE_RESOLUTION, 3, 2.0, 0x1234_5678, true);
        assert_eq!(
            ridged.len(),
            (CLOUD_DETAIL_NOISE_RESOLUTION.pow(3)) as usize
        );
        assert!(ridged.iter().any(|&value| value > 0));
    }

    #[test]
    fn padded_volume_rows_meet_copy_alignment() {
        let data = build_noise(8, 2, 1.0, 0x0, false);
        let (padded, bytes_per_row) = pad_volume_rows(&data, 8);
        assert_eq!(bytes_per_row % COPY_ROW_ALIGNMENT, 0);
        assert_eq!(padded.len(), (bytes_per_row * 8 * 8) as usize);
    }
}
