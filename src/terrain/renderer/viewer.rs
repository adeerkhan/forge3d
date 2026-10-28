use super::*;
use crate::core::resource_tracker::{
    tracked_create_buffer_init, tracked_create_texture, TrackedBuffer,
    TrackedTexture,
};

/// STUDIO-FORK (M9/R1): Studio's default relief ramp, used when the active
/// document names no colormap. The bytes are sRGB-encoded and uploaded into an
/// `Rgba8Unorm` texture — the same legacy treatment
/// `ColormapLUT::new_single_palette` applies by default, so the viewport and
/// the exported frame sample identically encoded ramps.
const STUDIO_RELIEF_STOPS: [(f32, [u8; 3]); 4] = [
    (0.0, [0x18, 0x39, 0x1f]),
    (0.40, [0x4e, 0x7c, 0x35]),
    (0.72, [0x9a, 0x85, 0x52]),
    (1.0, [0xf3, 0xf5, 0xf9]),
];

/// Interpolate [`STUDIO_RELIEF_STOPS`] into the 256x1 RGBA ramp the LUT
/// texture expects.
fn relief_lut_bytes() -> Vec<u8> {
    let mut lut = Vec::with_capacity(256 * 4);
    for i in 0..256u32 {
        let t = i as f32 / 255.0;
        let mut rgb = STUDIO_RELIEF_STOPS[STUDIO_RELIEF_STOPS.len() - 1].1;
        for window in STUDIO_RELIEF_STOPS.windows(2) {
            if t >= window[0].0 && t <= window[1].0 {
                let span = (window[1].0 - window[0].0).max(1e-6);
                let f = (t - window[0].0) / span;
                rgb = [
                    (window[0].1[0] as f32 + f * (window[1].1[0] as f32 - window[0].1[0] as f32))
                        .round() as u8,
                    (window[0].1[1] as f32 + f * (window[1].1[1] as f32 - window[0].1[1] as f32))
                        .round() as u8,
                    (window[0].1[2] as f32 + f * (window[1].1[2] as f32 - window[0].1[2] as f32))
                        .round() as u8,
                ];
                break;
            }
        }
        lut.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    lut
}

/// An immutable copy of the viewer terrain's scalars.
///
/// `render_viewer_terrain` needs `&mut self` to refresh its resource cache, so
/// it snapshots the viewer state first instead of holding a borrow of
/// `viewer_heightmap` for the whole frame.
struct ViewerSnapshot {
    world_span: f32,
    z_scale: f32,
    cam_target: [f32; 2],
    cam_radius: f32,
    cam_phi_deg: f32,
    cam_theta_deg: f32,
    cam_fov_deg: f32,
    sun_azimuth_deg: f32,
    sun_elevation_deg: f32,
    sun_intensity: f32,
    domain: (f32, f32),
}

/// Everything a viewer frame binds, built once and reused until the terrain,
/// the colormap or the target size changes. An orbit therefore costs two
/// uniform-buffer writes and one render pass instead of re-allocating the
/// fallback textures, the depth target and five bind groups every frame.
pub(super) struct ViewerResources {
    size: (u32, u32),
    uniforms: TrackedBuffer,
    shading: TrackedBuffer,
    /// Bound once into `main`; kept so the ledger accounts for it while the
    /// bind group lives.
    _overlay: TrackedBuffer,
    _depth: TrackedTexture,
    depth_view: wgpu::TextureView,
    /// Fallback textures whose views the cached bind groups reference.
    _material_fallback: TrackedTexture,
    _material_layer_fallback: TrackedTexture,
    _ibl_cube: TrackedTexture,
    _ibl_lut: TrackedTexture,
    _ibl_uniforms: TrackedBuffer,
    main: wgpu::BindGroup,
    ibl: wgpu::BindGroup,
    fog: wgpu::BindGroup,
    water: wgpu::BindGroup,
    material_layer: wgpu::BindGroup,
}

/// Direction toward the sun in the terrain shader's Z-up world.
///
/// The live path derives its light direction the same way
/// (`render_params::decode_lighting`): azimuth 0 = +X, 90 = +Y, elevation on
/// +Z. The previous viewer used a Y-up vector, so its sun sat in the ground
/// plane of a Z-up world and the terrain lit from a direction the shader never
/// produced.
fn sun_direction(azimuth_deg: f32, elevation_deg: f32) -> glam::Vec3 {
    let az = azimuth_deg.to_radians();
    let el = elevation_deg.to_radians();
    glam::Vec3::new(el.cos() * az.cos(), el.cos() * az.sin(), el.sin()).normalize()
}

impl ViewerSnapshot {
    /// Per-frame `[TerrainUniforms; 44 f32]` and `[TerrainShadingUniforms; 44 f32]`.
    fn frame_uniforms(&self, width: u32, height: u32) -> ([f32; 44], [f32; 44]) {
        let phi_rad = self.cam_phi_deg.to_radians();
        let theta_rad = self.cam_theta_deg.to_radians();
        let radius = self.cam_radius;
        let terrain_span = self.world_span.max(1e-3);

        // STUDIO-FORK (M9/R1): the live shader's mesh mode builds a grid in the
        // XY plane with elevation along +Z, centred on the origin. The camera
        // must orbit that same Z-up world; the previous Y-up orbit put the eye
        // above a horizontal plane the shader never produces.
        let target = glam::Vec3::new(self.cam_target[0], self.cam_target[1], 0.0);
        let eye = target
            + glam::Vec3::new(
                radius * theta_rad.sin() * phi_rad.cos(),
                radius * theta_rad.sin() * phi_rad.sin(),
                radius * theta_rad.cos(),
            );
        let up = if theta_rad.sin().abs() < 1e-4 {
            glam::Vec3::Y
        } else {
            glam::Vec3::Z
        };
        let view = glam::Mat4::look_at_rh(eye, target, up);
        let aspect = width as f32 / height as f32;
        let clip_near = (radius * 0.01).max(0.1);
        let clip_far = (radius * 20.0).max(clip_near + 1.0);
        let proj = glam::Mat4::perspective_rh(
            self.cam_fov_deg.to_radians(),
            aspect,
            clip_near,
            clip_far,
        );

        // Mesh grid resolution scales with the target so a large viewport keeps
        // its silhouette. The vertex stage emits 6 vertices per quad and reads
        // no vertex buffer, so the draw range must match `grid_size`.
        let grid_size = width.max(height).clamp(128, 768);

        let sun_dir = sun_direction(self.sun_azimuth_deg, self.sun_elevation_deg);

        // STUDIO-FORK (M9/R1): mesh geometry. The live shader's `vs_main`
        // rebuilds the terrain grid procedurally from `vertex_index`; screen
        // mode is a 1-unit fullscreen quad that is invisible to a perspective
        // camera. `spacing` is the horizontal extent in world units, `z_scale`
        // is the vertical exaggeration applied to the (absolute) DEM heights.
        let mut uniforms = [0.0f32; 44];
        let v = view.to_cols_array_2d();
        for col in 0..4 {
            for row in 0..4 {
                uniforms[col * 4 + row] = v[col][row];
            }
        }
        let p = proj.to_cols_array_2d();
        for col in 0..4 {
            for row in 0..4 {
                uniforms[16 + col * 4 + row] = p[col][row];
            }
        }
        uniforms[32..36].copy_from_slice(&[
            sun_dir.x,
            sun_dir.y,
            sun_dir.z,
            self.sun_intensity,
        ]);
        uniforms[36..40].copy_from_slice(&[terrain_span, terrain_span, self.z_scale, 1.0]);
        uniforms[40..44].copy_from_slice(&[1.0, grid_size as f32, clip_near, clip_far]);

        // STUDIO-FORK (M9/R1): binding 5 is TerrainShadingUniforms (44 f32);
        // the pre-drift viewer fed view/proj bytes into it. Write honest viewer
        // values: no triplanar/POM, one material layer, sun-tinted light,
        // colormap albedo with a strength-1 alpha LUT overlay.
        let sun_tinted = self.sun_intensity.max(0.0);
        let (h_min, h_max) = self.domain;
        let shading: [f32; 44] = [
            1.0, 0.0, 1.0, 0.0, // triplanar: scale, blend, normal_strength, pom_scale (0 = off)
            0.0, 0.0, 0.0, 0.0, // pom steps/flags: disabled
            0.0, 0.33, 0.66, 1.0, // layer heights
            0.85, 1.0, 1.0, 1.0, // layer roughness
            0.0, 0.0, 0.0, 0.0, // layer metallic
            1.0, 1.0, 0.0, 0.0, // layer count, blend half-width, lod biases
            sun_tinted, sun_tinted, sun_tinted, 1.0, // light color * intensity, exposure
            h_min, h_max, 0.0, 90.0, // clamp0: height min/max (raw DEM units), slope min/max
            0.02, 0.45, 0.0, 1.0, // clamp1: ambient, shadow
            0.0, 1.0, 0.0, 8.0, // clamp2: occlusion, lod level, anisotropy
            0.0, 0.0, 1.0, 0.0, // height curve: linear, no strength, power 1
        ];
        (uniforms, shading)
    }

    /// `OverlayUniforms` for the colormap albedo pass (constant per terrain).
    fn overlay_uniforms(&self) -> crate::terrain::renderer::uniforms::OverlayUniforms {
        let (h_min, h_max) = self.domain;
        let h_range = (h_max - h_min).max(1e-3);
        crate::terrain::renderer::uniforms::OverlayUniforms {
            // Height-domain normalisation for the colour ramp: raw DEM min and
            // inverse range, full strength, no offset.
            params0: [h_min, 1.0 / h_range, 1.0, 0.0],
            // alpha blend, no debug view, mix albedo, full colormap strength
            params1: [1.0, 0.0, 2.0, 1.0],
            params2: [1.0, 1.0, 0.0, 1.0], // gamma, roughness mult, no spec AA
            params3: [0.0, 0.0, 0.08, 0.0], // ao weight, fallback, hue variation
            params4: [0.0, 2.0, 0.35, 0.0], // micro-detail disabled
            params5: [0.0, 1.0, 0.0, 0.0],
            params6: [1.0, 0.0, 0.0, 0.0],
            params7: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

impl TerrainScene {
    pub fn load_terrain_for_viewer(&mut self, dem_path: &str) -> Result<()> {
        use std::path::Path;

        let path = Path::new(dem_path);
        if !path.exists() {
            return Err(anyhow!("DEM file not found: {}", dem_path));
        }

        let (heightmap, width, height, domain) = self.load_geotiff_heightmap(dem_path)?;
        let heightmap_texture = self.upload_heightmap_texture(width, height, &heightmap)?;
        let heightmap_view = heightmap_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let terrain_span = (width.max(height) as f32) * 1.0;
        let cam_radius = terrain_span * 1.8;

        self.viewer_heightmap = Some(ViewerTerrainData {
            heightmap,
            dimensions: (width, height),
            domain,
            heightmap_texture,
            heightmap_view,
            world_span: terrain_span,
            z_scale: 0.2,
            cam_target: [0.0, 0.0],
            cam_radius,
            cam_phi_deg: 135.0,
            cam_theta_deg: 45.0,
            cam_fov_deg: 55.0,
            sun_azimuth_deg: 135.0,
            sun_elevation_deg: 35.0,
            sun_intensity: 3.0,
        });
        // The cached frame resources reference the previous heightmap view.
        self.viewer_resources = None;
        // A fresh terrain without a palette would sample an unbound LUT slot.
        if self.viewer_colormap_view.is_none() {
            self.set_viewer_colormap(None)?;
        }

        println!(
            "[terrain] Loaded {}x{} DEM, domain: {:.1}..{:.1}",
            width, height, domain.0, domain.1
        );

        Ok(())
    }

    fn load_geotiff_heightmap(&self, path: &str) -> Result<(Vec<f32>, u32, u32, (f32, f32))> {
        use std::fs::File;

        let file = File::open(path).map_err(|e| anyhow!("Failed to open DEM file: {}", e))?;
        let mut decoder = tiff::decoder::Decoder::new(file)
            .map_err(|e| anyhow!("Failed to decode TIFF: {}", e))?;

        let (width, height) = decoder
            .dimensions()
            .map_err(|e| anyhow!("Failed to get TIFF dimensions: {}", e))?;

        let image = decoder
            .read_image()
            .map_err(|e| anyhow!("Failed to read TIFF image: {}", e))?;

        let heightmap: Vec<f32> = match image {
            tiff::decoder::DecodingResult::F32(data) => data,
            tiff::decoder::DecodingResult::F64(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::I16(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::U16(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::I32(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::U32(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::U8(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::I8(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::U64(data) => data.iter().map(|&v| v as f32).collect(),
            tiff::decoder::DecodingResult::I64(data) => data.iter().map(|&v| v as f32).collect(),
        };

        let mut min_h = f32::MAX;
        let mut max_h = f32::MIN;
        for &h in &heightmap {
            if h.is_finite() {
                min_h = min_h.min(h);
                max_h = max_h.max(h);
            }
        }

        let heightmap: Vec<f32> = heightmap
            .iter()
            .map(|&h| if h.is_finite() { h } else { min_h })
            .collect();

        Ok((heightmap, width, height, (min_h, max_h)))
    }

    pub fn has_viewer_terrain(&self) -> bool {
        self.viewer_heightmap.is_some()
    }

    /// Unload the current viewer terrain, if any. Used by the Studio desktop
    /// app (studio/) when a project is closed or a dataset is removed.
    pub fn clear_viewer_terrain(&mut self) {
        self.viewer_heightmap = None;
        self.viewer_resources = None;
    }

    pub fn viewer_terrain(&self) -> Option<&ViewerTerrainData> {
        self.viewer_heightmap.as_ref()
    }

    pub fn viewer_terrain_mut(&mut self) -> Option<&mut ViewerTerrainData> {
        self.viewer_heightmap.as_mut()
    }

    pub fn set_viewer_camera(&mut self, phi_deg: f32, theta_deg: f32, radius: f32, fov_deg: f32) {
        if let Some(ref mut terrain) = self.viewer_heightmap {
            terrain.cam_phi_deg = phi_deg;
            terrain.cam_theta_deg = theta_deg;
            terrain.cam_radius = radius;
            terrain.cam_fov_deg = fov_deg;
        }
    }

    /// World scale for the native viewer: horizontal span in world units and
    /// the vertical exaggeration applied to raw DEM heights. Studio sets both
    /// from the active document so the viewport matches the exported frame's
    /// framing.
    pub fn set_viewer_world_scale(&mut self, span: f32, z_scale: f32) {
        if let Some(ref mut terrain) = self.viewer_heightmap {
            terrain.world_span = span.max(1e-3);
            terrain.z_scale = z_scale.max(1e-3);
        }
    }

    /// Ground-plane orbit target (shader XY plane).
    pub fn set_viewer_target(&mut self, x: f32, y: f32) {
        if let Some(ref mut terrain) = self.viewer_heightmap {
            terrain.cam_target = [x, y];
        }
    }

    pub fn set_viewer_sun(&mut self, azimuth_deg: f32, elevation_deg: f32, intensity: f32) {
        if let Some(ref mut terrain) = self.viewer_heightmap {
            terrain.sun_azimuth_deg = azimuth_deg;
            terrain.sun_elevation_deg = elevation_deg;
            terrain.sun_intensity = intensity;
        }
    }

    /// Select the viewer's albedo ramp: a forge3d colormap name (`viridis`,
    /// `magma`, `terrain`, optionally `forge3d:`-prefixed) or `None` for
    /// Studio's relief ramp.
    ///
    /// An unknown name is an error, not a silent fall back to the default —
    /// the caller can then say which colormap it could not resolve.
    pub fn set_viewer_colormap(&mut self, name: Option<&str>) -> Result<()> {
        let requested = name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| name.rsplit(':').next().unwrap_or(name).to_owned());
        if self.viewer_colormap_name == requested && self.viewer_colormap_view.is_some() {
            return Ok(());
        }

        let lut = match requested.as_deref() {
            Some(name) => crate::colormap::decode_png_rgba8(name)
                .map_err(|error| anyhow!("{error}"))?,
            None => relief_lut_bytes(),
        };
        if lut.len() != 256 * 4 {
            return Err(anyhow!(
                "colormap '{}' is not a 256x1 RGBA ramp ({} bytes)",
                requested.as_deref().unwrap_or("default"),
                lut.len()
            ));
        }

        let texture = tracked_create_texture(
            self.device.as_ref(),
            &wgpu::TextureDescriptor {
                label: Some("terrain.viewer.colormap_lut"),
                size: wgpu::Extent3d {
                    width: 256,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &lut,
            wgpu::ImageDataLayout {
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
        self.viewer_colormap_view =
            Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.viewer_colormap_texture = Some(texture);
        self.viewer_colormap_name = requested;
        // The cached main bind group still points at the previous palette.
        self.viewer_resources = None;
        Ok(())
    }

    fn viewer_snapshot(&self) -> Option<ViewerSnapshot> {
        let terrain = self.viewer_heightmap.as_ref()?;
        Some(ViewerSnapshot {
            world_span: terrain.world_span,
            z_scale: terrain.z_scale,
            cam_target: terrain.cam_target,
            cam_radius: terrain.cam_radius,
            cam_phi_deg: terrain.cam_phi_deg,
            cam_theta_deg: terrain.cam_theta_deg,
            cam_fov_deg: terrain.cam_fov_deg,
            sun_azimuth_deg: terrain.sun_azimuth_deg,
            sun_elevation_deg: terrain.sun_elevation_deg,
            sun_intensity: terrain.sun_intensity,
            domain: terrain.domain,
        })
    }

    /// Build the frame resources for `size`, reusing the scene's fallbacks for
    /// the pipeline's seven bind groups.
    ///
    /// STUDIO-FORK (M9/R1): the pipeline layout declares seven groups (main,
    /// light, IBL, shadow, fog, water, material layer). The viewer assembles
    /// each from the scene's own fallback resources — fog and material layers
    /// are disabled/uniform-fallback, water reflections zeroed — so the
    /// viewer draws through the live pipeline and a pipeline change cannot
    /// leave a hand-maintained binding list behind.
    fn build_viewer_resources(
        &self,
        snapshot: &ViewerSnapshot,
        width: u32,
        height: u32,
    ) -> Result<ViewerResources> {
        let (frame_uniforms, frame_shading) = snapshot.frame_uniforms(width, height);
        let uniforms = tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.uniforms"),
                contents: bytemuck::cast_slice(&frame_uniforms[..]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        )?;
        let shading = tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.shading_uniforms"),
                contents: bytemuck::cast_slice(&frame_shading[..]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        )?;
        let overlay = tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.overlay_uniforms"),
                contents: bytemuck::bytes_of(&snapshot.overlay_uniforms()),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        )?;

        // STUDIO-FORK (M9/R1): binding 3 expects a material-atlas array; the
        // viewer has no material set, so bind a 1x1x1 D2Array fallback.
        let material_fallback = tracked_create_texture(
            self.device.as_ref(),
            &wgpu::TextureDescriptor {
                label: Some("terrain.viewer.material_fallback"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )?;
        let material_layer_fallback = tracked_create_texture(
            self.device.as_ref(),
            &wgpu::TextureDescriptor {
                label: Some("terrain.viewer.material_map_fallback"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )?;
        let material_array_view = material_fallback.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..wgpu::TextureViewDescriptor::default()
        });
        let material_map_view =
            material_layer_fallback.create_view(&wgpu::TextureViewDescriptor::default());

        // Group 2: IBL disabled — black 1x1 cube pair, 1x1 ambient LUT, and a
        // zero-intensity uniform so the shader contribution is zero.
        let ibl_cube = tracked_create_texture(
            self.device.as_ref(),
            &wgpu::TextureDescriptor {
                label: Some("terrain.viewer.ibl_fallback_cube"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        for layer in 0..6u32 {
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &ibl_cube,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                &[0u8; 4],
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        let ibl_cube_view = ibl_cube.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..wgpu::TextureViewDescriptor::default()
        });
        let ibl_lut = tracked_create_texture(
            self.device.as_ref(),
            &wgpu::TextureDescriptor {
                label: Some("terrain.viewer.ibl_fallback_lut"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &ibl_lut,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8; 4],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let ibl_lut_view = ibl_lut.create_view(&wgpu::TextureViewDescriptor::default());
        let ibl_uniforms = tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.ibl_uniforms"),
                contents: bytemuck::bytes_of(&IblUniforms {
                    intensity: 0.0,
                    sin_theta: 0.0,
                    cos_theta: 1.0,
                    specular_mip_count: 1.0,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        )?;
        let ibl = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain.viewer.ibl_bind_group"),
            layout: &self.ibl_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&ibl_cube_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ibl_cube_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&ibl_lut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: ibl_uniforms.as_entire_binding(),
                },
            ],
        });

        // Group 4: fog disabled — zeroed uniforms over the fallback sky.
        let fog = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain.viewer.fog_bind_group"),
            layout: &self.fog_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.fog_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.sky_fallback_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &self.atmosphere_scattering_fallback_view,
                    ),
                },
            ],
        });

        // Group 5: water reflections disabled — zeroed uniform over the
        // fallback reflection texture. A real render may later swap the
        // reflection view; the zero uniform keeps that stale view harmless.
        let water_view_guard = self
            .water_reflection_view
            .lock()
            .map_err(|_| anyhow!("water reflection view mutex poisoned"))?;
        let water = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain.viewer.water_bind_group"),
            layout: &self.water_reflection_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.water_reflection_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&water_view_guard),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                },
            ],
        });
        drop(water_view_guard);

        // Group 6: material layer disabled — disabled uniform block, VT
        // fallback atlas/page-table, and 1x1 material-map fallbacks.
        let atlas_refs: Vec<&wgpu::TextureView> = self.vt_atlas_fallback_views.iter().collect();
        let atlas_resource =
            if crate::terrain::renderer::virtual_texture::bindless_bc_supported(
                self.device.as_ref(),
            ) {
                wgpu::BindingResource::TextureViewArray(&atlas_refs)
            } else {
                wgpu::BindingResource::TextureView(
                    self.vt_atlas_fallback_views
                        .first()
                        .expect("VT fallback always has an atlas view"),
                )
            };
        let material_layer = self
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain.viewer.material_layer_bind_group"),
                layout: &self.material_layer_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.material_layer_uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.probe_grid_uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.probe_ssbo.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.reflection_probe_grid_uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&self.reflection_probe_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.reflection_probe_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.vt_uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self.vt_fallback_uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: atlas_resource,
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::Sampler(&self.vt_atlas_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::TextureView(
                            &self.vt_page_table_fallback_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: self.vt_feedback_fallback_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: wgpu::BindingResource::TextureView(&material_map_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: wgpu::BindingResource::TextureView(&material_map_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: wgpu::BindingResource::TextureView(&material_map_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 15,
                        resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                    },
                    wgpu::BindGroupEntry {
                        binding: 16,
                        resource: self.vt_frame_counters_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 17,
                        resource: wgpu::BindingResource::TextureView(&material_map_view),
                    },
                ],
            });

        // STUDIO-FORK (M9/R1): binding 6 is the colormap LUT; the identity
        // height curve is grayscale, so the palette comes from the viewer's
        // own 256x1 ramp and the identity LUT stays bound to the height-curve
        // slot.
        let colormap_view = self
            .viewer_colormap_view
            .as_ref()
            .ok_or_else(|| anyhow!("viewer colormap LUT missing; call set_viewer_colormap"))?;
        let heightmap_view = &self
            .viewer_heightmap
            .as_ref()
            .ok_or_else(|| anyhow!("viewer heightmap missing while building resources"))?
            .heightmap_view;
        let main = self.create_terrain_main_bind_group(
            "terrain.viewer.bind_group",
            &uniforms,
            heightmap_view,
            &material_array_view,
            &self.sampler_linear,
            &shading,
            colormap_view,
            &self.sampler_linear,
            &overlay,
            None,
            &self.height_curve_identity_view,
            None,
            false,
            false,
        )?;

        let depth = tracked_create_texture(
            self.device.as_ref(),
            &wgpu::TextureDescriptor {
                label: Some("terrain.viewer.depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: super::core::TERRAIN_DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        )?;
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(ViewerResources {
            size: (width, height),
            uniforms,
            shading,
            _overlay: overlay,
            _depth: depth,
            depth_view,
            _material_fallback: material_fallback,
            _material_layer_fallback: material_layer_fallback,
            _ibl_cube: ibl_cube,
            _ibl_lut: ibl_lut,
            _ibl_uniforms: ibl_uniforms,
            main,
            ibl,
            fog,
            water,
            material_layer,
        })
    }

    /// Draw one viewer frame into `target_view`.
    ///
    /// Returns `false` when there is no viewer terrain or a resource could not
    /// be created; the caller reports that rather than presenting a stale or
    /// blank frame as a success.
    pub fn render_viewer_terrain(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        _target_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        background_initialized: bool,
    ) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        let snapshot = match self.viewer_snapshot() {
            Some(snapshot) => snapshot,
            None => return false,
        };

        if self.viewer_resources.as_ref().map(|res| res.size) != Some((width, height)) {
            match self.build_viewer_resources(&snapshot, width, height) {
                Ok(resources) => self.viewer_resources = Some(resources),
                Err(error) => {
                    log::error!(target: "terrain.viewer", "failed to build viewer resources: {error}");
                    return false;
                }
            }
        }
        let resources = match self.viewer_resources.as_ref() {
            Some(resources) => resources,
            None => return false,
        };

        let (uniforms, shading) = snapshot.frame_uniforms(width, height);
        self.queue.write_buffer(
            &resources.uniforms,
            0,
            bytemuck::cast_slice(&uniforms[..]),
        );
        self.queue.write_buffer(
            &resources.shading,
            0,
            bytemuck::cast_slice(&shading[..]),
        );
        // Nothing but the viewer may contribute to a viewer frame: fog, water
        // reflections and material layers stay disabled every frame so a
        // later real render cannot leave its state behind in this one.
        self.queue.write_buffer(
            &self.fog_uniform_buffer,
            0,
            bytemuck::bytes_of(&crate::terrain::renderer::uniforms::FogUniforms::disabled()),
        );
        let water_size = self.water_reflection_uniform_buffer.size();
        self.queue
            .write_buffer(&self.water_reflection_uniform_buffer, 0, &vec![0u8; water_size as usize]);
        self.queue.write_buffer(
            &self.material_layer_uniform_buffer,
            0,
            bytemuck::bytes_of(
                &crate::terrain::renderer::uniforms::MaterialLayerUniforms::disabled(),
            ),
        );

        let pipeline_guard = self.pipeline.lock().unwrap();
        // Group 1: the pipeline samples the light buffer's storage buffer.
        let light_buffer_guard = match self.light_buffer.lock() {
            Ok(guard) => guard,
            Err(_) => {
                log::error!(target: "terrain.viewer", "Light buffer mutex poisoned");
                return false;
            }
        };
        let light_bind_group = match light_buffer_guard.bind_group() {
            Some(group) => group,
            None => {
                log::error!(target: "terrain.viewer", "LightBuffer should always provide a bind group");
                return false;
            }
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terrain.viewer.render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if background_initialized {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: (snapshot.sun_intensity.min(1.0) * 0.5) as f64,
                            g: (snapshot.sun_intensity.min(1.0) * 0.7) as f64,
                            b: (snapshot.sun_intensity.min(1.0) * 0.9) as f64,
                            a: 1.0,
                        })
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &resources.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&pipeline_guard.pipeline);
        pass.set_bind_group(0, &resources.main, &[]);
        pass.set_bind_group(1, &light_bind_group, &[]);
        pass.set_bind_group(2, &resources.ibl, &[]);
        pass.set_bind_group(3, &self.noop_shadow.bind_group, &[]);
        pass.set_bind_group(4, &resources.fog, &[]);
        pass.set_bind_group(5, &resources.water, &[]);
        pass.set_bind_group(6, &resources.material_layer, &[]);
        // `vs_main` reads no vertex buffer: mesh mode derives each vertex from
        // its `vertex_index`, so the draw range carries the grid.
        let grid_size = width.max(height).clamp(128, 768);
        pass.draw(0..6 * (grid_size - 1) * (grid_size - 1), 0..1);
        true
    }
}
