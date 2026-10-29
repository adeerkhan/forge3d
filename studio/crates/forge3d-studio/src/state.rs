//! Application state.
//!
//! `Studio` is the single owner of the project document, the selection, both
//! engine workers and the interactive frame. Panels read it; actions mutate it;
//! it never blocks on the engine (every long job is a queued command whose
//! result arrives as an event).
//!
//! The command/event protocol, the project format and the viewport handoff are
//! the ones the engine-facing milestones proved; only the UI runtime changed.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::{DockArea, DockAreaState, DockEvent, DockSkin};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::*;
use studio_engine::{Command, EngineHandle, Event, ViewportCommand, ViewportEvent, ViewportWorker};
use studio_project::{CameraParams, DocHistory, LayerId, Project, SunParams};

use crate::actions::*;
use crate::ui;
use crate::ui::layers::{DropZone, LayerPanel};
use crate::ui::params::{ParamForm, ParamKind};

/// One line of the engine log.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub error: bool,
    pub text: String,
}

/// Which scene the app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneMode {
    Terrain,
    MapScene,
}

/// One examples-gallery entry, as `studio/examples/index.json` declares it.
#[derive(Clone, Debug)]
pub struct ExampleEntry {
    pub id: String,
    pub title: String,
    pub category: String,
    /// `A` runs today; `B`/`C` name the milestone that unlocks them.
    pub tier: char,
    pub description: String,
    /// The `examples/*.py` script this entry reproduces (shown as provenance).
    #[allow(dead_code)]
    pub example: String,
    pub unlock: String,
    pub scene: Option<serde_json::Value>,
    pub thumbnail: Option<String>,
    /// Bridge ops the scene needs, as declared by the index.
    pub ops: Vec<String>,
}

/// Blender-style workspaces: one row of tabs, each a working arrangement
/// (which properties page is open, which left editor is showing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Layout,
    Terrain,
    Cartography,
    Render,
    Look,
    Data,
}

impl Workspace {
    pub const ALL: &'static [Workspace] = &[
        Workspace::Layout,
        Workspace::Terrain,
        Workspace::Cartography,
        Workspace::Render,
        Workspace::Look,
        Workspace::Data,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Layout => "Layout",
            Workspace::Terrain => "Terrain",
            Workspace::Cartography => "Cartography",
            Workspace::Render => "Render",
            Workspace::Look => "Look",
            Workspace::Data => "Data",
        }
    }
}

/// The properties editor's pages (the right-hand icon strip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertiesTab {
    Layer,
    Terrain,
    Render,
    World,
    Scene,
    Stats,
}

impl PropertiesTab {
    pub const ALL: &'static [PropertiesTab] = &[
        PropertiesTab::Layer,
        PropertiesTab::Terrain,
        PropertiesTab::Render,
        PropertiesTab::World,
        PropertiesTab::Scene,
        PropertiesTab::Stats,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PropertiesTab::Layer => "layer",
            PropertiesTab::Terrain => "terrain",
            PropertiesTab::Render => "render",
            PropertiesTab::World => "world",
            PropertiesTab::Scene => "scene",
            PropertiesTab::Stats => "stats",
        }
    }
}

/// The bottom strip's editors (Blender's timeline slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Timeline,
    Console,
    Certificates,
}

impl BottomTab {
    pub const ALL: &'static [BottomTab] = &[
        BottomTab::Timeline,
        BottomTab::Console,
        BottomTab::Certificates,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BottomTab::Timeline => "timeline",
            BottomTab::Console => "console",
            BottomTab::Certificates => "certificates",
        }
    }
}

/// What the left-hand editor shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftTab {
    Layers,
    Examples,
}

/// Where a gallery card is in its open flow.
#[derive(Clone, Debug, PartialEq)]
pub enum CardState {
    Idle,
    Loading,
    Loaded,
    Failed(String),
}

/// How one asset a scene needs is found on disk.
#[derive(Clone, Debug, PartialEq)]
pub enum AssetKind {
    Present(u64),
    /// A git-LFS pointer: the file is tracked but not materialized.
    LfsPointer,
    Missing,
    Remote(String),
}

/// One downloadable asset of a gallery scene.
#[derive(Clone, Debug)]
pub struct AssetStatus {
    pub label: String,
    /// As declared in the index (repo-relative).
    pub declared: String,
    /// Absolute path the app would open (absent for remote assets).
    pub kind: AssetKind,
}

impl AssetStatus {
    pub fn ready(&self) -> bool {
        matches!(self.kind, AssetKind::Present(_))
    }
}

/// MapScene mode state: the recipe the last build produced and its report.
#[derive(Clone, Debug, Default)]
pub struct MapSceneState {
    pub template: String,
    pub templates: Vec<String>,
    pub recipe_json: Option<String>,
    pub last_status: Option<String>,
    pub certificate_sha: Option<String>,
}

/// One certificate this session produced.
#[derive(Clone, Debug)]
pub struct SessionCertificate {
    pub path: PathBuf,
    pub source: String,
    pub payload_sha256: Option<String>,
    pub verified: Option<bool>,
}

/// The camera rig (G6). The keyframes are the engine's, rescaled by the bridge
/// into renderer world units, so replay is exactly what an export renders.
#[derive(Clone, Debug)]
pub struct CameraRig {
    pub kind: String,
    pub duration: f32,
    pub radius_rel: f32,
    pub samples_per_second: u32,
    pub keyframes: Vec<CameraParams>,
    pub frame: usize,
    pub playing: bool,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            kind: "orbit".to_owned(),
            duration: 8.0,
            radius_rel: 0.4,
            samples_per_second: 12,
            keyframes: Vec::new(),
            frame: 0,
            playing: false,
        }
    }
}

/// A running sequence export.
#[derive(Clone, Debug)]
pub struct SequenceRun {
    pub out_dir: PathBuf,
    pub total: u32,
    pub finished: u32,
}

/// The two-run SHA-256 comparison `determinism.check` reports.
#[derive(Clone, Debug)]
pub struct DeterminismReport {
    pub sha_a: String,
    pub sha_b: String,
    pub matched: bool,
}

/// What a queued bridge job was, so its result can be routed.
#[derive(Clone, Copy, Debug)]
pub enum PendingJob {
    RigBake,
    DeterminismCheck,
    CertificateVerify(usize),
    DiagnosticsSnapshot,
    Capabilities,
    SequenceFrame,
    MapSceneTemplate,
    MapSceneValidate,
    MapSceneRender,
    AtmosphereBake,
    AstroObserve,
    SmokeLoad,
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
    /// Interactive rendering on. When off, the viewport draws only on an
    /// explicit request or when a gesture ends — the escape hatch for a slow
    /// GPU or a heavy dataset.
    pub live_viewport: bool,
    /// A render is on the worker right now. Requests are dropped (not queued)
    /// while this is set, so a fast orbit can never build a backlog.
    render_in_flight: bool,
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

    pub certificates: Vec<SessionCertificate>,
    pub sign_renders: bool,
    pub offline_caps: Option<serde_json::Value>,
    pub diagnostics: Option<serde_json::Value>,
    pub rig: CameraRig,
    pub sequence: Option<SequenceRun>,
    pub determinism: Option<DeterminismReport>,
    pub pending_jobs: std::collections::HashMap<u64, PendingJob>,

    pub colormaps: Vec<String>,
    pub params_schema: Option<serde_json::Value>,
    pub params_error: Option<String>,
    pub params_form: ParamForm,

    /// Layers panel UI state (anchor, rename, drop preview, collapsed groups).
    pub layers: LayerPanel,

    pub workspace: Workspace,
    pub properties_tab: PropertiesTab,
    pub left_tab: LeftTab,
    pub bottom_tab: BottomTab,
    pub show_gizmo: bool,
    pub show_hud: bool,

