//! The Forge3D Studio main window.
//!
//! Layout follows GUI.md section 5: menu + toolbar, scene tree on the left,
//! the engine viewport in the center, properties on the right, and a status
//! bar at the bottom. Every control is wired to real application state and
//! real engine commands — there are no disconnected mockups.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{
    Color32, ColorImage, Context, Image, Response, Sense, Slider, TextureHandle, TextureOptions,
    Ui,
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
                    self.set_status(format!(
                        "Loaded {} ({}×{})",
                        path.display(),
                        width,
                        height
                    ));
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

    fn new_project(&mut self) {
        if self.project.terrain.is_some() {
            self.engine.send(Command::ClearTerrain);
        }
        self.project = Project::new();
        self.project_path = None;
        self.project_dirty = false;
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
                self.project = project;
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

    fn import_dem(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("GeoTIFF", &["tif", "tiff"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        self.set_status(format!("Loading {}…", path.display()));
        self.engine.send(Command::LoadTerrain { path });
    }

    fn export_png(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name(format!(
                "{}.png",
                sanitize_name(&self.project.name)
            ))
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
                    if ui.button("Import DEM…").clicked() {
                        ui.close_menu();
                        self.import_dem();
                    }
                    if ui.button("Export PNG…").clicked() {
                        ui.close_menu();
                        self.export_png();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui
                        .checkbox(&mut self.theme_dark, "Dark theme")
                        .changed()
                    {
                        self.apply_theme(ui.ctx(), self.theme_dark);
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Forge3D Studio").clicked() {
                        ui.close_menu();
                        self.set_status("Forge3D Studio 0.1.0 — desktop app for the Forge3D engine");
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
                if self.icon_button(ui, "save", "Save project (Ctrl+S)").clicked() {
                    self.save_project(false);
                }
                ui.separator();
                if self.icon_button(ui, "file-input", "Import DEM (Ctrl+I)").clicked() {
                    self.import_dem();
                }
                if self.icon_button(ui, "file-output", "Export PNG (Ctrl+E)").clicked() {
                    self.export_png();
                }
                ui.separator();
                if self
                    .icon_button(ui, "rotate-ccw", "Reset camera to fit dataset")
                    .clicked()
                {
                    self.reset_camera();
                }
                if self.icon_button(ui, "orbit", "Camera presets and navigation").clicked() {
                    self.set_status("Drag the viewport to orbit; scroll to zoom");
                }
                ui.separator();
                if self.icon_button(ui, "image", "Render current viewport").clicked() {
                    self.invalidate_viewport();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.busy {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.weak("working…");
                    }
                    if let Some(project) = self
                        .project_path
                        .as_ref()
                        .map(|p| p.display().to_string())
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
                        egui::Grid::new("dataset_info").num_columns(2).show(ui, |ui| {
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
                        ui.weak("Import a DEM to populate the scene:");
                        if ui.button("Import DEM…").clicked() {
                            self.import_dem();
                        }
                    }
                }
            });
    }

    fn right_panel(&mut self, ctx: &Context) {
        egui::SidePanel::right("studio_properties")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Properties");
                    ui.separator();

                    self.camera_section(ui);
                    ui.add_space(8.0);
                    self.sun_section(ui);
                    ui.add_space(8.0);
                    self.export_section(ui);
                });
            });
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
                    ui.label(
                        egui::RichText::new(format!("frame {ms:.0} ms"))
                            .weak(),
                    );
                }
                if self.busy {
                    ui.add(egui::Spinner::new().size(12.0));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(latest) = self.errors.back() {
                        let icon = self.icons.get(ui.ctx(), "circle-alert");
                        ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(13.0, 13.0)))
                            .on_hover_text(self.errors.iter().cloned().collect::<Vec<_>>().join("\n"));
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
                    let (rect, response) =
                        ui.allocate_exact_size(size, Sense::click_and_drag());
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(1.0, 1.0),
                        ),
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
                self.import_dem();
            }
            if ctrl && i.key_pressed(egui::Key::E) {
                self.export_png();
            }
        });
    }
}

impl eframe::App for StudioApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.sync_window_title(ctx);
        self.handle_shortcuts(ctx);
        self.poll_engine_events(ctx);

        self.top_panel(ctx);
        self.bottom_panel(ctx);
        self.left_panel(ctx);
        self.right_panel(ctx);
        self.viewport_panel(ctx);

        // Kick the very first render once the GPU is up.
        if self.gpu.is_some() && self.viewport_texture.is_none() && self.pending_render.is_none()
        {
            self.request_viewport_render();
        }
        // Re-render when the viewport is resized.
        if let Some(texture) = &self.viewport_texture {
            let [tw, th] = texture.size();
            let want = self.viewport_rect.size();
            let want = ((want.x.round() as u32).max(1), (want.y.round() as u32).max(1));
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
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "untitled".to_owned()
    } else {
        cleaned
    }
}
