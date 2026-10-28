//! The Forge3D Studio main window.
//!
//! Layout follows GUI.md section 5: menu + toolbar, scene tree on the left,
//! the engine viewport in the center, properties on the right, and a status
//! bar at the bottom. Every control is wired to real application state and
//! real engine commands — there are no disconnected mockups.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Context, Response, Slider, TextureHandle, TextureOptions, Ui};
use studio_engine::{Command, EngineHandle, Event};
use studio_project::document::{DocHistory, LayerId, LayerKind};
use studio_project::{CameraParams, Layer, Project, SunParams};

use crate::actions::Action;
use crate::icons::Icons;
use crate::ui::dock::{self, DockState, LayoutPreset};
use crate::ui::palette::Palette;
use crate::ui::panels::layers::{self, LayersCommand, LayersState, LayersView};
use crate::ui::status::StatusParts;
use crate::ui::theme::{self, Tokens};
use crate::ui::{menu, shortcuts, status, toolbar};

const RENDER_DEBOUNCE: Duration = Duration::from_millis(120);
/// M4: how long a terrain-parameter edit settles before it is re-validated
/// by the engine (typing a value should not dispatch one job per keystroke).
const PARAMS_VALIDATE_DEBOUNCE: Duration = Duration::from_millis(700);
const MAX_STATUS_ERRORS: usize = 4;
/// Console history kept in the bottom dock.
const MAX_LOG_LINES: usize = 500;
/// Undo depth for document edits (whole-document snapshots).
const HISTORY_LIMIT: usize = 200;

struct GpuInfo {
    adapter: String,
    backend: String,
    software_fallback: bool,
}

/// One line in the bottom dock's Console tab.
struct LogEntry {
    error: bool,
    text: String,
}

/// M5: one signed RenderCertificate the session produced.
#[derive(Debug, Clone)]
struct CertRecord {
    source: String,
    path: PathBuf,
    payload_sha256: Option<String>,
    verified: Option<bool>,
}

struct PendingRender {
    width: u32,
    height: u32,
    requested_at: Instant,
}

/// Which scene pipeline the viewport is showing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SceneMode {
    Terrain,
    MapScene,
}

/// What a dispatched engine job was for, so `JobDone` can be routed.
#[derive(Debug, Clone)]
enum PendingJob {
    MapSceneTemplate,
    MapSceneRender,
    /// U2: document -> SceneRecipe compilation.
    SceneCompile {
        render_after: bool,
    },
    /// M5: verify one stored certificate.
    CertificateVerify {
        index: usize,
    },
    /// M5: two-run SHA-256 comparison of the current frame.
    DeterminismCheck,
    /// M5: degradation ledger + memory telemetry snapshot.
    DiagnosticsSnapshot,
    MapSceneValidate,
    ColormapsList,
    PresetsList,
    /// Unified import dialog metadata preview (M3).
    DatasetInspect {
        path: PathBuf,
    },
    /// Camera-rig keyframe bake (W1).
    RigBake,
    /// Renderer-preset fetch for sun mapping (W1).
    PresetApply,
    /// One frame of a queued render-sequence job (W1).
    SequenceFrame {
        out_dir: PathBuf,
        total: u32,
    },
    /// M4: the terrain-parameter schema (warm once, at engine ready).
    ParamsSchema,
    /// M4: validation of the current terrain params against the engine.
    ParamsValidate,
}

/// One baked camera-rig keyframe in renderer world units.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RigKeyframe {
    pub t: f32,
    pub phi_deg: f32,
    pub theta_deg: f32,
    pub radius: f32,
    pub fov_deg: f32,
}

impl Default for RigKeyframe {
    fn default() -> Self {
        Self {
            t: 0.0,
            phi_deg: 0.0,
            theta_deg: 45.0,
            radius: 1000.0,
            fov_deg: 55.0,
        }
    }
}

impl RigKeyframe {
    /// Linear interpolation between two baked keyframes (the bake samples
    /// rigs densely — 12 Hz default — so linear replay is visually exact).
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let span = (b.t - a.t).max(1e-6);
        let alpha = ((t - a.t) / span).clamp(0.0, 1.0);
        Self {
            t,
            phi_deg: a.phi_deg + (b.phi_deg - a.phi_deg) * alpha,
            theta_deg: a.theta_deg + (b.theta_deg - a.theta_deg) * alpha,
            radius: a.radius + (b.radius - a.radius) * alpha,
            fov_deg: a.fov_deg + (b.fov_deg - a.fov_deg) * alpha,
        }
    }

    fn at(keyframes: &[Self], t: f32) -> Option<Self> {
        if keyframes.is_empty() {
            return None;
        }
        if t <= keyframes[0].t {
            return Some(keyframes[0]);
        }
        for pair in keyframes.windows(2) {
            if t <= pair[1].t {
                return Some(Self::lerp(&pair[0], &pair[1], t));
            }
        }
        keyframes.last().copied()
    }
}

/// Persisted camera-rig spec + baked keyframes (`project.camera_rig_json`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RigSpec {
    pub kind: String,
    pub duration: f32,
    pub phi_start_deg: f32,
    pub phi_end_deg: f32,
    pub theta_start_deg: f32,
    /// Orbit radius as a fraction of max(DEM dimension); baked keyframes
    /// carry the resolved renderer-unit radii.
    pub radius_rel: f32,
    /// Rail/follow path, one "x,z" pair per line in normalized [0,1] DEM
    /// coordinates.
    pub path_text: String,
    pub samples_per_second: u32,
    pub keyframes: Vec<RigKeyframe>,
}

impl Default for RigSpec {
    fn default() -> Self {
        Self {
            kind: "orbit".to_owned(),
            duration: 8.0,
            phi_start_deg: 0.0,
            phi_end_deg: 360.0,
            theta_start_deg: 45.0,
            // The baked eye must stay inside the DEM bounds (the engine
            // validates this); 1.0 put it outside on every real dataset.
            radius_rel: 0.32,
            path_text: "0.15,0.15\n0.5,0.5\n0.85,0.85".to_owned(),
            samples_per_second: 12,
            keyframes: Vec::new(),
        }
    }
}

/// Timeline/playback state for the baked rig (not persisted).
struct RigPlayback {
    playing: bool,
    frame: u32,
    play_started: Option<Instant>,
    play_t0: f32,
}

/// In-flight multi-frame sequence render (W1 "render sequence").
struct SequenceState {
    out_dir: PathBuf,
    total: u32,
    done: u32,
    failed: Vec<String>,
}

/// A metadata preview from `dataset.inspect`, awaiting user action in the
/// import dialog.
struct ImportPreview {
    path: PathBuf,
    result: serde_json::Value,
}

/// State of the MapScene (recipe) scene mode.
#[derive(Debug, Clone, Default)]
struct MapScenePanel {
    template: String,
    /// U2: scene camera (compiles into the recipe).
    camera_distance: f64,
    camera_azimuth: f64,
    camera_elevation: f64,
    camera_fov: f64,
    /// U2: diagnostics from the last `scene.compile`.
    compile_diagnostics: Vec<serde_json::Value>,
    raster_path: Option<PathBuf>,
    label_text: String,
    recipe_json: Option<serde_json::Value>,
    last_status: Option<String>,
    last_backend: Option<String>,
    last_diagnostics: Vec<String>,
    last_metadata: Option<serde_json::Value>,
}

pub struct StudioApp {
    engine: EngineHandle,
    events: std::sync::mpsc::Receiver<Event>,
    icons: Icons,
    project: Project,
    project_path: Option<PathBuf>,
    project_dirty: bool,
    gpu: Option<GpuInfo>,
    busy: bool,
    errors: VecDeque<String>,
    status: Option<String>,
    viewport_texture: Option<TextureHandle>,
    last_frame_ms: Option<f32>,
    pending_render: Option<PendingRender>,
    viewport_rect: egui::Rect,
    theme_dark: bool,
    scene_mode: SceneMode,
    mapscene: MapScenePanel,
    pending_jobs: std::collections::HashMap<u64, PendingJob>,
    colormaps: Vec<String>,
    presets: Vec<String>,
    mapscene_certificate: bool,
    /// M3: unified import dialog preview state.
    import_preview: Option<ImportPreview>,
    /// W1: camera-rig spec, baked keyframes, and timeline playback.
    rig: RigSpec,
    rig_playback: RigPlayback,
    /// W1: in-flight sequence render, if any.
    sequence: Option<SequenceState>,
    /// M4: `terrain.params_schema` — the sections, fields, defaults, enum
    /// choices and pinned params introspected from the engine's
    /// `make_terrain_params_config` (None until the engine reports ready).
    params_schema: Option<serde_json::Value>,
    /// M4: why the schema request failed, if it did — shown in place of the
    /// pages so a broken bridge is visible instead of an eternal "loading".
    params_schema_error: Option<String>,
    /// M4: first line of the engine's rejection of the current params, shown
    /// inline under the pages (None = the current params validate).
    params_validation: Option<String>,
    /// M4: when the debounced `terrain.params_validate` job should fire.
    params_validate_due: Option<Instant>,
    /// M4: a validate job is in flight; further edits wait for it.
    params_validate_pending: bool,
    /// M4: text being typed into a parse-on-commit field (nullable numbers
    /// and arrays), so partial input like "1." survives across frames.
    param_drafts: HashMap<String, String>,
    /// U0: dock sizes, active tab per slot and the last layout preset.
    dock: DockState,
    /// U0: the dock state we last wrote to disk (save on change, not per frame).
    saved_dock: DockState,
    /// U0: design tokens for the active theme (panel fill, density, accent).
    tokens: Tokens,
    /// U0: `Ctrl+K` command palette.
    palette: Palette,
    /// U0: the keyboard reference window.
    shortcuts_open: bool,
    /// U0: console lines (newest last), fed by errors and status messages.
    log: VecDeque<LogEntry>,
    /// Last renderer preset applied to the scene (shown in the Inspector).
    applied_preset: Option<String>,
    /// U1: the selected layers (topmost-first order, like the panel) and the
    /// primary one the Inspector edits.
    selection: Vec<LayerId>,
    /// U1: transient Layers-panel state (drag, inline rename).
    layers_ui: LayersState,
    /// U1: undo/redo over the document.
    history: DocHistory,
    /// U1: the layer whose opacity is being dragged, so one gesture is one
    /// undo step instead of one per frame.
    opacity_edit: Option<LayerId>,
    /// U4: the examples gallery window + its thumbnails.
    gallery: crate::ui::examples::GalleryState,
    /// U4: in-flight `git lfs pull` for an example fixture (receiver end).
    lfs_fetch: Option<std::sync::mpsc::Receiver<std::io::Result<bool>>>,
    /// U3: the viewport tool owning the left mouse button.
    viewport_tool: crate::ui::viewport::ViewportTool,
    /// U3: draw the safe-frame overlay in the viewport.
    show_safe_frame: bool,
    /// U3: A/B split against the last exported frame.
    compare_mode: bool,
    compare_split: f32,
    /// M5: request a signed RenderCertificate for every terrain render.
    request_certificate: bool,
    /// M5: certificates seen this session (path + sha + verify state).
    certificates: Vec<CertRecord>,
    /// M5: last diagnostics snapshot (degradations + memory).
    stats: Option<serde_json::Value>,
    compare_texture: Option<TextureHandle>,
    /// U4: the gallery entry waiting for its dataset to load.
    pending_example: Option<crate::ui::examples::ExampleEntry>,
    /// U4: camera override for the next MapScene template build.
    pending_mapscene_camera: Option<serde_json::Value>,
    /// U4: render the MapScene recipe as soon as the template job returns.
    render_after_recipe: bool,
    /// M10: GPU-shared native viewport (egui-native texture, no readback).
    native: NativeViewportState,
}

/// M10: the GPU-shared viewport's lifecycle.
///
/// The scene is built on a worker thread (shader compilation is the expensive
/// part), then owned by the UI thread and re-rendered whenever the view
/// changes. `texture_id` is the egui-side registration of the scene's sRGB
/// view; `registered_generation` tracks target reallocation on resize.
struct NativeViewportState {
    /// eframe's wgpu render state — the device/queue shared with the engine.
    wgpu: Option<eframe::egui_wgpu::RenderState>,
    viewport: Option<studio_engine::NativeViewport>,
    /// Scene construction in flight (worker thread).
    building: Option<std::sync::mpsc::Receiver<Result<studio_engine::NativeViewport, String>>>,
    texture_id: Option<egui::TextureId>,
    registered_generation: u64,
    /// Last requested render size; native frames re-render when it changes.
    size: (u32, u32),
    /// A DEM that arrived while the scene was still being built.
    pending_load: Option<(PathBuf, f32)>,
    /// Set on camera/sun/size/terrain changes; cleared once a frame renders.
    dirty: bool,
    /// Colormap currently pushed into the scene (None = Studio's relief
    /// ramp). Kept separate from `Project::colormap` so one failed lookup is
    /// reported once instead of every frame.
    colormap_synced: Option<String>,
    /// Why the native path is unavailable, if it is. The app then keeps the
    /// bridge readback and says so instead of silently degrading.
    error: Option<String>,
}

impl Default for NativeViewportState {
    fn default() -> Self {
        Self {
            wgpu: None,
            viewport: None,
            building: None,
            texture_id: None,
            registered_generation: 0,
            size: (0, 0),
            pending_load: None,
            dirty: true,
            colormap_synced: None,
            error: None,
        }
    }
}

impl NativeViewportState {
    /// Start building the native scene if eframe gave us a wgpu device.
    fn start(cc: &eframe::CreationContext<'_>) -> Self {
        let Some(render_state) = cc.wgpu_render_state.clone() else {
            return Self {
                error: Some("eframe has no wgpu render state".to_owned()),
                ..Self::default()
            };
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let device = render_state.device.clone();
        let queue = render_state.queue.clone();
        let adapter = render_state.adapter.clone();
        let spawned = std::thread::Builder::new()
            .name("studio-native-gpu".to_owned())
            // The terrain scene compiles the mega-shaders recursively; the
            // default test/worker stack overflows (same reason the engine
            // worker uses a big stack).
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let result = studio_engine::NativeViewport::new(device, queue, adapter);
                let _ = tx.send(result);
            });
        if spawned.is_err() {
            return Self {
                error: Some("could not start the native viewport thread".to_owned()),
                ..Self::default()
            };
        }
        Self {
            wgpu: Some(render_state),
            building: Some(rx),
            ..Self::default()
        }
    }

    /// True when the interactive viewport can paint from the native scene.
    fn is_live(&self) -> bool {
        self.viewport
            .as_ref()
            .map(|viewport| viewport.has_terrain())
            .unwrap_or(false)
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// Resolve a dataset path that may be relative to the repository root.
///
/// Gallery scenes and project files reference engine fixtures like
/// `assets/tif/Mount_Fuji_30m.tif`; those are only meaningful relative to
/// the checkout, never the process CWD (which depends on how the binary was
/// launched). Absolute paths pass through untouched; missing relative paths
/// are returned unchanged so the engine's own error text stays honest.
pub(crate) fn resolve_repo_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let candidate = root.join(path);
    if candidate.exists() {
        candidate.canonicalize().unwrap_or(candidate)
    } else {
        path.to_path_buf()
    }
}

