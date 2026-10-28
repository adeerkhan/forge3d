use super::*;
use crate::core::resource_tracker::tracked_create_buffer_init;

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
            cam_radius,
            cam_phi_deg: 135.0,
            cam_theta_deg: 45.0,
            cam_fov_deg: 55.0,
            sun_azimuth_deg: 135.0,
            sun_elevation_deg: 35.0,
            sun_intensity: 3.0,
        });

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

    pub fn set_viewer_sun(&mut self, azimuth_deg: f32, elevation_deg: f32, intensity: f32) {
        if let Some(ref mut terrain) = self.viewer_heightmap {
            terrain.sun_azimuth_deg = azimuth_deg;
            terrain.sun_elevation_deg = elevation_deg;
            terrain.sun_intensity = intensity;
        }
    }

    pub fn render_viewer_terrain(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        _target_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        background_initialized: bool,
    ) -> bool {
        let terrain = match &self.viewer_heightmap {
            Some(t) => t,
            None => return false,
        };

        let phi_rad = terrain.cam_phi_deg.to_radians();
        let theta_rad = terrain.cam_theta_deg.to_radians();
        let radius = terrain.cam_radius;
        let (tw, th) = terrain.dimensions;

        // STUDIO-FORK (M9/R1): the live shader's mesh mode builds a grid in
        // the XY plane with elevation along +Z, centred on the origin. The
        // camera must orbit that same Z-up world; the previous Y-up orbit put
        // the eye above a horizontal plane the shader never produces.
        let terrain_span = tw.max(th) as f32;
        let eye = glam::Vec3::new(
            radius * theta_rad.sin() * phi_rad.cos(),
            radius * theta_rad.sin() * phi_rad.sin(),
            radius * theta_rad.cos(),
        );
        let up = if theta_rad.sin().abs() < 1e-4 {
            glam::Vec3::Y
        } else {
            glam::Vec3::Z
        };
        let view = glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, up);
        let aspect = width as f32 / height as f32;
        let clip_near = (radius * 0.01).max(0.1);
        let clip_far = (radius * 20.0).max(clip_near + 1.0);
        let proj = glam::Mat4::perspective_rh(
            terrain.cam_fov_deg.to_radians(),
            aspect,
            clip_near,
            clip_far,
        );

        // Mesh grid resolution scales with the target so a large viewport
        // keeps its silhouette. The vertex stage emits 6 vertices per quad and
        // reads no vertex buffer, so the draw range must match `grid_size`.
        let grid_size = width.max(height).clamp(128, 768);
        let draw_vertices = 6 * (grid_size - 1) * (grid_size - 1);

        let sun_az_rad = terrain.sun_azimuth_deg.to_radians();
        let sun_el_rad = terrain.sun_elevation_deg.to_radians();
        let sun_dir = glam::Vec3::new(
            sun_el_rad.cos() * sun_az_rad.sin(),
            sun_el_rad.sin(),
            sun_el_rad.cos() * sun_az_rad.cos(),
        )
        .normalize();

        let h_range = (terrain.domain.1 - terrain.domain.0).max(1e-3);
        let h_min = terrain.domain.0;
        let h_max = terrain.domain.1;
        // STUDIO-FORK (M9/R1): mesh geometry. The live shader's `vs_main`
        // rebuilds the terrain grid procedurally from `vertex_index`; screen
        // mode is a 1-unit fullscreen quad that is invisible to a perspective
        // camera. `spacing` is the horizontal extent in world units, `z_scale`
        // is the vertical exaggeration applied to the (absolute) DEM heights.
        let z_scale: f32 = 0.2;
        let viewer_uniforms: [f32; 44] = {
            let mut u = [0.0f32; 44];
            // View/projection, column-major (matches mat4x4<f32> in WGSL).
            let v = view.to_cols_array_2d();
            for col in 0..4 {
                for row in 0..4 {
                    u[col * 4 + row] = v[col][row];
                }
            }
            let p = proj.to_cols_array_2d();
            for col in 0..4 {
                for row in 0..4 {
                    u[16 + col * 4 + row] = p[col][row];
                }
            }
            u[32..36].copy_from_slice(&[sun_dir.x, sun_dir.y, sun_dir.z, terrain.sun_intensity]);
            u[36..40].copy_from_slice(&[terrain_span, terrain_span, z_scale, 1.0]);
            u[40..44].copy_from_slice(&[1.0, grid_size as f32, clip_near, clip_far]);
            u
        };

        let uniform_buffer = match tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.uniforms"),
                contents: bytemuck::cast_slice(&viewer_uniforms[..]),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        ) {
            Ok(buffer) => buffer,
            Err(err) => {
                log::error!(target: "terrain.viewer", "failed to allocate viewer uniform buffer: {err}");
                return false;
            }
        };

        // STUDIO-FORK (M9/R1): the shader's binding 5 is TerrainShadingUniforms
        // (44 f32) and binding 8 is OverlayUniforms (32 f32) — the pre-drift
        // viewer fed view/proj bytes into both. Write honest viewer values:
        // no triplanar/POM, one material layer, sun-tinted light, colormap
        // albedo with a strength-1 alpha LUT overlay.
        let sun_tinted = terrain.sun_intensity.max(0.0);
        let viewer_shading: [f32; 44] = [
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
        let viewer_shading_buffer = match tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.shading_uniforms"),
                contents: bytemuck::cast_slice(&viewer_shading),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        ) {
            Ok(buffer) => buffer,
            Err(err) => {
                log::error!(target: "terrain.viewer", "failed to allocate viewer shading buffer: {err}");
                return false;
            }
        };
        let viewer_overlay = crate::terrain::renderer::uniforms::OverlayUniforms {
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
        };
        let viewer_overlay_buffer = match tracked_create_buffer_init(
            self.device.as_ref(),
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain.viewer.overlay_uniforms"),
                contents: bytemuck::bytes_of(&viewer_overlay),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        ) {
            Ok(buffer) => buffer,
            Err(err) => {
                log::error!(target: "terrain.viewer", "failed to allocate viewer overlay uniform buffer: {err}");
                return false;
            }
        };

        let pipeline_guard = self.pipeline.lock().unwrap();
        let pipeline = &pipeline_guard.pipeline;
        let lut_view = &self.height_curve_identity_view;
        let lut_sampler = &self.sampler_linear;

        // STUDIO-FORK (M9/R1): binding 6 is the colormap LUT. The identity
        // height curve is grayscale; build a proper terrain ramp (the same
        // stops Studio's bridge uses) once per scene and sample it here.
        if self.viewer_colormap_view.is_none() {
            let stops: [(f32, [u8; 3]); 4] = [
                (0.0, [0x18, 0x39, 0x1f]),
                (0.40, [0x4e, 0x7c, 0x35]),
                (0.72, [0x9a, 0x85, 0x52]),
                (1.0, [0xf3, 0xf5, 0xf9]),
            ];
            let mut lut = Vec::with_capacity(256 * 4);
            for i in 0..256u32 {
                let t = i as f32 / 255.0;
                let mut rgb = stops[stops.len() - 1].1;
                for w in stops.windows(2) {
                    if t >= w[0].0 && t <= w[1].0 {
                        let span = (w[1].0 - w[0].0).max(1e-6);
                        let f = (t - w[0].0) / span;
                        rgb = [
                            (w[0].1[0] as f32 + f * (w[1].1[0] as f32 - w[0].1[0] as f32)) as u8,
                            (w[0].1[1] as f32 + f * (w[1].1[1] as f32 - w[0].1[1] as f32)) as u8,
                            (w[0].1[2] as f32 + f * (w[1].1[2] as f32 - w[0].1[2] as f32)) as u8,
                        ];
                        break;
                    }
                }
                lut.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            let texture = match crate::core::resource_tracker::tracked_create_texture(
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
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
            ) {
                Ok(texture) => texture,
                Err(err) => {
                    log::error!(target: "terrain.viewer", "failed to allocate viewer colormap LUT: {err}");
                    return false;
                }
            };
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
            self.viewer_colormap_view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let colormap_view = self
            .viewer_colormap_view
            .as_ref()
            .expect("colormap LUT built above");

        // STUDIO-FORK (M9/R1): the hand-rolled 17-entry bind group here had
        // drifted from the current 23-entry layout (bindings 17-22 added by
        // VT/height-atlas work) and the pipeline now requires a Depth32Float
        // attachment. Build the bind group through the shared live-path
        // helper, bind a 1x1x1 D2Array fallback for the material slot
        // (binding 3 expects a material-atlas array; the viewer has no
        // material set), and attach a transient depth target.
        let material_fallback_texture = match crate::core::resource_tracker::tracked_create_texture(
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
        ) {
            Ok(texture) => texture,
            Err(err) => {
                log::error!(target: "terrain.viewer", "failed to allocate viewer material fallback: {err}");
                return false;
            }
        };
        let material_fallback_view = material_fallback_texture.create_view(
            &wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            },
        );

        let depth_texture = match crate::core::resource_tracker::tracked_create_texture(
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
        ) {
            Ok(texture) => texture,
            Err(err) => {
                log::error!(target: "terrain.viewer", "failed to allocate viewer depth target: {err}");
                return false;
            }
        };
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = match self.create_terrain_main_bind_group(
            "terrain.viewer.bind_group",
            &uniform_buffer,
            &terrain.heightmap_view,
            &material_fallback_view,
            lut_sampler,
            &viewer_shading_buffer,
            colormap_view,
            lut_sampler,
            &viewer_overlay_buffer,
            None,
            lut_view,
            None,
            false,
            false,
        ) {
            Ok(group) => group,
            Err(err) => {
                log::error!(target: "terrain.viewer", "failed to create viewer bind group: {err}");
                return false;
            }
        };

        // STUDIO-FORK (M9/R1): the pipeline layout declares seven groups
        // (main, light, IBL, shadow, fog, water, material layer). The viewer
        // assembles each from the scene's own fallback resources — fog and
        // material layers are disabled/uniform-fallback, water reflections
        // zeroed — so the legacy path draws through the live pipeline.
        {
            // Group 1: light buffer (the pipeline samples its storage buffer).
            let light_buffer_guard = match self.light_buffer.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    log::error!(target: "terrain.viewer", "Light buffer mutex poisoned");
                    return false;
                }
            };
            // The guard stays alive for the pass; `light_bind_group` borrows it.
            let light_bind_group = match light_buffer_guard.bind_group() {
                Some(group) => group,
                None => {
                    log::error!(target: "terrain.viewer", "LightBuffer should always provide a bind group");
                    return false;
                }
            };

            // Group 2: IBL disabled — black 1x1 cube pair, 1x1 ambient LUT,
            // zero-intensity uniform so the shader contribution is zero.
            let black_cube = match crate::core::resource_tracker::tracked_create_texture(
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
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
            ) {
                Ok(texture) => texture,
                Err(err) => {
                    log::error!(target: "terrain.viewer", "failed to allocate viewer IBL fallback cube: {err}");
                    return false;
                }
            };
            for layer in 0..6u32 {
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: &black_cube,
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
            let black_cube_view = black_cube.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..wgpu::TextureViewDescriptor::default()
            });
            let black_lut = match crate::core::resource_tracker::tracked_create_texture(
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
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
            ) {
                Ok(texture) => texture,
                Err(err) => {
                    log::error!(target: "terrain.viewer", "failed to allocate viewer IBL fallback LUT: {err}");
                    return false;
                }
            };
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &black_lut,
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
            let ibl_uniforms_buffer = match crate::core::resource_tracker::tracked_create_buffer_init(
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
            ) {
                Ok(buffer) => buffer,
                Err(err) => {
                    log::error!(target: "terrain.viewer", "failed to allocate viewer IBL uniforms: {err}");
                    return false;
                }
            };
            let ibl_lut_view = material_fallback_texture.create_view(
                &wgpu::TextureViewDescriptor::default(),
            );
            let ibl_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain.viewer.ibl_bind_group"),
                layout: &self.ibl_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&black_cube_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&black_cube_view),
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
                        resource: ibl_uniforms_buffer.as_entire_binding(),
                    },
                ],
            });

            // Group 4: fog disabled — zeroed uniforms over the fallback sky.
            self.queue.write_buffer(
                &self.fog_uniform_buffer,
                0,
                bytemuck::bytes_of(&crate::terrain::renderer::uniforms::FogUniforms::disabled()),
            );
            let fog_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            // fallback reflection texture.
            let water_size = self.water_reflection_uniform_buffer.size();
            self.queue.write_buffer(
                &self.water_reflection_uniform_buffer,
                0,
                &vec![0u8; water_size as usize],
            );
            let water_view_guard = match self.water_reflection_view.lock() {
                Ok(view) => view,
                Err(_) => {
                    log::error!(target: "terrain.viewer", "water reflection view mutex poisoned");
                    return false;
                }
            };
            let water_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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

            // Group 6: material layer disabled — disabled uniform block, VT
            // fallback atlas/page-table, and 1x1 material-map fallbacks.
            self.queue.write_buffer(
                &self.material_layer_uniform_buffer,
                0,
                bytemuck::bytes_of(
                    &crate::terrain::renderer::uniforms::MaterialLayerUniforms::disabled(),
                ),
            );
            let material_map_view = material_fallback_texture.create_view(
                &wgpu::TextureViewDescriptor::default(),
            );
            let atlas_refs: Vec<&wgpu::TextureView> =
                self.vt_atlas_fallback_views.iter().collect();
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
            let material_layer_bind_group = self
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
                            resource: self
                                .reflection_probe_grid_uniform_buffer
                                .as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(
                                &self.reflection_probe_view,
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::Sampler(
                                &self.reflection_probe_sampler,
                            ),
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
                                r: terrain.sun_intensity.min(1.0) as f64 * 0.5,
                                g: terrain.sun_intensity.min(1.0) as f64 * 0.7,
                                b: terrain.sun_intensity.min(1.0) as f64 * 0.9,
                                a: 1.0,
                            })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_bind_group(1, &light_bind_group, &[]);
            pass.set_bind_group(2, &ibl_bind_group, &[]);
            pass.set_bind_group(3, &self.noop_shadow.bind_group, &[]);
            pass.set_bind_group(4, &fog_bind_group, &[]);
            pass.set_bind_group(5, &water_bind_group, &[]);
            pass.set_bind_group(6, &material_layer_bind_group, &[]);
            // `vs_main` reads no vertex buffer: mesh mode derives each vertex
            // from its `vertex_index`, so the draw range carries the grid.
            pass.draw(0..draw_vertices, 0..1);
        }

        true
    }
}