    pub scene_mode: SceneMode,
    pub mapscene: MapSceneState,
    pub examples: Vec<ExampleEntry>,
    pub gallery_filter: String,
    pub gallery_runnable_only: bool,
    pub atmosphere_meta: Option<serde_json::Value>,
    pub smoke_report: Option<serde_json::Value>,
    pending_example: Option<ExampleEntry>,
    /// Per-card open state, keyed by example id.
    pub gallery_cards: std::collections::HashMap<String, CardState>,
    /// The example shown in the download carousel.
    pub gallery_selected: Option<String>,
    /// Decoded thumbnails, loaded from disk once.
    pub thumbnails: std::collections::HashMap<String, Option<Arc<gpui_kit::RenderImage>>>,
    /// A running `git lfs pull`, so the UI can say it is working.
    pub lfs_fetch: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// Ops the bridge actually implements (`ops.list`), so a card can check
    /// its declared ops instead of failing at click time.
    pub available_ops: std::collections::HashSet<String>,

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
        cx.set_global(crate::ui::StudioHandle(cx.entity().downgrade()));
        crate::ui::register_panels(cx);
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
            live_viewport: true,
            render_in_flight: false,
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
            certificates: Vec::new(),
            sign_renders: false,
            offline_caps: None,
            diagnostics: None,
            rig: CameraRig::default(),
            sequence: None,
            determinism: None,
            pending_jobs: std::collections::HashMap::new(),
            colormaps: Vec::new(),
            params_schema: None,
            params_error: None,
            params_form: ParamForm::default(),
            layers: LayerPanel::default(),
            workspace: Workspace::Layout,
            properties_tab: PropertiesTab::Layer,
            left_tab: LeftTab::Layers,
            bottom_tab: BottomTab::Timeline,
            show_gizmo: true,
            show_hud: true,
            scene_mode: SceneMode::Terrain,
            mapscene: MapSceneState {
                template: "terrain_raster".to_owned(),
                ..MapSceneState::default()
            },
            examples: load_examples(),
            gallery_filter: "All".to_owned(),
            gallery_runnable_only: false,
            atmosphere_meta: None,
            smoke_report: None,
            pending_example: None,
            gallery_cards: std::collections::HashMap::new(),
            gallery_selected: None,
            thumbnails: std::collections::HashMap::new(),
            lfs_fetch: None,
            available_ops: std::collections::HashSet::new(),
            focus_handle,
            drag: None,
            last_export: None,
        };

        Self::install_actions(&cx.entity().downgrade(), cx);
        studio.refresh_menus(cx);
        studio.restore_layout(window, cx);
        studio.watch_layout(window, cx);
        // Write the layout we ended up with, so the first run leaves a file and
        // a corrupt or stale one is replaced by something valid.
        studio.save_layout(&studio.dock_area.clone(), cx);
        studio.start_poll(window, cx);

        // Ask the engine what it is and what palettes it has. Both are ops the
        // bridge already exposes; nothing is invented on the UI side.
        studio.engine.call("probe", serde_json::json!({}));
        studio.engine.call("colormaps.list", serde_json::json!({}));
        studio
            .engine
            .call("terrain.params_schema", serde_json::json!({}));
        studio.refresh_capabilities(cx);
        if let Ok(id) = std::env::var("FORGE3D_STUDIO_OPEN_EXAMPLE") {
            if let Some(entry) = studio.examples.iter().find(|entry| entry.id == id).cloned() {
                log::info!("opening example from environment: {}", entry.id);
                studio.open_example(entry, cx);
            } else {
                log::warn!("FORGE3D_STUDIO_OPEN_EXAMPLE names no known example: {id}");
            }
        }
        log::info!(
            "gallery: {} entries, {} runnable today",
            studio.examples.len(),
            studio
                .examples
                .iter()
                .filter(|entry| entry.tier == 'A')
                .count()
        );
        studio
            .engine
            .call("mapscene.templates", serde_json::json!({}));
        studio.engine.call("ops.list", serde_json::json!({}));

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
        self.params_form.revision += 1;
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
                self.params_form.revision += 1;
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

    /// View ▸ Live viewport: stop or resume rendering while the camera moves.
    pub fn toggle_live_viewport(&mut self, cx: &mut Context<Self>) {
        self.live_viewport = !self.live_viewport;
        self.refresh_menus(cx);
        if self.live_viewport {
            self.invalidate_viewport(cx);
        }
        self.set_status(if self.live_viewport {
            "Live viewport on"
        } else {
            "Live viewport off — the frame updates when you let go"
        });
        cx.notify();
    }

    /// Rebuild the menu tree so check marks match the current state.
    pub fn refresh_menus(&mut self, cx: &mut Context<Self>) {
        crate::actions::apply_menus(
            crate::actions::MenuState {
                live_viewport: self.live_viewport,
                certificate: self.sign_renders,
                path_traced: matches!(
                    self.project.render_quality.mode,
                    studio_project::RenderMode::PathTraced
                ),
            },
            cx,
        );
        self.menu_bar.update(cx, |bar, cx| bar.reload(cx));
    }

    /// Render ▸ Render frame: a still at the panel's current size.
    ///
    /// In a quality mode the still is the bridge's render (offline accumulation
    /// or path tracing), because that is what the engine's progress, denoiser
    /// and certificate provenance describe.
    pub fn request_still(&mut self, cx: &mut Context<Self>) {
        if self.quality_mode() {
            let (width, height) =
                render_size(self.viewport_size.get().0, self.viewport_size.get().1);
            if width < 2 || height < 2 || self.project.terrain.is_none() {
                return;
            }
            self.busy = true;
            self.engine.send(Command::RenderFrame {
                width,
                height,
                force: true,
            });
            self.viewport_dirty = false;
            self.set_status("Rendering still…");
            cx.notify();
            return;
        }
        self.viewport_dirty = true;
        self.request_frame(cx, true);
    }

    pub fn toggle_layer_visible(&mut self, id: LayerId, cx: &mut Context<Self>) {
        let before = self.project.document.clone();
        if let Some(layer) = self.project.document.layer_mut(id) {
            layer.visible = !layer.visible;
            let visible = layer.visible;
            if self.project.document.terrain_layer().map(|layer| layer.id) == Some(id) {
                self.project.terrain_visible = visible;
            }
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

    /// A drag or zoom finished: draw the settled view at full size and make
    /// sure the next export renders what the viewport showed.
    pub fn end_gesture(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.commit_view();
        self.viewport_dirty = true;
        // A release is worth a frame even with the live viewport off.
        if !self.render_in_flight {
            self.request_frame(cx, false);
        }
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
        // One render at a time. A pointer drag emits events far faster than a
        // frame can be drawn; without this gate the queue grows and the image
        // lags the camera by seconds.
        if self.render_in_flight {
            self.viewport_dirty = true;
            return;
        }
        let size = render_size(width, height);
        self.last_requested_size = size;
        self.viewport_dirty = false;
        // During a live drag the frame is drawn at half resolution: the camera
        // answers immediately and the release draws the full-size frame.
        let scale = if self.drag.is_some() && self.live_viewport && !force {
            0.5
        } else {
            1.0
        };
        let width = ((size.0 as f32 * scale) as u32).max(2);
        let height = ((size.1 as f32 * scale) as u32).max(2);
        self.render_in_flight = true;
        self.viewport.send(ViewportCommand::Render {
            width,
            height,
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

    // ------------------------------------------------------------------ layout

    /// Where the dock layout lives: the platform config dir, never the project
    /// (a project must open identically on another machine).
    pub fn layout_path() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            })?;
        Some(base.join("forge3d-studio").join("layout.json"))
    }

    /// Restore a saved layout, if its version still matches the panel set.
    /// Anything else — missing file, older schema, unknown panels — silently
    /// falls back to the default layout the caller already built.
    pub fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = Self::layout_path() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let Ok(state) = serde_json::from_str::<DockAreaState>(&text) else {
            self.push_log(
                true,
                format!("Ignored unreadable layout: {}", path.display()),
            );
            return;
        };
        if state.version != Some(crate::ui::DOCK_LAYOUT_VERSION) {
            self.push_log(
                false,
                "Layout from an older version ignored; building the default one".to_owned(),
            );
            return;
        }
        let area = self.dock_area.clone();
        match area.update(cx, |area, cx| area.load(state, window, cx)) {
            Ok(()) => self.push_log(
                false,
                format!("Restored dock layout from {}", path.display()),
            ),
            Err(error) => self.push_log(true, format!("Could not restore layout: {error:#}")),
        }
    }

    /// Persist the layout whenever the user changes it (debounced).
    fn watch_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.dock_area.clone();
        cx.subscribe_in(
            &area,
            window,
            |_this, area, event: &DockEvent, _window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    let area = area.clone();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(600))
                            .await;
                        let _ = this.update(cx, |this, cx| this.save_layout(&area, cx));
                    })
                    .detach();
                }
            },
        )
        .detach();
    }

    fn save_layout(&mut self, area: &Entity<DockArea>, cx: &mut Context<Self>) {
        let Some(path) = Self::layout_path() else {
            return;
        };
        let state = area.read(cx).dump(cx);
        let Ok(json) = serde_json::to_string_pretty(&state) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, json) {
            self.push_log(true, format!("Could not save layout: {error}"));
        }
    }

    // ------------------------------------------------------------------ layers

    /// Document rows, minus the children of collapsed groups.
    pub fn visible_rows(&self) -> Vec<studio_project::Row> {
        let collapsed = &self.layers.collapsed;
        self.project
            .document
            .rows()
            .into_iter()
            .filter(|row| {
                if collapsed.is_empty() {
                    return true;
                }
                !self
                    .project
                    .document
                    .ancestors(row.id)
                    .iter()
                    .any(|ancestor| collapsed.contains(ancestor))
            })
            .collect()
    }

    pub fn toggle_layer_collapsed(&mut self, id: LayerId, cx: &mut Context<Self>) {
        if !self.layers.collapsed.remove(&id) {
            self.layers.collapsed.insert(id);
        }
        cx.notify();
    }

    /// A click in the layer tree: replace, extend a range (shift) or toggle one
    /// row (ctrl/cmd).
    pub fn select_layer_row(
        &mut self,
        id: LayerId,
        range: bool,
        additive: bool,
        cx: &mut Context<Self>,
    ) {
        if range {
            if let Some(anchor) = self.layers.anchor {
                let rows = self.visible_rows();
                let order: Vec<LayerId> = rows.iter().map(|row| row.id).collect();
                if let (Some(from), Some(to)) = (
                    order.iter().position(|candidate| *candidate == anchor),
                    order.iter().position(|candidate| *candidate == id),
                ) {
                    let (low, high) = if from <= to { (from, to) } else { (to, from) };
                    self.selection = order[low..=high].to_vec();
                    cx.notify();
                    return;
                }
            }
        } else if additive {
            if let Some(index) = self.selection.iter().position(|other| *other == id) {
                self.selection.remove(index);
            } else {
                self.selection.push(id);
            }
            self.layers.anchor = Some(id);
            cx.notify();
            return;
        }
        self.selection = vec![id];
        self.layers.anchor = Some(id);
        self.layers.dragging = None;
        self.layers.drop_target = None;
        cx.notify();
    }

    /// Start an inline rename. The panel creates the input (it has a window)
    /// and hands it back through [`attach_rename_input`](Self::attach_rename_input),
    /// so the text and cursor survive re-renders.
    pub fn begin_rename(&mut self, id: LayerId, cx: &mut Context<Self>) {
        if self.project.document.layer(id).is_none() {
            return;
        }
        self.selection = vec![id];
        self.layers.renaming = Some(id);
        self.layers.rename_input = None;
        self.layers.rename_subscription = None;
        cx.notify();
    }

    /// Bind the freshly created rename box: Enter or blur commits, and a
    /// cancelled rename (panel closed) just drops the box.
    pub fn attach_rename_input(
        &mut self,
        id: LayerId,
        input: Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        let subscription =
            cx.subscribe(
                &input,
                move |this, input, event: &InputEvent, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        let text = input.read(cx).value().to_string();
                        this.commit_rename(id, &text, cx);
                    }
                    _ => {}
                },
            );
        self.layers.rename_input = Some(input);
        self.layers.rename_subscription = Some(subscription);
        cx.notify();
    }

    fn commit_rename(&mut self, id: LayerId, name: &str, cx: &mut Context<Self>) {
        let trimmed = name.trim().to_owned();
        if !trimmed.is_empty() {
            let before = self.project.document.clone();
            if let Some(layer) = self.project.document.layer_mut(id) {
                layer.name = trimmed;
            }
            self.history.record(before);
            self.project_dirty = true;
        }
        self.layers.renaming = None;
        self.layers.rename_input = None;
        cx.notify();
    }

    /// A drag started: snapshot the document so the whole drag is one undo step.
    pub fn begin_layer_drag(&mut self, id: LayerId, cx: &mut Context<Self>) {
        self.layers.dragging = Some(id);
        self.layers.drag_before = Some(self.project.document.clone());
        self.layers.drop_target = None;
        cx.notify();
    }

    /// The pointer is over a row while dragging: reorder to match, so the tree
    /// previews the drop. The snapshot is recorded on the first real move only.
    pub fn preview_drop(&mut self, target: LayerId, zone: DropZone, cx: &mut Context<Self>) {
        let Some(dragged) = self.layers.dragging else {
            return;
        };
        if dragged == target || self.layers.drop_target == Some((target, zone)) {
            return;
        }
        let target_is_group = self
            .project
            .document
            .layer(target)
            .map(|layer| layer.kind.is_group())
            .unwrap_or(false);
        let moved = match zone {
            DropZone::Into if target_is_group => self.project.document.move_into(dragged, target),
            DropZone::Before => self.project.document.move_beside(dragged, target, false),
            DropZone::After => self.project.document.move_beside(dragged, target, true),
            // Into a non-group behaves like after: a leaf cannot hold children.
            DropZone::Into => self.project.document.move_beside(dragged, target, true),
        };
        self.layers.drop_target = Some((target, zone));
        if moved {
            if let Some(before) = self.layers.drag_before.take() {
                self.history.record(before);
            }
            self.project_dirty = true;
            self.invalidate_viewport(cx);
            self.set_status("Reordered layers");
        }
        cx.notify();
    }

    pub fn toggle_layer_lock(&mut self, id: LayerId, cx: &mut Context<Self>) {
        let before = self.project.document.clone();
        if let Some(layer) = self.project.document.layer_mut(id) {
            layer.locked = !layer.locked;
        }
        self.history.record(before);
        self.project_dirty = true;
        cx.notify();
    }

    pub fn group_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let before = self.project.document.clone();
        let ids = self.selection.clone();
        if let Some(group) = self.project.document.group(&ids, "Group") {
            self.history.record(before);
            self.selection = vec![group];
            self.layers.anchor = Some(group);
            self.project_dirty = true;
            self.set_status("Grouped layers");
        }
        cx.notify();
    }

    pub fn ungroup_selection(&mut self, cx: &mut Context<Self>) {
        let groups: Vec<LayerId> = self
            .selection
            .iter()
            .copied()
            .filter(|id| {
                self.project
                    .document
                    .layer(*id)
                    .map(|layer| layer.kind.is_group())
                    .unwrap_or(false)
            })
            .collect();
        if groups.is_empty() {
            return;
        }
        let before = self.project.document.clone();
        for group in groups {
            self.project.document.ungroup(group);
        }
        self.history.record(before);
        self.selection.clear();
        self.project_dirty = true;
        self.set_status("Ungrouped");
        cx.notify();
    }

    pub fn move_layer(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(id) = self.primary_layer() else {
            return;
        };
        let before = self.project.document.clone();
        if self.project.document.move_by(id, delta) {
            self.history.record(before);
            self.project_dirty = true;
            self.invalidate_viewport(cx);
        }
        cx.notify();
    }

    pub fn move_layer_to_edge(&mut self, top: bool, cx: &mut Context<Self>) {
        let Some(id) = self.primary_layer() else {
            return;
        };
        let before = self.project.document.clone();
        if self.project.document.move_to_edge(id, top) {
            self.history.record(before);
            self.project_dirty = true;
            self.invalidate_viewport(cx);
        }
        cx.notify();
    }

    /// Opacity belongs to the selection (Photoshop), stepped in tenths.
    pub fn step_selection_opacity(&mut self, delta: f32, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let before = self.project.document.clone();
        for id in self.selection.clone() {
            if let Some(layer) = self.project.document.layer_mut(id) {
                layer.opacity = (layer.opacity + delta).clamp(0.0, 1.0);
            }
        }
        self.history.record(before);
        self.project_dirty = true;
        cx.notify();
    }

    // ----------------------------------------------------------------- quality

    /// A quality mode is a still: the native viewport stops painting and the
    /// image comes from the bridge (offline accumulation or path tracing).
    pub fn quality_mode(&self) -> bool {
        let quality = &self.project.render_quality;
        matches!(quality.mode, studio_project::RenderMode::PathTraced) || quality.offline.enabled
    }

    pub fn set_render_mode(&mut self, mode: studio_project::RenderMode, cx: &mut Context<Self>) {
        if self.project.render_quality.mode == mode {
            return;
        }
        self.project.render_quality.mode = mode;
        self.project_dirty = true;
        self.refresh_menus(cx);
        self.apply_quality(cx);
        self.set_status(format!("Render mode: {}", mode.label()));
        cx.notify();
    }

    fn apply_quality(&mut self, cx: &mut Context<Self>) {
        self.engine.send(Command::SetRenderQuality(
            self.project.render_quality.clone(),
        ));
        self.viewport_dirty = true;
        // A still is worth drawing immediately in a quality mode.
        if self.quality_mode() {
            self.request_still(cx);
        }
        cx.notify();
    }

    pub fn set_offline_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.project.render_quality.offline.enabled = enabled;
        self.project_dirty = true;
        self.apply_quality(cx);
    }

    pub fn step_offline_samples(&mut self, delta: i32, cx: &mut Context<Self>) {
        let samples = self.project.render_quality.offline.samples as i32 + delta;
        self.project.render_quality.offline.samples = samples.clamp(1, 512) as u32;
        self.project_dirty = true;
        self.apply_quality(cx);
    }

    pub fn set_denoiser(&mut self, denoiser: String, cx: &mut Context<Self>) {
        self.project.render_quality.offline.denoiser = denoiser;
        self.project_dirty = true;
        self.apply_quality(cx);
    }

    /// Path-trace knobs. `None` leaves a value alone.
    pub fn step_path_trace(
        &mut self,
        spp: i32,
        frames: i32,
        variance: f32,
        env: f32,
        seed: i32,
        cx: &mut Context<Self>,
    ) {
        let trace = &mut self.project.render_quality.path_trace;
        if spp != 0 {
            trace.spp = (trace.spp as i32 + spp).clamp(1, 64) as u32;
        }
        if frames != 0 {
            trace.min_frames = (trace.min_frames as i32 + frames).max(1) as u32;
            trace.max_frames = trace.max_frames.max(trace.min_frames);
        }
        if variance != 1.0 {
            trace.variance_threshold = (trace.variance_threshold * variance).clamp(1e-6, 1.0);
        }
        if env != 1.0 {
            trace.env_intensity = (trace.env_intensity * env).clamp(0.0, 10.0);
        }
        if seed != 0 {
            trace.seed = (trace.seed as i32 + seed) as u32;
        }
        self.project_dirty = true;
        self.apply_quality(cx);
    }

    pub fn refresh_capabilities(&mut self, cx: &mut Context<Self>) {
        let job = self
            .engine
            .call("offline.capabilities", serde_json::json!({}));
        self.pending_jobs.insert(job, PendingJob::Capabilities);
        cx.notify();
    }

    pub fn refresh_diagnostics(&mut self, cx: &mut Context<Self>) {
        let job = self
            .engine
            .call("diagnostics.snapshot", serde_json::json!({}));
        self.pending_jobs
            .insert(job, PendingJob::DiagnosticsSnapshot);
        self.set_status("Reading diagnostics…");
        cx.notify();
    }

    /// Certificates: sign every render, and verify a produced one.
    pub fn set_sign_renders(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.sign_renders = enabled;
        self.refresh_menus(cx);
        self.engine.send(Command::SetCertificate(enabled));
        self.set_status(if enabled {
            "Signing every render"
        } else {
            "Certificate signing off"
        });
        cx.notify();
    }

    pub fn verify_certificate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(certificate) = self.certificates.get(index).cloned() else {
            return;
        };
        let job = self.engine.call(
            "certificate.verify",
            serde_json::json!({ "path": certificate.path.to_string_lossy() }),
        );
        self.pending_jobs
            .insert(job, PendingJob::CertificateVerify(index));
        self.set_status(format!("Verifying {}", certificate.path.display()));
        cx.notify();
    }

    fn note_certificate(&mut self, path: PathBuf, source: String) {
        let payload_sha256 = read_certificate_sha(&path);
        self.certificates.push(SessionCertificate {
            path,
            source,
            payload_sha256,
            verified: None,
        });
    }

    /// Determinism: two runs, one process, one backend, compared by SHA-256.
    pub fn run_determinism_check(&mut self, cx: &mut Context<Self>) {
        if self.project.terrain.is_none() {
            self.push_error("Determinism", "no terrain loaded");
            return;
        }
        let scratch = std::env::temp_dir();
        let tag = format!("{}-{}", std::process::id(), self.certificates.len());
        let camera = self.project.camera;
        let sun = self.project.sun;
        let job = self.engine.call(
            "determinism.check",
            serde_json::json!({
                "width": 320,
                "height": 240,
                "out_a": scratch.join(format!("det-{tag}-a.png")).to_string_lossy(),
                "out_b": scratch.join(format!("det-{tag}-b.png")).to_string_lossy(),
                "camera": {
                    "phi_deg": camera.phi_deg,
                    "theta_deg": camera.theta_deg,
                    "radius": camera.radius,
                    "fov_deg": camera.fov_deg,
                },
                "sun": {
                    "azimuth_deg": sun.azimuth_deg,
                    "elevation_deg": sun.elevation_deg,
                    "intensity": sun.intensity,
                },
            }),
        );
        self.pending_jobs.insert(job, PendingJob::DeterminismCheck);
        self.set_status("Rendering twice for the determinism check…");
        cx.notify();
    }

    // -------------------------------------------------------------------- rig

    pub fn set_rig_kind(&mut self, kind: String, cx: &mut Context<Self>) {
        self.rig.kind = kind;
        self.rig.keyframes.clear();
        self.rig.frame = 0;
        self.set_status("Rig kind changed — bake to sample it");
        cx.notify();
    }

    pub fn step_rig_duration(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.rig.duration = (self.rig.duration + delta).clamp(1.0, 60.0);
        cx.notify();
    }

    pub fn step_rig_radius(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.rig.radius_rel = (self.rig.radius_rel + delta).clamp(0.05, 0.9);
        cx.notify();
    }

    /// Bake the rig through the engine's own `camera_rigs`.
    pub fn bake_rig(&mut self, cx: &mut Context<Self>) {
        let Some(terrain) = self.project.terrain.clone() else {
            self.push_error("Camera rig", "load a DEM dataset first");
            return;
        };
        // The bridge works in scatter-source units (terrain width = max
        // dimension); the UI's radius slip is relative to that.
        let extent = terrain.width.max(terrain.height) as f64;
        let params = match self.rig.kind.as_str() {
            "orbit" => serde_json::json!({
                "duration": self.rig.duration,
                "radius": self.rig.radius_rel as f64 * extent,
                "phi_start_deg": 0.0,
                "phi_end_deg": 360.0,
                "theta_start_deg": 45.0,
            }),
            kind => serde_json::json!({
                "duration": self.rig.duration,
                if kind == "follow" { "target_path_xz" } else { "path_xz" }: [
                    [extent * 0.2, extent * 0.8],
                    [extent * 0.5, extent * 0.5],
                    [extent * 0.8, extent * 0.2],
                ],
            }),
        };
        let job = self.engine.call(
            "rig.bake",
            serde_json::json!({
                "kind": self.rig.kind,
                "params": params,
                "samples_per_second": self.rig.samples_per_second,
                "domain": [terrain.height_min, terrain.height_max],
            }),
        );
        self.pending_jobs.insert(job, PendingJob::RigBake);
        self.set_status("Baking camera rig…");
        cx.notify();
    }

    pub fn scrub_rig(&mut self, delta: i32, cx: &mut Context<Self>) {
        let count = self.rig.keyframes.len();
        if count == 0 {
            return;
        }
        let next = (self.rig.frame as i32 + delta).rem_euclid(count as i32) as usize;
        self.apply_rig_frame(next, cx);
    }

    fn apply_rig_frame(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(keyframe) = self.rig.keyframes.get(index).copied() else {
            return;
        };
        self.rig.frame = index;
        self.set_camera(keyframe, cx);
        self.commit_view();
        cx.notify();
    }

    pub fn toggle_rig_playback(&mut self, cx: &mut Context<Self>) {
        if self.rig.keyframes.is_empty() {
            self.push_error("Timeline", "bake a camera rig first");
            return;
        }
        self.rig.playing = !self.rig.playing;
        if self.rig.playing {
            cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(83))
                    .await;
                let playing = this
                    .update(cx, |this, cx| {
                        if this.rig.playing {
                            this.scrub_rig(1, cx);
                        }
                        this.rig.playing
                    })
                    .unwrap_or(false);
                if !playing {
                    break;
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// Export the baked keyframes as `frame_%04d.png` through the bridge.
    pub fn render_sequence(&mut self, cx: &mut Context<Self>) {
        if self.sequence.is_some() {
            self.push_error("Sequence render", "a sequence is already running");
            return;
        }
        if self.rig.keyframes.len() < 2 {
            self.push_error("Sequence render", "bake a camera rig first");
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose the sequence output folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = first_path(rx.await.ok().and_then(|result| result.ok()));
            let Some(out_dir) = picked else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.queue_sequence(&out_dir, cx));
        })
        .detach();
    }

    fn queue_sequence(&mut self, out_dir: &Path, cx: &mut Context<Self>) {
        let total = (self.rig.keyframes.len() as u32).min(240);
        let width = self.project.export.width;
        let height = self.project.export.height;
        let sun = self.project.sun;
        let colormap = self.project.colormap.clone();
        for index in 0..total {
            let keyframe = self.rig.keyframes[index as usize];
            let path = out_dir.join(format!("frame_{index:04}.png"));
            let job = self.engine.call(
                "terrain.render",
                serde_json::json!({
                    "width": width,
                    "height": height,
                    "camera": {
                        "phi_deg": keyframe.phi_deg,
                        "theta_deg": keyframe.theta_deg,
                        "radius": keyframe.radius,
                        "fov_deg": keyframe.fov_deg,
                    },
                    "sun": {
                        "azimuth_deg": sun.azimuth_deg,
                        "elevation_deg": sun.elevation_deg,
                        "intensity": sun.intensity,
                    },
                    "colormap": colormap,
                    "out_png": path.to_string_lossy(),
                    "certificate": self.sign_renders,
                }),
            );
            self.pending_jobs.insert(job, PendingJob::SequenceFrame);
        }
        if (self.rig.keyframes.len() as u32) > total {
            self.push_log(
                true,
                format!(
                    "Sequence capped at {total} frames (baked {})",
                    self.rig.keyframes.len()
                ),
            );
        }
        self.sequence = Some(SequenceRun {
            out_dir: out_dir.to_path_buf(),
            total,
            finished: 0,
        });
        self.set_status(format!("Rendering {total} frames to {}", out_dir.display()));
        cx.notify();
    }

    // ------------------------------------------------------------------- scene

    pub fn set_scene_mode(&mut self, mode: SceneMode, cx: &mut Context<Self>) {
        self.scene_mode = mode;
        self.project.scene_mode = match mode {
            SceneMode::Terrain => "terrain".to_owned(),
            SceneMode::MapScene => "mapscene".to_owned(),
        };
        self.project_dirty = true;
        self.invalidate_viewport(cx);
        self.set_status(match mode {
            SceneMode::Terrain => "Terrain mode",
            SceneMode::MapScene => "MapScene mode — build and render a recipe",
        });
        cx.notify();
    }

    /// Declared ops this bridge does not implement (empty = the scene's engine
    /// surface is present).
    pub fn missing_ops(&self, entry: &ExampleEntry) -> Vec<String> {
        if self.available_ops.is_empty() {
            // The catalog has not arrived yet: do not claim a scene is broken.
            return Vec::new();
        }
        entry
            .ops
            .iter()
            .filter(|op| !self.available_ops.contains(*op))
            .cloned()
            .collect()
    }

    /// Everything a gallery scene needs from disk, with its status.
    ///
    /// The index names assets as repo-relative paths inside `scene` (dem,
    /// raster, …) plus a thumbnail; anything path-shaped is reported, so a new
    /// asset key does not need a UI change to be disclosed.
    pub fn example_assets(&self, entry: &ExampleEntry) -> Vec<AssetStatus> {
        let mut seen = HashSet::new();
        let mut assets = Vec::new();
        let mut push = |label: &str, declared: &str| {
            let key = declared.to_owned();
            if !seen.insert(key) {
                return;
            }
            let is_remote = declared.starts_with("http://") || declared.starts_with("https://");
            if is_remote {
                assets.push(AssetStatus {
                    label: label.to_owned(),
                    declared: declared.to_owned(),
                    kind: AssetKind::Remote(declared.to_owned()),
                });
                return;
            }
            let resolved = resolve_repo_path(Path::new(declared));
            let kind = match std::fs::metadata(&resolved) {
                Ok(_) if is_lfs_pointer(&resolved) => AssetKind::LfsPointer,
                Ok(metadata) => AssetKind::Present(metadata.len()),
                Err(_) => AssetKind::Missing,
            };
            assets.push(AssetStatus {
                label: label.to_owned(),
                declared: declared.to_owned(),
                kind,
            });
        };

        if let Some(thumbnail) = entry.thumbnail.as_deref() {
            push("Thumbnail", thumbnail);
        }
        if let Some(scene) = entry.scene.as_ref().and_then(|scene| scene.as_object()) {
            for (key, value) in scene {
                let Some(text) = value.as_str() else {
                    continue;
                };
                if looks_like_asset(key, text) {
                    push(asset_label(key), text);
                }
            }
        }
        assets
    }

    /// The decoded thumbnail for an entry, loaded from disk once.
    pub fn example_thumbnail(
        &mut self,
        entry: &ExampleEntry,
        cx: &mut Context<Self>,
    ) -> Option<Arc<gpui_kit::RenderImage>> {
        if let Some(cached) = self.thumbnails.get(&entry.id) {
            return cached.clone();
        }
        let loaded = entry
            .thumbnail
            .as_deref()
            .map(|declared| resolve_repo_path(Path::new(declared)))
            .filter(|path| path.is_file())
            .and_then(|path| image::open(&path).ok())
            .map(|image| {
                Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(
                    image.to_rgba8(),
                )]))
            });
        self.thumbnails.insert(entry.id.clone(), loaded.clone());
        cx.notify();
        loaded
    }

    /// Switch workspace: which properties page and left editor are showing.
    pub fn set_workspace(&mut self, workspace: Workspace, cx: &mut Context<Self>) {
        self.workspace = workspace;
        match workspace {
            Workspace::Layout => {
                self.properties_tab = PropertiesTab::Layer;
                self.left_tab = LeftTab::Layers;
            }
            Workspace::Terrain => {
                self.properties_tab = PropertiesTab::Terrain;
                self.left_tab = LeftTab::Layers;
            }
            Workspace::Cartography => {
                self.properties_tab = PropertiesTab::Scene;
                self.left_tab = LeftTab::Layers;
            }
            Workspace::Render => {
                self.properties_tab = PropertiesTab::Render;
                self.left_tab = LeftTab::Layers;
            }
            Workspace::Look => {
                self.properties_tab = PropertiesTab::World;
                self.left_tab = LeftTab::Layers;
            }
            Workspace::Data => {
                self.properties_tab = PropertiesTab::Layer;
                self.left_tab = LeftTab::Examples;
            }
        }
        self.set_status(format!("Workspace: {}", workspace.label()));
        cx.notify();
    }

    pub fn set_properties_tab(&mut self, tab: PropertiesTab, cx: &mut Context<Self>) {
        self.properties_tab = tab;
        cx.notify();
    }

    pub fn set_bottom_tab(&mut self, tab: BottomTab, cx: &mut Context<Self>) {
        self.bottom_tab = tab;
        cx.notify();
    }

    pub fn set_left_tab(&mut self, tab: LeftTab, cx: &mut Context<Self>) {
        self.left_tab = tab;
        cx.notify();
    }

    pub fn toggle_gizmo(&mut self, cx: &mut Context<Self>) {
        self.show_gizmo = !self.show_gizmo;
        cx.notify();
    }

    /// Bring the Examples tab forward (Help ▸ Examples gallery).
    pub fn focus_examples(&mut self, cx: &mut Context<Self>) {
        self.left_tab = LeftTab::Examples;
        self.set_status("Examples gallery");
        cx.notify();
    }

    /// Select a card for the download carousel (the dialog reads this).
    pub fn preview_example(&mut self, id: impl Into<String>, cx: &mut Context<Self>) {
        self.gallery_selected = Some(id.into());
        cx.notify();
    }

    /// Move the carousel by `delta` through the currently filtered list.
    pub fn step_carousel(&mut self, delta: i32, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .visible_examples()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let current = self
            .gallery_selected
            .as_ref()
            .and_then(|selected| ids.iter().position(|id| id == selected))
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(ids.len() as i32) as usize;
        self.gallery_selected = Some(ids[next].clone());
        cx.notify();
    }

    /// Examples passing the current category / runnable filters, in tier order
    /// (runnable first) so the gallery reads as a list of what works.
    pub fn visible_examples(&self) -> Vec<ExampleEntry> {
        let mut entries: Vec<ExampleEntry> = self
            .examples
            .iter()
            .filter(|entry| {
                (self.gallery_filter == "All" || entry.category == self.gallery_filter)
                    && (!self.gallery_runnable_only || entry.tier == 'A')
            })
            .cloned()
            .collect();
        entries.sort_by_key(|entry| (entry.tier, entry.title.clone()));
        entries
    }

    /// Materialize a scene's LFS assets, then open it. Runs `git lfs pull`
    /// for exactly the paths the card disclosed.
    pub fn download_and_open(&mut self, entry: ExampleEntry, cx: &mut Context<Self>) {
        let assets = self.example_assets(&entry);
        let mut wanted: Vec<String> = Vec::new();
        for asset in &assets {
            match &asset.kind {
                AssetKind::Present(_) => {}
                AssetKind::LfsPointer | AssetKind::Missing => {
                    wanted.push(asset.declared.clone());
                }
                AssetKind::Remote(url) => {
                    self.push_error(
                        "Examples gallery",
                        format!(
                            "{} is a remote asset ({url}); Studio does not fetch it",
                            asset.label
                        ),
                    );
                }
            }
        }
        if wanted.is_empty() {
            self.open_example(entry, cx);
            return;
        }
        self.gallery_cards
            .insert(entry.id.clone(), CardState::Loading);
        let (tx, rx) = std::sync::mpsc::channel();
        self.lfs_fetch = Some(rx);
        let root = repo_root();
        self.set_status(format!(
            "Fetching {} asset(s) through git lfs…",
            wanted.len()
        ));
        std::thread::spawn(move || {
            let include = wanted.join(",");
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["lfs", "pull", "--include"])
                .arg(&include)
                .output();
            let outcome = match result {
                Ok(output) if output.status.success() => Ok(()),
                Ok(output) => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
                Err(error) => Err(format!("git lfs: {error}")),
            };
            let _ = tx.send(outcome);
        });
        // The card keeps the entry so the open can resume when the fetch ends.
        self.pending_example = Some(entry);
        cx.notify();
    }

    /// Poll the background `git lfs pull` and resume the open when it lands.
    fn poll_lfs_fetch(&mut self, cx: &mut Context<Self>) {
        let Some(rx) = self.lfs_fetch.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.set_status("Assets fetched");
                if let Some(entry) = self.pending_example.clone() {
                    self.open_example(entry, cx);
                }
            }
            Ok(Err(message)) => {
                let id = self
                    .pending_example
                    .take()
                    .map(|entry| entry.id)
                    .unwrap_or_default();
                if !id.is_empty() {
                    self.gallery_cards
                        .insert(id, CardState::Failed(message.clone()));
                }
                self.push_error(
                    "Examples gallery",
                    format!("git lfs pull failed: {message}"),
                );
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.lfs_fetch = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }
        cx.notify();
    }

    /// Open a gallery entry: load its DEM if needed, then apply its scene.
    pub fn open_example(&mut self, entry: ExampleEntry, cx: &mut Context<Self>) {
        let Some(scene) = entry.scene.clone() else {
            return;
        };
        let dem = scene
            .get("dem")
            .and_then(|value| value.as_str())
            .map(|path| resolve_repo_path(Path::new(path)));
        if let Some(path) = &dem {
            if !path.exists() {
                self.push_error(
                    "Examples gallery",
                    format!(
                        "fixture {} is missing — engine fixtures live in git LFS (git lfs pull --include={})",
                        path.display(),
                        path.display()
                    ),
                );
                cx.notify();
                return;
            }
        }
        let needs_load = match (&dem, &self.project.terrain) {
            (Some(path), Some(terrain)) => terrain.path != *path,
            (Some(_), None) => true,
            (None, _) => false,
        };
        self.set_status(format!("Opening example '{}'…", entry.title));
        self.gallery_cards
            .insert(entry.id.clone(), CardState::Loading);
        if needs_load {
            if let Some(path) = dem {
                self.pending_example = Some(entry);
                self.import_dem(&path, cx);
                return;
            }
        }
        self.pending_example = Some(entry);
        self.apply_pending_example(cx);
    }

    /// Apply the scene of the example whose DEM just finished loading.
    fn apply_pending_example(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.pending_example.take() else {
            return;
        };
        let Some(scene) = entry.scene.clone() else {
            return;
        };
        let kind = scene
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("terrain");
        let span = self
            .project
            .terrain
            .as_ref()
            .map(|terrain| terrain_span(terrain.height_min, terrain.height_max))
            .unwrap_or(1000.0);
        if let Some(camera) = scene.get("camera") {
            let next = scene_camera(camera, span, self.project.camera);
            self.set_camera(next, cx);
        }
        if let Some(sun) = scene.get("sun") {
            let next = scene_sun(sun, self.project.sun);
            self.set_sun(next, cx);
        }
        if let Some(params) = scene.get("params") {
            if let Some(map) = params.as_object() {
                for (key, value) in map {
                    self.set_param("root", key, value.clone(), cx);
                }
            }
        }
        match kind {
            "mapscene" => {
                if let Some(template) = scene.get("template").and_then(|value| value.as_str()) {
                    self.mapscene.template = template.to_owned();
                }
                self.set_scene_mode(SceneMode::MapScene, cx);
                self.build_mapscene(cx);
            }
            "terrain_rig" => {
                self.set_scene_mode(SceneMode::Terrain, cx);
                self.set_rig_kind("orbit".to_owned(), cx);
                self.bake_rig(cx);
            }
            _ => self.set_scene_mode(SceneMode::Terrain, cx),
        }
        self.project.name = entry.title.clone();
        log::info!(
            "example applied: {} (kind {}, phi {:.0}, theta {:.0}, radius {:.0}, sun {:.0}/{:.0})",
            entry.title,
            kind,
            self.project.camera.phi_deg,
            self.project.camera.theta_deg,
            self.project.camera.radius,
            self.project.sun.azimuth_deg,
            self.project.sun.elevation_deg
        );
        self.gallery_cards
            .insert(entry.id.clone(), CardState::Loaded);
        self.gallery_selected = Some(entry.id.clone());
        self.set_status(format!("Opened '{}'", entry.title));
        cx.notify();
    }

    /// `mapscene.template`: build the recipe the canonical example builds.
    pub fn build_mapscene(&mut self, cx: &mut Context<Self>) {
        let Some(terrain) = self.project.terrain.clone() else {
            self.push_error("MapScene template", "load a DEM dataset first");
            return;
        };
        self.busy = true;
        let job = self.engine.call(
            "mapscene.template",
            serde_json::json!({
                "template": self.mapscene.template,
                "dem_path": terrain.path.to_string_lossy(),
                "width": self.project.export.width,
                "height": self.project.export.height,
                "label_text": "Studio",
            }),
        );
        self.pending_jobs.insert(job, PendingJob::MapSceneTemplate);
        self.set_status("Building MapScene recipe…");
        cx.notify();
    }

    pub fn validate_mapscene(&mut self, cx: &mut Context<Self>) {
        let Some(recipe) = self.mapscene.recipe_json.clone() else {
            self.push_error("MapScene", "build a recipe first");
            return;
        };
        let job = self.engine.call(
            "mapscene.validate",
            serde_json::json!({ "recipe_json": recipe }),
        );
        self.pending_jobs.insert(job, PendingJob::MapSceneValidate);
        self.set_status("Validating recipe…");
        cx.notify();
    }

    /// `mapscene.render`: the certified `gpu_terrain` path.
    pub fn render_mapscene(&mut self, cx: &mut Context<Self>) {
        let Some(recipe) = self.mapscene.recipe_json.clone() else {
            self.push_error("MapScene", "build a recipe first");
            return;
        };
        let out = std::env::temp_dir().join(format!(
            "studio-mapscene-{}-{}.png",
            std::process::id(),
            self.certificates.len()
        ));
        self.busy = true;
        let job = self.engine.call(
            "mapscene.render",
            serde_json::json!({
                "recipe_json": recipe,
                "out_png": out.to_string_lossy(),
                "certificate": self.sign_renders,
            }),
        );
        self.pending_jobs.insert(job, PendingJob::MapSceneRender);
        self.set_status("Rendering the recipe through MapScene…");
        cx.notify();
    }

    // --------------------------------------------------------------- atmosphere

    pub fn set_atmosphere_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.project.atmosphere.enabled = enabled;
        self.project_dirty = true;
        self.engine
            .send(Command::SetAtmosphere(self.project.atmosphere.clone()));
        self.invalidate_viewport(cx);
        if enabled {
            self.bake_atmosphere(cx);
        }
    }

    /// One combined stepper: turbidity delta, intensity factor, mie-g delta.
    pub fn step_atmosphere(
        &mut self,
        turbidity: f32,
        intensity: f32,
        mie: f32,
        cx: &mut Context<Self>,
    ) {
        let atmosphere = &mut self.project.atmosphere;
        atmosphere.turbidity = (atmosphere.turbidity + turbidity).clamp(1.0, 20.0);
        atmosphere.intensity = (atmosphere.intensity * intensity).clamp(0.1, 10.0);
        atmosphere.mie_g = (atmosphere.mie_g + mie).clamp(0.0, 0.99);
        self.project_dirty = true;
        self.engine
            .send(Command::SetAtmosphere(self.project.atmosphere.clone()));
        self.bake_atmosphere(cx);
    }

    /// Bake the AETHER sky; the bridge writes the HDR and the provenance.
    pub fn bake_atmosphere(&mut self, cx: &mut Context<Self>) {
        let job = self.engine.call(
            "atmosphere.bake",
            serde_json::json!({
                "settings": {
                    "turbidity": self.project.atmosphere.turbidity,
                    "ozone_du": self.project.atmosphere.ozone_du,
                    "mie_g": self.project.atmosphere.mie_g,
                    "ground_albedo": self.project.atmosphere.ground_albedo,
                    "scattering_orders": self.project.atmosphere.scattering_orders,
                },
                "sun_elevation_deg": self.project.sun.elevation_deg,
                "sun_azimuth_deg": self.project.sun.azimuth_deg,
            }),
        );
        self.pending_jobs.insert(job, PendingJob::AtmosphereBake);
        self.set_status("Baking the AETHER sky…");
        cx.notify();
    }

    // ------------------------------------------------------------- observation

    pub fn step_observation(&mut self, dlat: f32, dlon: f32, cx: &mut Context<Self>) {
        let observation = &mut self.project.observation;
        observation.latitude_deg = (observation.latitude_deg + dlat).clamp(-89.0, 89.0);
        observation.longitude_deg = (observation.longitude_deg + dlon).clamp(-180.0, 180.0);
        self.project_dirty = true;
        cx.notify();
    }

    /// `astro.observe` → the terrain sun, with the compass→renderer conversion
    /// the engine's own recipes use (`A − 90`).
    pub fn observe_sun(&mut self, cx: &mut Context<Self>) {
        let observation = self.project.observation.clone();
        let job = self.engine.call(
            "astro.observe",
            serde_json::json!({
                "datetime_utc": observation.datetime_utc,
                "latitude_deg": observation.latitude_deg,
                "longitude_deg": observation.longitude_deg,
                "height_m": observation.height_m,
                "refraction": observation.refraction,
            }),
        );
        self.pending_jobs.insert(job, PendingJob::AstroObserve);
        self.set_status("Asking SIDERA for the Sun…");
        cx.notify();
    }

    // ------------------------------------------------------------------- smoke

    pub fn set_smoke_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.project.smoke.enabled = enabled;
        self.project.sync_smoke_layer();
        self.project_dirty = true;
        self.engine.send(Command::SetSmoke {
            params: self.project.smoke.clone(),
            opacity: 1.0,
        });
        self.invalidate_viewport(cx);
    }

    pub fn pick_smoke(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Load a smoke density volume (.npy or .npz)".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = first_path(rx.await.ok().and_then(|result| result.ok()));
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| this.load_smoke(&path, cx));
            }
        })
        .detach();
    }

    fn load_smoke(&mut self, path: &Path, cx: &mut Context<Self>) {
        let job = self.engine.call(
            "smoke.load",
            serde_json::json!({ "path": path.to_string_lossy() }),
        );
        self.pending_jobs.insert(job, PendingJob::SmokeLoad);
        self.project.smoke.path = Some(path.to_path_buf());
        self.project.smoke.enabled = true;
        self.project.sync_smoke_layer();
        self.project_dirty = true;
        self.set_status(format!("Loading smoke volume {}", path.display()));
        cx.notify();
    }

    pub fn clear_smoke(&mut self, cx: &mut Context<Self>) {
        let job = self.engine.call("smoke.clear", serde_json::json!({}));
        self.pending_jobs.insert(job, PendingJob::SmokeLoad);
        self.project.smoke.path = None;
        self.smoke_report = None;
        self.project.sync_smoke_layer();
        self.project_dirty = true;
        self.engine.send(Command::SetSmoke {
            params: self.project.smoke.clone(),
            opacity: 1.0,
        });
        self.set_status("Smoke volume cleared");
        cx.notify();
    }

    pub fn step_smoke_exposure(&mut self, delta: f32, cx: &mut Context<Self>) {
        let smoke = &mut self.project.smoke;
        smoke.exposure = (smoke.exposure + delta).clamp(0.05, 10.0);
        self.project_dirty = true;
        self.engine.send(Command::SetSmoke {
            params: smoke.clone(),
            opacity: 1.0,
        });
        self.invalidate_viewport(cx);
    }

    // ------------------------------------------------------------------ params

    /// The committed value of one parameter, falling back to the schema default
    /// (the value a render would really have used).
    pub fn param_value(
        &self,
        section: &str,
        key: &str,
        default: &serde_json::Value,
    ) -> serde_json::Value {
        let params = &self.project.terrain_params;
        let found = if section == "root" {
            params.get(key)
        } else {
            params.get(section).and_then(|entry| entry.get(key))
        };
        found.cloned().unwrap_or_else(|| default.clone())
    }

    /// Apply one edited parameter: project, engine, then a debounced look at
    /// what the engine thinks of it.
    pub fn set_param(
        &mut self,
        section: &str,
        key: &str,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let schema = self
            .params_schema
            .clone()
            .unwrap_or(serde_json::Value::Null);
        if !self.project.terrain_params.is_object() {
            self.project.terrain_params = serde_json::Value::Object(serde_json::Map::new());
        }
        if section == "root" {
            if let Some(map) = self.project.terrain_params.as_object_mut() {
                map.insert(key.to_owned(), value);
            }
        } else {
            let defaults = crate::ui::params::section_defaults(&schema, section);
            if let Some(map) = self.project.terrain_params.as_object_mut() {
                let entry = map.entry(section.to_owned()).or_insert(defaults.clone());
                if !entry.is_object() {
                    *entry = defaults;
                }
                if let Some(section_map) = entry.as_object_mut() {
                    section_map.insert(key.to_owned(), value);
                }
            }
        }
        self.project_dirty = true;
        self.engine.send(Command::SetRenderParams(
            self.project.terrain_params.clone(),
        ));
        self.schedule_params_validate();
        cx.notify();
    }

    fn schedule_params_validate(&mut self) {
        self.params_form.validate_due =
            Some(std::time::Instant::now() + Duration::from_millis(350));
    }

    fn flush_params_validate(&mut self) {
        if let Some(due) = self.params_form.validate_due {
            if std::time::Instant::now() >= due {
                self.params_form.validate_due = None;
                self.engine.call(
                    "terrain.params_validate",
                    serde_json::json!({ "params": self.project.terrain_params }),
                );
            }
        }
    }

    /// The live input for one schema field, created on first use and kept, so
    /// typing is never interrupted by a re-render.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_param_input(
        &mut self,
        section: &str,
        key: &str,
        text: &str,
        kind: ParamKind,
        nullable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let id = format!("{section}.{key}");
        if let Some(state) = self.params_form.inputs.get(&id) {
            return state.clone();
        }
        let placeholder = if nullable {
            "default".to_owned()
        } else {
            String::new()
        };
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(text.to_owned())
        });
        self.params_form.kinds.insert(id.clone(), (kind, nullable));
        self.params_form.inputs.insert(id.clone(), state.clone());

        cx.subscribe_in(
            &state,
            window,
            move |this, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    this.on_param_text(&id, &text, cx);
                }
            },
        )
        .detach();
        state
    }

    /// Text changed in a parameter box: commit it only when it is a complete
    /// value, so a half-typed number never reaches the renderer.
    fn on_param_text(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        let Some((section, key)) = id.split_once('.') else {
            return;
        };
        let Some((kind, nullable)) = self.params_form.kinds.get(id).copied() else {
            return;
        };
        if let Some(value) = kind.parse(text, nullable) {
            let (section, key) = (section.to_owned(), key.to_owned());
            self.set_param(&section, &key, value, cx);
        }
    }

    // ------------------------------------------------------------------- polls

    /// A 20 Hz UI pump: drains both workers, coalesces viewport renders and
    /// repaints only when something changed.
    fn start_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
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

        self.poll_lfs_fetch(cx);
        self.flush_params_validate();
        crate::ui::params::sync_inputs(self, window, cx);

        // Dev hook: a scripted orbit, so interaction cadence can be measured
        // (and compared against a release build) without a human dragging.
        if let Ok(speed) = std::env::var("FORGE3D_STUDIO_AUTORBIT") {
            if let Ok(speed) = speed.parse::<f32>() {
                if speed > 0.0 && self.project.terrain.is_some() {
                    let mut camera = self.project.camera;
                    camera.phi_deg = (camera.phi_deg + speed * 0.016).rem_euclid(360.0);
                    self.drag = Some(ViewDrag::Orbit {
                        last_x: 0.0,
                        last_y: 0.0,
                    });
                    self.set_camera(camera, cx);
                    changed = true;
                }
            }
        }

        // A dirty view or a new panel size asks for one frame, but only when
        // nothing is in flight: throughput, not input rate, sets the cadence.
        // With the live viewport off, a drag only marks the view dirty; the
        // gesture end (or Ctrl+R) draws it.
        let dragging_without_live = self.drag.is_some() && !self.live_viewport;
        if self.project.terrain.is_some() && !dragging_without_live && !self.quality_mode() {
            let (width, height) = self.viewport_size.get();
            let size = render_size(width, height);
            if width > 1.0
                && height > 1.0
                && !self.render_in_flight
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
                self.render_in_flight = false;
                self.push_log(true, format!("{context}: {message}"));
                self.set_status(format!("{context} failed"));
            }
            ViewportEvent::TerrainLoaded => {
                self.render_in_flight = false;
                self.viewport_dirty = true;
            }
            ViewportEvent::TerrainCleared => {
                self.render_in_flight = false;
                self.frame = None;
            }
            ViewportEvent::Frame(frame) => {
                self.render_in_flight = false;
                self.frame_ms = frame.render_ms;
                if std::env::var_os("FORGE3D_STUDIO_FRAME_LOG").is_some() {
                    log::info!(
                        "frame {}x{} in {:.1} ms (live {}, dragging {})",
                        frame.width,
                        frame.height,
                        frame.render_ms,
                        self.live_viewport,
                        self.drag.is_some()
                    );
                }
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
                // A gallery card that needed this DEM resumes here — without
                // this the DEM loads and the example's camera/sun/params/
                // MapScene never apply.
                if self.pending_example.is_some() {
                    self.apply_pending_example(cx);
                }
            }
            Event::TerrainCleared => {
                self.busy = false;
                self.project.terrain = None;
                self.frame = None;
                self.set_status("Terrain cleared");
            }
            Event::FrameReady {
                width,
                height,
                rgba,
                quality,
                certificate,
                ..
            } => {
                self.busy = false;
                self.render_in_flight = false;
                // A quality still is the bridge's pixels, not the native frame.
                if quality.is_some() {
                    self.present_rgba(width, height, rgba);
                    self.viewport_dirty = false;
                }
                if let Some(path) = certificate {
                    let source = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    self.note_certificate(path, source);
                }
            }
            Event::ExportFinished {
                path, certificate, ..
            } => {
                self.busy = false;
                self.last_export = Some(path.clone());
                self.set_status(format!("Exported {}", path.display()));
                self.push_log(false, format!("Wrote {}", path.display()));
                if let Some(certificate) = certificate {
                    let source = certificate
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| certificate.display().to_string());
                    self.note_certificate(certificate, source);
                }
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
            Event::JobDone { job, op, result } => self.handle_job_done(job, &op, result, cx),
            Event::JobFailed { op, message, .. } => {
                self.busy = false;
                if op == "terrain.load" {
                    if let Some(entry) = self.pending_example.take() {
                        self.gallery_cards
                            .insert(entry.id, CardState::Failed(message.clone()));
                    }
                }
                self.push_log(true, format!("{op}: {message}"));
                if op.starts_with("terrain.params") {
                    self.params_error = Some(message.clone());
                }
                self.push_error(op, message);
            }
            Event::Error { context, message } => {
                self.busy = false;
                if context.starts_with("terrain load") || context == "LoadTerrain" {
                    if let Some(entry) = self.pending_example.take() {
                        self.gallery_cards
                            .insert(entry.id, CardState::Failed(message.clone()));
                    }
                }
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

    fn handle_job_done(
        &mut self,
        job: u64,
        op: &str,
        result: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if let Some(kind) = self.pending_jobs.remove(&job) {
            self.route_job(kind, op, &result, cx);
        }
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
            "ops.list" => {
                self.available_ops = result
                    .get("ops")
                    .and_then(|value| value.as_array())
                    .map(|ops| {
                        ops.iter()
                            .filter_map(|op| op.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "mapscene.templates" => {
                self.mapscene.templates = result
                    .get("templates")
                    .and_then(|value| value.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
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

    /// Results of the jobs this session queued, by what they were.
    fn route_job(
        &mut self,
        kind: PendingJob,
        op: &str,
        result: &serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        match kind {
            PendingJob::RigBake => {
                let keyframes = result
                    .get("keyframes")
                    .and_then(|value| value.as_array())
                    .map(|frames| {
                        frames
                            .iter()
                            .filter_map(|frame| {
                                Some(CameraParams {
                                    phi_deg: frame.get("phi_deg")?.as_f64()? as f32,
                                    theta_deg: frame.get("theta_deg")?.as_f64()? as f32,
                                    radius: frame.get("radius")?.as_f64()? as f32,
                                    fov_deg: frame
                                        .get("fov_deg")
                                        .and_then(|value| value.as_f64())
                                        .unwrap_or(54.0)
                                        as f32,
                                    target_x: 0.0,
                                    target_z: 0.0,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let count = keyframes.len();
                if count == 0 {
                    self.push_error("Camera rig", "the engine baked no keyframes");
                } else {
                    self.rig.keyframes = keyframes;
                    self.rig.frame = 0;
                    self.set_status(format!("Rig baked: {count} keyframes"));
                    self.apply_rig_frame(0, cx);
                }
            }
            PendingJob::DeterminismCheck => {
                let sha_a = result
                    .get("sha_a")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let sha_b = result
                    .get("sha_b")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let matched = result
                    .get("match")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                self.set_status(if matched {
                    "Determinism: two runs identical"
                } else {
                    "Determinism: runs DIFFER — this needs a look"
                });
                self.determinism = Some(DeterminismReport {
                    sha_a,
                    sha_b,
                    matched,
                });
            }
            PendingJob::CertificateVerify(index) => {
                let valid = result
                    .get("valid")
                    .or_else(|| result.get("verified"))
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if let Some(certificate) = self.certificates.get_mut(index) {
                    certificate.verified = Some(valid);
                }
                self.set_status(if valid {
                    "Certificate verified"
                } else {
                    "Certificate REJECTED by the engine"
                });
            }
            PendingJob::DiagnosticsSnapshot => {
                self.diagnostics = Some(result.clone());
                self.set_status("Diagnostics snapshot updated");
            }
            PendingJob::Capabilities => {
                self.offline_caps = Some(result.clone());
            }
            PendingJob::MapSceneTemplate => {
                let recipe = result
                    .get("recipe")
                    .or_else(|| result.get("recipe_json"))
                    .or_else(|| result.get("recipe_dict"))
                    .cloned();
                match recipe {
                    Some(recipe) => {
                        let text = recipe
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| recipe.to_string());
                        self.mapscene.recipe_json = Some(text);
                        self.mapscene.last_status =
                            Some("Recipe built from the template".to_owned());
                        self.busy = false;
                        self.set_status("Recipe built — validate or render it");
                    }
                    None => {
                        self.busy = false;
                        self.push_error("MapScene", "the engine returned no recipe");
                    }
                }
            }
            PendingJob::MapSceneValidate => {
                let status = result
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let diagnostics = result
                    .get("diagnostics")
                    .and_then(|value| value.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                self.mapscene.last_status = Some(if diagnostics.is_empty() {
                    format!("validation: {status}")
                } else {
                    format!("validation: {status} — {diagnostics}")
                });
                self.set_status(format!("Recipe validation: {status}"));
            }
            PendingJob::MapSceneRender => {
                self.busy = false;
                let status = result
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("rendered")
                    .to_owned();
                let png = result
                    .get("png")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from);
                if let Some(sha) = result
                    .get("metadata")
                    .and_then(|meta| meta.get("certificate_payload_sha256"))
                    .and_then(|value| value.as_str())
                {
                    self.mapscene.certificate_sha = Some(sha.to_owned());
                }
                self.mapscene.last_status = Some(format!("rendered: {status}"));
                if let Some(path) = png {
                    self.present_png(&path);
                }
                self.set_status("MapScene frame rendered");
            }
            PendingJob::AtmosphereBake => {
                self.atmosphere_meta = Some(result.clone());
                let sha = result
                    .get("hdr_sha256")
                    .and_then(|value| value.as_str())
                    .map(|sha| sha.chars().take(16).collect::<String>())
                    .unwrap_or_else(|| "no hash".to_owned());
                self.set_status(format!("Sky baked ({sha}…)"));
                self.invalidate_viewport(cx);
            }
            PendingJob::AstroObserve => {
                let sun = result.get("bodies").and_then(|bodies| bodies.get("sun"));
                if let Some(sun) = sun {
                    let azimuth = sun
                        .get("azimuth_deg")
                        .and_then(|value| value.as_f64())
                        .unwrap_or(0.0) as f32;
                    let altitude = sun
                        .get("altitude_deg")
                        .and_then(|value| value.as_f64())
                        .unwrap_or(0.0) as f32;
                    let mut next = self.project.sun;
                    next.azimuth_deg = (azimuth - 90.0).rem_euclid(360.0);
                    if altitude < 0.0 {
                        self.push_log(
                            true,
                            format!(
                                "The Sun is {altitude:.1}° below the horizon; clamped to 0° for the terrain light"
                            ),
                        );
                        next.elevation_deg = 0.0;
                    } else {
                        next.elevation_deg = altitude;
                    }
                    self.set_sun(next, cx);
                    self.commit_view();
                    self.set_status(format!(
                        "Sun from SIDERA: az {azimuth:.1}° / alt {altitude:.1}°"
                    ));
                }
            }
            PendingJob::SmokeLoad => {
                self.smoke_report = Some(result.clone());
                let status = result
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("loaded");
                self.set_status(format!("Smoke: {status}"));
            }
            PendingJob::SequenceFrame => {
                if let Some(sequence) = self.sequence.as_mut() {
                    sequence.finished += 1;
                    let (finished, total, dir) =
                        (sequence.finished, sequence.total, sequence.out_dir.clone());
                    if finished >= total {
                        self.sequence = None;
                        self.set_status(format!("Sequence written to {}", dir.display()));
                    } else {
                        self.set_status(format!("Sequence {finished}/{total}"));
                    }
                }
            }
        }
        let _ = op;
        cx.notify();
    }

    /// Present a PNG written by the engine (MapScene render).
    fn present_png(&mut self, path: &Path) {
        let Ok(image) = image::open(path) else {
            self.push_log(true, format!("Could not read {}", path.display()));
            return;
        };
        let rgba = image.to_rgba8();
        self.frame_size = rgba.dimensions();
        self.frame = Some(Arc::new(gpui_kit::RenderImage::new(vec![
            image::Frame::new(rgba),
        ])));
        self.viewport_dirty = false;
    }

    /// Present RGBA pixels as the viewport image (a quality still).
    fn present_rgba(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        let buffer = image::RgbaImage::from_raw(width, height, rgba)
            .unwrap_or_else(|| image::RgbaImage::new(width.max(1), height.max(1)));
        self.frame_size = (width, height);
        self.frame = Some(Arc::new(gpui_kit::RenderImage::new(vec![
            image::Frame::new(buffer),
        ])));
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

/// The render target size for a panel size, quantized to even pixels.
///
/// The HUD prints the frame size and the render time; unquantized bounds turn a
/// one-pixel text-width change into a new render, which changes the printed
/// numbers again — a feedback loop that keeps rendering while the app is idle.
pub fn render_size(width: f32, height: f32) -> (u32, u32) {
    let quantize = |value: f32| (((value as u32) / 2) * 2).max(2);
    (quantize(width), quantize(height))
}

/// A gallery scene's camera in renderer units.
///
/// Scenes pin `radius_rel` (a fraction of the terrain span) so a scene frames
/// the same way on any DEM; everything else is the example's own value.
pub fn scene_camera(camera: &serde_json::Value, span: f32, current: CameraParams) -> CameraParams {
    let mut next = current;
    let radius_rel = camera
        .get("radius_rel")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.4) as f32;
    if let Some(value) = camera.get("phi_deg").and_then(|value| value.as_f64()) {
        next.phi_deg = value as f32;
    }
    if let Some(value) = camera.get("theta_deg").and_then(|value| value.as_f64()) {
        next.theta_deg = value as f32;
    }
    if let Some(value) = camera.get("fov_deg").and_then(|value| value.as_f64()) {
        next.fov_deg = value as f32;
    }
    next.radius = (radius_rel * span).clamp(10.0, 400_000.0);
    next
}

/// A gallery scene's sun.
pub fn scene_sun(sun: &serde_json::Value, current: SunParams) -> SunParams {
    let mut next = current;
    if let Some(value) = sun.get("azimuth_deg").and_then(|value| value.as_f64()) {
        next.azimuth_deg = value as f32;
    }
    if let Some(value) = sun.get("elevation_deg").and_then(|value| value.as_f64()) {
        next.elevation_deg = value as f32;
    }
    if let Some(value) = sun.get("intensity").and_then(|value| value.as_f64()) {
        next.intensity = value as f32;
    }
    next
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

/// The repository checkout the app is running from.
///
/// Gallery scenes and engine fixtures are repo-relative; resolving them against
/// the process CWD breaks as soon as Studio is launched from elsewhere.
pub fn repo_root() -> PathBuf {
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        if dir
            .join("studio")
            .join("examples")
            .join("index.json")
            .is_file()
        {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// A git-LFS pointer file (the tracked path, not the content).
fn is_lfs_pointer(path: &Path) -> bool {
    use std::io::Read as _;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 64];
    let Ok(read) = file.read(&mut head) else {
        return false;
    };
    head[..read].starts_with(b"version https://git-lfs")
}

/// Extension whitelist for values that are asset paths, not scene settings.
fn looks_like_asset(key: &str, value: &str) -> bool {
    if value.starts_with("http://") || value.starts_with("https://") {
        return true;
    }
    const EXTENSIONS: &[&str] = &[
        ".tif", ".tiff", ".npy", ".npz", ".laz", ".copc", ".ept", ".geojson", ".json", ".glb",
        ".gltf", ".obj", ".hdr", ".exr", ".f3dz", ".f3z", ".png", ".jpg", ".jpeg", ".gpkg",
        ".parquet",
    ];
    let lower = value.to_ascii_lowercase();
    EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
        && !matches!(
            key,
            "colormap" | "label_text" | "template" | "kind" | "name"
        )
}

/// Human label for an asset key (`dem` → "DEM", `raster` → "Raster").
fn asset_label(key: &str) -> &str {
    match key {
        "dem" => "DEM",
        "raster" => "Raster",
        "hdr" => "Environment HDR",
        "vector" | "vectors" => "Vector data",
        "buildings" => "Buildings",
        "points" | "pointcloud" => "Point cloud",
        "mesh" => "Mesh",
        "tiles" | "tileset" => "3D Tiles",
        "smoke" => "Smoke volume",
        "thumbnail" => "Thumbnail",
        other => other,
    }
}

/// Resolve a repo-relative path against the checkout.
pub fn resolve_repo_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let candidate = repo_root().join(path);
    if candidate.exists() {
        candidate
    } else {
        path.to_path_buf()
    }
}

/// Read the examples gallery index.
fn load_examples() -> Vec<ExampleEntry> {
    let path = repo_root()
        .join("studio")
        .join("examples")
        .join("index.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(index) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    index
        .get("entries")
        .and_then(|entries| entries.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(ExampleEntry {
                        id: entry.get("id")?.as_str()?.to_owned(),
                        title: entry.get("title")?.as_str()?.to_owned(),
                        category: entry
                            .get("category")
                            .and_then(|value| value.as_str())
                            .unwrap_or("Other")
                            .to_owned(),
                        tier: entry
                            .get("tier")
                            .and_then(|value| value.as_str())
                            .and_then(|tier| tier.chars().next())
                            .unwrap_or('C'),
                        description: entry
                            .get("description")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_owned(),
                        example: entry
                            .get("example")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_owned(),
                        unlock: entry
                            .get("unlock")
                            .and_then(|value| value.as_str())
                            .unwrap_or("needs a later milestone")
                            .to_owned(),
                        scene: entry.get("scene").cloned(),
                        thumbnail: entry
                            .get("thumbnail")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned),
                        ops: entry
                            .get("ops")
                            .and_then(|value| value.as_array())
                            .map(|ops| {
                                ops.iter()
                                    .filter_map(|op| op.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The certificate's `payload_sha256`, when it carries one.
fn read_certificate_sha(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("payload_sha256")
        .or_else(|| value.get("payload_sha"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
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
    fn a_scene_camera_is_span_relative() {
        let scene = serde_json::json!({
            "phi_deg": 120.0,
            "theta_deg": 14.0,
            "radius_rel": 0.16,
            "fov_deg": 52.0
        });
        let span = terrain_span(469.0, 4391.0);
        let camera = scene_camera(&scene, span, CameraParams::default());
        assert_eq!(camera.phi_deg, 120.0);
        assert_eq!(camera.theta_deg, 14.0);
        assert_eq!(camera.fov_deg, 52.0);
        assert!(
            (camera.radius - span * 0.16).abs() < 1.0,
            "radius must scale with the DEM span, got {} for span {span}",
            camera.radius
        );
    }

    #[test]
    fn a_scene_sun_keeps_what_the_scene_does_not_set() {
        let before = SunParams {
            azimuth_deg: 10.0,
            elevation_deg: 20.0,
            intensity: 1.0,
        };
        let sun = scene_sun(&serde_json::json!({ "elevation_deg": 16.0 }), before);
        assert_eq!(sun.azimuth_deg, 10.0, "unset fields keep the document");
        assert_eq!(sun.elevation_deg, 16.0);
        assert_eq!(sun.intensity, 1.0);
    }

    #[test]
    fn asset_labels_read_like_the_scene() {
        assert_eq!(asset_label("dem"), "DEM");
        assert!(looks_like_asset("dem", "assets/tif/Mount_Fuji_30m.tif"));
        assert!(looks_like_asset("hdr", "https://example.com/sky.hdr"));
        assert!(!looks_like_asset("label_text", "Mount Fuji"));
        assert!(!looks_like_asset("colormap", "magma"));
    }

    #[test]
    fn render_sizes_are_even_pixels() {
        // Quantized so a one-pixel layout jitter cannot start a render loop.
        assert_eq!(render_size(1000.0, 601.0), (1000, 600));
        assert_eq!(render_size(1.0, 1.0), (2, 2));
        assert_eq!(render_size(0.0, 0.0), (2, 2));
    }

    #[test]
    fn the_gallery_index_parses_and_its_tier_a_scenes_are_openable() {
        let entries = load_examples();
        assert!(
            entries.len() >= 50,
            "every examples/*.py should be indexed, got {}",
            entries.len()
        );
        assert!(entries
            .iter()
            .all(|entry| matches!(entry.tier, 'A' | 'B' | 'C')));
        for entry in entries.iter().filter(|entry| entry.tier == 'A') {
            let scene = entry
                .scene
                .as_ref()
                .unwrap_or_else(|| panic!("tier A entry {} has no scene", entry.id));
            let kind = scene.get("kind").and_then(|value| value.as_str());
            assert!(
                kind.is_some(),
                "tier A entry {} names no scene kind",
                entry.id
            );
            if kind != Some("terrain_rig") {
                assert!(
                    scene.get("dem").and_then(|value| value.as_str()).is_some(),
                    "tier A entry {} names no DEM",
                    entry.id
                );
            }
        }
        assert!(
            entries
                .iter()
                .filter(|entry| entry.tier == 'B' || entry.tier == 'C')
                .all(|entry| !entry.unlock.is_empty()),
            "a not-yet-runnable entry must say what unlocks it"
        );
    }

    #[test]
    fn world_span_follows_the_engine_convention() {
        // Six times the relief, and the camera sits 1.65 spans out.
        let span = terrain_span(469.0, 4391.0);
        assert!((span - 23_532.0).abs() < 1.0);
        assert!((fit_radius(span) - span * 1.65).abs() < 1.0);
    }
}
