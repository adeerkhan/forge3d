//! M10: GPU-shared native viewport.
//!
//! Drives `forge3d::terrain::renderer::TerrainScene` directly on the same wgpu
//! device and queue eframe presents with. Frames are rendered into an
//! offscreen texture each time the view changes and handed to egui as a
//! native texture — no readback, no PNG round-trip, no embedded-Python call
//! in the interactive path.
//!
//! The bridge (embedded Python) stays the correctness anchor for exports,
//! certificates and MapScene; this module only paints the interactive
//! viewport. When it cannot run (no wgpu render state, DEM load failure) the
//! app keeps the previous bridge-readback path and says so in the console.
//!
//! Colour: `TerrainScene`'s pipeline is built for `Rgba8Unorm` (the same
//! format the engine's own viewer targets), so the offscreen texture is
//! `Rgba8Unorm` with an `Rgba8UnormSrgb` view format. The render pass writes
//! the linear buffer; egui samples the sRGB view so the presented image is
//! decoded exactly once.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use forge3d::terrain::renderer::TerrainScene;
use studio_project::{CameraParams, SunParams};

/// Vertical exaggeration Studio uses for the interactive terrain. Matches the
/// bridge's `z_scale` default, so the viewport and exported frames share the
/// same relief to span ratio.
pub const STUDIO_Z_SCALE: f32 = 1.35;

/// The single colour format both `TerrainScene` and this viewport target.
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const PRESENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct NativeViewport {
    scene: TerrainScene,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    color: Option<wgpu::Texture>,
    /// Render-attachment view in the pipeline's `Rgba8Unorm` format.
    render_view: Option<wgpu::TextureView>,
    /// Sampling view in `Rgba8UnormSrgb`, registered with egui.
    present_view: Option<wgpu::TextureView>,
    size: (u32, u32),
    /// Bumped whenever `present_view` is replaced (size change), so the app
    /// knows to re-register the egui texture.
    generation: u64,
    has_terrain: bool,
    last_render_ms: f32,
}

impl NativeViewport {
    /// Build the shared scene. Expensive (compiles the terrain shaders), so
    /// callers should do this on a worker thread with a large stack.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter: Arc<wgpu::Adapter>,
    ) -> Result<Self, String> {
        let scene = TerrainScene::new(device.clone(), queue.clone(), adapter)
            .map_err(|error| format!("native terrain scene: {error}"))?;
        Ok(Self {
            scene,
            device,
            queue,
            color: None,
            render_view: None,
            present_view: None,
            size: (0, 0),
            generation: 0,
            has_terrain: false,
            last_render_ms: 0.0,
        })
    }

    /// Load a DEM into the shared scene. `span` is the document's horizontal
    /// world extent; relief is scaled by [`STUDIO_Z_SCALE`].
    pub fn load_dem(&mut self, path: &Path, span: f32) -> Result<(), String> {
        self.scene
            .load_terrain_for_viewer(&path.to_string_lossy())
            .map_err(|error| format!("native terrain load: {error}"))?;
        self.scene
            .set_viewer_world_scale(span.max(1e-3), STUDIO_Z_SCALE);
        self.has_terrain = true;
        Ok(())
    }

    pub fn clear_dem(&mut self) {
        self.scene.clear_viewer_terrain();
        self.has_terrain = false;
    }

    pub fn has_terrain(&self) -> bool {
        self.has_terrain
    }

    /// Select the albedo ramp (`None` = Studio's relief ramp). Returns an
    /// error for a name the engine cannot resolve, so the app reports it
    /// instead of painting the previous palette as if it were the new one.
    pub fn set_colormap(&mut self, name: Option<&str>) -> Result<(), String> {
        self.scene
            .set_viewer_colormap(name)
            .map_err(|error| format!("native viewport colormap: {error}"))
    }

    /// Push the document's camera and sun into the scene. Cheap; call every
    /// frame before [`render`](Self::render).
    pub fn set_view(&mut self, camera: &CameraParams, sun: &SunParams) {
        self.scene.set_viewer_camera(
            camera.phi_deg,
            camera.theta_deg,
            camera.radius,
            camera.fov_deg,
        );
        self.scene
            .set_viewer_target(camera.target_x, camera.target_z);
        self.scene
            .set_viewer_sun(sun.azimuth_deg, sun.elevation_deg, sun.intensity);
    }

    /// Render one frame into the offscreen texture. Reallocates the target
    /// when the requested size changes.
    pub fn render(&mut self, width: u32, height: u32) -> Result<(), String> {
        if !self.has_terrain {
            return Err("no terrain loaded into the native viewport".to_owned());
        }
        let (width, height) = (width.max(1), height.max(1));
        self.ensure_target(width, height);

        let started = Instant::now();
        let view = self
            .render_view
            .as_ref()
            .ok_or_else(|| "native viewport target missing".to_owned())?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("studio.native_viewport.encoder"),
            });
        let drawn = self.scene.render_viewer_terrain(
            &mut encoder,
            view,
            COLOR_FORMAT,
            width,
            height,
            false,
        );
        if !drawn {
            return Err("native terrain render declined the frame".to_owned());
        }
        self.queue.submit(Some(encoder.finish()));
        self.last_render_ms = started.elapsed().as_secs_f32() * 1000.0;
        Ok(())
    }

    /// sRGB view of the last rendered frame, for `Renderer::register_native_texture`.
    pub fn present_view(&self) -> Option<&wgpu::TextureView> {
        self.present_view.as_ref()
    }

    /// Changes whenever `present_view` is replaced (i.e. on resize).
    pub fn texture_generation(&self) -> u64 {
        self.generation
    }

    pub fn last_render_ms(&self) -> f32 {
        self.last_render_ms
    }

    fn ensure_target(&mut self, width: u32, height: u32) {
        if self.size == (width, height) && self.color.is_some() {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("studio.native_viewport.color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[PRESENT_FORMAT],
        });
        let render_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let present_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(PRESENT_FORMAT),
            ..wgpu::TextureViewDescriptor::default()
        });
        self.color = Some(texture);
        self.render_view = Some(render_view);
        self.present_view = Some(present_view);
        self.size = (width, height);
        self.generation = self.generation.wrapping_add(1);
    }
}