/// True when a file exists but is an unresolved git-LFS pointer rather than
/// its content (the fixture was not materialized with `git lfs pull`).
pub(crate) fn is_lfs_pointer(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 32];
    match file.read_exact(&mut head) {
        Ok(()) => head.starts_with(b"version https://git-lfs"),
        Err(_) => false,
    }
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx, true);
        let icons = Icons::load(&cc.egui_ctx, Color32::from_white_alpha(230));

        let (engine, events) = EngineHandle::spawn();
        let dock = DockState::load();
        Self {
            engine,
            events,
            icons,
            project: Project::new(),
            project_path: None,
            project_dirty: false,
            gpu: None,
            busy: false,
            errors: VecDeque::new(),
            status: None,
            viewport_texture: None,
            last_frame_ms: None,
            pending_render: None,
            viewport_rect: egui::Rect::ZERO,
            theme_dark: true,
            scene_mode: SceneMode::Terrain,
            mapscene: MapScenePanel {
                template: "terrain".to_owned(),
                camera_distance: 1.2,
                camera_azimuth: 25.0,
                camera_elevation: 45.0,
                camera_fov: 45.0,
                ..Default::default()
            },
            pending_jobs: std::collections::HashMap::new(),
            colormaps: Vec::new(),
            presets: Vec::new(),
            mapscene_certificate: false,
            import_preview: None,
            rig: RigSpec::default(),
            rig_playback: RigPlayback {
                playing: false,
                frame: 0,
                play_started: None,
                play_t0: 0.0,
            },
            sequence: None,
            params_schema: None,
            params_schema_error: None,
            params_validation: None,
            params_validate_due: None,
            params_validate_pending: false,
            param_drafts: HashMap::new(),
            saved_dock: dock.clone(),
            dock,
            tokens: Tokens::dark(),
            palette: Palette::default(),
            shortcuts_open: false,
            log: VecDeque::new(),
            applied_preset: None,
            selection: Vec::new(),
            layers_ui: LayersState::default(),
            history: DocHistory::new(HISTORY_LIMIT),
            opacity_edit: None,
            gallery: crate::ui::examples::GalleryState::default(),
            lfs_fetch: None,
            viewport_tool: crate::ui::viewport::ViewportTool::Orbit,
            show_safe_frame: false,
            compare_mode: false,
            compare_split: 0.5,
            request_certificate: false,
            certificates: Vec::new(),
            stats: None,
            compare_texture: None,
            pending_example: None,
            pending_mapscene_camera: None,
            render_after_recipe: false,
            native: NativeViewportState::start(cc),
        }
    }

    fn push_error(&mut self, context: impl Into<String>, message: impl Into<String>) {
        let entry = format!("{}: {}", context.into(), message.into());
        log::error!("{entry}");
        if self.errors.back() != Some(&entry) {
            self.errors.push_back(entry);
        }
        while self.errors.len() > MAX_STATUS_ERRORS {
            self.errors.pop_front();
        }
        let text = self.errors.back().cloned().unwrap_or_default();
        self.push_log(true, text);
    }

    fn set_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = Some(message.clone());
        self.push_log(false, message);
    }

    /// Append a line to the Console tab, keeping a bounded history.
    fn push_log(&mut self, error: bool, text: String) {
        if self.log.back().map(|entry| entry.text.as_str()) == Some(text.as_str()) {
            return;
        }
        self.log.push_back(LogEntry { error, text });
        while self.log.len() > MAX_LOG_LINES {
            self.log.pop_front();
        }
    }

    fn sync_window_title(&self, ctx: &Context) {
        let name = &self.project.name;
        let file = self
            .project_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unsaved".to_owned());
        let dirty = if self.project_dirty { "*" } else { "" };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{name}{dirty} — Forge3D Studio ({file})"
        )));
    }

    fn apply_theme(&mut self, ctx: &Context, dark: bool) {
        self.theme_dark = dark;
        self.tokens = Tokens::for_theme(dark);
        theme::apply(ctx, dark);
        let fg = if dark {
            Color32::from_white_alpha(230)
        } else {
            Color32::from_black_alpha(235)
        };
        self.icons = Icons::load(ctx, fg);
    }

    // ---------------------------------------------------------------- events

    fn poll_engine_events(&mut self, ctx: &Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Ready {
                    adapter,
                    backend,
                    software_fallback,
                } => {
                    self.gpu = Some(GpuInfo {
                        adapter,
                        backend,
                        software_fallback,
                    });
                    // Warm the catalogs the UI dropdowns need.
                    self.pending_jobs.insert(
                        self.engine.call("colormaps.list", serde_json::json!({})),
                        PendingJob::ColormapsList,
                    );
                    self.pending_jobs.insert(
                        self.engine.call("presets.list", serde_json::json!({})),
                        PendingJob::PresetsList,
                    );
                    // M4: the property pages are schema-driven — warm the
                    // schema once so the panel paints complete on first use.
                    self.pending_jobs.insert(
                        self.engine
                            .call("terrain.params_schema", serde_json::json!({})),
                        PendingJob::ParamsSchema,
                    );
                }
                Event::TerrainLoaded {
                    path,
                    width,
                    height,
                    height_min,
                    height_max,
                    camera,
                    sun,
                } => {
                    self.project.terrain = Some(studio_project::TerrainRef {
                        path: path.clone(),
                        width,
                        height,
                        height_min,
                        height_max,
                    });
                    self.project.terrain_visible = true;
                    self.project.camera = camera;
                    self.project.sun = sun;
                    self.project_dirty = true;
                    self.last_frame_ms = None;
                    self.project.sync_terrain_layer();
                    self.selection = self
                        .project
                        .document
                        .terrain_layer()
                        .map(|layer| vec![layer.id])
                        .unwrap_or_default();
                    self.history.clear();
                    self.layers_ui = LayersState::default();
                    // M10: the native viewport shares the engine's wgpu
                    // device, so it renders from the same DEM. `span` mirrors
                    // the bridge's world scale (`max(range, 1) * 6`).
                    let span = (height_max - height_min).abs().max(1.0) * 6.0;
                    self.load_native_dem(&path, span);
                    self.request_viewport_render();
                    self.set_status(format!("Loaded {} ({}×{})", path.display(), width, height));
                    if self.pending_example.is_some() {
                        self.apply_pending_example();
                    }
                }
                Event::TerrainCleared => {
                    if let Some(viewport) = self.native.viewport.as_mut() {
                        viewport.clear_dem();
                    }
                    self.native.pending_load = None;
                    self.native.dirty = true;
                    self.project.terrain = None;
                    self.project.terrain_visible = true;
                    self.project.sync_terrain_layer();
                    self.selection.clear();
                    self.history.clear();
                    self.project_dirty = true;
                    self.set_status("Terrain unloaded");
                }
                Event::FrameReady {
                    width,
                    height,
                    rgba,
                    render_ms,
                } => {
                    // M5: the worker writes a signed certificate next to the
                    // scratch frame when signing is on; record it.
                    if self.request_certificate {
                        let cert_path = std::env::temp_dir().join("forge3d-studio-frame.cert.json");
                        if let Ok(text) = std::fs::read_to_string(&cert_path) {
                            let sha = serde_json::from_str::<serde_json::Value>(&text)
                                .ok()
                                .and_then(|cert| {
                                    cert.get("payload_sha256")
                                        .and_then(|v| v.as_str().map(String::from))
                                });
                            let record = CertRecord {
                                source: "terrain".to_owned(),
                                path: cert_path.clone(),
                                payload_sha256: sha,
                                verified: None,
                            };
                            if !self.certificates.iter().any(|c| {
                                c.path == record.path && c.payload_sha256 == record.payload_sha256
                            }) {
                                self.certificates.push(record);
                            }
                        }
                    }
                    // M10: while the native viewport paints, a readback frame
                    // is only a timing sample — its pixels are stale by the
                    // time they arrive and are not shown.
                    if !self.native_paints() {
                        let texture = self.viewport_texture.get_or_insert_with(|| {
                            ctx.load_texture(
                                "studio-viewport",
                                ColorImage::new([1, 1], Color32::BLACK),
                                TextureOptions::LINEAR,
                            )
                        });
                        texture.set(
                            ColorImage::from_rgba_unmultiplied(
                                [width as usize, height as usize],
                                &rgba,
                            ),
                            TextureOptions::LINEAR,
                        );
                    }
                    self.last_frame_ms = Some(render_ms);
                    ctx.request_repaint();
                }
                Event::ExportFinished {
                    path,
                    width,
                    height,
                    render_ms,
                } => {
                    self.set_status(format!(
                        "Exported {} ({}×{}, {render_ms:.0} ms)",
                        path.display(),
                        width,
                        height
                    ));
                }
                Event::ExportFailed { path, message } => {
                    self.push_error(format!("Exporting {}", path.display()), message);
                }
                Event::Error { context, message } => {
                    self.push_error(context, message);
                }
                Event::JobDone { job, op, result } => {
                    let kind = self.pending_jobs.remove(&job);
                    self.handle_job_done(&op, kind, result, ctx);
                }
                Event::JobFailed { job, op, message } => {
                    let kind = self.pending_jobs.remove(&job);
                    self.handle_job_failed(&op, kind, &message);
                }
                Event::Busy(busy) => {
                    self.busy = busy;
                }
            }
        }
    }

    // -------------------------------------------------------------- actions

    fn request_viewport_render(&mut self) {
        // M10: with a live native viewport the frame is drawn on the engine's
        // own device, so the request is just "the view changed".
        if self.native_paints() {
            self.native.mark_dirty();
            return;
        }
        if self.busy {
            return;
        }
        self.queue_viewport_render();
    }

    /// Queue a viewport render regardless of the busy flag. Used when a job's
    /// own completion invalidates the frame: `JobDone` arrives before
    /// `Busy(false)`, so the gated request would be dropped.
    fn queue_viewport_render(&mut self) {
        let size = self.viewport_rect.size();
        let width = (size.x.round() as u32).max(1);
        let height = (size.y.round() as u32).max(1);
        self.pending_render = Some(PendingRender {
            width,
            height,
            requested_at: Instant::now(),
        });
    }

    /// Drop the current frame and request a fresh one (used for resizes and
    /// layer visibility changes where the old image must not linger).
    fn invalidate_viewport(&mut self) {
        self.viewport_texture = None;
        self.request_viewport_render();
    }

    fn flush_pending_render(&mut self) {
        if self.busy {
            return;
        }
        let due = self
            .pending_render
            .as_ref()
            .map(|p| p.requested_at.elapsed() >= RENDER_DEBOUNCE)
            .unwrap_or(false);
        if due {
            let pending = self.pending_render.take().expect("checked");
            self.engine.send(Command::RenderFrame {
                width: pending.width,
                height: pending.height,
            });
        }
    }

    // ------------------------------------------------- M10: native GPU path

    /// True when the viewport paints from the native scene: terrain mode with
    /// a loaded native terrain. MapScene renders through the bridge and owns
    /// the panel in its own mode.
    fn native_paints(&self) -> bool {
        self.scene_mode == SceneMode::Terrain && self.native.is_live()
    }

    /// Image the viewport paints: the native scene's texture when it is live,
    /// otherwise the bridge's frame (readback or MapScene PNG).
    fn viewport_image(&self) -> Option<egui::TextureId> {
        if self.native_paints() {
            if let Some(id) = self.native.texture_id {
                return Some(id);
            }
        }
        self.viewport_texture.as_ref().map(|texture| texture.id())
    }

    /// Advance the native viewport's handshake with its worker thread.
    ///
    /// The scene build is slow (it compiles the terrain shaders), so it runs
    /// off the UI thread; a DEM that arrives meanwhile waits in
    /// `pending_load`.
    fn poll_native_viewport(&mut self, ctx: &Context) {
        let received = self.native.building.as_ref().map(|rx| rx.try_recv());
        match received {
            None | Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {}
            Some(Ok(Ok(viewport))) => {
                self.native.building = None;
                self.native.viewport = Some(viewport);
                self.native.dirty = true;
                self.push_log(
                    false,
                    "Native GPU viewport ready — the viewport now paints on the engine's own device (no readback, no PNG round-trip)".to_owned(),
                );
                ctx.request_repaint();
            }
            Some(Ok(Err(error))) => {
                self.native.building = None;
                self.native_failed(error);
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.native.building = None;
                self.native_failed(
                    "the native viewport thread exited while building the scene".to_owned(),
                );
            }
        }
        self.apply_pending_native_load();
    }

    /// Feed a DEM that arrived while the scene was still being built.
    fn apply_pending_native_load(&mut self) {
        let Some((path, span)) = self.native.pending_load.clone() else {
            return;
        };
        if self.native.viewport.is_none() {
            return;
        }
        self.native.pending_load = None;
        self.load_native_dem(&path, span);
    }

    /// Record why the native path cannot paint, and hand the viewport back to
    /// the bridge readback instead of leaving a blank panel.
    fn native_failed(&mut self, error: String) {
        self.release_native_texture();
        self.native.viewport = None;
        self.native.pending_load = None;
        let already_reported = self.native.error.as_deref() == Some(error.as_str());
        self.native.error = Some(error.clone());
        if !already_reported {
            self.push_error("Native viewport", error);
        }
        self.queue_viewport_render();
    }

    /// Drop the egui registration of the native texture.
    fn release_native_texture(&mut self) {
        let Some(id) = self.native.texture_id.take() else {
            return;
        };
        if let Some(state) = self.native.wgpu.as_ref() {
            state.renderer.write().free_texture(&id);
        }
        self.native.registered_generation = 0;
    }

    /// Mirror the document's DEM into the native scene. Queued when the scene
    /// is still compiling.
    fn load_native_dem(&mut self, path: &Path, span: f32) {
        let loaded = self
            .native
            .viewport
            .as_mut()
            .map(|viewport| viewport.load_dem(path, span));
        match loaded {
            Some(Ok(())) => {
                self.native.dirty = true;
                // A fresh terrain re-derives its palette.
                self.native.colormap_synced = None;
                self.sync_native_colormap();
            }
            Some(Err(error)) => self.native_failed(error),
            None => {
                if self.native.building.is_some() {
                    self.native.pending_load = Some((path.to_path_buf(), span));
                }
            }
        }
    }

    /// Push the document's colormap into the native scene when it changed.
    fn sync_native_colormap(&mut self) {
        let wanted = self.project.colormap.clone();
        if self.native.colormap_synced == wanted {
            return;
        }
        let result = self
            .native
            .viewport
            .as_mut()
            .map(|viewport| viewport.set_colormap(wanted.as_deref()));
        // Record the attempt either way: a rejected name keeps the previous
        // palette and is reported once, not once per rendered frame.
        self.native.colormap_synced = wanted;
        self.native.dirty = true;
        if let Some(Err(error)) = result {
            self.push_error("Colormap", error);
        }
    }

    /// Render the native scene into its offscreen target and keep the egui
    /// registration in step. Does nothing when the view is unchanged.
    fn render_native_viewport(&mut self, ctx: &Context) {
        let Some(render_state) = self.native.wgpu.clone() else {
            return;
        };
        if self.native.viewport.is_none() || self.scene_mode != SceneMode::Terrain {
            return;
        }
        self.sync_native_colormap();

        let size = self.viewport_rect.size();
        let width = (size.x.round() as u32).max(1);
        let height = (size.y.round() as u32).max(1);
        let has_terrain = self
            .native
            .viewport
            .as_ref()
            .map(|viewport| viewport.has_terrain())
            .unwrap_or(false);
        if !has_terrain || width < 2 || height < 2 {
            return;
        }
        if !self.native.dirty && self.native.size == (width, height) {
            return;
        }

        let camera = self.project.camera;
        let sun = self.project.sun;
        let (result, generation, render_ms) = {
            let viewport = match self.native.viewport.as_mut() {
                Some(viewport) => viewport,
                None => return,
            };
            viewport.set_view(&camera, &sun);
            let result = viewport.render(width, height);
            (
                result,
                viewport.texture_generation(),
                viewport.last_render_ms(),
            )
        };
        if let Err(error) = result {
            self.native_failed(error);
            return;
        }
        self.native.size = (width, height);
        self.native.dirty = false;
        self.last_frame_ms = Some(render_ms);

        if self.native.texture_id.is_none() || generation != self.native.registered_generation {
            let present = self
                .native
                .viewport
                .as_ref()
                .and_then(|viewport| viewport.present_view());
            let Some(present) = present else {
                return;
            };
            let mut renderer = render_state.renderer.write();
            if let Some(stale) = self.native.texture_id.take() {
                renderer.free_texture(&stale);
            }
            self.native.texture_id = Some(renderer.register_native_texture(
                &render_state.device,
                present,
                eframe::wgpu::FilterMode::Linear,
            ));
            self.native.registered_generation = generation;
        }
        ctx.request_repaint();
    }

    // ------------------------------------------------------------ engine ops

    /// Route a completed generic engine job.
    fn handle_job_done(
        &mut self,
        op: &str,
        kind: Option<PendingJob>,
        result: serde_json::Value,
        ctx: &Context,
    ) {
        match kind {
            Some(PendingJob::MapSceneTemplate) => match result.get("recipe") {
                Some(recipe) => {
                    self.mapscene.recipe_json = Some(recipe.clone());
                    self.project.scene_recipe_json =
                        Some(serde_json::to_string(recipe).unwrap_or_default());
                    self.project_dirty = true;
                    self.sync_document_from_recipe(recipe);
                    self.set_status(format!(
                        "MapScene scene built from template '{op}' ({} layers) —                  the Layers panel now drives it",
                        recipe
                            .get("layers")
                            .and_then(|l| l.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0)
                    ));
                    if self.render_after_recipe {
                        self.render_after_recipe = false;
                        self.compile_and_stage_recipe(true);
                    }
                }
                None => self.push_error("MapScene template", "no recipe in result"),
            },
            Some(PendingJob::SceneCompile { render_after }) => match result.get("recipe") {
                Some(recipe) => {
                    self.mapscene.recipe_json = Some(recipe.clone());
                    self.project.scene_recipe_json =
                        Some(serde_json::to_string(recipe).unwrap_or_default());
                    self.project_dirty = true;
                    let diagnostics = result
                        .get("diagnostics")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    self.mapscene.compile_diagnostics = diagnostics;
                    if render_after {
                        self.render_mapscene();
                    } else {
                        self.set_status(format!(
                            "Scene compiled ({} recipe layer(s))",
                            result
                                .get("layer_count")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0)
                        ));
                    }
                }
                None => self.push_error("Scene compile", "no recipe in result"),
            },
            Some(PendingJob::MapSceneRender) => {
                let png = result
                    .get("png")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from);
                let status = result
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_owned();
                let backend = result
                    .get("backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_owned();
                let diagnostics = result
                    .get("diagnostics")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|d| d.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.mapscene.last_status = Some(status.clone());
                self.mapscene.last_backend = Some(backend.clone());
                self.mapscene.last_diagnostics = diagnostics;
                self.mapscene.last_metadata = Some(result.clone());
                if let Some(cert_path) = result
                    .get("certificate_path")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                {
                    let sha = std::fs::read_to_string(&cert_path)
                        .ok()
                        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                        .and_then(|cert| {
                            cert.get("payload_sha256")
                                .and_then(|v| v.as_str().map(String::from))
                        });
                    self.certificates.push(CertRecord {
                        source: "mapscene".to_owned(),
                        path: cert_path,
                        payload_sha256: sha,
                        verified: None,
                    });
                }
                match png {
                    Some(path) => {
                        if self.load_png_to_viewport(&path, ctx) {
                            self.set_status(format!("MapScene rendered ({status}, {backend})"));
                        } else {
                            self.push_error(
                                "MapScene render",
                                format!("rendered but could not decode {}", path.display()),
                            );
                        }
                    }
                    None => self.push_error("MapScene render", "no png path in result"),
                }
            }
            Some(PendingJob::MapSceneValidate) => {
                let status = result
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_owned();
                self.mapscene.last_status = Some(status.clone());
                self.set_status(format!("MapScene validation: {status}"));
            }
            Some(PendingJob::CertificateVerify { index }) => {
                let valid = result.get("valid").and_then(|v| v.as_bool());
                if let Some(cert) = self.certificates.get_mut(index) {
                    cert.verified = valid;
                    if let Some(sha) = result.get("payload_sha256").and_then(|v| v.as_str()) {
                        cert.payload_sha256 = Some(sha.to_owned());
                    }
                }
                match valid {
                    Some(true) => self.set_status("Certificate verified (development key)."),
                    Some(false) => {
                        self.push_error("Certificate", "signature INVALID for this certificate")
                    }
                    None => self.push_error("Certificate", "verify returned no verdict"),
                }
            }
            Some(PendingJob::DeterminismCheck) => {
                let sha_a = result.get("sha_a").and_then(|v| v.as_str()).unwrap_or("?");
                let sha_b = result.get("sha_b").and_then(|v| v.as_str()).unwrap_or("?");
                let matched = result
                    .get("match")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if matched {
                    self.set_status(format!(
                        "Determinism check: PASS — both runs produced {sha_a}"
                    ));
                } else {
                    self.push_error(
                        "Determinism check",
                        format!("FAIL — run A {sha_a} vs run B {sha_b}"),
                    );
                }
            }
            Some(PendingJob::DiagnosticsSnapshot) => {
                self.stats = Some(result.clone());
                self.set_status("Statistics refreshed");
            }
            Some(PendingJob::ColormapsList) => {
                if let Some(list) = result.get("colormaps").and_then(|c| c.as_array()) {
                    self.colormaps = list
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                }
            }
            Some(PendingJob::PresetsList) => {
                if let Some(list) = result.get("presets").and_then(|c| c.as_array()) {
                    self.presets = list
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                }
            }
            Some(PendingJob::PresetApply) => {
                // Map the preset's first directional light onto the terrain
                // viewport sun (two different lighting models — the mapping
                // is directional, with the intensity scaled into our slider).
                let preset = result.get("preset").cloned().unwrap_or_default();
                let light = preset
                    .pointer("/lighting/lights/0")
                    .filter(|l| {
                        l.get("type")
                            .and_then(|t| t.as_str())
                            .map(|t| t.eq_ignore_ascii_case("directional"))
                            .unwrap_or(false)
                    })
                    .or_else(|| preset.pointer("/lighting/lights/0"));
                let Some(light) = light else {
                    self.push_error("Preset", "preset has no directional light to map");
                    return;
                };
                let dir: Vec<f64> = light
                    .get("direction")
                    .and_then(|d| d.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                    .unwrap_or_default();
                let intensity = light
                    .get("intensity")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(3.0);
                if dir.len() != 3 {
                    self.push_error("Preset", "directional light direction must be 3D");
                    return;
                }
                // Preset directions point from the light towards the ground;
                // the terrain sun convention is ground → sun.
                let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2])
                    .sqrt()
                    .max(1e-9);
                let (sx, sy, sz) = (-dir[0] / len, -dir[1] / len, -dir[2] / len);
                let elevation = sy.clamp(-1.0, 1.0).asin().to_degrees().clamp(0.0, 90.0);
                let azimuth = sx.atan2(sz).to_degrees().rem_euclid(360.0);
                self.project.sun.azimuth_deg = azimuth as f32;
                self.project.sun.elevation_deg = elevation as f32;
                self.project.sun.intensity = (intensity * 0.5).clamp(0.0, 10.0) as f32;
                self.project_dirty = true;
                self.engine.send(Command::SetSun(self.project.sun));
                self.invalidate_viewport();
                let name = result
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_owned();
                self.applied_preset = Some(name.clone());
                self.set_status(format!(
                    "Preset '{name}' → sun az {azimuth:.0}° el {elevation:.0}° (intensity ×0.5); exposure/shadows stay manual"
                ));
            }
            Some(PendingJob::DatasetInspect { path }) => {
                self.import_preview = Some(ImportPreview { path, result });
                self.set_status("Dataset inspected — review the preview");
            }
            Some(PendingJob::RigBake) => {
                let keyframes: Vec<RigKeyframe> = result
                    .get("keyframes")
                    .and_then(|k| k.as_array())
                    .map(|list| {
                        list.iter()
                            .map(|kf| RigKeyframe {
                                t: kf.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                                phi_deg: kf.get("phi_deg").and_then(|v| v.as_f64()).unwrap_or(0.0)
                                    as f32,
                                theta_deg: kf
                                    .get("theta_deg")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(45.0)
                                    as f32,
                                radius: kf.get("radius").and_then(|v| v.as_f64()).unwrap_or(1000.0)
                                    as f32,
                                fov_deg: kf.get("fov_deg").and_then(|v| v.as_f64()).unwrap_or(55.0)
                                    as f32,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if keyframes.is_empty() {
                    self.push_error("Camera rig", "bake returned no keyframes");
                } else {
                    let count = keyframes.len();
                    self.rig.keyframes = keyframes;
                    self.rig_playback.frame = 0;
                    self.rig_playback.playing = false;
                    self.rig_playback.play_started = None;
                    // Persist spec + keyframes into the project.
                    self.project.camera_rig_json = serde_json::to_string(&self.rig).ok();
                    self.project_dirty = true;
                    // Jump the viewport to the first keyframe.
                    self.scrub_to_frame(0);
                    self.set_status(format!(
                        "Baked {} rig: {} keyframes over {:.1}s",
                        self.rig.kind, count, self.rig.duration
                    ));
                }
            }
            Some(PendingJob::SequenceFrame { out_dir, total }) => {
                let (finished, done_now, failed) = match self.sequence.as_mut() {
                    Some(state) => {
                        state.done += 1;
                        (state.done >= state.total, state.done, state.failed.len())
                    }
                    None => (false, 0, 0),
                };
                if finished {
                    self.sequence = None;
                    self.set_status(format!(
                        "Sequence render finished: {done_now}/{total} frames in {} ({failed} failed)",
                        out_dir.display()
                    ));
                } else {
                    self.set_status(format!(
                        "Rendering sequence frame {} / {total}…",
                        done_now + 1
                    ));
                }
            }
            Some(PendingJob::ParamsSchema) => {
                self.params_schema = Some(result);
                self.params_schema_error = None;
            }
            Some(PendingJob::ParamsValidate) => {
                self.params_validate_pending = false;
                // The engine accepted the current params: hand them to the
                // renderer and refresh the frame (they were never applied
                // while invalid, so the viewport may still show old params).
                let had_error = self.params_validation.take().is_some();
                self.engine.send(Command::SetRenderParams(
                    self.project.terrain_params.clone(),
                ));
                if self.scene_mode == SceneMode::Terrain {
                    // MapScene owns the viewport texture in its own mode.
                    self.viewport_texture = None;
                    if self.native_paints() {
                        self.native.mark_dirty();
                    } else {
                        self.queue_viewport_render();
                    }
                }
                if had_error {
                    self.set_status("Terrain parameters valid again");
                }
            }
            None => {
                self.set_status(format!("Engine op '{op}' finished"));
            }
        }
    }

    /// Route a failed generic engine job.
    fn handle_job_failed(&mut self, op: &str, kind: Option<PendingJob>, message: &str) {
        if matches!(&kind, Some(PendingJob::ParamsValidate)) {
            self.params_validate_pending = false;
            // The engine's own validation text, inline under the pages: the
            // viewport keeps the last params it accepted, so nothing else
            // fails and no error toast is raised.
            self.params_validation =
                Some(message.lines().next().unwrap_or(message).trim().to_owned());
            return;
        }
        if matches!(&kind, Some(PendingJob::ParamsSchema)) {
            // The pages cannot be built: say so where they would be, and keep
            // the engine's own reason in the error list.
            self.params_schema_error =
                Some(message.lines().next().unwrap_or(message).trim().to_owned());
            self.push_error("Terrain parameters", message.to_owned());
            return;
        }
        if let Some(PendingJob::SequenceFrame { total, .. }) = kind {
            if let Some(state) = self.sequence.as_mut() {
                state.failed.push(message.to_owned());
                state.done += 1;
                if state.done >= state.total {
                    let failed = state.failed.len();
                    self.push_error(
                        "Sequence render",
                        format!("{failed} of {total} frames failed — see log"),
                    );
                    self.sequence = None;
                    return;
                }
            }
        }
        self.push_error(format!("Engine op '{op}'"), message.to_owned());
    }

    /// Decode a rendered PNG file into the viewport texture.
    fn load_png_to_viewport(&mut self, path: &Path, ctx: &Context) -> bool {
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                let texture = self.viewport_texture.get_or_insert_with(|| {
                    ctx.load_texture(
                        "studio-viewport",
                        ColorImage::new([1, 1], Color32::BLACK),
                        TextureOptions::LINEAR,
                    )
                });
                texture.set(
                    ColorImage::from_rgba_unmultiplied([w, h], rgba.into_raw().as_slice()),
                    TextureOptions::LINEAR,
                );
                true
            }
            Err(_) => false,
        }
    }

    fn build_mapscene_template(&mut self) {
        let Some(terrain) = self.project.terrain.clone() else {
            self.push_error("MapScene template", "load a DEM dataset first");
            return;
        };
        let mut params = serde_json::json!({
            "template": self.mapscene.template,
            "dem_path": terrain.path.to_string_lossy(),
            "width": self.project.export.width,
            "height": self.project.export.height,
            "raster_path": self.mapscene.raster_path.as_ref().map(|p| p.to_string_lossy()),
            "label_text": self.mapscene.label_text,
        });
        // A gallery scene pins the recipe camera it was verified with.
        if let Some(camera) = self.pending_mapscene_camera.take() {
            if let Some(map) = params.as_object_mut() {
                for key in ["distance", "azimuth_deg", "elevation_deg", "fov_deg"] {
                    if let Some(value) = camera.get(key) {
                        map.insert(key.to_owned(), value.clone());
                    }
                }
            }
        }
        let job = self.engine.call("mapscene.template", params);
        self.pending_jobs.insert(job, PendingJob::MapSceneTemplate);
        self.set_status("Building MapScene recipe from template…");
    }

    fn render_mapscene(&mut self) {
        let Some(recipe) = self.mapscene.recipe_json.clone() else {
            self.push_error("MapScene render", "build a recipe first");
            return;
        };
        let out = std::env::temp_dir().join("forge3d-studio-mapscene.png");
        let cert_path = std::env::temp_dir().join("forge3d-studio-mapscene-cert.json");
        if self.mapscene_certificate {
            let _ = std::fs::remove_file(&cert_path);
        }
        let params = serde_json::json!({
            "recipe_json": serde_json::to_string(&recipe).unwrap_or_default(),
            "out_png": out.to_string_lossy(),
            "certificate": self.mapscene_certificate,
            "certificate_path": self
                .mapscene_certificate
                .then(|| cert_path.to_string_lossy().into_owned()),
        });
        let job = self.engine.call("mapscene.render", params);
        self.pending_jobs.insert(job, PendingJob::MapSceneRender);
        self.set_status("Rendering MapScene…");
    }

    fn validate_mapscene(&mut self) {
        let Some(recipe) = self.mapscene.recipe_json.clone() else {
            self.push_error("MapScene validate", "build a recipe first");
            return;
        };
        let params = serde_json::json!({
            "recipe_json": serde_json::to_string(&recipe).unwrap_or_default(),
        });
        let job = self.engine.call("mapscene.validate", params);
        self.pending_jobs.insert(job, PendingJob::MapSceneValidate);
        self.set_status("Validating MapScene recipe…");
    }

    /// Patch the recipe's OrbitCamera fields from the properties sliders.
    fn patch_mapscene_camera(&mut self, key: &str, value: f64) {
        if let Some(recipe) = self.mapscene.recipe_json.as_mut() {
            if let Some(camera) = recipe.get_mut("camera") {
                camera[key] = serde_json::json!(value);
                self.project.scene_recipe_json =
                    Some(serde_json::to_string(recipe).unwrap_or_default());
                self.project_dirty = true;
            }
        }
    }

    /// Patch the recipe's lighting preset name (engine `LightingPreset`).
    fn patch_mapscene_lighting_preset(&mut self, name: &str) {
        let Some(recipe) = self.mapscene.recipe_json.as_mut() else {
            self.push_error("Lighting preset", "build a recipe first");
            return;
        };
        let Some(lighting) = recipe.get_mut("lighting").and_then(|l| l.as_object_mut()) else {
            self.push_error("Lighting preset", "recipe has no lighting section");
            return;
        };
        lighting.insert("name".to_owned(), serde_json::json!(name));
        lighting
            .entry("intensity")
            .or_insert(serde_json::json!(1.1));
        self.project.scene_recipe_json = Some(serde_json::to_string(recipe).unwrap_or_default());
        self.project_dirty = true;
        self.set_status(format!("Recipe lighting preset → '{name}'"));
    }

    // ------------------------------------------------------ M3: unified import

    /// Open the unified import dialog: pick any supported dataset, inspect
    /// its metadata through the engine, then load or register it.
    fn import_dataset(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Geospatial datasets",
                &[
                    "tif", "tiff", "f3dz", "f3z", "npy", "laz", "json", "geojson", "gpkg", "shp",
                    "obj", "gltf", "glb", "stl",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        self.set_status(format!("Inspecting {}…", path.display()));
        let job = self.engine.call(
            "dataset.inspect",
            serde_json::json!({ "path": path.to_string_lossy() }),
        );
        self.pending_jobs
            .insert(job, PendingJob::DatasetInspect { path });
    }

    /// Import-dialog action: load the inspected file into the terrain slot.
    fn load_preview_as_terrain(&mut self) {
        let Some(preview) = self.import_preview.take() else {
            return;
        };
        self.set_status(format!("Loading {}…", preview.path.display()));
        self.engine
            .send(Command::LoadTerrain { path: preview.path });
    }

    /// Import-dialog action: register the inspected dataset in the project
    /// (path + kind reference; never pixel data).
    fn add_preview_to_project(&mut self) {
        let Some(preview) = self.import_preview.take() else {
            return;
        };
        let kind = preview
            .result
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("dataset")
            .to_owned();
        let label = preview
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| preview.path.display().to_string());
        let source = preview.path.display().to_string();
        let layer_kind = LayerKind::from_dataset_kind(&kind);
        self.project.datasets.push(studio_project::DatasetRef {
            path: preview.path,
            kind,
            label: label.clone(),
        });
        self.edit_document(|project| {
            project.document.push_detailed(
                layer_kind,
                label,
                true,
                Some(source),
                serde_json::Value::Null,
                None,
            );
        });
        self.project_dirty = true;
        self.set_status("Dataset added to project");
    }

    // ---------------------------------------------------------- W1: presets

    /// Fetch a renderer preset and map its first directional light onto the
    /// terrain viewport sun.
    fn apply_preset(&mut self, name: String) {
        let job = self
            .engine
            .call("presets.get", serde_json::json!({ "name": name }));
        self.pending_jobs.insert(job, PendingJob::PresetApply);
        self.set_status(format!("Applying preset '{name}'…"));
    }

    // ------------------------------------------------------- W1: camera rigs

    /// Rig parameters for `rig.bake`, in engine scatter-source units.
    fn rig_params_json(&self) -> serde_json::Value {
        let (w, h) = self
            .project
            .terrain
            .as_ref()
            .map(|t| (t.width as f64, t.height as f64))
            .unwrap_or((1.0, 1.0));
        match self.rig.kind.as_str() {
            "orbit" => serde_json::json!({
                "duration": self.rig.duration,
                "radius": (self.rig.radius_rel as f64) * w.max(h),
                "phi_start_deg": self.rig.phi_start_deg,
                "phi_end_deg": self.rig.phi_end_deg,
                "theta_start_deg": self.rig.theta_start_deg,
            }),
            _ => {
                let path_key = if self.rig.kind == "follow" {
                    "target_path_xz"
                } else {
                    "path_xz"
                };
                let points: Vec<serde_json::Value> = self
                    .rig
                    .path_text
                    .lines()
                    .filter_map(|line| {
                        let mut parts = line.split(',');
                        let x: f64 = parts.next()?.trim().parse().ok()?;
                        let z: f64 = parts.next()?.trim().parse().ok()?;
                        Some(serde_json::json!([
                            x.clamp(0.0, 1.0) * w,
                            z.clamp(0.0, 1.0) * h
                        ]))
                    })
                    .collect();
                serde_json::json!({
                    "duration": self.rig.duration,
                    path_key: points,
                })
            }
        }
    }

    fn bake_rig(&mut self) {
        if self.project.terrain.is_none() {
            self.push_error("Camera rig", "load a DEM dataset first");
            return;
        }
        let params = self.rig_params_json();
        let domain = self
            .project
            .terrain
            .as_ref()
            .map(|t| serde_json::json!([t.height_min, t.height_max]));
        let job = self.engine.call(
            "rig.bake",
            serde_json::json!({
                "kind": self.rig.kind,
                "params": params,
                "samples_per_second": self.rig.samples_per_second,
                "domain": domain,
            }),
        );
        self.pending_jobs.insert(job, PendingJob::RigBake);
        self.set_status("Baking camera rig…");
    }

    /// Jump the viewport to a baked keyframe (linear replay between the
    /// densely-sampled bake points).
    fn scrub_to_frame(&mut self, frame: u32) {
        if self.rig.keyframes.is_empty() {
            return;
        }
        let total = self.rig.keyframes.len() as u32;
        let frame = frame.min(total - 1);
        self.rig_playback.frame = frame;
        let t = self.rig.keyframes[frame as usize].t;
        let Some(kf) = RigKeyframe::at(&self.rig.keyframes, t) else {
            return;
        };
        self.project.camera.phi_deg = kf.phi_deg.rem_euclid(360.0);
        self.project.camera.theta_deg = kf.theta_deg.clamp(1.0, 89.0);
        self.project.camera.radius = kf.radius.max(1.0);
        self.project.camera.fov_deg = kf.fov_deg;
        self.project_dirty = true;
        self.engine.send(Command::SetCamera(self.project.camera));
        self.request_viewport_render();
    }

    fn toggle_playback(&mut self) {
        if self.rig_playback.playing {
            self.rig_playback.playing = false;
            self.rig_playback.play_started = None;
            self.set_status("Playback paused");
        } else if !self.rig.keyframes.is_empty() {
            let t = self
                .rig
                .keyframes
                .get(self.rig_playback.frame as usize)
                .map(|k| k.t)
                .unwrap_or(0.0);
            self.rig_playback.playing = true;
            self.rig_playback.play_t0 = t;
            self.rig_playback.play_started = Some(Instant::now());
            self.set_status("Playing camera rig…");
        }
    }

    /// Advance wall-clock playback when the engine is idle; renders are
    /// on-demand, so playback runs at whatever rate frames complete.
    fn tick_playback(&mut self, ctx: &Context) {
        if !self.rig_playback.playing || self.rig.keyframes.is_empty() {
            return;
        }
        if self.busy || self.pending_render.is_some() {
            return;
        }
        let Some(started) = self.rig_playback.play_started else {
            return;
        };
        let duration = self.rig.duration.max(0.001);
        let t = (self.rig_playback.play_t0 + started.elapsed().as_secs_f32()).min(duration);
        let last_t = self.rig.keyframes.last().map(|k| k.t).unwrap_or(0.0);
        if t >= duration
            || (t >= last_t && self.rig_playback.frame as usize >= self.rig.keyframes.len() - 1)
        {
            self.rig_playback.playing = false;
            self.rig_playback.play_started = None;
            self.set_status("Playback finished");
            return;
        }
        let idx = self
            .rig
            .keyframes
            .iter()
            .rposition(|k| k.t <= t)
            .unwrap_or(0) as u32;
        if idx != self.rig_playback.frame {
            self.scrub_to_frame(idx);
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    /// Queue every baked keyframe as a `terrain.render` job writing
    /// `frame_%04d.png` into the chosen directory. Jobs serialize in the
    /// worker; the UI shows progress and honest per-frame failures.
    fn render_sequence(&mut self) {
        if self.sequence.is_some() {
            self.push_error("Sequence render", "a sequence is already running");
            return;
        }
        if self.rig.keyframes.len() < 2 {
            self.push_error("Sequence render", "bake a camera rig first");
            return;
        }
        let Some(out_dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let total = (self.rig.keyframes.len() as u32).min(240);
        if (self.rig.keyframes.len() as u32) > total {
            self.push_error(
                "Sequence render",
                format!(
                    "capped at {total} frames (baked {})",
                    self.rig.keyframes.len()
                ),
            );
        }
        let width = self.project.export.width;
        let height = self.project.export.height;
        let sun = serde_json::json!({
            "azimuth_deg": self.project.sun.azimuth_deg,
            "elevation_deg": self.project.sun.elevation_deg,
            "intensity": self.project.sun.intensity,
        });
        let colormap = self.project.colormap.clone();
        for index in 0..total {
            let kf = self.rig.keyframes[index as usize];
            let out = out_dir.join(format!("frame_{index:04}.png"));
            let job = self.engine.call(
                "terrain.render",
                serde_json::json!({
                    "width": width,
                    "height": height,
                    "camera": {
                        "phi_deg": kf.phi_deg,
                        "theta_deg": kf.theta_deg,
                        "radius": kf.radius,
                        "fov_deg": kf.fov_deg,
                    },
                    "sun": sun,
                    "colormap": colormap,
                    "out_png": out.to_string_lossy(),
                }),
            );
            self.pending_jobs.insert(
                job,
                PendingJob::SequenceFrame {
                    out_dir: out_dir.clone(),
                    total,
                },
            );
        }
        self.sequence = Some(SequenceState {
            out_dir,
            total,
            done: 0,
            failed: Vec::new(),
        });
        self.set_status(format!("Rendering {total}-frame sequence…"));
    }

    fn new_project(&mut self) {
        if self.project.terrain.is_some() {
            self.engine.send(Command::ClearTerrain);
        }
        self.project = Project::new();
        self.project_path = None;
        self.project_dirty = false;
        self.scene_mode = SceneMode::Terrain;
        self.mapscene = MapScenePanel {
            template: self.mapscene.template.clone(),
            ..Default::default()
        };
        let rig_kind = self.rig.kind.clone();
        self.rig = RigSpec {
            kind: rig_kind,
            ..RigSpec::default()
        };
        self.rig_playback = RigPlayback {
            playing: false,
            frame: 0,
            play_started: None,
            play_t0: 0.0,
        };
        self.sequence = None;
        self.selection.clear();
        self.history.clear();
        self.layers_ui = LayersState::default();
        self.params_validation = None;
        self.params_validate_due = None;
        self.param_drafts.clear();
        self.sync_engine_state();
        self.invalidate_viewport();
        self.set_status("New project");
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Forge3D Studio project", &["forge3d", "json"])
            .pick_file()
        else {
            return;
        };
        match Project::load(&path) {
            Ok(project) => {
                for warning in project.validation_warnings() {
                    self.push_error("Opening project", warning);
                }
                self.scene_mode = if project.scene_mode == studio_project::SCENE_MODE_MAPSCENE {
                    SceneMode::MapScene
                } else {
                    SceneMode::Terrain
                };
                self.mapscene.recipe_json = project
                    .scene_recipe_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok());
                self.mapscene.last_status = None;
                self.mapscene.last_backend = None;
                self.mapscene.last_diagnostics = Vec::new();
                self.project = project;
                self.rig = self
                    .project
                    .camera_rig_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or_default();
                self.rig_playback = RigPlayback {
                    playing: false,
                    frame: 0,
                    play_started: None,
                    play_t0: 0.0,
                };
                self.project_path = Some(path);
                self.project_dirty = false;
                // M4: the newly opened params replace the old pages' state.
                self.selection = self
                    .project
                    .document
                    .terrain_layer()
                    .map(|layer| vec![layer.id])
                    .unwrap_or_default();
                self.history.clear();
                self.layers_ui = LayersState::default();
                self.params_validation = None;
                self.params_validate_due = None;
                self.param_drafts.clear();
                self.sync_engine_state();
                self.invalidate_viewport();
                self.set_status(format!("Opened {}", self.project_path_display()));
            }
            Err(err) => self.push_error("Opening project", err.to_string()),
        }
    }

    fn save_project(&mut self, save_as: bool) {
        let path = if save_as || self.project_path.is_none() {
            rfd::FileDialog::new()
                .add_filter("Forge3D Studio project", &["forge3d"])
                .set_file_name(format!("{}.forge3d", sanitize_name(&self.project.name)))
                .save_file()
        } else {
            self.project_path.clone()
        };
        let Some(path) = path else { return };
        match self.project.save(&path) {
            Ok(()) => {
                self.project_path = Some(path);
                self.project_dirty = false;
                self.set_status(format!("Saved {}", self.project_path_display()));
            }
            Err(err) => self.push_error("Saving project", err.to_string()),
        }
    }

    fn export_png(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name(format!("{}.png", sanitize_name(&self.project.name)))
            .save_file()
        else {
            return;
        };
        self.set_status(format!(
            "Rendering {}×{} to {}…",
            self.project.export.width,
            self.project.export.height,
            path.display()
        ));
        self.engine.send(Command::RenderToFile {
            width: self.project.export.width,
            height: self.project.export.height,
            path,
        });
    }

    fn sync_engine_state(&mut self) {
        self.engine.send(Command::SetCamera(self.project.camera));
        self.engine.send(Command::SetSun(self.project.sun));
        // M4: the project's terrain params travel with it. They were validated
        // by the pages when they were last edited; a hand-edited project file
        // surfaces the engine's own error on the first render.
        self.engine.send(Command::SetRenderParams(
            self.project.terrain_params.clone(),
        ));
    }

    /// M4: mark the terrain params for the debounced engine validation.
    fn schedule_params_validate(&mut self) {
        // A stale message would describe values the user just replaced.
        self.params_validation = None;
        self.params_validate_due = Some(Instant::now() + PARAMS_VALIDATE_DEBOUNCE);
    }

    /// M4: dispatch the debounced terrain-params validation once the edits
    /// settle. One job at a time: edits made while a validation is in flight
    /// leave the due time set, so the next pass re-checks the newest values.
    fn flush_params_validate(&mut self) {
        if self.params_validate_pending {
            return;
        }
        let due = self
            .params_validate_due
            .map(|at| Instant::now() >= at)
            .unwrap_or(false);
        if !due {
            return;
        }
        self.params_validate_due = None;
        self.params_validate_pending = true;
        let params = self.project.terrain_params.clone();
        self.pending_jobs.insert(
            self.engine.call(
                "terrain.params_validate",
                serde_json::json!({ "params": params }),
            ),
            PendingJob::ParamsValidate,
        );
    }

    fn reset_camera(&mut self) {
        if let Some(terrain) = &self.project.terrain {
            // Restore the fit radius the engine chose when the dataset loaded.
            self.project.camera.radius = (terrain.width.max(terrain.height) as f32) * 0.8;
        }
        self.project.camera.phi_deg = 135.0;
        self.project.camera.theta_deg = 45.0;
        self.project.camera.fov_deg = 55.0;
        self.project_dirty = true;
        self.engine.send(Command::SetCamera(self.project.camera));
        self.invalidate_viewport();
    }

    fn project_path_display(&self) -> String {
        self.project_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unsaved".to_owned())
    }

    // -------------------------------------------------------------- panels

    /// Menu bar + toolbar (GUI.md III.1/III.2). Every entry is an [`Action`]
    /// from the registry, so the menus, the toolbar, the shortcut window and
    /// the command palette cannot drift apart.
    fn top_panel(&mut self, ctx: &Context) {
        let mut chosen: Option<Action> = None;
        egui::TopBottomPanel::top("studio_top").show(ctx, |ui| {
            ui.add_space(2.0);
            if let Some(action) = menu::menu_bar(
                ui,
                |action| self.action_checked(action),
                |action| self.action_enabled(action),
            ) {
                chosen = Some(action);
            }
            ui.add_space(1.0);
            let project = self
                .project_path
                .as_ref()
                .map(|path| path.display().to_string());
            ui.horizontal(|ui| {
                if let Some(action) = toolbar::toolbar(
                    ui,
                    &self.icons,
                    self.tokens.icon + 2.0,
                    |action| self.action_enabled(action),
                    |action| self.action_checked(action),
                ) {
                    chosen = Some(action);
                }
                toolbar::project_chip(ui, project.as_deref(), self.busy);
            });
            ui.add_space(2.0);
        });
        if let Some(action) = chosen {
            self.run_action(ctx, action);
        }
    }

    /// Whether a toggle-style action is currently on (theme, layout preset).
    fn action_checked(&self, action: Action) -> bool {
        match action {
            Action::ThemeDark => self.theme_dark,
            Action::ThemeLight => !self.theme_dark,
            Action::LayoutDefault => self.dock.preset == LayoutPreset::Default,
            Action::LayoutCartography => self.dock.preset == LayoutPreset::Cartography,
            Action::LayoutModeling => self.dock.preset == LayoutPreset::Modeling,
            Action::LayoutData => self.dock.preset == LayoutPreset::Data,
            Action::ToolOrbit => self.viewport_tool == crate::ui::viewport::ViewportTool::Orbit,
            Action::ToolPan => self.viewport_tool == crate::ui::viewport::ViewportTool::Pan,
            Action::ToolZoom => self.viewport_tool == crate::ui::viewport::ViewportTool::Zoom,
            Action::ToggleSafeFrame => self.show_safe_frame,
            Action::ToggleCompare => self.compare_mode,
            _ => false,
        }
    }

    /// Whether an action can run right now, driven by the registry's [`Need`]
    /// so the menus, the toolbar and the panel all agree.
    fn action_enabled(&self, action: Action) -> bool {
        match action.need() {
            crate::actions::Need::Nothing => match action {
                Action::ExportPng
                | Action::RenderFrame
                | Action::FitView
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::CameraTop
                | Action::CameraFront
                | Action::CameraRight
                | Action::CameraIso
                | Action::CameraReset => self.project.terrain.is_some(),
                _ => true,
            },
            crate::actions::Need::Layers => !self.project.document.is_empty(),
            crate::actions::Need::Selection => self.selection.first().is_some_and(|id| {
                self.project
                    .document
                    .layer(*id)
                    .map(|layer| !layer.locked)
                    .unwrap_or(false)
            }),
            crate::actions::Need::Undo => self.history.can_undo(),
            crate::actions::Need::Redo => self.history.can_redo(),
        }
    }

    /// Left dock: Layers (the scene objects), Scene (mode + MapScene recipe),
    /// Assets (colormaps and presets). Collapses to a 30 px rail.
    fn left_panel(&mut self, ctx: &Context) {
        const TABS: [&str; 3] = ["Layers", "Scene", "Assets"];
        if self.dock.left_collapsed {
            egui::SidePanel::left("studio_dock_left_rail")
                .exact_width(30.0)
                .resizable(false)
                .frame(dock::panel_frame(&self.tokens))
                .show(ctx, |ui| {
                    ui.add_space(2.0);
                    if dock::collapsed_rail(ui, &TABS, &mut self.dock.left_tab) {
                        self.dock.left_collapsed = false;
                    }
                });
            return;
        }
        let response = egui::SidePanel::left("studio_dock_left")
            .resizable(true)
            .default_width(self.dock.left_width)
            .width_range(self.tokens.dock_min..=460.0)
            .frame(dock::panel_frame(&self.tokens))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let _ = dock::tab_strip(ui, &TABS, &mut self.dock.left_tab);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("«")
                            .on_hover_text("Collapse the left dock")
                            .clicked()
                        {
                            self.dock.left_collapsed = true;
                        }
                    });
                });
                dock::strip_separator(ui);
                egui::ScrollArea::vertical()
                    .id_source("left_dock_scroll")
                    .show(ui, |ui| match self.dock.left_tab {
                        0 => self.layers_tab(ui),
                        1 => self.scene_section(ui),
                        _ => self.assets_section(ui),
                    });
            });
        self.dock.left_width = response.response.rect.width();
    }

    /// Scene tab: the render pipeline switch plus the MapScene recipe editor.
    fn scene_section(&mut self, ui: &mut Ui) {
        ui.label(theme::group_label("Pipeline"));
        let mode_before = self.scene_mode;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.scene_mode, SceneMode::Terrain, "Terrain");
            ui.selectable_value(&mut self.scene_mode, SceneMode::MapScene, "MapScene");
        });
        if self.scene_mode != mode_before {
            self.project.scene_mode = match self.scene_mode {
                SceneMode::Terrain => studio_project::SCENE_MODE_TERRAIN.to_owned(),
                SceneMode::MapScene => studio_project::SCENE_MODE_MAPSCENE.to_owned(),
            };
            self.project_dirty = true;
            // Entering terrain mode: the native scene (or the bridge) has to
            // draw before the panel shows it. MapScene keeps its own frame.
            if self.scene_mode == SceneMode::Terrain {
                self.native.mark_dirty();
                self.invalidate_viewport();
            }
        }
        ui.add_space(8.0);
        match self.scene_mode {
            SceneMode::Terrain => {
                ui.weak("Terrain scenes are edited through Layers and the Inspector.");
                ui.weak("MapScene adds cartographic layers, labels and furniture.");
            }
            SceneMode::MapScene => self.mapscene_section(ui),
        }
    }

    /// Assets tab: the engine's colormap and renderer-preset registries.
    fn assets_section(&mut self, ui: &mut Ui) {
        ui.label(theme::group_label("Colormaps"));
        if self.colormaps.is_empty() {
            ui.weak("no colormaps loaded");
        }
        let current = self
            .project
            .colormap
            .clone()
            .unwrap_or_else(|| "default".to_owned());
        egui::ScrollArea::vertical()
            .id_source("assets_colormaps")
            .max_height(240.0)
            .show(ui, |ui| {
                let is_default = self.project.colormap.is_none();
                if ui
                    .selectable_label(is_default, "default (relief)")
                    .clicked()
                    && !is_default
                {
                    self.project.colormap = None;
                    self.project_dirty = true;
                    self.engine.send(Command::SetColormap(None));
                    self.invalidate_viewport();
                }
                let names = self.colormaps.clone();
                for name in names {
                    let selected = current == name;
                    if ui.selectable_label(selected, &name).clicked() && !selected {
                        self.project.colormap = Some(name.clone());
                        self.project_dirty = true;
                        self.engine.send(Command::SetColormap(Some(name.clone())));
                        self.invalidate_viewport();
                    }
                }
            });
        ui.add_space(10.0);
        ui.label(theme::group_label("Renderer presets"));
        if self.presets.is_empty() {
            ui.weak("no presets loaded");
        }
        let presets = self.presets.clone();
        egui::ScrollArea::vertical()
            .id_source("assets_presets")
            .max_height(220.0)
            .show(ui, |ui| {
                for name in presets {
                    if ui
                        .selectable_label(false, &name)
                        .on_hover_text("Map the preset's first directional light onto the sun")
                        .clicked()
                    {
                        self.apply_preset(name);
                    }
                }
            });
    }

    /// Layers tab: the scene document, edited as rows (GUI.md III.3).
    ///
    /// The panel draws and reports; every mutation happens in
    /// [`Self::apply_layers_command`] so undo recording is uniform.
    fn layers_tab(&mut self, ui: &mut Ui) {
        let view = LayersView {
            document: &self.project.document,
            selected: &self.selection,
            primary: self.selection.first().copied(),
            icons: &self.icons,
            tokens: &self.tokens,
        };
        let commands = layers::panel(ui, &mut self.layers_ui, &view);
        for command in commands {
            self.apply_layers_command(command);
        }
    }

    /// Apply one Layers-panel command, recording undo for real edits.
    fn apply_layers_command(&mut self, command: LayersCommand) {
        // Dragging opacity emits a command per frame; one gesture is one step.
        let continuing_opacity =
            matches!(&command, LayersCommand::SetOpacity(id, _) if self.opacity_edit == Some(*id));
        if !continuing_opacity {
            self.opacity_edit = None;
        }
        match command {
            LayersCommand::Select { ids, additive } => {
                if additive {
                    for id in ids {
                        match self.selection.iter().position(|selected| *selected == id) {
                            Some(index) => {
                                self.selection.remove(index);
                            }
                            None => self.selection.push(id),
                        }
                    }
                } else {
                    self.selection = ids;
                }
                self.selection
                    .retain(|id| self.project.document.layer(*id).is_some());
            }
            LayersCommand::SetVisible(id, visible) => {
                self.edit_document(|project| {
                    if let Some(layer) = project.document.layer_mut(id) {
                        layer.visible = visible;
                    }
                    if project.document.terrain_layer().map(|layer| layer.id) == Some(id) {
                        project.terrain_visible = visible;
                    }
                });
                if self.is_terrain(id) {
                    self.invalidate_viewport();
                }
            }
            LayersCommand::SetLocked(id, locked) => self.edit_document(|project| {
                if let Some(layer) = project.document.layer_mut(id) {
                    layer.locked = locked;
                }
            }),
            LayersCommand::SetOpacity(id, opacity) => {
                if continuing_opacity {
                    if let Some(layer) = self.project.document.layer_mut(id) {
                        layer.opacity = opacity.clamp(0.0, 1.0);
                    }
                    self.project_dirty = true;
                } else {
                    self.edit_document(|project| {
                        if let Some(layer) = project.document.layer_mut(id) {
                            layer.opacity = opacity.clamp(0.0, 1.0);
                        }
                    });
                    self.opacity_edit = Some(id);
                }
                if self.is_terrain(id) {
                    self.invalidate_viewport();
                }
            }
            LayersCommand::SetStyle(id, style) => {
                self.edit_document(|project| {
                    if let Some(layer) = project.document.layer_mut(id) {
                        layer.style = style;
                    }
                });
                // A style edit can change the compiled recipe.
                if self.scene_mode == SceneMode::MapScene && self.mapscene.recipe_json.is_some() {
                    self.compile_and_stage_recipe(false);
                }
            }
            LayersCommand::SetExpanded(id, expanded) => self.edit_document(|project| {
                if let Some(layer) = project.document.layer_mut(id) {
                    layer.expanded = expanded;
                }
            }),
            LayersCommand::BeginRename(id) => {
                if let Some(layer) = self.project.document.layer(id) {
                    let name = layer.name.clone();
                    self.layers_ui.begin_rename(id, &name);
                }
            }
            LayersCommand::Rename(id, name) => self.edit_document(|project| {
                if let Some(layer) = project.document.layer_mut(id) {
                    layer.name = name;
                }
            }),
            LayersCommand::Move { id, spot } => {
                self.dock.left_tab = 0;
                self.edit_document(|project| match spot {
                    layers::DropSpot::Before(target) => {
                        project.document.move_beside(id, target, false);
                    }
                    layers::DropSpot::After(target) => {
                        project.document.move_beside(id, target, true);
                    }
                    layers::DropSpot::Into(target) => {
                        project.document.move_into(id, target);
                    }
                });
            }
            LayersCommand::Duplicate(id) => {
                let mut copy = None;
                self.edit_document(|project| {
                    copy = project.document.duplicate(id);
                });
                if let Some(copy) = copy {
                    self.selection = vec![copy];
                }
            }
            LayersCommand::Delete(ids) => self.delete_layers(&ids),
            LayersCommand::Group(ids) => {
                let mut group = None;
                self.edit_document(|project| {
                    group = project.document.group(&ids, "Group");
                });
                if let Some(group) = group {
                    self.selection = vec![group];
                }
            }
            LayersCommand::Ungroup(id) => {
                self.edit_document(|project| {
                    project.document.ungroup(id);
                });
                self.selection.clear();
            }
            LayersCommand::MoveUp(id) => self.edit_document(|project| {
                project.document.move_by(id, 1);
            }),
            LayersCommand::MoveDown(id) => self.edit_document(|project| {
                project.document.move_by(id, -1);
            }),
            LayersCommand::ZoomTo(id) => {
                if self.is_terrain(id) {
                    self.fit_view();
                } else {
                    self.set_status(
                        "Zoom to layer needs the layer's bounds; only the terrain has them today",
                    );
                }
            }
            LayersCommand::AddTerrain | LayersCommand::ImportDataset => self.import_dataset(),
        }
    }

    /// Remove layers (and the project state they own), with undo.
    fn delete_layers(&mut self, ids: &[LayerId]) {
        let mut cleared_terrain = false;
        let mut removed_dataset = false;
        let mut removed = false;
        let before = self.project.document.clone();
        for id in ids {
            let Some(layer) = self.project.document.layer(*id).cloned() else {
                continue;
            };
            if layer.kind == LayerKind::Terrain {
                self.project.terrain = None;
                self.project.terrain_visible = true;
                self.engine.send(Command::ClearTerrain);
                cleared_terrain = true;
            }
            if let Some(source) = &layer.source {
                let path = std::path::PathBuf::from(source);
                let before_len = self.project.datasets.len();
                self.project.datasets.retain(|dataset| dataset.path != path);
                removed_dataset |= self.project.datasets.len() != before_len;
            }
            removed |= !self.project.document.remove(*id).is_empty();
        }
        if self.project.document != before {
            self.history.record(before);
            self.project_dirty = true;
        }
        self.selection.clear();
        if removed || cleared_terrain || removed_dataset {
            self.set_status(format!(
                "Removed {} layer(s){}",
                ids.len(),
                if cleared_terrain {
                    " and unloaded the terrain"
                } else {
                    ""
                }
            ));
        }
    }

    fn is_terrain(&self, id: LayerId) -> bool {
        self.project.document.terrain_layer().map(|layer| layer.id) == Some(id)
    }

    /// Mutate the project with undo recording: nothing is recorded when the
    /// document did not actually change.
    fn edit_document(&mut self, edit: impl FnOnce(&mut Project)) {
        let before = self.project.document.clone();
        edit(&mut self.project);
        if self.project.document != before {
            self.history.record(before);
            self.project_dirty = true;
        }
    }

    fn mapscene_section(&mut self, ui: &mut Ui) {
        let terrain_present = self.project.terrain.is_some();
        if !terrain_present {
            ui.weak("MapScene recipes build on the loaded DEM.");
            ui.weak("Import a dataset first:");
            if ui.button("Import Dataset…").clicked() {
                self.import_dataset();
            }
            return;
        }

        ui.label(egui::RichText::new("Template").strong());
        egui::ComboBox::from_id_source("mapscene_template")
            .selected_text(self.mapscene.template.clone())
            .show_ui(ui, |ui| {
                for t in ["terrain", "terrain_raster", "terrain_vector_labels"] {
                    ui.selectable_value(&mut self.mapscene.template, t.to_owned(), t);
                }
            });
        if self.mapscene.template == "terrain_raster" {
            ui.horizontal(|ui| {
                if ui.button("Pick raster…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images & rasters", &["png", "tif", "tiff", "jpg"])
                        .pick_file()
                    {
                        self.mapscene.raster_path = Some(path);
                    }
                }
                ui.weak(
                    self.mapscene
                        .raster_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "no raster".to_owned()),
                );
            });
        }
        if self.mapscene.template == "terrain_vector_labels" {
            ui.text_edit_singleline(&mut self.mapscene.label_text);
        }
        if ui.button("Build recipe from template").clicked() {
            self.build_mapscene_template();
        }

        // W1: renderer presets apply to the recipe as engine LightingPreset
        // names (the certified path fuji_labels_demo / offline_quality use).
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Lighting preset").strong());
        if self.presets.is_empty() {
            ui.weak("no presets loaded");
        } else {
            let mut chosen: Option<String> = None;
            egui::ComboBox::from_id_source("mapscene_lighting_preset")
                .selected_text("apply preset…")
                .show_ui(ui, |ui| {
                    for name in &self.presets {
                        if ui.selectable_label(false, name).clicked() {
                            chosen = Some(name.clone());
                        }
                    }
                });
            if let Some(name) = chosen {
                self.patch_mapscene_lighting_preset(&name);
            }
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Recipe").strong());
        match &self.mapscene.recipe_json {
            Some(recipe) => {
                let layers = recipe
                    .get("layers")
                    .and_then(|l| l.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                ui.label(format!("{} layer(s)", layers));
                for layer in recipe
                    .get("layers")
                    .and_then(|l| l.as_array())
                    .unwrap_or(&vec![])
                {
                    let id = layer
                        .get("layer_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("layer");
                    let kind = layer.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                    ui.horizontal(|ui| {
                        ui.label(kind);
                        ui.weak(id);
                    });
                }
                if let Some(status) = &self.mapscene.last_status {
                    ui.label(format!("Validation/render: {status}"));
                }
                if let Some(backend) = &self.mapscene.last_backend {
                    ui.weak(format!("backend: {backend}"));
                }
                for diag in &self.mapscene.last_diagnostics {
                    ui.weak(format!("diag: {diag}"));
                }
                ui.checkbox(&mut self.mapscene_certificate, "Sign render certificate");
                ui.horizontal(|ui| {
                    if ui.button("Compile & render scene").clicked() {
                        self.compile_and_stage_recipe(true);
                    }
                    if ui.button("Compile only").clicked() {
                        self.compile_and_stage_recipe(false);
                    }
                    if ui.button("Validate").clicked() {
                        self.validate_mapscene();
                    }
                });
                // U2: compile diagnostics (per-layer honesty from scene.compile).
                for diag in &self.mapscene.compile_diagnostics {
                    let severity = diag.get("severity").and_then(|v| v.as_str()).unwrap_or("?");
                    let code = diag.get("code").and_then(|v| v.as_str()).unwrap_or("?");
                    let message = diag.get("message").and_then(|v| v.as_str()).unwrap_or("");
                    ui.label(
                        egui::RichText::new(format!("{severity}/{code}: {message}"))
                            .small()
                            .color(if severity == "error" {
                                ui.visuals().error_fg_color
                            } else {
                                ui.visuals().warn_fg_color
                            }),
                    );
                }
                if let Some(sha) = self
                    .mapscene
                    .last_metadata
                    .as_ref()
                    .and_then(|m| m.get("certificate_payload_sha256"))
                    .and_then(|v| v.as_str())
                {
                    ui.weak(format!("cert sha256: {}", &sha[..sha.len().min(24)]))
                        .on_hover_text(sha);
                }
            }
            None => {
                ui.weak("No recipe yet — build one from a template.");
            }
        }
    }

    fn right_panel(&mut self, ctx: &Context) {
        let response = egui::SidePanel::right("studio_dock_right")
            .resizable(true)
            .default_width(self.dock.right_width)
            .width_range(self.tokens.dock_min..=520.0)
            .frame(dock::panel_frame(&self.tokens))
            .show(ctx, |ui| {
                let _ = dock::tab_strip(ui, &["Inspector", "Render"], &mut self.dock.right_tab);
                dock::strip_separator(ui);
                egui::ScrollArea::vertical()
                    .id_source("right_dock_scroll")
                    .show(ui, |ui| match self.dock.right_tab {
                        0 => self.inspector_tab(ui),
                        _ => self.render_params_section(ui),
                    });
            });
        self.dock.right_width = response.response.rect.width();
    }

    /// Inspector tab: properties of what is selected. Until U1 introduces
    /// selectable layers, the selection is the scene itself (camera, sun,
    /// output), which is exactly what the old "Properties" column mixed in.
    fn inspector_tab(&mut self, ui: &mut Ui) {
        if let Some(id) = self.selection.first().copied() {
            self.layer_inspector(ui, id);
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);
        }
        ui.label(theme::group_label("Scene"));
        match self.scene_mode {
            SceneMode::Terrain => {
                self.camera_section(ui);
                ui.add_space(10.0);
                self.sun_section(ui);
            }
            SceneMode::MapScene => self.mapscene_camera_section(ui),
        }
        ui.add_space(10.0);
        self.export_section(ui);
    }

    /// Properties of the selected layer. Kind-specific editors arrive with U2;
    /// what exists today is real and says which layer kinds render.
    fn layer_inspector(&mut self, ui: &mut Ui, id: LayerId) {
        let Some(layer) = self.project.document.layer(id).cloned() else {
            return;
        };
        ui.horizontal(|ui| {
            let texture = self.icons.texture(ui.ctx(), layers::layer_icon(layer.kind));
            ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(16.0, 16.0)));
            ui.label(egui::RichText::new(&layer.name).strong());
        });
        ui.label(theme::meta(format!(
            "{} layer{}",
            layer.kind.label(),
            if layer.kind.renders() {
                ""
            } else {
                " · registered only"
            }
        )));
        if let Some(source) = &layer.source {
            ui.label(theme::meta(source.clone()))
                .on_hover_text(source.clone());
        }
        if !layer.kind.renders() && !layer.kind.is_group() {
            ui.weak(
                "Referenced by the project, but Studio has no renderer for this kind yet —                  the gallery and this panel say so instead of pretending.",
            );
        }
        ui.add_space(6.0);
        let mut visible = layer.visible;
        if ui.checkbox(&mut visible, "Visible").changed() {
            self.apply_layers_command(LayersCommand::SetVisible(id, visible));
        }
        if !layer.kind.is_group() {
            let mut opacity = layer.opacity;
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Opacity"))
                .changed()
            {
                self.apply_layers_command(LayersCommand::SetOpacity(id, opacity));
            }
        }
        let mut locked = layer.locked;
        if ui.checkbox(&mut locked, "Locked").changed() {
            self.apply_layers_command(LayersCommand::SetLocked(id, locked));
        }
        match layer.kind {
            LayerKind::Terrain => {
                ui.add_space(4.0);
                if ui.button("Edit render parameters").clicked() {
                    self.dock.right_tab = 1;
                }
                ui.weak("Sun, camera and output sit under Scene below.");
            }
            LayerKind::RasterOverlay => {
                ui.add_space(4.0);
                ui.label(theme::group_label("Raster overlay"));
                let crs = layer
                    .style
                    .get("crs")
                    .and_then(|v| v.as_str())
                    .unwrap_or("inherits terrain")
                    .to_owned();
                ui.label(theme::meta(format!("CRS: {crs}")));
                if let Some(path) = &layer.source {
                    ui.label(theme::meta(path.to_string()))
                        .on_hover_text(path.to_string());
                }
                ui.weak("Opacity feeds the compiled RasterOverlay.");
            }
            LayerKind::Vector => {
                ui.add_space(4.0);
                ui.label(theme::group_label("Vector line"));
                let mut color = layer
                    .style
                    .pointer("/mapbox/layers/0/paint/line-color")
                    .and_then(|v| v.as_str())
                    .unwrap_or("#ffffff")
                    .to_owned();
                ui.horizontal(|ui| {
                    ui.label("Stroke");
                    if ui.text_edit_singleline(&mut color).changed()
                        && color.starts_with('#')
                        && color.len() == 7
                    {
                        let mut style = layer.style.clone();
                        if style.get("mapbox").is_none() {
                            style["mapbox"] = serde_json::json!({
                                "version": 8,
                                "layers": [{"id": layer.name, "type": "line",
                                            "paint": {"line-color": color}}]
                            });
                        } else {
                            style["mapbox"]["layers"][0]["paint"]["line-color"] =
                                serde_json::json!(color);
                        }
                        self.apply_layers_command(LayersCommand::SetStyle(id, style));
                    }
                });
                ui.weak("Features and Mapbox paint compile into the VectorOverlay.");
            }
            LayerKind::Labels => {
                ui.add_space(4.0);
                ui.label(theme::group_label("Labels"));
                let mut text = layer
                    .style
                    .pointer("/labels/0/text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                if ui
                    .add_sized(
                        [ui.available_width().max(80.0), 20.0],
                        egui::TextEdit::singleline(&mut text).hint_text("Label text"),
                    )
                    .lost_focus()
                    && !text.is_empty()
                {
                    let mut style = layer.style.clone();
                    style["labels"][0]["text"] = serde_json::json!(text);
                    self.apply_layers_command(LayersCommand::SetStyle(id, style));
                }
                ui.weak("Label candidates compile into the LabelLayer.");
            }
            _ => {}
        }
    }

    /// Bottom dock: the rig timeline and the console. Collapses to one row.
    fn bottom_dock(&mut self, ctx: &Context) {
        if self.dock.bottom_collapsed {
            egui::TopBottomPanel::bottom("studio_dock_bottom_rail")
                .exact_height(24.0)
                .frame(dock::panel_frame(&self.tokens))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("▴")
                            .on_hover_text("Expand the bottom dock")
                            .clicked()
                        {
                            self.dock.bottom_collapsed = false;
                        }
                        for (index, name) in
                            ["Timeline", "Console", "Certs", "Stats"].iter().enumerate()
                        {
                            if ui
                                .selectable_label(self.dock.bottom_tab == index, *name)
                                .clicked()
                            {
                                self.dock.bottom_tab = index;
                                self.dock.bottom_collapsed = false;
                            }
                        }
                    });
                });
            return;
        }
        let response = egui::TopBottomPanel::bottom("studio_dock_bottom")
            .resizable(true)
            .default_height(self.dock.bottom_height)
            .height_range(90.0..=440.0)
            .frame(dock::panel_frame(&self.tokens))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let _ = dock::tab_strip(
                        ui,
                        &["Timeline", "Console", "Certs", "Stats"],
                        &mut self.dock.bottom_tab,
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("▾")
                            .on_hover_text("Collapse the bottom dock")
                            .clicked()
                        {
                            self.dock.bottom_collapsed = true;
                        }
                    });
                });
                dock::strip_separator(ui);
                match self.dock.bottom_tab {
                    0 => self.timeline_tab(ui),
                    1 => self.console_tab(ui),
                    2 => self.certificates_tab(ui),
                    _ => self.statistics_tab(ui),
                }
            });
        self.dock.bottom_height = response.response.rect.height();
    }

    /// M5: certificates produced this session, with one-click verification.
    fn certificates_tab(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(theme::group_label("Render certificates"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("refresh stats")
                    .on_hover_text("Fetch the degradation ledger and memory telemetry")
                    .clicked()
                {
                    let job = self
                        .engine
                        .call("diagnostics.snapshot", serde_json::json!({}));
                    self.pending_jobs
                        .insert(job, PendingJob::DiagnosticsSnapshot);
                }
            });
        });
        ui.separator();
        if self.certificates.is_empty() {
            ui.weak(
                "No certificates yet — enable 'Request render certificate' (Render menu)                  or the MapScene scene panel, then render.",
            );
            return;
        }
        egui::ScrollArea::vertical()
            .id_source("certs_scroll")
            .show(ui, |ui| {
                for (index, cert) in self.certificates.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let icon = self
                            .icons
                            .texture(ui.ctx(), crate::icons::Icon::CircleCheck);
                        ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(13.0, 13.0)));
                        ui.label(theme::meta(&cert.source));
                        if let Some(sha) = &cert.payload_sha256 {
                            ui.monospace(theme::meta(&sha[..sha.len().min(20)]))
                                .on_hover_text(sha);
                        }
                        match cert.verified {
                            Some(true) => {
                                ui.label(
                                    egui::RichText::new("verified")
                                        .small()
                                        .color(Color32::from_rgb(120, 200, 140)),
                                );
                            }
                            Some(false) => {
                                ui.label(
                                    egui::RichText::new("INVALID")
                                        .small()
                                        .color(ui.visuals().error_fg_color),
                                );
                            }
                            None => {
                                if ui.small_button("verify").clicked() {
                                    let job = self.engine.call(
                                        "certificate.verify",
                                        serde_json::json!({
                                            "path": cert.path.to_string_lossy()
                                        }),
                                    );
                                    self.pending_jobs
                                        .insert(job, PendingJob::CertificateVerify { index });
                                }
                            }
                        }
                    });
                }
            });
    }

    /// M5: adapter, degradations and the memory ledger. Never silently
    /// degrade — the ledger is the engine's own record.
    fn statistics_tab(&mut self, ui: &mut Ui) {
        if let Some(gpu) = &self.gpu {
            ui.label(theme::group_label("Device"));
            ui.label(theme::meta(format!(
                "{} · {}{}",
                gpu.adapter,
                gpu.backend,
                if gpu.software_fallback {
                    " · software fallback"
                } else {
                    ""
                }
            )));
        }
        if let Some(ms) = self.last_frame_ms {
            ui.label(theme::meta(format!("last terrain frame: {ms:.0} ms")));
        }
        ui.add_space(4.0);
        let Some(stats) = &self.stats else {
            ui.weak("Press 'refresh stats' in the Certificates tab (or render) to collect.");
            return;
        };
        ui.label(theme::group_label("Degradation ledger"));
        let degradations = stats
            .get("degradations")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        if degradations.is_empty() {
            ui.weak("No degradations recorded — nothing was silently disabled.");
        } else {
            for d in degradations {
                let kind = d.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let consequence = d.get("consequence").and_then(|v| v.as_str()).unwrap_or("");
                ui.label(
                    egui::RichText::new(format!("{kind}/{name}: {consequence}"))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
            }
        }
        ui.add_space(4.0);
        ui.label(theme::group_label("Memory ledger"));
        if let Some(memory) = stats.get("memory").and_then(|m| m.as_object()) {
            for (key, value) in memory {
                let v = value.as_f64().unwrap_or(0.0);
                ui.label(theme::meta(format!("{key}: {v:.0}")));
            }
        }
    }

    /// Timeline tab: the camera-rig bake, keyframe scrubber and playback.
    fn timeline_tab(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .id_source("timeline_scroll")
            .show(ui, |ui| self.rig_section(ui));
    }

    /// Console tab: every engine error and status line, newest last.
    fn console_tab(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(theme::group_label("Engine log"));
            ui.weak(format!("{} line(s)", self.log.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.log.is_empty(), egui::Button::new("Clear").small())
                    .clicked()
                {
                    self.log.clear();
                }
            });
        });
        ui.add_space(2.0);
        if self.log.is_empty() {
            ui.weak("No engine messages yet — imports, renders and errors appear here.");
            return;
        }
        egui::ScrollArea::vertical()
            .id_source("console_scroll")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in &self.log {
                    let text = egui::RichText::new(&entry.text).monospace().size(11.5);
                    let text = if entry.error {
                        text.color(ui.visuals().error_fg_color)
                    } else {
                        text
                    };
                    ui.label(text);
                }
            });
    }

    /// Label describing the current selection for the status bar.
    fn selection_label(&self) -> Option<String> {
        if let Some(layer) = self.primary_layer() {
            let mut label = format!("{} · {}", layer.name, layer.kind.label());
            if !layer.kind.renders() && !layer.kind.is_group() {
                label.push_str(" · registered only");
            }
            if self.selection.len() > 1 {
                label.push_str(&format!(" +{}", self.selection.len() - 1));
            }
            return Some(label);
        }
        match self.scene_mode {
            SceneMode::Terrain => self.project.terrain.as_ref().map(|terrain| {
                let name = terrain
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| terrain.path.display().to_string());
                format!("{name} · {}×{}", terrain.width, terrain.height)
            }),
            SceneMode::MapScene => self
                .mapscene
                .last_status
                .as_ref()
                .map(|status| format!("MapScene · {status}")),
        }
    }

    /// Recipe camera editing in MapScene mode: patches the serialized
    /// SceneRecipe's OrbitCamera fields directly.
    fn mapscene_camera_section(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Camera (recipe)").strong());
        let Some(recipe) = self.mapscene.recipe_json.as_ref() else {
            ui.weak("Build a recipe to edit its camera.");
            return;
        };
        let camera = recipe.get("camera").cloned().unwrap_or_default();
        let get = |key: &str| -> f64 { camera.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) };
        let mut azimuth = get("azimuth_deg");
        let mut distance = get("distance");
        let mut elevation = get("elevation_deg");
        let mut fov = get("fov_deg");
        let changed = ui
            .add(Slider::new(&mut azimuth, 0.0..=360.0).text("Azimuth"))
            .changed()
            | ui.add(
                Slider::new(&mut distance, 0.1..=20_000.0)
                    .text("Distance")
                    .logarithmic(true),
            )
            .changed()
            | ui.add(Slider::new(&mut elevation, 1.0..=89.0).text("Elevation"))
                .changed()
            | ui.add(Slider::new(&mut fov, 10.0..=120.0).text("FOV"))
                .changed();
        if changed {
            self.patch_mapscene_camera("azimuth_deg", azimuth);
            self.patch_mapscene_camera("distance", distance);
            self.patch_mapscene_camera("elevation_deg", elevation);
            self.patch_mapscene_camera("fov_deg", fov);
        }
    }

    fn camera_section(&mut self, ui: &mut Ui) {
        ui.label(theme::group_label("Camera"));
        let before = self.project.camera;
        let camera: &mut CameraParams = &mut self.project.camera;
        ui.add(Slider::new(&mut camera.phi_deg, 0.0..=360.0).text("Orbit φ"));
        ui.add(Slider::new(&mut camera.theta_deg, 1.0..=89.0).text("Elevation θ"));
        ui.add(
            Slider::new(&mut camera.radius, 10.0..=100_000.0)
                .text("Distance")
                .logarithmic(true),
        );
        ui.add(Slider::new(&mut camera.fov_deg, 10.0..=120.0).text("FOV"));
        if ui.button("Reset view").clicked() {
            self.reset_camera();
        }
        if self.project.camera != before {
            self.project_dirty = true;
            self.engine.send(Command::SetCamera(self.project.camera));
            self.invalidate_viewport();
        }
    }

    fn sun_section(&mut self, ui: &mut Ui) {
        ui.label(theme::group_label("Sun"));
        let before = self.project.sun;
        let sun: &mut SunParams = &mut self.project.sun;
        ui.add(Slider::new(&mut sun.azimuth_deg, 0.0..=360.0).text("Azimuth"));
        ui.add(Slider::new(&mut sun.elevation_deg, 0.0..=90.0).text("Elevation"));
        ui.add(Slider::new(&mut sun.intensity, 0.0..=10.0).text("Intensity"));
        if self.project.sun != before {
            self.project_dirty = true;
            self.engine.send(Command::SetSun(self.project.sun));
            self.invalidate_viewport();
        }
        ui.add_space(4.0);
        egui::Grid::new("active_look")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                ui.label(theme::meta("Colormap"));
                ui.label(
                    self.project
                        .colormap
                        .clone()
                        .unwrap_or_else(|| "default (relief)".to_owned()),
                );
                ui.end_row();
                ui.label(theme::meta("Preset"));
                ui.label(
                    self.applied_preset
                        .clone()
                        .unwrap_or_else(|| "none".to_owned()),
                );
                ui.end_row();
            });
        ui.weak("Colormaps and presets live in the Assets tab of the left dock.");
    }

    /// M4: schema-driven `TerrainRenderParams` property pages.
    ///
    /// Sections, fields, defaults, enum choices and the pinned-parameter list
    /// all come from `terrain.params_schema`, which the bridge introspects
    /// from `make_terrain_params_config` and the nested settings dataclasses —
    /// so the panel cannot drift from the engine. An edit updates the project
    /// and schedules a debounced `terrain.params_validate`; only params the
    /// engine accepted reach the renderer, and its rejection text is shown
    /// inline instead of breaking the viewport.
    fn render_params_section(&mut self, ui: &mut Ui) {
        let Some(schema) = self.params_schema.as_ref() else {
            ui.label(egui::RichText::new("Terrain parameters").strong());
            match &self.params_schema_error {
                Some(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {error}"));
                    ui.weak(
                        "The engine did not report its parameter schema, so the pages \
                         cannot be built.",
                    );
                }
                None => {
                    ui.weak("loading the engine's parameter schema…");
                }
            }
            return;
        };
        let changes = params_pages(
            ui,
            schema,
            self.params_validation.as_deref(),
            &self.project.terrain_params,
            &mut self.param_drafts,
        );
        if changes.is_empty() {
            return;
        }
        for (section_key, key, value) in changes {
            if section_key == "root" {
                insert_root_param(&mut self.project.terrain_params, &key, value);
            } else {
                let defaults = self.section_defaults(&section_key);
                insert_section_param(
                    &mut self.project.terrain_params,
                    &section_key,
                    &defaults,
                    &key,
                    value,
                );
            }
        }
        self.project_dirty = true;
        self.schedule_params_validate();
    }

    /// Materialization defaults for one section (copied out of the schema) so
    /// the first edit to a section keeps its siblings' engine defaults.
    fn section_defaults(&self, section_key: &str) -> serde_json::Value {
        self.params_schema
            .as_ref()
            .and_then(|schema| schema.get("sections"))
            .and_then(|sections| sections.as_array())
            .and_then(|sections| {
                sections.iter().find(|section| {
                    section.get("key").and_then(|k| k.as_str()) == Some(section_key)
                })
            })
            .and_then(|section| section.get("defaults"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}))
    }

    /// W1: camera-rig spec, keyframe bake, timeline scrubber, playback, and
    /// sequence rendering.
    fn rig_section(&mut self, ui: &mut Ui) {
        ui.label(theme::group_label("Camera rig"));
        egui::ComboBox::from_id_source("rig_kind")
            .selected_text(self.rig.kind.clone())
            .show_ui(ui, |ui| {
                for kind in ["orbit", "rail", "follow"] {
                    ui.selectable_value(&mut self.rig.kind, kind.to_owned(), kind);
                }
            });
        match self.rig.kind.as_str() {
            "orbit" => {
                ui.add(Slider::new(&mut self.rig.duration, 1.0..=60.0).text("Duration (s)"));
                ui.add(Slider::new(&mut self.rig.radius_rel, 0.1..=4.0).text("Radius × DEM width"));
                ui.add(Slider::new(&mut self.rig.phi_start_deg, 0.0..=360.0).text("φ start"));
                ui.add(Slider::new(&mut self.rig.phi_end_deg, 0.0..=720.0).text("φ end"));
                ui.add(Slider::new(&mut self.rig.theta_start_deg, 5.0..=89.0).text("Elevation θ"));
            }
            _ => {
                ui.add(Slider::new(&mut self.rig.duration, 1.0..=60.0).text("Duration (s)"));
                ui.label("Path (x,z per line, normalized 0–1):");
                ui.add(
                    egui::TextEdit::multiline(&mut self.rig.path_text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace),
                );
            }
        }
        if ui.button("Bake keyframes").clicked() {
            self.bake_rig();
        }
        if !self.rig.keyframes.is_empty() {
            ui.separator();
            let total = self.rig.keyframes.len();
            ui.label(format!(
                "{} keyframes over {:.1}s ({})",
                total, self.rig.duration, self.rig.kind
            ));
            let mut frame = self.rig_playback.frame;
            let changed = ui
                .add(Slider::new(&mut frame, 0..=(total as u32 - 1)).text("Frame"))
                .changed();
            if changed {
                self.rig_playback.playing = false;
                self.rig_playback.play_started = None;
                self.scrub_to_frame(frame);
            }
            ui.horizontal(|ui| {
                let label = if self.rig_playback.playing {
                    "Pause"
                } else {
                    "Play"
                };
                if ui.button(label).clicked() {
                    self.toggle_playback();
                }
                if ui.button("Render sequence…").clicked() {
                    self.render_sequence();
                }
                if self.rig_playback.playing {
                    ui.add(egui::Spinner::new().size(12.0));
                }
            });
            if let Some(sequence) = &self.sequence {
                ui.weak(format!(
                    "sequence: {} / {} frames → {}",
                    sequence.done,
                    sequence.total,
                    sequence.out_dir.display()
                ));
            }
        }
    }

    fn export_section(&mut self, ui: &mut Ui) {
        ui.label(theme::group_label("Export"));
        let export = &mut self.project.export;
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut export.width)
                    .clamp_range(16..=8192)
                    .suffix(" px"),
            );
            ui.label("×");
            ui.add(
                egui::DragValue::new(&mut export.height)
                    .clamp_range(16..=8192)
                    .suffix(" px"),
            );
        });
        if ui.button("Export PNG…").clicked() {
            self.export_png();
        }
    }

    /// Status bar: adapter, frame time, selection, jobs, latest message.
    fn status_panel(&mut self, ctx: &Context) {
        let selection = self.selection_label();
        let mut clear = false;
        egui::TopBottomPanel::bottom("studio_status").show(ctx, |ui| {
            ui.add_space(1.0);
            let parts = StatusParts {
                adapter: self.gpu.as_ref().map(|gpu| gpu.adapter.as_str()),
                backend: self.gpu.as_ref().map(|gpu| gpu.backend.as_str()),
                software: self
                    .gpu
                    .as_ref()
                    .map(|gpu| gpu.software_fallback)
                    .unwrap_or(false),
                frame_ms: self.last_frame_ms,
                busy: self.busy,
                selection: selection.as_deref(),
                status: self.status.as_deref(),
                latest_error: self.errors.back().map(String::as_str),
                error_count: self.errors.len(),
            };
            clear = status::status_bar(ui, &self.icons, &parts);
            ui.add_space(1.0);
        });
        if clear {
            self.errors.clear();
        }
        if self.gpu.is_some() && self.last_frame_ms.is_none() && !self.busy {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Apply a camera preset through the shared gizmo mapping (U3).
    fn apply_camera_preset(&mut self, action: Action) {
        if self.project.terrain.is_none() {
            return;
        }
        if let Some(camera) = crate::ui::viewport::preset_camera(action, &self.project.camera) {
            self.project.camera = camera;
            self.push_camera();
        }
    }

    /// U3: pan the orbit target on the ground plane by screen pixels.
    fn pan_target(&mut self, dx: f32, dy: f32) {
        let camera = &self.project.camera;
        let (right, _up, forward) =
            crate::ui::viewport::camera_basis(camera.phi_deg, camera.theta_deg);
        let wpp = crate::ui::viewport::world_width_at_target(camera)
            / self.viewport_rect.width().max(1.0);
        // Screen-right on the ground plane, and screen-up's ground shadow
        // (the horizontal direction away from the camera).
        let right_xz = glam::vec2(right.x, right.z);
        let right_xz = if right_xz.length() > 1e-5 {
            right_xz / right_xz.length()
        } else {
            glam::Vec2::X
        };
        let away_xz = -glam::vec2(forward.x, forward.z);
        let away_xz = if away_xz.length() > 1e-5 {
            away_xz / away_xz.length()
        } else {
            glam::Vec2::X
        };
        let world_dx = right_xz * (-dx * wpp) + away_xz * (dy * wpp);
        let camera = &mut self.project.camera;
        // Keep the target near the dataset; the engine's span is 6x the
        // height domain, so half of that is a generous bound.
        let bound = 5.0;
        camera.target_x = (camera.target_x + world_dx.x).clamp(-bound, bound);
        camera.target_z = (camera.target_z + world_dx.y).clamp(-bound, bound);
        self.push_camera();
        self.request_viewport_render();
    }

    fn viewport_panel(&mut self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(12, 12, 14)))
            .show(ctx, |ui| {
                self.viewport_rect = ui.available_rect_before_wrap();
                let size = self.viewport_rect.size();
                let rect = self.viewport_rect;

                if let Some(image_id) = self.viewport_image() {
                    let (paint_rect, response) =
                        ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                    // U3 compare mode: A/B split against the last export.
                    let split_x = rect.left() + rect.width() * self.compare_split.clamp(0.05, 0.95);
                    if self.compare_mode {
                        if let Some(compare) = &self.compare_texture {
                            // Left of the split: current frame. Right: export.
                            if split_x > paint_rect.left() {
                                ui.painter().image(
                                    image_id,
                                    egui::Rect::from_min_max(
                                        paint_rect.left_top(),
                                        egui::pos2(split_x.min(paint_rect.right()), paint_rect.bottom()),
                                    ),
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(
                                            (split_x - paint_rect.left()) / paint_rect.width(),
                                            1.0,
                                        ),
                                    ),
                                    Color32::WHITE,
                                );
                            }
                            let right_part = egui::Rect::from_min_max(
                                egui::pos2(split_x.max(paint_rect.left()), paint_rect.top()),
                                paint_rect.right_bottom(),
                            );
                            if right_part.width() > 1.0 {
                                ui.painter().image(
                                    compare.id(),
                                    right_part,
                                    egui::Rect::from_min_max(
                                        egui::pos2(
                                            (split_x - paint_rect.left()) / paint_rect.width(),
                                            0.0,
                                        ),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    Color32::WHITE,
                                );
                            }
                            // Divider.
                            ui.painter().line_segment(
                                [
                                    egui::pos2(split_x, paint_rect.top()),
                                    egui::pos2(split_x, paint_rect.bottom()),
                                ],
                                egui::Stroke::new(1.5_f32, Color32::from_white_alpha(160)),
                            );
                        } else {
                            ui.painter().image(
                                image_id,
                                paint_rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                    } else {
                        ui.painter().image(
                            image_id,
                            paint_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }

                    self.handle_viewport_input(&response);
                    self.paint_hud(ui);
                    self.paint_gizmo(ui);
                    if self.show_safe_frame {
                        Self::paint_safe_frame(ui, rect);
                    }
                } else {
                    ui.allocate_rect(rect, egui::Sense::hover());
                    let message = if self.gpu.is_none() {
                        "Starting Forge3D engine…"
                    } else if self.project.terrain.is_some() {
                        "Rendering first frame…"
                    } else {
                        "No terrain loaded — use Scene \u{25b8} Add layer \u{25b8} Terrain DEM… (or open an example)"
                    };
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(message)
                                .size(16.0)
                                .color(Color32::from_gray(150)),
                        );
                    });
                }
            });
    }

    /// U3: zoom read-out, cursor read-out, scale bar and render time.
    fn paint_hud(&self, ui: &mut Ui) {
        use crate::ui::viewport as vp;
        let Some(terrain) = self.project.terrain.as_ref() else {
            return;
        };
        let camera = &self.project.camera;
        let span = (terrain.height_max - terrain.height_min).abs().max(1.0) * 6.0;
        let zoom = vp::zoom_percent(camera.radius, span * 1.65);
        let (bar_world, bar_label) = vp::scale_bar(vp::world_width_at_target(camera), 0.2);
        let bar_px = if vp::world_width_at_target(camera) > 0.0 {
            (bar_world / vp::world_width_at_target(camera)) * self.viewport_rect.width()
        } else {
            0.0
        };

        let painter = ui.painter();
        let origin = self.viewport_rect.left_bottom() + egui::vec2(12.0, -12.0);
        let text = egui::RichText::new(format!(
            "zoom {:.0}%{}",
            zoom,
            self.last_frame_ms
                .map(|ms| format!("  ·  frame {:.0} ms", ms))
                .unwrap_or_default(),
        ))
        .monospace()
        .size(11.0)
        .color(Color32::from_white_alpha(190));
        painter.text(
            origin + egui::vec2(0.0, -26.0),
            egui::Align2::LEFT_BOTTOM,
            text.text(),
            egui::FontId::monospace(11.0),
            Color32::from_white_alpha(190),
        );
        // Scale bar.
        let bar_y = origin.y - 8.0;
        painter.line_segment(
            [
                egui::pos2(origin.x, bar_y),
                egui::pos2(origin.x + bar_px, bar_y),
            ],
            egui::Stroke::new(1.5_f32, Color32::from_white_alpha(190)),
        );
        painter.line_segment(
            [
                egui::pos2(origin.x, bar_y - 4.0),
                egui::pos2(origin.x, bar_y + 4.0),
            ],
            egui::Stroke::new(1.5_f32, Color32::from_white_alpha(190)),
        );
        painter.line_segment(
            [
                egui::pos2(origin.x + bar_px, bar_y - 4.0),
                egui::pos2(origin.x + bar_px, bar_y + 4.0),
            ],
            egui::Stroke::new(1.5_f32, Color32::from_white_alpha(190)),
        );
        painter.text(
            egui::pos2(origin.x + bar_px + 6.0, bar_y),
            egui::Align2::LEFT_CENTER,
            bar_label,
            egui::FontId::monospace(11.0),
            Color32::from_white_alpha(190),
        );
        // Scale-bar provenance note: derived from the orbit camera maths.
        painter.text(
            egui::pos2(origin.x, bar_y + 14.0),
            egui::Align2::LEFT_CENTER,
            "scale at target plane",
            egui::FontId::monospace(9.0),
            Color32::from_white_alpha(110),
        );
    }

    /// U3: Blender-style navigation gizmo, top-right of the viewport.
    fn paint_gizmo(&mut self, ui: &mut Ui) {
        use crate::ui::viewport as vp;
        use glam::Vec3;
        let center = self.viewport_rect.right_top() - egui::vec2(52.0, 52.0);
        let radius = 34.0;
        let basis = vp::camera_basis(self.project.camera.phi_deg, self.project.camera.theta_deg);
        let painter = ui.painter();
        // Back hemisphere axes first, dimmed.
        let axes = [
            (vp::GizmoZone::PosX, Vec3::X, Color32::from_rgb(224, 94, 94)),
            (
                vp::GizmoZone::NegX,
                -Vec3::X,
                Color32::from_rgb(224, 94, 94),
            ),
            (
                vp::GizmoZone::PosY,
                Vec3::Y,
                Color32::from_rgb(148, 214, 118),
            ),
            (
                vp::GizmoZone::NegY,
                -Vec3::Y,
                Color32::from_rgb(148, 214, 118),
            ),
            (
                vp::GizmoZone::PosZ,
                Vec3::Z,
                Color32::from_rgb(108, 150, 235),
            ),
            (
                vp::GizmoZone::NegZ,
                -Vec3::Z,
                Color32::from_rgb(108, 150, 235),
            ),
        ];
        let forward = basis.2;
        let mut order: Vec<(vp::GizmoZone, Vec3, Color32)> =
            axes.iter().map(|(z, a, c)| (*z, *a, *c)).collect();
        order.sort_by_key(|(_z, axis, _c)| {
            let depth = axis.dot(forward);
            (depth * 1000.0) as i32
        });
        for (zone, axis, color) in order {
            let dir = vp::axis_screen_dir(basis, axis);
            let tip = center + egui::vec2(dir.x, dir.y) * radius;
            let towards_viewer = axis.dot(forward) > 0.0;
            let fill = if towards_viewer {
                color
            } else {
                color.gamma_multiply(0.35)
            };
            painter.line_segment([center, tip], egui::Stroke::new(2.0_f32, fill));
            painter.circle_filled(tip, 7.0, fill);
            let label = match zone {
                vp::GizmoZone::PosX => "X",
                vp::GizmoZone::NegX => "-X",
                vp::GizmoZone::PosY => "Y",
                vp::GizmoZone::NegY => "-Y",
                vp::GizmoZone::PosZ => "Z",
                vp::GizmoZone::NegZ => "-Z",
                vp::GizmoZone::Center => "",
            };
            if !label.is_empty() {
                painter.text(
                    tip,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::monospace(9.0),
                    Color32::from_black_alpha(210),
                );
            }
        }
        // Centre ball (isometric).
        painter.circle_filled(center, 6.0, Color32::from_white_alpha(210));
        painter.circle_stroke(
            center,
            6.0,
            egui::Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
        );

        // Interaction: click a handle to snap, drag anywhere on the gizmo to
        // orbit. The gizmo reserves its corner from the viewport response
        // only when the cursor is over it.
        let gizmo_rect =
            egui::Rect::from_center_size(center, egui::vec2(radius * 2.8, radius * 2.8));
        let response = ui.allocate_rect(gizmo_rect, egui::Sense::click_and_drag());
        if response.dragged() {
            let delta = response.drag_delta();
            let camera = &mut self.project.camera;
            camera.phi_deg = (camera.phi_deg + delta.x * 0.4).rem_euclid(360.0);
            camera.theta_deg = (camera.theta_deg + delta.y * 0.4).clamp(1.0, 89.0);
            self.push_camera();
            self.request_viewport_render();
        } else if response.clicked() {
            let cursor = response.interact_pointer_pos().unwrap_or(egui::Pos2::ZERO);
            if let Some(zone) = vp::gizmo_zone_at(
                glam::vec2(cursor.x, cursor.y),
                glam::vec2(center.x, center.y),
                radius,
                basis,
            ) {
                self.project.camera = vp::snap_camera(zone, &self.project.camera);
                self.push_camera();
                self.request_viewport_render();
            }
        }
    }

    fn paint_safe_frame(ui: &mut Ui, rect: egui::Rect) {
        let painter = ui.painter();
        for (fraction, label) in [(0.9, "action"), (0.8, "title")] {
            let inset = egui::vec2(
                rect.width() * (1.0 - fraction) / 2.0,
                rect.height() * (1.0 - fraction) / 2.0,
            );
            let inner = rect.shrink2(inset);
            painter.rect_stroke(
                inner,
                0.0,
                egui::Stroke::new(1.0_f32, Color32::from_white_alpha(70)),
            );
            painter.text(
                inner.left_top() + egui::vec2(4.0, 4.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::monospace(9.0),
                Color32::from_white_alpha(90),
            );
        }
    }

    /// U3: tool-aware viewport input.
    ///
    /// Left-drag is owned by the active tool; middle-drag always pans and the
    /// wheel always zooms, Blender-style. The gizmo consumes its own corner
    /// before this runs (it allocates its rect later in the same panel, which
    /// egui hit-tests on top).
    fn handle_viewport_input(&mut self, response: &Response) {
        use crate::ui::viewport::ViewportTool;
        // Middle-drag pan, regardless of tool.
        if response.dragged_by(egui::PointerButton::Middle) {
            let delta = response.drag_delta();
            self.pan_target(delta.x, delta.y);
            return;
        }
        let scroll_y = response.ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_y != 0.0 && response.hovered() {
            self.zoom_camera(1.0 + scroll_y * 0.0015);
            return;
        }
        if !response.dragged_by(egui::PointerButton::Primary) {
            return;
        }
        let delta = response.drag_delta();
        match self.viewport_tool {
            ViewportTool::Orbit => {
                let camera = &mut self.project.camera;
                camera.phi_deg = (camera.phi_deg + delta.x * 0.4).rem_euclid(360.0);
                camera.theta_deg = (camera.theta_deg + delta.y * 0.4).clamp(1.0, 89.0);
                self.push_camera();
                self.request_viewport_render();
            }
            ViewportTool::Pan => self.pan_target(delta.x, delta.y),
            ViewportTool::Zoom => self.zoom_camera(1.0 - delta.y * 0.004),
        }
    }

    /// Dispatch the registry's shortcuts. Generated from [`crate::actions`], so
    /// a new action's binding is live the moment it is declared, and plain-key
    /// bindings never fire while a text field owns the keyboard.
    fn handle_shortcuts(&mut self, ctx: &Context) {
        let text_focused = ctx.memory(|memory| memory.focused().is_some());
        let mut chosen = None;
        for def in crate::actions::ACTIONS {
            let Some(shortcut) = def.shortcut else {
                continue;
            };
            if text_focused && !shortcut.ctrl && shortcut.key != egui::Key::F1 {
                continue;
            }
            if ctx.input_mut(|input| input.consume_shortcut(&shortcut.to_egui())) {
                chosen = Some(def.action);
                break;
            }
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) && self.palette.open {
            self.palette.open = false;
        }
        if let Some(action) = chosen {
            self.run_action(ctx, action);
        }
    }

    /// Run one registry action. The match is exhaustive: declaring an action
    /// without handling it does not compile.
    fn run_action(&mut self, ctx: &Context, action: Action) {
        match action {
            Action::NewProject => self.new_project(),
            Action::OpenProject => self.open_project(),
            Action::SaveProject => self.save_project(false),
            Action::SaveProjectAs => self.save_project(true),
            Action::ImportDataset => self.import_dataset(),
            Action::ExportPng => self.export_png(),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::ZoomIn => self.zoom_camera(1.25),
            Action::ZoomOut => self.zoom_camera(0.8),
            Action::FitView => self.fit_view(),
            // U3: presets and the gizmo share one mapping (viewport.rs), so
            // the angles cannot drift — and the old hard-coded values were
            // inverted against the engine's polar convention (theta 0 is
            // top-down, 90 the horizon).
            Action::CameraTop => self.apply_camera_preset(Action::CameraTop),
            Action::CameraFront => self.apply_camera_preset(Action::CameraFront),
            Action::CameraRight => self.apply_camera_preset(Action::CameraRight),
            Action::CameraIso => self.apply_camera_preset(Action::CameraIso),
            Action::CameraReset => self.apply_camera_preset(Action::CameraReset),
            Action::ToolOrbit | Action::ToolPan | Action::ToolZoom => {
                if let Some(tool) = crate::ui::viewport::ViewportTool::from_action(action) {
                    self.viewport_tool = tool;
                    self.set_status(tool.label());
                }
            }
            Action::ToggleSafeFrame => {
                self.show_safe_frame = !self.show_safe_frame;
            }
            Action::RequestCertificate => {
                self.request_certificate = !self.request_certificate;
                self.engine
                    .send(Command::SetCertificate(self.request_certificate));
                self.set_status(if self.request_certificate {
                    "Every terrain render will now be signed (dev key; see the Certs tab)"
                } else {
                    "Terrain renders are no longer signed"
                });
            }
            Action::DeterminismCheck => {
                let params = serde_json::json!({
                    "width": 320,
                    "height": 240,
                    "camera": {
                        "phi_deg": self.project.camera.phi_deg,
                        "theta_deg": self.project.camera.theta_deg,
                        "radius": self.project.camera.radius,
                        "fov_deg": self.project.camera.fov_deg,
                        "target": [self.project.camera.target_x, 0.0,
                                   self.project.camera.target_z],
                    },
                    "sun": {
                        "azimuth_deg": self.project.sun.azimuth_deg,
                        "elevation_deg": self.project.sun.elevation_deg,
                        "intensity": self.project.sun.intensity,
                    },
                });
                let job = self.engine.call("determinism.check", params);
                self.pending_jobs.insert(job, PendingJob::DeterminismCheck);
                self.set_status("Determinism check: rendering the frame twice…");
            }
            Action::ToggleCompare => {
                self.compare_mode = !self.compare_mode;
                if self.compare_mode && self.compare_texture.is_none() {
                    self.set_status(
                        "Compare needs an exported frame — use File \u{25b8} Export PNG first",
                    );
                }
            }
            Action::LayoutDefault => self.set_layout(LayoutPreset::Default),
            Action::LayoutCartography => self.set_layout(LayoutPreset::Cartography),
            Action::LayoutModeling => self.set_layout(LayoutPreset::Modeling),
            Action::LayoutData => self.set_layout(LayoutPreset::Data),
            Action::ResetLayout => {
                self.dock = DockState::default();
                self.persist_layout();
            }
            Action::ThemeDark => self.apply_theme(ctx, true),
            Action::ThemeLight => self.apply_theme(ctx, false),
            Action::Undo => {
                let mut current = std::mem::take(&mut self.project.document);
                let moved = self.history.undo(&mut current);
                self.project.document = current;
                if moved {
                    self.after_document_history();
                }
            }
            Action::Redo => {
                let mut current = std::mem::take(&mut self.project.document);
                let moved = self.history.redo(&mut current);
                self.project.document = current;
                if moved {
                    self.after_document_history();
                }
            }
            Action::DuplicateLayer => {
                if let Some(id) = self.selection.first().copied() {
                    self.apply_layers_command(LayersCommand::Duplicate(id));
                }
            }
            Action::DeleteLayer => {
                let ids = self.selection.clone();
                self.apply_layers_command(LayersCommand::Delete(ids));
            }
            Action::GroupLayers => {
                let ids = self.selection.clone();
                self.apply_layers_command(LayersCommand::Group(ids));
            }
            Action::UngroupLayer => {
                if let Some(id) = self.selection.first().copied() {
                    self.apply_layers_command(LayersCommand::Ungroup(id));
                }
            }
            Action::RenameLayer => {
                if let Some(id) = self.selection.first().copied() {
                    self.apply_layers_command(LayersCommand::BeginRename(id));
                    self.dock.left_tab = 0;
                }
            }
            Action::SelectAllLayers => {
                self.selection = self
                    .project
                    .document
                    .layers
                    .iter()
                    .map(|layer| layer.id)
                    .collect();
            }
            Action::AddTerrainLayer => self.import_dataset(),
            Action::AddGroup => {
                let mut created = None;
                self.edit_document(|project| {
                    created = Some(project.document.push(LayerKind::Group, "Group", None));
                });
                if let Some(id) = created {
                    self.selection = vec![id];
                    self.set_status("Group added — drag layers into it");
                }
            }
            Action::ZoomToLayer => {
                if let Some(id) = self.selection.first().copied() {
                    self.apply_layers_command(LayersCommand::ZoomTo(id));
                }
            }
            Action::ToggleLayerVisibility => {
                if let Some(layer) = self.primary_layer() {
                    self.apply_layers_command(LayersCommand::SetVisible(layer.id, !layer.visible));
                }
            }
            Action::ToggleLayerLock => {
                if let Some(layer) = self.primary_layer() {
                    self.apply_layers_command(LayersCommand::SetLocked(layer.id, !layer.locked));
                }
            }
            Action::LayerMoveUp => {
                if let Some(id) = self.selection.first().copied() {
                    self.apply_layers_command(LayersCommand::MoveUp(id));
                }
            }
            Action::LayerMoveDown => {
                if let Some(id) = self.selection.first().copied() {
                    self.apply_layers_command(LayersCommand::MoveDown(id));
                }
            }
            Action::LayerToTop => {
                if let Some(id) = self.selection.first().copied() {
                    self.edit_document(|project| {
                        project.document.move_to_edge(id, true);
                    });
                }
            }
            Action::LayerToBottom => {
                if let Some(id) = self.selection.first().copied() {
                    self.edit_document(|project| {
                        project.document.move_to_edge(id, false);
                    });
                }
            }
            Action::RenderFrame => {
                if self.project.terrain.is_some() {
                    self.invalidate_viewport();
                    self.set_status("Rendering the current view…");
                } else {
                    self.set_status(
                        "No terrain loaded — use File ▸ Import dataset… to add a DEM first",
                    );
                }
            }
            Action::ShowExamples => {
                self.gallery.open = true;
                self.prefetch_gallery_thumbnails();
            }
            Action::ShowShortcuts => self.shortcuts_open = true,
            Action::ShowAbout => self.set_status(format!(
                "Forge3D Studio {} — embedded forge3d engine, bridge v2 (17 operations)",
                env!("CARGO_PKG_VERSION")
            )),
            Action::OpenPalette => self.palette.toggle(),
        }
    }

    fn set_layout(&mut self, preset: LayoutPreset) {
        self.dock.apply_preset(preset);
        self.persist_layout();
    }

    fn persist_layout(&mut self) {
        self.dock.save();
        self.saved_dock = self.dock.clone();
    }

    /// Zoom the orbit camera by `factor` (>1 zooms out, <1 zooms in).
    fn zoom_camera(&mut self, factor: f32) {
        if self.project.terrain.is_none() {
            return;
        }
        self.project.camera.radius = (self.project.camera.radius * factor).clamp(10.0, 100_000.0);
        self.push_camera();
    }

    /// Named camera direction (phi = orbit azimuth, theta = elevation).
    /// Re-fit the orbit radius to the loaded dataset, keeping the angles.
    fn fit_view(&mut self) {
        let Some(terrain) = &self.project.terrain else {
            return;
        };
        // World scale, not pixels: the terrain spans `range * 6` world units
        // (the bridge's and the native viewport's shared contract), and a
        // fresh load frames it at 1.65x that span.
        let span = (terrain.height_max - terrain.height_min).abs().max(1.0) * 6.0;
        self.project.camera.radius = span * 1.65;
        self.push_camera();
    }

    fn push_camera(&mut self) {
        self.project_dirty = true;
        self.engine.send(Command::SetCamera(self.project.camera));
        self.invalidate_viewport();
    }

    /// U4: materialize committed golden thumbnails before the gallery shows.
    /// One batched `git lfs pull` for every missing reference render; the
    /// status bar discloses that this downloads files.
    fn prefetch_gallery_thumbnails(&mut self) {
        if self.lfs_fetch.is_some() {
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let missing: Vec<PathBuf> = crate::ui::examples::entries()
            .iter()
            .filter_map(|entry| entry.thumbnail.as_deref())
            .map(|rel| crate::app::resolve_repo_path(Path::new(rel)))
            .filter(|path| !path.exists() || is_lfs_pointer(path))
            .collect();
        if missing.is_empty() {
            return;
        }
        let includes = missing
            .iter()
            .filter_map(|path| path.strip_prefix(&root).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(",");
        self.set_status(format!(
            "Fetching {} reference render(s) for the gallery (git lfs pull — this downloads files)…",
            missing.len()
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        self.lfs_fetch = Some(rx);
        std::thread::Builder::new()
            .name("studio-lfs-thumbs".to_owned())
            .spawn(move || {
                let output = std::process::Command::new("git")
                    .args(["lfs", "pull", "--include"])
                    .arg(&includes)
                    .current_dir(&root)
                    .output();
                let _ = tx.send(output.map(|o| o.status.success()));
            })
            .ok();
    }

    /// Materialize an unresolved git-LFS fixture in the background and open
    /// the pending example when it lands. The user is told this is a
    /// download: the status bar names the command being run.
    fn materialize_lfs_asset(&mut self, path: PathBuf) {
        if self.lfs_fetch.is_some() {
            self.push_error(
                "Examples gallery",
                "an asset fetch is already running; try again when it finishes",
            );
            return;
        }
        let rel = resolve_repo_path(&path); // absolute now
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let include = rel
            .strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or(rel.clone());
        self.set_status(format!(
            "Fetching engine fixture {} (git lfs pull — this downloads the file)…",
            include.display()
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        self.lfs_fetch = Some(rx);
        std::thread::Builder::new()
            .name("studio-lfs-fetch".to_owned())
            .spawn(move || {
                let output = std::process::Command::new("git")
                    .args(["lfs", "pull", "--include"])
                    .arg(&include)
                    .current_dir(&root)
                    .output();
                let _ = tx.send(output.map(|o| o.status.success()));
            })
            .ok();
    }

    /// Poll a running LFS fetch; when it finishes, retry the pending example
    /// or surface the failure with the exact remedy.
    fn poll_lfs_fetch(&mut self) {
        let done = match &self.lfs_fetch {
            Some(rx) => rx.try_recv().ok(),
            None => None,
        };
        if let Some(result) = done {
            self.lfs_fetch = None;
            self.gallery.failed.clear();
            match result {
                Ok(true) => {
                    self.set_status("Engine fixture fetched; opening example…");
                    if let Some(path) = self
                        .pending_example
                        .as_ref()
                        .and_then(|entry| entry.scene.as_ref())
                        .and_then(|scene| scene.get("dem"))
                        .and_then(|dem| dem.as_str())
                        .map(PathBuf::from)
                    {
                        if path.is_file() && !is_lfs_pointer(&path) {
                            self.apply_pending_example();
                            return;
                        }
                    }
                    self.push_error(
                        "Examples gallery",
                        "git lfs pull reported success but the fixture is still not materialized",
                    );
                    self.pending_example = None;
                }
                Ok(false) => {
                    self.push_error(
                        "Examples gallery",
                        "git lfs pull failed. Materialize the fixture manually:                          `git lfs pull --include=<asset>` in the repository root.",
                    );
                    self.pending_example = None;
                }
                Err(err) => {
                    self.push_error(
                        "Examples gallery",
                        format!(
                            "could not run `git lfs pull` ({err}). Materialize the fixture                              manually with `git lfs pull` in the repository root."
                        ),
                    );
                    self.pending_example = None;
                }
            }
        }
    }

    /// U2: serialize the document (plus environment) for `scene.compile`.
    fn document_compile_params(&self) -> serde_json::Value {
        let layers = serde_json::to_value(&self.project.document.layers)
            .unwrap_or(serde_json::Value::Array(vec![]));
        let terrain = self.project.terrain.as_ref();
        serde_json::json!({
            "document": { "layers": layers },
            "environment": {
                "camera": {
                    "distance": self.mapscene.camera_distance,
                    "azimuth_deg": self.mapscene.camera_azimuth,
                    "elevation_deg": self.mapscene.camera_elevation,
                    "fov_deg": self.mapscene.camera_fov,
                },
                "lighting": { "name": "daylight", "intensity": 1.1 },
                "output": {
                    "width": self.project.export.width,
                    "height": self.project.export.height,
                },
                "_terrain_present": terrain.is_some(),
            },
        })
    }

    /// U2: compile the document into a recipe (and optionally render it).
    /// This is the only path to a MapScene render in Studio: the document is
    /// the source of truth, the recipe is compiled, never hand-edited.
    fn compile_and_stage_recipe(&mut self, render_after: bool) {
        if self.scene_mode != SceneMode::MapScene {
            return;
        }
        let params = self.document_compile_params();
        let job = self.engine.call("scene.compile", params);
        self.pending_jobs
            .insert(job, PendingJob::SceneCompile { render_after });
    }

    /// U2: mirror a template's recipe layers into the document so the Layers
    /// panel and the inspector drive them. The terrain row already exists
    /// (the DEM import created it); generated rows are replaced on rebuild.
    fn sync_document_from_recipe(&mut self, recipe: &serde_json::Value) {
        use studio_project::document::LayerKind;
        // Remove previously generated rows (keep the terrain layer).
        let generated = self
            .project
            .document
            .layers
            .iter()
            .filter(|l| l.kind != LayerKind::Terrain)
            .map(|l| l.id)
            .collect::<Vec<_>>();
        for id in generated {
            self.project.document.remove(id);
        }
        self.selection.clear();
        if let Some(layers) = recipe.get("layers").and_then(|l| l.as_array()) {
            for layer in layers {
                let kind_str = layer
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("reference");
                let kind = LayerKind::from_recipe_type(kind_str);
                let name = layer
                    .get("layer_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(kind_str)
                    .to_owned();
                let source = layer.get("path").and_then(|v| v.as_str()).map(String::from);
                let mut style = serde_json::json!({});
                if let Some(features) = layer.get("features") {
                    style["features"] = features.clone();
                }
                if let Some(mapbox) = layer.get("style") {
                    style["mapbox"] = mapbox.clone();
                }
                if let Some(labels) = layer.get("labels") {
                    style["labels"] = labels.clone();
                }
                style["crs"] = layer.get("crs").cloned().unwrap_or(serde_json::Value::Null);
                self.project
                    .document
                    .push_detailed(kind, name, true, source, style, None);
            }
        }
    }

    /// Open one gallery entry: load its dataset (async) and apply the scene.
    fn open_example(&mut self, entry: crate::ui::examples::ExampleEntry) {
        let Some(scene) = entry.scene.clone() else {
            return;
        };
        self.set_status(format!("Opening example '{}'…", entry.title));
        let mut entry = entry;
        if let Some(scene) = entry.scene.as_mut() {
            // Gallery scenes reference repo-root-relative fixtures; resolve
            // them before anything touches the filesystem.
            for key in ["dem", "raster"] {
                if let Some(raw) = scene.get(key).and_then(|v| v.as_str()) {
                    scene[key] = serde_json::json!(resolve_repo_path(Path::new(raw)));
                }
            }
        }
        self.pending_example = Some(entry);
        let dem = scene
            .get("dem")
            .and_then(|dem| dem.as_str())
            .map(PathBuf::from);
        if let Some(path) = &dem {
            if path.is_file() && is_lfs_pointer(path) {
                self.materialize_lfs_asset(path.clone());
                return;
            }
            if !path.exists() {
                let hint = format!(
                    "Example dataset '{}' is missing. Engine fixtures live in git LFS —                      run `git lfs pull --include={}` in the repository, or pick another example.",
                    path.display(),
                    path.display()
                );
                self.push_error("Examples gallery", hint);
                self.pending_example = None;
                return;
            }
        }
        let needs_load = match (&dem, &self.project.terrain) {
            (Some(path), Some(terrain)) => terrain.path != *path,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if needs_load {
            if let Some(path) = dem {
                self.engine.send(Command::LoadTerrain { path });
            }
            return;
        }
        self.apply_pending_example();
    }

    /// Finish an example once its dataset is available.
    fn apply_pending_example(&mut self) {
        let Some(entry) = self.pending_example.take() else {
            return;
        };
        let Some(scene) = entry.scene.clone() else {
            return;
        };
        let kind = scene
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or("terrain");
        match kind {
            "mapscene" => {
                self.scene_mode = SceneMode::MapScene;
                self.project.scene_mode = studio_project::SCENE_MODE_MAPSCENE.to_owned();
                self.mapscene.template = scene
                    .get("template")
                    .and_then(|template| template.as_str())
                    .unwrap_or("terrain")
                    .to_owned();
                self.mapscene.raster_path = scene
                    .get("raster")
                    .and_then(|raster| raster.as_str())
                    .map(PathBuf::from);
                if let Some(text) = scene.get("label_text").and_then(|text| text.as_str()) {
                    self.mapscene.label_text = text.to_owned();
                }
                self.pending_mapscene_camera = scene.get("camera").cloned();
                self.render_after_recipe = true;
                self.dock.left_tab = 1;
                self.build_mapscene_template();
            }
            _ => {
                self.scene_mode = SceneMode::Terrain;
                self.project.scene_mode = studio_project::SCENE_MODE_TERRAIN.to_owned();
                if let Some(camera) = scene.get("camera") {
                    // Scenes express distance relative to the terrain span so a
                    // framing survives a dataset change; the engine's world is
                    // (domain span × 6) wide (see `_op_terrain_render`).
                    let span = self
                        .project
                        .terrain
                        .as_ref()
                        .map(|terrain| {
                            (terrain.height_max - terrain.height_min).abs().max(1.0) * 6.0
                        })
                        .unwrap_or(1.0);
                    let radius = match camera.get("radius_rel").and_then(|rel| rel.as_f64()) {
                        Some(relative) => span * relative as f32,
                        None => number(camera, "radius", self.project.camera.radius),
                    };
                    self.project.camera = CameraParams {
                        phi_deg: number(camera, "phi_deg", self.project.camera.phi_deg),
                        theta_deg: number(camera, "theta_deg", self.project.camera.theta_deg),
                        radius,
                        fov_deg: number(camera, "fov_deg", self.project.camera.fov_deg),
                        ..self.project.camera
                    };
                }
                if let Some(sun) = scene.get("sun") {
                    self.project.sun = SunParams {
                        azimuth_deg: number(sun, "azimuth_deg", self.project.sun.azimuth_deg),
                        elevation_deg: number(sun, "elevation_deg", self.project.sun.elevation_deg),
                        intensity: number(sun, "intensity", self.project.sun.intensity),
                    };
                }
                if let Some(params) = scene.get("params") {
                    self.project.terrain_params = params.clone();
                    self.schedule_params_validate();
                }
                self.sync_engine_state();
                self.dock.left_tab = 0;
                if kind == "terrain_rig" {
                    if let Some(rig) = scene.get("rig") {
                        self.rig.kind = rig
                            .get("kind")
                            .and_then(|kind| kind.as_str())
                            .unwrap_or("orbit")
                            .to_owned();
                        self.rig.duration = number(rig, "duration", self.rig.duration);
                        self.rig.radius_rel = number(rig, "radius_rel", self.rig.radius_rel);
                        self.rig.phi_start_deg =
                            number(rig, "phi_start_deg", self.rig.phi_start_deg);
                        self.rig.phi_end_deg = number(rig, "phi_end_deg", self.rig.phi_end_deg);
                        self.rig.theta_start_deg =
                            number(rig, "theta_start_deg", self.rig.theta_start_deg);
                    }
                    self.dock.bottom_tab = 0;
                    self.bake_rig();
                } else {
                    self.invalidate_viewport();
                }
                self.set_status(format!("Example '{}' applied", entry.title));
            }
        }
    }

    /// The primary selected layer, if it still exists.
    fn primary_layer(&self) -> Option<&Layer> {
        self.selection
            .first()
            .and_then(|id| self.project.document.layer(*id))
    }

    /// After an undo/redo: prune dead selection, keep the file dirty, and
    /// re-sync engine-facing state the document mirrors.
    fn after_document_history(&mut self) {
        self.project_dirty = true;
        self.selection
            .retain(|id| self.project.document.layer(*id).is_some());
        self.project.terrain_visible = self
            .project
            .document
            .terrain_layer()
            .map(|layer| layer.visible)
            .unwrap_or(true);
        self.invalidate_viewport();
    }

    /// M3: the unified import dialog — per-format metadata preview from
    /// `dataset.inspect`, then load-as-terrain or register-in-project.
    fn import_window(&mut self, ctx: &Context) {
        let Some((path, result)) = self
            .import_preview
            .as_ref()
            .map(|preview| (preview.path.clone(), preview.result.clone()))
        else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut load_terrain = false;
        let mut add_project = false;
        egui::Window::new("Import dataset")
            .id(egui::Id::new("studio_import_window"))
            .open(&mut open)
            .default_width(440.0)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(path.display().to_string()).strong());
                let format = result.get("format").and_then(|v| v.as_str()).unwrap_or("?");
                let kind = result.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                ui.horizontal(|ui| {
                    ui.label(format!("format: {format}"));
                    ui.weak(format!("kind: {kind}"));
                });
                ui.separator();
                egui::Grid::new("import_metadata")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        if let Some(meta) = result.get("metadata").and_then(|m| m.as_object()) {
                            for (key, value) in meta {
                                ui.label(key);
                                let text = if value.is_null() {
                                    "—".to_owned()
                                } else if value.is_object() || value.is_array() {
                                    value.to_string()
                                } else {
                                    value
                                        .as_str()
                                        .map(String::from)
                                        .unwrap_or_else(|| value.to_string())
                                };
                                ui.label(text).on_hover_text(value.to_string());
                                ui.end_row();
                            }
                        }
                    });
                if let Some(notes) = result.get("notes").and_then(|n| n.as_array()) {
                    for note in notes.iter().filter_map(|n| n.as_str()) {
                        ui.weak(note);
                    }
                }
                ui.separator();
                let loadable = result
                    .get("terrain_loadable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(loadable, egui::Button::new("Load as terrain"))
                        .on_hover_text(if loadable {
                            "Load this dataset into the terrain slot"
                        } else {
                            "this format cannot become the terrain slot"
                        })
                        .clicked()
                    {
                        load_terrain = true;
                    }
                    if ui.button("Add to project").clicked() {
                        add_project = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            open = false;
        }
        if load_terrain {
            self.load_preview_as_terrain();
        } else if add_project {
            self.add_preview_to_project();
        } else if !open {
            self.import_preview = None;
        }
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.sync_window_title(ctx);
        self.handle_shortcuts(ctx);
        self.poll_engine_events(ctx);
        self.poll_native_viewport(ctx);
        self.poll_lfs_fetch();
        self.tick_playback(ctx);

        self.top_panel(ctx);
        self.status_panel(ctx);
        self.bottom_dock(ctx);
        self.left_panel(ctx);
        self.right_panel(ctx);
        // M10: draw before laying the panel out, using the previous frame's
        // viewport rect (a size change re-renders on the next frame).
        self.render_native_viewport(ctx);
        self.viewport_panel(ctx);
        self.import_window(ctx);
        if let Some(action) = crate::ui::palette::window(ctx, &mut self.palette) {
            self.run_action(ctx, action);
        }
        shortcuts::window(ctx, &mut self.shortcuts_open);
        if let Some(entry) =
            crate::ui::examples::window(ctx, &mut self.gallery, &self.icons, &self.tokens)
        {
            self.open_example(entry);
        }

        // Persist layout changes (splitter drags included) once they settle.
        let (width, height) = (ctx.screen_rect().width(), ctx.screen_rect().height());
        self.dock.clamp(width, height);
        if self.dock != self.saved_dock {
            self.persist_layout();
        }

        // Kick the very first render once the GPU is up. With a live native
        // viewport there is nothing to queue: it redraws only when its view
        // changes (`invalidate_viewport` marks it dirty).
        if !self.native_paints()
            && self.gpu.is_some()
            && self.viewport_texture.is_none()
            && self.pending_render.is_none()
        {
            self.request_viewport_render();
        }
        // Re-render when the viewport is resized.
        if let Some(texture) = &self.viewport_texture {
            let [tw, th] = texture.size();
            let want = self.viewport_rect.size();
            let want = (
                (want.x.round() as u32).max(1),
                (want.y.round() as u32).max(1),
            );
            if (tw as u32, th as u32) != want && self.pending_render.is_none() && !self.busy {
                self.viewport_texture = None;
                self.request_viewport_render();
            }
        }
        self.flush_params_validate();
        self.flush_pending_render();

        // A native repaint that could not draw this frame (the viewport rect
        // was not known yet, say) needs one more frame to land.
        if self.native_paints() && self.native.dirty {
            ctx.request_repaint();
        }
    }
}

/// Read a number from a scene spec, falling back to the current value.
fn number(value: &serde_json::Value, key: &str, fallback: f32) -> f32 {
    value
        .get(key)
        .and_then(|number| number.as_f64())
        .map(|number| number as f32)
        .unwrap_or(fallback)
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "untitled".to_owned()
    } else {
        cleaned
    }
}

// ------------------------------------------------------- M4: params widgets

/// `height_curve_mode` -> `Height Curve Mode` (schema keys are snake_case).
fn title_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for (index, word) in key.split('_').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Draw the whole parameter page tree and return the pending edits.
///
/// Independent of `StudioApp` so it can be rendered headlessly in tests; the
/// caller applies the returned edits and schedules the engine validation.
fn params_pages(
    ui: &mut Ui,
    schema: &serde_json::Value,
    validation: Option<&str>,
    params: &serde_json::Value,
    drafts: &mut HashMap<String, String>,
) -> Vec<(String, String, serde_json::Value)> {
    ui.label(egui::RichText::new("Terrain parameters").strong());
    if let Some(message) = validation {
        ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {message}"));
        ui.weak("The viewport keeps the last valid parameters until this is fixed.");
        ui.add_space(4.0);
    }
    let mut changes = Vec::new();
    let Some(sections) = schema
        .get("sections")
        .and_then(|sections| sections.as_array())
    else {
        ui.weak("the engine reported no editable terrain parameters");
        return changes;
    };
    for section in sections {
        let Some(section_key) = section.get("key").and_then(|key| key.as_str()) else {
            continue;
        };
        let fields = section.get("fields").and_then(|fields| fields.as_array());
        if section_key == "root" {
            // Root scalars sit flat under the header; settings collapse.
            if let Some(fields) = fields {
                for field in fields {
                    if let Some(change) = param_field(ui, section_key, field, params, drafts) {
                        changes.push(change);
                    }
                }
            }
            continue;
        }
        egui::CollapsingHeader::new(title_case(section_key))
            .id_source(format!("params_section_{section_key}"))
            .show(ui, |ui| {
                if !section
                    .get("editable")
                    .and_then(|editable| editable.as_bool())
                    .unwrap_or(false)
                {
                    ui.weak(
                        section
                            .get("reason")
                            .and_then(|reason| reason.as_str())
                            .unwrap_or("not editable from Studio"),
                    );
                    return;
                }
                let Some(fields) = fields else { return };
                for field in fields {
                    if let Some(change) = param_field(ui, section_key, field, params, drafts) {
                        changes.push(change);
                    }
                }
            });
    }
    // Honest reporting: params the pages cannot edit say why.
    if let Some(excluded) = schema.get("excluded").and_then(|e| e.as_array()) {
        if !excluded.is_empty() {
            egui::CollapsingHeader::new(format!("Not exposed ({})", excluded.len()))
                .id_source("params_excluded")
                .show(ui, |ui| {
                    for entry in excluded {
                        let key = entry.get("key").and_then(|k| k.as_str()).unwrap_or("?");
                        let reason = entry
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .unwrap_or("engine-internal");
                        ui.weak(format!("{key} — {reason}"));
                    }
                });
        }
    }
    changes
}

/// Render one schema field; returns the `(section, key, value)` change, if any.
///
/// The widget type follows the schema's field type: checkbox for booleans,
/// drag values for numbers, a combo box for engine-validated enums, a text
/// field otherwise. Enum choices come from the engine, so a bad value can
/// only come from free text — where `terrain.params_validate` catches it.
fn param_field(
    ui: &mut Ui,
    section_key: &str,
    field: &serde_json::Value,
    params: &serde_json::Value,
    drafts: &mut HashMap<String, String>,
) -> Option<(String, String, serde_json::Value)> {
    let key = field.get("key").and_then(|key| key.as_str())?;
    let kind = field.get("type").and_then(|kind| kind.as_str())?;
    let nullable = field
        .get("nullable")
        .and_then(|nullable| nullable.as_bool())
        .unwrap_or(false);
    let default = field
        .get("default")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let current = match section_key {
        "root" => params.get(key),
        _ => params.get(section_key).and_then(|section| section.get(key)),
    }
    .cloned()
    .unwrap_or(default);
    let label = title_case(key);
    let id = format!("param_{section_key}_{key}");
    let change = |value: serde_json::Value| Some((section_key.to_owned(), key.to_owned(), value));

    match kind {
        "bool" => {
            let mut value = current.as_bool().unwrap_or(false);
            ui.checkbox(&mut value, label)
                .changed()
                .then(|| change(serde_json::json!(value)))?
        }
        "int" if !nullable => {
            let mut value = current.as_i64().unwrap_or(0);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&label);
                changed = ui
                    .add(egui::DragValue::new(&mut value).speed(1.0))
                    .changed();
            });
            if changed {
                return change(serde_json::json!(value));
            }
            None
        }
        "float" if !nullable => {
            let mut value = current.as_f64().unwrap_or(0.0);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&label);
                changed = ui.add(egui::DragValue::new(&mut value)).changed();
            });
            if changed {
                return change(serde_json::json!(value));
            }
            None
        }
        "str" => {
            if let Some(choices) = field.get("choices").and_then(|c| c.as_array()) {
                let current_text = current.as_str().unwrap_or_default().to_owned();
                let mut selected = current_text.clone();
                let mut picked = false;
                ui.horizontal(|ui| {
                    ui.label(&label);
                    egui::ComboBox::from_id_source(&id)
                        .selected_text(selected.clone())
                        .show_ui(ui, |ui| {
                            for choice in choices.iter().filter_map(|c| c.as_str()) {
                                if ui
                                    .selectable_value(&mut selected, choice.to_owned(), choice)
                                    .changed()
                                {
                                    picked = true;
                                }
                            }
                        });
                });
                return if picked && selected != current_text {
                    change(serde_json::json!(selected))
                } else {
                    None
                };
            }
            // Free-form string: the engine's validator is the authority.
            let mut text = current.as_str().unwrap_or_default().to_owned();
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&label);
                changed = ui
                    .add(egui::TextEdit::singleline(&mut text).desired_width(140.0))
                    .changed();
            });
            if changed {
                return change(serde_json::json!(text));
            }
            None
        }
        _ => param_text_field(ui, &id, &label, &current, kind, nullable, drafts).and_then(change),
    }
}

