//! Application state.
//!
//! `Studio` is the single owner of the project document, the selection, both
//! engine workers and the interactive frame. Panels read it; actions mutate it;
//! it never blocks on the engine (every long job is a queued command whose
//! result arrives as an event).
//!
//! Ported from the egui shell with the UI calls removed: the command/event
//! protocol, the project format and the viewport handoff are the same.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::{DockArea, DockSkin};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::*;
use studio_engine::{Command, EngineHandle, Event, ViewportCommand, ViewportEvent, ViewportWorker};
use studio_project::{CameraParams, DocHistory, LayerId, Project, SunParams};

use crate::actions::*;
use crate::ui;

/// One line of the engine log.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub error: bool,
    pub text: String,
}

/// Interactive viewport drag, held on the state so a drag survives frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewDrag {
    Orbit { last_x: f32, last_y: f32 },
    Pan { last_x: f32, last_y: f32 },
}

pub struct Studio {
    pub project: Project,
    pub project_path: Option<PathBuf>,
    pub project_dirty: bool,
    pub selection: Vec<LayerId>,
    pub history: DocHistory,

    pub menu_bar: Entity<AppMenuBar>,
    pub dock_area: Entity<DockArea>,
    #[allow(dead_code)]
    pub skin: Rc<DockSkin>,

    pub engine: EngineHandle,
    engine_rx: Receiver<Event>,
    pub viewport: ViewportWorker,
    viewport_rx: Receiver<ViewportEvent>,

    /// Latest interactive frame, ready to present.
    pub frame: Option<Arc<gpui_kit::RenderImage>>,
    pub frame_size: (u32, u32),
    pub frame_ms: f32,
    /// The frame no longer matches the document (camera, sun, colormap, DEM).
    pub viewport_dirty: bool,
    /// Size the viewport panel reported last paint, shared with the panel so
    /// the paint path never mutates state.
    pub viewport_size: Rc<Cell<(f32, f32)>>,
    /// Size of the last render requested, so a resize re-renders exactly once.
    last_requested_size: (u32, u32),

    pub adapter: String,
    pub backend: String,
    pub software: bool,
    pub viewport_ready: bool,

    pub log: Vec<LogEntry>,
    pub status: String,
    pub busy: bool,
    pub progress: Option<f32>,

    pub colormaps: Vec<String>,
    pub params_schema: Option<serde_json::Value>,
    pub params_error: Option<String>,

    pub focus_handle: FocusHandle,
    pub drag: Option<ViewDrag>,
    pub last_export: Option<PathBuf>,
}

impl Studio {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (engine, engine_rx) = EngineHandle::spawn();
        let (viewport, viewport_rx) = ViewportWorker::spawn();
        let focus_handle = cx.focus_handle();

        // The dock needs a weak handle on this entity, which exists while the
        // constructor runs, so it is built here rather than after `cx.new`.
        let (dock_area, skin, menu_bar) = ui::build_dock(cx.entity().downgrade(), window, cx);

        let mut studio = Self {
            project: Project::default(),
            project_path: None,
            project_dirty: false,
            selection: Vec::new(),
            history: DocHistory::new(200),
            menu_bar,
            dock_area,
            skin,
            engine,
            engine_rx,
            viewport,
            viewport_rx,
            frame: None,
            frame_size: (0, 0),
            frame_ms: 0.0,
            viewport_dirty: true,
            viewport_size: Rc::new(Cell::new((0.0, 0.0))),
            last_requested_size: (0, 0),
            adapter: "starting…".to_owned(),
            backend: String::new(),
            software: false,
            viewport_ready: false,
            log: Vec::new(),
            status: "Ready".to_owned(),
            busy: false,
            progress: None,
            colormaps: Vec::new(),
            params_schema: None,
            params_error: None,
            focus_handle,
            drag: None,
            last_export: None,
        };

        Self::install_actions(&cx.entity().downgrade(), cx);
        studio.start_poll(window, cx);

