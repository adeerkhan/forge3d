//! The interactive terrain viewport, rendered by the engine and handed to the
//! UI as pixels.
//!
//! `NativeViewport` drives `forge3d::terrain::renderer::TerrainScene` on the
//! engine's own GPU context (the negotiated global one — never a private
//! `DeviceDescriptor::default()`), renders into an offscreen target and reads
//! the frame back as RGBA.
//!
//! Why readback: the UI runtime (GPUI, via `gpui-pre-wgpu`) renders through
//! wgpu 30 while forge3d is built against wgpu 0.19. Two wgpu majors cannot
//! share a `Device` or a `Texture`, so the frame crosses the boundary as
//! bytes. A frame is only read back when the view actually changed (gesture
//! end, committed edit, resize), which is how the viewport already behaved.
//!
//! `ViewportWorker` owns the scene on its own thread: building it compiles the
//! terrain shaders (seconds, deep recursion — hence the 64 MB stack) and a
//! render blocks the device, so neither may happen on the UI thread.
//!
//! Colour: `TerrainScene`'s viewer pipeline targets `Rgba8Unorm` and writes
//! linear values; the display formats want sRGB. The readback path therefore
//! encodes linear → sRGB exactly once (a 256-entry table), which is the same
//! conversion the previous sRGB view format performed when sampled.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use forge3d::terrain::renderer::TerrainScene;
use studio_project::{CameraParams, SunParams};

/// Vertical exaggeration Studio uses for the interactive terrain. Matches the
/// bridge's `z_scale` default, so the viewport and exported frames share the
/// same relief to span ratio.
pub const STUDIO_Z_SCALE: f32 = 1.35;

/// The single colour format both `TerrainScene` and this viewport target.
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One frame of pixels the UI can present.
#[derive(Clone)]
pub struct ViewportFrame {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA, sRGB-encoded, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    pub render_ms: f32,
}

impl std::fmt::Debug for ViewportFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .field("render_ms", &self.render_ms)
            .finish()
    }
}

pub struct NativeViewport {
    scene: TerrainScene,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    color: Option<wgpu::Texture>,
    render_view: Option<wgpu::TextureView>,
    size: (u32, u32),
    has_terrain: bool,
    last_render_ms: f32,
    /// Linear (stored u8) → sRGB u8, computed once.
    srgb_lut: [u8; 256],
    layer_visible: bool,
    layer_opacity: f32,
    /// Last sun intensity pushed into the scene: the viewer clears to
    /// `(i*0.5, i*0.7, i*0.9)`, which is the backdrop a faded layer shows.
    sun_intensity: f32,
}

impl NativeViewport {
    /// Build the shared scene on the engine's negotiated GPU context.
    ///
    /// Expensive (compiles the terrain shaders), so callers should do this on a
    /// worker thread with a large stack — [`ViewportWorker::spawn`] does.
    pub fn new() -> Result<Self, String> {
        let context = forge3d::core::gpu::try_ctx()
            .map_err(|error| format!("engine GPU context unavailable: {error}"))?;
        let scene = TerrainScene::new(
            context.device.clone(),
            context.queue.clone(),
            context.adapter.clone(),
        )
        .map_err(|error| format!("native terrain scene: {error}"))?;
        Ok(Self {
            scene,
            device: context.device.clone(),
            queue: context.queue.clone(),
            color: None,
            render_view: None,
            size: (0, 0),
            has_terrain: false,
            last_render_ms: 0.0,
            srgb_lut: srgb_encode_table(),
            layer_visible: true,
            layer_opacity: 1.0,
            sun_intensity: 1.0,
        })
    }

    /// Load a DEM into the shared scene and frame it with the engine's own
    /// world-span convention. Returns the DEM metadata for the UI.
    pub fn load_dem(&mut self, path: &Path) -> Result<(u32, u32, f32, f32), String> {
        self.scene
            .load_terrain_for_viewer(&path.to_string_lossy())
            .map_err(|error| format!("native terrain load: {error}"))?;
        let terrain = self
            .scene
            .viewer_terrain()
            .ok_or_else(|| "native terrain load produced no heightmap".to_owned())?;
        let (width, height) = terrain.dimensions;
        let (height_min, height_max) = terrain.domain;
        let span = (height_max - height_min).abs().max(1.0) * 6.0;
        self.scene
            .set_viewer_world_scale(span.max(1e-3), STUDIO_Z_SCALE);
        self.has_terrain = true;
        Ok((width, height, height_min, height_max))
    }

    pub fn clear_dem(&mut self) {
        self.scene.clear_viewer_terrain();
        self.has_terrain = false;
    }

    pub fn has_terrain(&self) -> bool {
        self.has_terrain
    }