/// Text field whose contents are parsed before they are committed — nullable
/// numbers (empty box = engine default) and numeric arrays. Unparseable input
/// stays in the draft, so a half-typed `1.` or `1, 2,` is not fought across
/// frames; nothing reaches the project until the text is a complete value.
fn param_text_field(
    ui: &mut Ui,
    id: &str,
    label: &str,
    committed: &serde_json::Value,
    kind: &str,
    nullable: bool,
    drafts: &mut HashMap<String, String>,
) -> Option<serde_json::Value> {
    let canonical = match kind {
        "float[]" | "int[]" => committed
            .as_array()
            .map(|items| {
                items
                    .iter()
                    // 6000.0 reads better as "6000" in an editable box.
                    .map(|item| {
                        item.as_f64()
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| item.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => committed
            .as_f64()
            .map(|value| value.to_string())
            .unwrap_or_default(),
    };
    let mut text = drafts
        .entry(id.to_owned())
        .or_insert_with(|| canonical.clone())
        .clone();
    let mut committed_value = None;
    ui.horizontal(|ui| {
        ui.label(label);
        let response = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(120.0)
                .hint_text(if nullable { "default" } else { "" }),
        );
        if response.changed() {
            committed_value = parse_param_text(&text, kind, nullable);
        }
    });
    drafts.insert(id.to_owned(), text);
    committed_value
}

/// Parse a text field into a params value; `None` means "not a complete value
/// yet" (keep the draft). An empty box is the engine default when nullable.
fn parse_param_text(text: &str, kind: &str, nullable: bool) -> Option<serde_json::Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return nullable.then_some(serde_json::Value::Null);
    }
    match kind {
        "float" => trimmed
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| serde_json::json!(value)),
        "int" => trimmed
            .parse::<i64>()
            .ok()
            .map(|value| serde_json::json!(value)),
        "float[]" => {
            let mut values = Vec::new();
            for part in trimmed.split(',') {
                let value = part.trim().parse::<f64>().ok()?;
                if !value.is_finite() {
                    return None;
                }
                values.push(value);
            }
            Some(serde_json::json!(values))
        }
        "int[]" => {
            let mut values = Vec::new();
            for part in trimmed.split(',') {
                values.push(part.trim().parse::<i64>().ok()?);
            }
            Some(serde_json::json!(values))
        }
        _ => None,
    }
}