        // Ask the engine what it is and what palettes it has. Both are ops the
        // bridge already exposes; nothing is invented on the UI side.
        studio.engine.call("probe", serde_json::json!({}));
        studio.engine.call("colormaps.list", serde_json::json!({}));
        studio
            .engine
            .call("terrain.params_schema", serde_json::json!({}));

        window.focus(&studio.focus_handle, &mut *cx);
        studio
    }

    // ---------------------------------------------------------------- actions

    /// One handler per action, registered at the application level so menu
    /// clicks, keyboard shortcuts and toolbar buttons all reach the same code.
    ///
    /// GPUI dispatches app-level action listeners for every window, which is
    /// what the kit's menu bar (and the platform menu) expect.
    fn install_actions(studio: &WeakEntity<Studio>, cx: &mut App) {
        macro_rules! bind {
            ($($action:ident, $label:literal, $group:ident, $block:literal, $shortcut:expr;)*) => {
                $(
                    cx.on_action({
                        let studio = studio.clone();
                        move |_action: &crate::actions::$action, cx: &mut App| {
                            let _ = studio.update(cx, |this, cx| {
                                <crate::actions::$action as StudioAction>::run(this, cx)
                            });
                        }
                    });
                )*
            };
        }
        crate::studio_action_table!(bind);
    }

    /// Open a path from the command line: a project, or a DEM.
    pub fn open_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if path.extension().and_then(|ext| ext.to_str()) == Some("forge3d") {
            self.load_project(path, cx);
        } else {
            self.import_dem(path, cx);
        }
    }

    pub fn new_project(&mut self, cx: &mut Context<Self>) {
        self.project = Project::default();
        self.selection.clear();
        self.history = DocHistory::new(200);
        self.project_path = None;
        self.project_dirty = false;
        self.frame = None;
        self.viewport.send(ViewportCommand::ClearDem);
        self.engine.send(Command::ClearTerrain);
        self.set_status("New project");
        cx.notify();
    }

    pub fn open_project(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open a .forge3d project".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = first_path(rx.await.ok().and_then(|result| result.ok()));
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| this.load_project(&path, cx));
            }
        })
        .detach();
    }

    pub fn load_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        match Project::load(path) {
            Ok(project) => {
                self.project = project;
                self.project_path = Some(path.to_path_buf());
                self.project_dirty = false;
                self.selection.clear();
                self.history = DocHistory::new(200);
                let warnings = self.project.validation_warnings();
                for warning in warnings {
                    self.push_log(true, warning);
                }
                self.sync_engine_from_project(cx);
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(error) => {
                self.push_log(true, format!("Open failed: {error}"));
                self.push_error("Open project", error.to_string());
            }
        }
        cx.notify();
    }

    pub fn save_project(&mut self, cx: &mut Context<Self>, save_as: bool) {
        if let Some(path) = self.project_path.clone().filter(|_| !save_as) {
            self.write_project(&path, cx);
            return;
        }
        let directory = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let rx = cx.prompt_for_new_path(&directory, Some("untitled.forge3d"));
        cx.spawn(async move |this, cx| {
            let picked = one_path(rx.await.ok().and_then(|result| result.ok()));
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| this.write_project(&path, cx));
            }
        })
        .detach();
    }

    fn write_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.project.sync_terrain_layer();
        match self.project.save(path) {
            Ok(()) => {
                self.project_path = Some(path.to_path_buf());
                self.project_dirty = false;
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(error) => {
                self.push_log(true, format!("Save failed: {error}"));
                self.push_error("Save project", error.to_string());
            }
        }
        cx.notify();
    }

    /// The import dialog: a DEM path, loaded through the bridge.
    pub fn pick_dem(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import a DEM (GeoTIFF, COG, NPY, F3DZ)".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = first_path(rx.await.ok().and_then(|result| result.ok()));
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| this.import_dem(&path, cx));
            }
        })
        .detach();
    }

    /// Load a DEM into both paths: the certified bridge (exports) and the
    /// viewport worker (the interactive frame).
    pub fn import_dem(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.busy = true;
        self.set_status(format!("Loading {}", path.display()));
        self.engine.send(Command::LoadTerrain {
            path: path.to_path_buf(),
        });
        cx.notify();
    }

    pub fn export_png(&mut self, cx: &mut Context<Self>) {
        if self.project.terrain.is_none() {
            self.push_error("Export PNG", "no terrain loaded");
            cx.notify();
            return;
        }
        let directory = std::env::current_dir().unwrap_or_default();
        let name = format!("{}.png", slug(&self.project.name));
        let rx = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            let picked = one_path(rx.await.ok().and_then(|result| result.ok()));
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| {
                    let export = this.project.export;
                    this.engine.send(Command::RenderToFile {
                        width: export.width,
                        height: export.height,
                        path: path.clone(),
                    });
                    this.busy = true;
                    this.set_status(format!(
                        "Exporting {}×{} to {}",
                        export.width,
                        export.height,
                        path.display()
                    ));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // ----------------------------------------------------------- document ops

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if self.history.undo(&mut self.project.document) {
            self.prune_selection();
            self.project_dirty = true;
            self.invalidate_viewport(cx);
            self.set_status("Undo");
        }
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if self.history.redo(&mut self.project.document) {
            self.prune_selection();
            self.project_dirty = true;
            self.invalidate_viewport(cx);
            self.set_status("Redo");
        }
        cx.notify();
    }

    pub fn duplicate_layer(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.primary_layer() else {
            return;
        };
        let before = self.project.document.clone();
        if let Some(copy) = self.project.document.duplicate(id) {
            self.history.record(before);
            self.selection = vec![copy];
            self.project_dirty = true;
            self.set_status("Duplicated layer");
        }
        cx.notify();
    }

    pub fn delete_layer(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let before = self.project.document.clone();
        for id in self.selection.clone() {
            self.project.document.remove(id);
        }
        self.history.record(before);
        self.selection.clear();
        self.project_dirty = true;
        self.prune_terrain_slot();
        self.invalidate_viewport(cx);
        self.set_status("Deleted layer");
        cx.notify();
    }

    /// Layer ▸ Toggle visibility: the primary selection.
    pub fn toggle_selection_visibility(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.primary_layer() {
            self.toggle_layer_visible(id, cx);
        }
    }

    /// Render ▸ Render frame: a still at the panel's current size.
    pub fn request_still(&mut self, cx: &mut Context<Self>) {
        self.viewport_dirty = true;
        self.request_frame(cx, true);
    }

    pub fn toggle_layer_visible(&mut self, id: LayerId, cx: &mut Context<Self>) {
        let before = self.project.document.clone();
        if let Some(layer) = self.project.document.layer_mut(id) {
            layer.visible = !layer.visible;
        }
        self.history.record(before);
        self.project_dirty = true;
        self.invalidate_viewport(cx);
        cx.notify();
    }

    // ----------------------------------------------------------------- camera

    /// Apply a camera edit: the document, the live scene, and one frame.
    pub fn set_camera(&mut self, camera: CameraParams, cx: &mut Context<Self>) {
        self.project.camera = camera;
        self.project_dirty = true;
        self.viewport.send(ViewportCommand::SetView {
            camera,
            sun: self.project.sun,
        });
        self.viewport_dirty = true;
        cx.notify();
    }

    /// Send the camera and sun to the bridge as well, once an interaction
    /// settles, so exports render what the viewport showed.
    fn commit_view(&mut self) {
        self.engine.send(Command::SetCamera(self.project.camera));
        self.engine.send(Command::SetSun(self.project.sun));
    }

    pub fn step_camera(
        &mut self,
        dphi: f32,
        dtheta: f32,
        radius_factor: f32,
        fov_factor: f32,
        cx: &mut Context<Self>,
    ) {
        let mut camera = self.project.camera;
        camera.phi_deg = (camera.phi_deg + dphi).rem_euclid(360.0);
        camera.theta_deg = (camera.theta_deg + dtheta).clamp(1.0, 89.0);
        camera.radius = (camera.radius * radius_factor).clamp(10.0, 200_000.0);
        camera.fov_deg = (camera.fov_deg * fov_factor).clamp(10.0, 120.0);
        self.set_camera(camera, cx);
        self.commit_view();
    }

    pub fn preset_camera(&mut self, phi: f32, theta: f32, cx: &mut Context<Self>) {
        let mut camera = self.project.camera;
        camera.phi_deg = phi;
        camera.theta_deg = theta;
        self.set_camera(camera, cx);
        self.commit_view();
    }

    pub fn fit_camera(&mut self, cx: &mut Context<Self>) {
        if let Some(terrain) = &self.project.terrain {
            let mut camera = self.project.camera;
            camera.radius = fit_radius(terrain_span(terrain.height_min, terrain.height_max));
            camera.target_x = 0.0;
            camera.target_z = 0.0;
            self.set_camera(camera, cx);
            self.commit_view();
            self.set_status("Fit view");
        }
        cx.notify();
    }

    pub fn reset_camera(&mut self, cx: &mut Context<Self>) {
        let mut camera = CameraParams::default();
        if let Some(terrain) = &self.project.terrain {
            camera.radius = fit_radius(terrain_span(terrain.height_min, terrain.height_max));
        }
        self.set_camera(camera, cx);
        self.commit_view();
    }

    pub fn step_sun(
        &mut self,
        dazimuth: f32,
        delevation: f32,
        factor: f32,
        cx: &mut Context<Self>,
    ) {
        let mut sun = self.project.sun;
        sun.azimuth_deg = (sun.azimuth_deg + dazimuth).rem_euclid(360.0);
        sun.elevation_deg = (sun.elevation_deg + delevation).clamp(-5.0, 90.0);
        sun.intensity = (sun.intensity * factor).clamp(0.0, 10.0);
        self.set_sun(sun, cx);
        self.commit_view();
    }

    pub fn set_sun(&mut self, sun: SunParams, cx: &mut Context<Self>) {
        self.project.sun = sun;
        self.project_dirty = true;
        self.viewport.send(ViewportCommand::SetView {
            camera: self.project.camera,
            sun,
        });
        self.viewport_dirty = true;
        cx.notify();
    }

    // ------------------------------------------------------------------- drag

    /// Pointer moved while a drag is active. Orbit changes the camera; pan
    /// moves the ground target. No engine command: the bridge is not painting
    /// this frame, and a drag would flood its queue.
    pub fn drag_to(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        match self.drag {
            Some(ViewDrag::Orbit { last_x, last_y }) => {
                let mut camera = self.project.camera;
                camera.phi_deg = (camera.phi_deg - (x - last_x) * 0.4).rem_euclid(360.0);
                camera.theta_deg = (camera.theta_deg + (y - last_y) * 0.3).clamp(1.0, 89.0);
                self.drag = Some(ViewDrag::Orbit {
                    last_x: x,
                    last_y: y,
                });
                self.set_camera(camera, cx);
            }
            Some(ViewDrag::Pan { last_x, last_y }) => {
                let mut camera = self.project.camera;
                let scale = (camera.radius * 0.0018).max(0.001);
                camera.target_x -= (x - last_x) * scale;
                camera.target_z += (y - last_y) * scale;
                self.drag = Some(ViewDrag::Pan {
                    last_x: x,
                    last_y: y,
                });
                self.set_camera(camera, cx);
            }
            None => {}
        }
    }

    /// Wheel zoom: one step scales the orbit radius around the target.
    pub fn zoom_by(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut camera = self.project.camera;
        let factor = (-delta * 0.0015).exp().clamp(0.5, 2.0);
        camera.radius = (camera.radius * factor).clamp(10.0, 200_000.0);
        self.set_camera(camera, cx);
    }

    /// A drag or zoom finished: the next export must match what was shown.
    pub fn end_gesture(&mut self) {
        self.drag = None;
        self.commit_view();
    }

    // ------------------------------------------------------------------ frame

    pub fn invalidate_viewport(&mut self, cx: &mut Context<Self>) {
        self.viewport_dirty = true;
        self.viewport.send(ViewportCommand::SetView {
            camera: self.project.camera,
            sun: self.project.sun,
        });
        cx.notify();
    }

    /// Ask for one frame at the panel's current size.
    pub fn request_frame(&mut self, cx: &mut Context<Self>, force: bool) {
        let (width, height) = self.viewport_size.get();
        if width < 2.0 || height < 2.0 || self.project.terrain.is_none() {
            return;
        }
        let size = (width as u32, height as u32);
        self.last_requested_size = size;
        self.viewport_dirty = false;
        self.viewport.send(ViewportCommand::Render {
            width: size.0,
            height: size.1,
            force,
        });
        cx.notify();
    }

    pub fn set_colormap(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.project.colormap = name.clone();
        self.engine.send(Command::SetColormap(name.clone()));
        self.viewport.send(ViewportCommand::SetColormap(name));
        self.project_dirty = true;
        self.invalidate_viewport(cx);
        cx.notify();
    }

    // ------------------------------------------------------------------- polls

    /// A 20 Hz UI pump: drains both workers, coalesces viewport renders and
    /// repaints only when something changed.
    fn start_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let alive = this
                .update_in(cx, |this, window, cx| this.poll(window, cx))
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;

        while let Ok(event) = self.engine_rx.try_recv() {
            self.handle_engine_event(event, window, cx);
            changed = true;
        }
        while let Ok(event) = self.viewport_rx.try_recv() {
            self.handle_viewport_event(event, cx);
            changed = true;
        }

        // A dirty view or a new panel size asks for exactly one frame per tick,
        // which rate-limits a drag instead of queueing a render per event.
        if self.project.terrain.is_some() {
            let (width, height) = self.viewport_size.get();
            let size = (width as u32, height as u32);
            if width > 1.0
                && height > 1.0
                && (self.viewport_dirty || size != self.last_requested_size)
            {
                self.request_frame(cx, false);
                changed = true;
            }
        }

        if changed {
            cx.notify();
        }
    }

    fn handle_viewport_event(&mut self, event: ViewportEvent, cx: &mut Context<Self>) {
        match event {
            ViewportEvent::Ready {
                adapter,
                backend,
                software_fallback,
            } => {
                self.viewport_ready = true;
                self.adapter = adapter;
                self.backend = backend;
                self.software = software_fallback;
                self.push_log(
                    false,
                    format!(
                        "Viewport device: {} ({}){}",
                        self.adapter,
                        self.backend,
                        if software_fallback {
                            " — software fallback"
                        } else {
                            ""
                        }
                    ),
                );
                self.viewport_dirty = true;
            }
            ViewportEvent::Failed { context, message } => {
                self.push_log(true, format!("{context}: {message}"));
                self.set_status(format!("{context} failed"));
            }
            ViewportEvent::TerrainLoaded => {
                self.viewport_dirty = true;
            }
            ViewportEvent::TerrainCleared => {
                self.frame = None;
            }
            ViewportEvent::Frame(frame) => {
                self.frame_ms = frame.render_ms;
                self.frame_size = (frame.width, frame.height);
                if let Ok(path) = std::env::var("FORGE3D_STUDIO_DUMP_FRAME") {
                    // Dev/test hook: write the frame the UI is about to show, so
                    // the viewport can be compared against a certified export.
                    if let Some(buffer) =
                        image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.clone())
                    {
                        let _ = buffer.save(&path);
                        self.push_log(false, format!("Viewport frame written to {path}"));
                    }
                }
                self.frame = Some(Arc::new(frame_image(&frame)));
            }
        }
        cx.notify();
    }

    fn handle_engine_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Ready {
                adapter,
                backend,
                software_fallback,
            } => {
                self.adapter = adapter.clone();
                self.backend = backend.clone();
                self.software = software_fallback;
                self.push_log(
                    false,
                    format!(
                        "Engine: {adapter} · {backend}{}",
                        if software_fallback {
                            " · software fallback"
                        } else {
                            ""
                        }
                    ),
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
                self.busy = false;
                self.project.terrain = Some(studio_project::TerrainRef {
                    path: path.clone(),
                    width,
                    height,
                    height_min,
                    height_max,
                });
                self.project.sync_terrain_layer();
                if self.selection.is_empty() {
                    if let Some(id) = self.project.document.terrain_layer().map(|layer| layer.id) {
                        self.selection = vec![id];
                    }
                }
                let span = terrain_span(height_min, height_max);
                self.project.camera = CameraParams {
                    radius: fit_radius(span),
                    ..camera
                };
                self.project.sun = sun;
                self.project_dirty = true;

                // The live scene loads the same file on its own device.
                self.viewport.send(ViewportCommand::LoadDem {
                    path: path.clone(),
                    span,
                });
                self.viewport
                    .send(ViewportCommand::SetColormap(self.project.colormap.clone()));
                self.viewport.send(ViewportCommand::SetView {
                    camera: self.project.camera,
                    sun: self.project.sun,
                });

                self.set_status(format!(
                    "{} · {}×{} · {}–{}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                    width,
                    height,
                    height_min.round(),
                    height_max.round()
                ));
                self.viewport_dirty = true;
                let _ = window;
            }
            Event::TerrainCleared => {
                self.busy = false;
                self.project.terrain = None;
                self.frame = None;
                self.set_status("Terrain cleared");
            }
            Event::FrameReady { .. } => {
                self.busy = false;
            }
            Event::ExportFinished { path, .. } => {
                self.busy = false;
                self.last_export = Some(path.clone());
                self.set_status(format!("Exported {}", path.display()));
                self.push_log(false, format!("Wrote {}", path.display()));
            }
            Event::ExportFailed { path, message } => {
                self.busy = false;
                self.push_log(
                    true,
                    format!("Export to {} failed: {message}", path.display()),
                );
                self.push_error("Export PNG", message);
            }
            Event::Progress { fraction, label } => {
                self.progress = fraction;
                self.set_status(label);
            }
            Event::JobDone { op, result, .. } => self.handle_job_done(&op, result, cx),
            Event::JobFailed { op, message, .. } => {
                self.busy = false;
                self.push_log(true, format!("{op}: {message}"));
                if op.starts_with("terrain.params") {
                    self.params_error = Some(message.clone());
                }
                self.push_error(op, message);
            }
            Event::Error { context, message } => {
                self.busy = false;
                self.push_log(true, format!("{context}: {message}"));
                self.push_error(context, message);
            }
            Event::Busy(busy) => {
                self.busy = busy;
                if !busy {
                    self.progress = None;
                }
            }
        }
    }

    fn handle_job_done(&mut self, op: &str, result: serde_json::Value, cx: &mut Context<Self>) {
        match op {
            "probe" => {
                let adapter = result
                    .get("adapter")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                let backend = result
                    .get("backend")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                self.adapter = adapter;
                self.backend = backend;
            }
            "colormaps.list" => {
                self.colormaps = result
                    .get("colormaps")
                    .and_then(|value| value.as_array())
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|name| name.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "terrain.params_schema" => {
                self.params_schema = result.get("schema").cloned().or(Some(result));
            }
            "terrain.params_validate" => {
                self.params_error = result
                    .get("error")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
            }
            _ => {}
        }
        cx.notify();
    }

    // --------------------------------------------------------------- helpers

    fn sync_engine_from_project(&mut self, cx: &mut Context<Self>) {
        self.engine.send(Command::SetCamera(self.project.camera));
        self.engine.send(Command::SetSun(self.project.sun));
        self.engine
            .send(Command::SetColormap(self.project.colormap.clone()));
        if let Some(terrain) = &self.project.terrain {
            self.engine.send(Command::LoadTerrain {
                path: terrain.path.clone(),
            });
        }
        self.invalidate_viewport(cx);
    }

    fn prune_terrain_slot(&mut self) {
        if self.project.document.terrain_layer().is_none() {
            self.project.terrain = None;
            self.frame = None;
            self.viewport.send(ViewportCommand::ClearDem);
        }
    }

    fn prune_selection(&mut self) {
        let document = &self.project.document;
        self.selection.retain(|id| document.layer(*id).is_some());
    }

    pub fn primary_layer(&self) -> Option<LayerId> {
        self.selection.last().copied()
    }

    pub fn select(&mut self, id: LayerId, additive: bool, cx: &mut Context<Self>) {
        if additive {
            if let Some(index) = self.selection.iter().position(|other| *other == id) {
                self.selection.remove(index);
            } else {
                self.selection.push(id);
            }
        } else {
            self.selection = vec![id];
        }
        cx.notify();
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    pub fn push_log(&mut self, error: bool, text: String) {
        log::info!("{text}");
        self.log.push(LogEntry { error, text });
        if self.log.len() > 500 {
            self.log.remove(0);
        }
    }

    pub fn push_error(&mut self, context: impl AsRef<str>, message: impl AsRef<str>) {
        self.set_status(format!("{}: {}", context.as_ref(), message.as_ref()));
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        self.viewport.shutdown();
        self.engine.shutdown();
    }
}

impl Focusable for Studio {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// World span the engine gives a terrain: six times its elevation range.
///
/// This is the engine's own convention (`_op_terrain_render` in the bridge
/// computes the same), so the viewport, exports and rigs share one world scale.
pub fn terrain_span(height_min: f32, height_max: f32) -> f32 {
    (height_max - height_min).abs().max(1.0) * 6.0
}

/// Camera distance that frames a terrain of this world span.
pub fn fit_radius(span: f32) -> f32 {
    (span * 1.65).clamp(60.0, 400_000.0)
}

/// The first path of a file-pick result.
fn first_path(result: Option<Option<Vec<PathBuf>>>) -> Option<PathBuf> {
    result.flatten().and_then(|paths| paths.into_iter().next())
}

/// The path of a save-pick result.
fn one_path(result: Option<Option<PathBuf>>) -> Option<PathBuf> {
    result.flatten()
}

/// Convert one viewport frame into a GPUI image.
fn frame_image(frame: &studio_engine::ViewportFrame) -> gpui_kit::RenderImage {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.clone())
        .unwrap_or_else(|| image::RgbaImage::new(frame.width.max(1), frame.height.max(1)));
    gpui_kit::RenderImage::new(vec![image::Frame::new(buffer)])
}

/// A file-name-safe slug of the project name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_owned();
    if trimmed.is_empty() {
        "scene".to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The gpui glob also re-exports GPUI's `test` attribute; keep the built-in one.
    use core::prelude::v1::test;

    #[test]
    fn slug_is_a_file_name() {
        assert_eq!(slug("Mount Fuji"), "mount_fuji");
        assert_eq!(slug("  "), "scene");
        assert_eq!(slug("a/b"), "a_b");
    }

    #[test]
    fn world_span_follows_the_engine_convention() {
        // Six times the relief, and the camera sits 1.65 spans out.
        let span = terrain_span(469.0, 4391.0);
        assert!((span - 23_532.0).abs() < 1.0);
        assert!((fit_radius(span) - span * 1.65).abs() < 1.0);
    }
}