    /// Frame the terrain the way the app does: six-times-relief world span,
    /// camera 1.65 spans out. Used for the first frame after a load, before
    /// the document's own camera arrives.
    pub fn frame_default(&mut self) {
        let Some(terrain) = self.scene.viewer_terrain() else {
            return;
        };
        let span = (terrain.domain.1 - terrain.domain.0).abs().max(1.0) * 6.0;
        self.scene.set_viewer_camera(135.0, 30.0, span * 1.65, 54.0);
        self.scene.set_viewer_target(0.0, 0.0);
    }

    /// Select the albedo ramp (`None` = Studio's relief ramp). Returns an
    /// error for a name the engine cannot resolve, so the app reports it
    /// instead of painting the previous palette as if it were the new one.
    pub fn set_colormap(&mut self, name: Option<&str>) -> Result<(), String> {
        self.scene
            .set_viewer_colormap(name)
            .map_err(|error| format!("native viewport colormap: {error}"))
    }

    /// Apply the outliner's terrain style (visibility and opacity).
    pub fn set_layer_style(&mut self, visible: bool, opacity: f32) {
        self.layer_visible = visible;
        self.layer_opacity = opacity.clamp(0.0, 1.0);
    }

    /// Push the document's camera and sun into the scene. Cheap; call every
    /// frame before [`render_rgba`](Self::render_rgba).
    pub fn set_view(&mut self, camera: &CameraParams, sun: &SunParams) {
        self.sun_intensity = sun.intensity;
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

    /// Render one frame into the offscreen texture and read it back.
    /// Reallocates the target when the requested size changes.
    pub fn render_rgba(&mut self, width: u32, height: u32) -> Result<ViewportFrame, String> {
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

        // Copy the target into a mappable buffer. `copy_texture_to_buffer`
        // requires rows aligned to 256 bytes.
        let unpadded = width * 4;
        let padded = forge3d::core::gpu::align_copy_bpr(unpadded);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("studio.native_viewport.readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let color = self
            .color
            .as_ref()
            .ok_or_else(|| "native viewport target missing".to_owned())?;
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|_| "readback channel closed".to_owned())?
            .map_err(|error| format!("readback map: {error}"))?;

        // Layer opacity fades the frame toward the backdrop the render already
        // cleared to; a hidden layer shows only that backdrop. Doing it here
        // keeps it honest: the pixels the UI shows are the pixels the engine
        // drew, composited by one documented rule.
        let intensity = self.sun_intensity.clamp(0.0, 1.0);
        let background = [
            self.srgb_lut[(intensity * 0.5 * 255.0) as usize] as u32,
            self.srgb_lut[(intensity * 0.7 * 255.0) as usize] as u32,
            self.srgb_lut[(intensity * 0.9 * 255.0) as usize] as u32,
        ];
        let opacity = if self.layer_visible {
            self.layer_opacity
        } else {
            0.0
        };
        let keep = (opacity * 255.0).round() as u32;

        let mut rgba = Vec::with_capacity((unpadded * height) as usize);
        {
            let mapped = slice.get_mapped_range();
            for row in 0..height {
                let start = (row * padded) as usize;
                let end = start + unpadded as usize;
                for pixel in mapped[start..end].as_chunks::<4>().0 {
                    // Alpha stays opaque; RGB is linear storage → sRGB display.
                    for channel in 0..3 {
                        let source = self.srgb_lut[pixel[channel] as usize] as u32;
                        let mixed = (source * keep + background[channel] * (255 - keep)) / 255;
                        rgba.push(mixed as u8);
                    }
                    rgba.push(255);
                }
            }
        }
        buffer.unmap();

        self.last_render_ms = started.elapsed().as_secs_f32() * 1000.0;
        Ok(ViewportFrame {
            width,
            height,
            rgba,
            render_ms: self.last_render_ms,
        })
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let render_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.color = Some(texture);
        self.render_view = Some(render_view);
        self.size = (width, height);
    }
}

/// Linear u8 → sRGB u8, the encoding the sRGB view format used to perform.
fn srgb_encode_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let linear = index as f32 / 255.0;
        let encoded = if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        *slot = (encoded.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    table
}

/// Commands the UI sends to the viewport thread.
#[derive(Debug, Clone)]
pub enum ViewportCommand {
    LoadDem {
        path: PathBuf,
    },
    ClearDem,
    SetColormap(Option<String>),
    /// Terrain layer style from the outliner: a hidden layer paints the
    /// backdrop, an opacity below 1 fades the frame toward it.
    SetLayerStyle {
        visible: bool,
        opacity: f32,
    },
    SetView {
        camera: CameraParams,
        sun: SunParams,
    },
    /// Render one frame at this size. `force` skips the "nothing changed"
    /// check that keeps a stationary camera from re-rendering on every poll.
    Render {
        width: u32,
        height: u32,
        force: bool,
    },
    Shutdown,
}

/// Events the viewport thread sends back.
#[derive(Debug, Clone)]
pub enum ViewportEvent {
    Ready {
        adapter: String,
        backend: String,
        software_fallback: bool,
    },
    Failed {
        context: String,
        message: String,
    },
    /// The DEM is in the viewport, with the metadata the UI can show before
    /// the bridge finishes its own load.
    TerrainLoaded {
        width: u32,
        height: u32,
        height_min: f32,
        height_max: f32,
    },
    TerrainCleared,
    Frame(ViewportFrame),
}

/// Owns a [`NativeViewport`] on a dedicated thread.
pub struct ViewportWorker {
    tx: Sender<ViewportCommand>,
    join: std::sync::Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl ViewportWorker {
    pub fn spawn() -> (Self, Receiver<ViewportEvent>) {
        let (cmd_tx, cmd_rx) = channel::<ViewportCommand>();
        let (evt_tx, evt_rx) = channel::<ViewportEvent>();
        let join = std::thread::Builder::new()
            .name("studio-viewport".to_owned())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || viewport_loop(evt_tx, cmd_rx))
            .expect("spawn studio-viewport worker");
        (
            Self {
                tx: cmd_tx,
                join: std::sync::Arc::new(std::sync::Mutex::new(Some(join))),
            },
            evt_rx,
        )
    }

