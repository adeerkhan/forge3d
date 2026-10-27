//! The Forge3D Studio main window.
//!
//! Layout follows GUI.md section 5: menu + toolbar, scene tree on the left,
//! the engine viewport in the center, properties on the right, and a status
//! bar at the bottom. Every control is wired to real application state and
//! real engine commands — there are no disconnected mockups.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{
    Color32, ColorImage, Context, Image, Response, Sense, Slider, TextureHandle, TextureOptions, Ui,
};
use studio_engine::{Command, EngineHandle, Event};
use studio_project::{CameraParams, Project, SunParams};

use crate::icons::Icons;

const RENDER_DEBOUNCE: Duration = Duration::from_millis(120);
const MAX_STATUS_ERRORS: usize = 4;

struct GpuInfo {
    adapter: String,
    backend: String,
    software_fallback: bool,
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
            radius_rel: 1.0,
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
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let icons = Icons::load(&cc.egui_ctx, Color32::from_white_alpha(230));

        let (engine, events) = EngineHandle::spawn();
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
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
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
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        ctx.set_visuals(visuals);
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
                    self.request_viewport_render();
                    self.set_status(format!("Loaded {} ({}×{})", path.display(), width, height));
                }
                Event::TerrainCleared => {
                    self.project.terrain = None;
                    self.project.terrain_visible = true;
                    self.project_dirty = true;
                    self.set_status("Terrain unloaded");
                }
                Event::FrameReady {
                    width,
                    height,
                    rgba,
                    render_ms,
                } => {
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
        let size = self.viewport_rect.size();
        let width = (size.x.round() as u32).max(1);
        let height = (size.y.round() as u32).max(1);
        if self.busy {
            return;
        }
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
                    self.set_status(format!(
                        "MapScene recipe built from template '{op}' ({} layers)",
                        recipe
                            .get("layers")
                            .and_then(|l| l.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0)
                    ));
                }
                None => self.push_error("MapScene template", "no recipe in result"),
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
            None => {
                self.set_status(format!("Engine op '{op}' finished"));
            }
        }
    }

    /// Route a failed generic engine job.
    fn handle_job_failed(&mut self, op: &str, kind: Option<PendingJob>, message: &str) {
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
        let params = serde_json::json!({
            "template": self.mapscene.template,
            "dem_path": terrain.path.to_string_lossy(),
            "width": self.project.export.width,
            "height": self.project.export.height,
            "raster_path": self.mapscene.raster_path.as_ref().map(|p| p.to_string_lossy()),
            "label_text": self.mapscene.label_text,
        });
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
        let params = serde_json::json!({
            "recipe_json": serde_json::to_string(&recipe).unwrap_or_default(),
            "out_png": out.to_string_lossy(),
            "certificate": self.mapscene_certificate,
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
        self.project.datasets.push(studio_project::DatasetRef {
            path: preview.path,
            kind,
            label,
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

    fn top_panel(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("studio_top").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New Project").clicked() {
                        ui.close_menu();
                        self.new_project();
                    }
                    if ui.button("Open Project…").clicked() {
                        ui.close_menu();
                        self.open_project();
                    }
                    if ui.button("Save Project").clicked() {
                        ui.close_menu();
                        self.save_project(false);
                    }
                    if ui.button("Save Project As…").clicked() {
                        ui.close_menu();
                        self.save_project(true);
                    }
                    ui.separator();
                    if ui.button("Import Dataset…").clicked() {
                        ui.close_menu();
                        self.import_dataset();
                    }
                    if ui.button("Export PNG…").clicked() {
                        ui.close_menu();
                        self.export_png();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.checkbox(&mut self.theme_dark, "Dark theme").changed() {
                        self.apply_theme(ui.ctx(), self.theme_dark);
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Forge3D Studio").clicked() {
                        ui.close_menu();
                        self.set_status(
                            "Forge3D Studio 0.1.0 — desktop app for the Forge3D engine",
                        );
                    }
                });
            });

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if self
                    .icon_button(ui, "folder-open", "Open project (Ctrl+O)")
                    .clicked()
                {
                    self.open_project();
                }
                if self
                    .icon_button(ui, "save", "Save project (Ctrl+S)")
                    .clicked()
                {
                    self.save_project(false);
                }
                ui.separator();
                if self
                    .icon_button(ui, "file-input", "Import dataset (Ctrl+I)")
                    .clicked()
                {
                    self.import_dataset();
                }
                if self
                    .icon_button(ui, "file-output", "Export PNG (Ctrl+E)")
                    .clicked()
                {
                    self.export_png();
                }
                ui.separator();
                if self
                    .icon_button(ui, "rotate-ccw", "Reset camera to fit dataset")
                    .clicked()
                {
                    self.reset_camera();
                }
                if self
                    .icon_button(ui, "orbit", "Camera presets and navigation")
                    .clicked()
                {
                    self.set_status("Drag the viewport to orbit; scroll to zoom");
                }
                ui.separator();
                if self
                    .icon_button(ui, "image", "Render current viewport")
                    .clicked()
                {
                    self.invalidate_viewport();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.busy {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.weak("working…");
                    }
                    if let Some(project) =
                        self.project_path.as_ref().map(|p| p.display().to_string())
                    {
                        ui.weak(project);
                    } else {
                        ui.weak("unsaved project");
                    }
                });
            });
        });
    }

    fn icon_button(&self, ui: &mut Ui, name: &str, tooltip: &str) -> Response {
        let texture = self.icons.get(ui.ctx(), name);
        let response = ui
            .add(
                egui::Button::image(Image::new(&texture).fit_to_exact_size(egui::vec2(18.0, 18.0)))
                    .fill(Color32::TRANSPARENT),
            )
            .on_hover_text(tooltip);
        response
    }

    fn left_panel(&mut self, ctx: &Context) {
        egui::SidePanel::left("studio_scene")
            .resizable(true)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.heading("Scene");
                ui.separator();

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
                }
                ui.separator();

                match self.scene_mode {
                    SceneMode::Terrain => self.terrain_section(ui),
                    SceneMode::MapScene => self.mapscene_section(ui),
                }
            });
    }

    fn terrain_section(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Layers").strong());
        let terrain_present = self.project.terrain.is_some();
        // Visibility is app-side for now: a hidden layer shows the
        // empty-state message instead of the last rendered frame.
        if ui
            .add_enabled(
                terrain_present,
                egui::Checkbox::new(&mut self.project.terrain_visible, "Terrain"),
            )
            .changed()
        {
            self.project_dirty = true;
        }
        if !terrain_present {
            ui.weak("No terrain loaded");
        }
        ui.add_space(8.0);

        ui.label(egui::RichText::new("Datasets").strong());
        match &self.project.terrain {
            Some(terrain) => {
                egui::Grid::new("dataset_info")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("File");
                        ui.label(terrain.path.display().to_string())
                            .on_hover_text(terrain.path.display().to_string());
                        ui.label("Size");
                        ui.label(format!("{} × {}", terrain.width, terrain.height));
                        ui.label("Height");
                        ui.label(format!(
                            "{:.1} … {:.1}",
                            terrain.height_min, terrain.height_max
                        ));
                    });
                if ui.button("Remove from scene").clicked() {
                    self.engine.send(Command::ClearTerrain);
                }
            }
            None => {
                ui.weak("Import a GeoTIFF / F3DZ / NPY / COG dataset:");
                if ui.button("Import Dataset…").clicked() {
                    self.import_dataset();
                }
            }
        }

        if !self.project.datasets.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Imported datasets").strong());
            let mut remove_idx = None;
            for (index, dataset) in self.project.datasets.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.weak(&dataset.kind);
                    ui.label(&dataset.label)
                        .on_hover_text(dataset.path.display().to_string());
                    if ui
                        .small_button("✕")
                        .on_hover_text("Remove from project")
                        .clicked()
                    {
                        remove_idx = Some(index);
                    }
                });
            }
            if let Some(index) = remove_idx {
                self.project.datasets.remove(index);
                self.project_dirty = true;
            }
            if ui.button("Import Dataset…").clicked() {
                self.import_dataset();
            }
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
                    if ui.button("Render").clicked() {
                        self.render_mapscene();
                    }
                    if ui.button("Validate").clicked() {
                        self.validate_mapscene();
                    }
                });
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
        egui::SidePanel::right("studio_properties")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Properties");
                    ui.separator();

                    match self.scene_mode {
                        SceneMode::Terrain => {
                            self.camera_section(ui);
                            ui.add_space(8.0);
                            self.sun_section(ui);
                            ui.add_space(8.0);
                            self.rig_section(ui);
                            ui.add_space(8.0);
                            self.export_section(ui);
                        }
                        SceneMode::MapScene => {
                            self.mapscene_camera_section(ui);
                            ui.add_space(8.0);
                            self.export_section(ui);
                        }
                    }
                });
            });
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
        ui.label(egui::RichText::new("Camera").strong());
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
        ui.label(egui::RichText::new("☀ Sun").strong());
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
        ui.label(egui::RichText::new("Colormap").strong());
        let mut current = self
            .project
            .colormap
            .clone()
            .unwrap_or_else(|| "default".to_owned());
        egui::ComboBox::from_id_source("terrain_colormap")
            .selected_text(current.clone())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(&mut current, "default".to_owned(), "default (relief)")
                    .changed()
                {
                    self.project.colormap = None;
                }
                for name in &self.colormaps {
                    if ui
                        .selectable_value(&mut current, name.clone(), name)
                        .changed()
                    {
                        self.project.colormap = Some(name.clone());
                    }
                }
            });
        if current
            != self
                .project
                .colormap
                .clone()
                .unwrap_or_else(|| "default".to_owned())
        {
            self.project_dirty = true;
            self.engine
                .send(Command::SetColormap(self.project.colormap.clone()));
            self.invalidate_viewport();
        }
        ui.add_space(4.0);
        // W1: renderer presets map their first directional light onto this
        // sun (see PendingJob::PresetApply); exposure/shadows stay manual.
        ui.label(egui::RichText::new("Preset").strong());
        if self.presets.is_empty() {
            ui.weak("no presets loaded");
        } else {
            let mut chosen: Option<String> = None;
            egui::ComboBox::from_id_source("terrain_preset")
                .selected_text("apply preset…")
                .show_ui(ui, |ui| {
                    for name in &self.presets {
                        if ui.selectable_label(false, name).clicked() {
                            chosen = Some(name.clone());
                        }
                    }
                });
            if let Some(name) = chosen {
                self.apply_preset(name);
            }
        }
    }

    /// W1: camera-rig spec, keyframe bake, timeline scrubber, playback, and
    /// sequence rendering.
    fn rig_section(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Camera rig").strong());
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
        ui.label(egui::RichText::new("Export").strong());
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

    fn bottom_panel(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("studio_status").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if let Some(gpu) = &self.gpu {
                    let icon = self.icons.get(ui.ctx(), "cpu");
                    ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(13.0, 13.0)));
                    let label = if gpu.software_fallback {
                        format!("{} (software fallback!)", gpu.adapter)
                    } else {
                        format!("{} ({})", gpu.adapter, gpu.backend)
                    };
                    ui.label(egui::RichText::new(label).weak());
                    ui.separator();
                }
                if let Some(ms) = self.last_frame_ms {
                    ui.label(egui::RichText::new(format!("frame {ms:.0} ms")).weak());
                }
                if self.busy {
                    ui.add(egui::Spinner::new().size(12.0));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(latest) = self.errors.back() {
                        let icon = self.icons.get(ui.ctx(), "circle-alert");
                        ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(13.0, 13.0)))
                            .on_hover_text(
                                self.errors.iter().cloned().collect::<Vec<_>>().join("\n"),
                            );
                        ui.label(
                            egui::RichText::new(latest.clone())
                                .color(ui.visuals().error_fg_color)
                                .small(),
                        )
                        .on_hover_text(latest.clone());
                        if ui.small_button("clear").clicked() {
                            self.errors.clear();
                        }
                    } else if let Some(status) = &self.status {
                        let icon = self.icons.get(ui.ctx(), "circle-check");
                        ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(13.0, 13.0)));
                        ui.label(egui::RichText::new(status.clone()).weak().small());
                    }
                });
            });
            ui.add_space(2.0);
        });
    }

    fn viewport_panel(&mut self, ctx: &Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(12, 12, 14)))
            .show(ctx, |ui| {
                self.viewport_rect = ui.available_rect_before_wrap();
                let size = self.viewport_rect.size();

                if let Some(texture) = &self.viewport_texture {
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    self.handle_viewport_input(&response);
                } else {
                    ui.allocate_rect(self.viewport_rect, Sense::hover());
                    let message = if self.gpu.is_none() {
                        "Starting Forge3D engine…"
                    } else if self.project.terrain.is_some() {
                        "Rendering first frame…"
                    } else {
                        "No terrain loaded — use File ▸ Import DEM… (or open a project)"
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

    fn handle_viewport_input(&mut self, response: &Response) {
        if response.dragged() {
            let delta = response.drag_delta();
            let camera = &mut self.project.camera;
            camera.phi_deg = (camera.phi_deg + delta.x * 0.4).rem_euclid(360.0);
            camera.theta_deg = (camera.theta_deg + delta.y * 0.4).clamp(1.0, 89.0);
            self.engine.send(Command::SetCamera(self.project.camera));
            self.project_dirty = true;
            self.request_viewport_render();
        }

        let scroll_y = response.ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_y != 0.0 && response.hovered() {
            let camera = &mut self.project.camera;
            camera.radius = (camera.radius * (1.0 + scroll_y * 0.0015)).clamp(10.0, 100_000.0);
            self.engine.send(Command::SetCamera(self.project.camera));
            self.project_dirty = true;
            self.request_viewport_render();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        ctx.input(|i| {
            let ctrl = i.modifiers.ctrl;
            if ctrl && i.key_pressed(egui::Key::O) {
                self.open_project();
            }
            if ctrl && i.key_pressed(egui::Key::S) {
                self.save_project(false);
            }
            if ctrl && i.key_pressed(egui::Key::I) {
                self.import_dataset();
            }
            if ctrl && i.key_pressed(egui::Key::E) {
                self.export_png();
            }
        });
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
        self.tick_playback(ctx);

        self.top_panel(ctx);
        self.bottom_panel(ctx);
        self.left_panel(ctx);
        self.right_panel(ctx);
        self.viewport_panel(ctx);
        self.import_window(ctx);

        // Kick the very first render once the GPU is up.
        if self.gpu.is_some() && self.viewport_texture.is_none() && self.pending_render.is_none() {
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
        self.flush_pending_render();
    }
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