/// Write a root-level param into the project's params object.
fn insert_root_param(params: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    if !params.is_object() {
        *params = serde_json::Value::Object(serde_json::Map::new());
    }
    if let Some(map) = params.as_object_mut() {
        map.insert(key.to_owned(), value);
    }
}

/// Write a section field, materializing the section from its engine defaults
/// first so the sibling fields keep the values a render would really have used.
fn insert_section_param(
    params: &mut serde_json::Value,
    section: &str,
    defaults: &serde_json::Value,
    key: &str,
    value: serde_json::Value,
) {
    if !params.is_object() {
        *params = serde_json::Value::Object(serde_json::Map::new());
    }
    let Some(map) = params.as_object_mut() else {
        return;
    };
    let entry = map
        .entry(section.to_owned())
        .or_insert_with(|| defaults.clone());
    if !entry.is_object() {
        *entry = defaults.clone();
    }
    if let Some(section_map) = entry.as_object_mut() {
        section_map.insert(key.to_owned(), value);
    }
}

#[cfg(test)]
mod params_tests {
    use super::*;

    /// A miniature schema shaped exactly like `terrain.params_schema`, with one
    /// field per widget type the bridge can emit.
    fn test_schema() -> serde_json::Value {
        serde_json::json!({
            "excluded": [
                { "key": "size_px", "reason": "output size (viewport / export dialog)" },
                { "key": "vt", "reason": "requires an open virtual-texture store" }
            ],
            "sections": [
                {
                    "key": "root",
                    "editable": true,
                    "defaults": {},
                    "fields": [
                        { "key": "exposure", "type": "float", "nullable": false, "default": 1.0 },
                        { "key": "msaa_samples", "type": "int", "nullable": false, "default": 1 },
                        { "key": "ibl_enabled", "type": "bool", "nullable": false, "default": true },
                        {
                            "key": "albedo_mode",
                            "type": "str",
                            "nullable": false,
                            "default": "colormap",
                            "choices": ["colormap", "mix", "material"]
                        },
                        { "key": "culling", "type": "str", "nullable": false, "default": "frustum" },
                        {
                            "key": "clip",
                            "type": "float[]",
                            "nullable": true,
                            "default": [0.1, 6000.0]
                        },
                        {
                            "key": "nodata_height_below",
                            "type": "float",
                            "nullable": true,
                            "default": null
                        }
                    ]
                },
                {
                    "key": "shadows",
                    "editable": true,
                    "defaults": { "technique": "PCSS", "resolution": 4096 },
                    "fields": [
                        {
                            "key": "technique",
                            "type": "str",
                            "nullable": false,
                            "default": "PCSS",
                            "choices": ["HARD", "PCSS"]
                        },
                        {
                            "key": "resolution",
                            "type": "int",
                            "nullable": false,
                            "default": 4096
                        }
                    ]
                }
            ]
        })
    }