    pub fn send(&self, command: ViewportCommand) -> bool {
        self.tx.send(command).is_ok()
    }

    pub fn shutdown(&self) {
        if self.send(ViewportCommand::Shutdown) {
            if let Ok(mut slot) = self.join.lock() {
                if let Some(handle) = slot.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

fn viewport_loop(tx: Sender<ViewportEvent>, rx: Receiver<ViewportCommand>) {
    let mut viewport = match NativeViewport::new() {
        Ok(viewport) => {
            let (adapter, backend, software_fallback) = forge3d::core::gpu::try_ctx()
                .map(|context| {
                    let info = context.adapter.get_info();
                    (
                        info.name.clone(),
                        format!("{:?}", info.backend),
                        context.software_fallback,
                    )
                })
                .unwrap_or_else(|_| ("unknown".to_owned(), "unknown".to_owned(), false));
            if tx
                .send(ViewportEvent::Ready {
                    adapter,
                    backend,
                    software_fallback,
                })
                .is_err()
            {
                return;
            }
            viewport
        }
        Err(message) => {
            let _ = tx.send(ViewportEvent::Failed {
                context: "native viewport".to_owned(),
                message,
            });
            // Stay alive so commands drain instead of blocking the UI forever.
            while let Ok(command) = rx.recv() {
                if matches!(command, ViewportCommand::Shutdown) {
                    break;
                }
            }
            return;
        }
    };

    let mut last_size = (0u32, 0u32);
    let mut rendered_once = false;
    // The viewer's camera fields live in the terrain data, which a load
    // replaces; keep the last requested view here and re-apply it so the first
    // frame after a DEM load is framed like the one before it.
    let mut last_view: Option<(CameraParams, SunParams)> = None;

    while let Ok(command) = rx.recv() {
        match command {
            ViewportCommand::LoadDem { path } => match viewport.load_dem(&path) {
                Ok((width, height, height_min, height_max)) => {
                    rendered_once = false;
                    match last_view {
                        Some((camera, sun)) => viewport.set_view(&camera, &sun),
                        None => viewport.frame_default(),
                    }
                    let _ = tx.send(ViewportEvent::TerrainLoaded {
                        width,
                        height,
                        height_min,
                        height_max,
                    });
                }
                Err(message) => {
                    let _ = tx.send(ViewportEvent::Failed {
                        context: "native viewport DEM".to_owned(),
                        message,
                    });
                }
            },
            ViewportCommand::ClearDem => {
                viewport.clear_dem();
                rendered_once = false;
                let _ = tx.send(ViewportEvent::TerrainCleared);
            }
            ViewportCommand::SetLayerStyle { visible, opacity } => {
                viewport.set_layer_style(visible, opacity);
            }
            ViewportCommand::SetColormap(name) => {
                log::info!("viewport colormap request: {name:?}");
                if let Err(message) = viewport.set_colormap(name.as_deref()) {
                    let _ = tx.send(ViewportEvent::Failed {
                        context: "native viewport colormap".to_owned(),
                        message,
                    });
                }
            }
            ViewportCommand::SetView { camera, sun } => {
                last_view = Some((camera, sun));
                viewport.set_view(&camera, &sun);
            }
            ViewportCommand::Render {
                width,
                height,
                force,
            } => {
                if !viewport.has_terrain() {
                    continue;
                }
                let resized = last_size != (width, height);
                if !force && !resized && !rendered_once {
                    // First frame after a load always renders, even unforced.
                    rendered_once = true;
                }
                last_size = (width, height);
                match viewport.render_rgba(width, height) {
                    Ok(frame) => {
                        rendered_once = true;
                        let _ = tx.send(ViewportEvent::Frame(frame));
                    }
                    Err(message) => {
                        let _ = tx.send(ViewportEvent::Failed {
                            context: "native viewport render".to_owned(),
                            message,
                        });
                    }
                }
            }
            ViewportCommand::Shutdown => break,
        }
    }
}