    /// Lay the pages out twice in a headless egui context: every widget type
    /// must render without panicking and without inventing edits. (Collapsed
    /// section bodies are only laid out on screen, so this covers the root
    /// fields plus the shared header/excluded chrome.)
    #[test]
    fn param_pages_render_headlessly_without_inventing_edits() {
        let schema = test_schema();
        let params = serde_json::json!({});
        let mut drafts = HashMap::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(420.0, 700.0),
            )),
            ..Default::default()
        };
        for frame in 0..2 {
            let mut changes = None;
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changes = Some(params_pages(
                        ui,
                        &schema,
                        Some("msaa_samples must be 1, 2, 4, or 8"),
                        &params,
                        &mut drafts,
                    ));
                });
            });
            let changes = changes.expect("panel ran");
            assert!(
                changes.is_empty(),
                "frame {frame}: laying out the pages must not produce edits, got {changes:?}"
            );
        }
        // The parse-on-commit fields got their canonical text as a draft.
        assert_eq!(
            drafts.get("param_root_clip").map(String::as_str),
            Some("0.1, 6000")
        );
        assert_eq!(
            drafts
                .get("param_root_nodata_height_below")
                .map(String::as_str),
            Some("")
        );
        // Values that are not editable in a page leave no draft behind.
        assert!(!drafts.contains_key("param_root_exposure"));
    }

    #[test]
    fn title_case_builds_headers_from_schema_keys() {
        assert_eq!(title_case("shadows"), "Shadows");
        assert_eq!(title_case("height_curve_mode"), "Height Curve Mode");
        assert_eq!(title_case("pom"), "Pom");
        assert_eq!(title_case(""), "");
    }

    #[test]
    fn parse_param_text_waits_for_complete_values() {
        // Plain numbers commit as soon as they parse.
        assert_eq!(
            parse_param_text("1.5", "float", false),
            Some(serde_json::json!(1.5))
        );
        // "1." is a complete number, "-" is not: the draft must survive.
        assert_eq!(
            parse_param_text("1.", "float", false),
            Some(serde_json::json!(1.0))
        );
        assert_eq!(parse_param_text("-", "float", false), None);
        assert_eq!(parse_param_text("", "float", false), None);
        // An empty box means "engine default" only where null is legal.
        assert_eq!(
            parse_param_text("", "float", true),
            Some(serde_json::Value::Null)
        );
        // Arrays: a trailing comma is still being typed.
        assert_eq!(
            parse_param_text("0.1, 6000", "float[]", false),
            Some(serde_json::json!([0.1, 6000.0]))
        );
        assert_eq!(parse_param_text("1,2,", "float[]", false), None);
        assert_eq!(
            parse_param_text("", "float[]", true),
            Some(serde_json::Value::Null)
        );
        // Non-finite values never reach the project.
        assert_eq!(parse_param_text("inf", "float", false), None);
    }

    #[test]
    fn section_edits_materialize_the_engine_defaults() {
        let defaults = serde_json::json!({ "resolution": 4096, "cascades": 3 });
        let mut params = serde_json::json!({});
        insert_section_param(
            &mut params,
            "shadows",
            &defaults,
            "cascades",
            serde_json::json!(2),
        );
        assert_eq!(
            params,
            serde_json::json!({ "shadows": { "resolution": 4096, "cascades": 2 } }),
            "the first edit to a section must keep its engine defaults"
        );
        insert_section_param(
            &mut params,
            "shadows",
            &defaults,
            "resolution",
            serde_json::json!(1024),
        );
        assert_eq!(
            params,
            serde_json::json!({ "shadows": { "resolution": 1024, "cascades": 2 } }),
            "later edits must not re-materialize over earlier ones"
        );

        insert_root_param(&mut params, "exposure", serde_json::json!(1.5));
        assert_eq!(params["exposure"], serde_json::json!(1.5));

        // A malformed params document is repaired, never silently dropped.
        let mut broken = serde_json::Value::Null;
        insert_root_param(&mut broken, "z_scale", serde_json::json!(2.0));
        assert_eq!(broken, serde_json::json!({ "z_scale": 2.0 }));
    }
}
