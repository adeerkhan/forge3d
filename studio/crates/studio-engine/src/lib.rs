//! Forge3D Studio engine integration.
//!
//! The Studio worker embeds the CPython runtime (the engine checkout's
//! `.venv`) and drives forge3d through its certified, golden-tested terrain
//! entry point: `Session(window=False)` → `TerrainRenderer::
//! render_terrain_pbr_pom` → `Frame.save`. This is the exact pipeline the
//! upstream recipe goldens and determinism tests exercise, so Studio renders
//! are real engine renders by construction.
//!
//! Why embedded Python instead of the pure-Rust `TerrainScene`: the native
//! viewer path has drifted from the live pipeline upstream (bind-group
//! layout, depth attachment, camera plumbing) and only the Python-driven
//! path is maintained. The binary links libpython either way (see build.rs),
//! so embedding costs nothing extra. A pure-Rust viewport is GUI.md
//! milestone 5 and would start by repairing that drift.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use studio_project::{CameraParams, SunParams};

/// Command sent from the UI to the engine worker.
#[derive(Debug, Clone)]
pub enum Command {
    /// Load a DEM (GeoTIFF) and prepare the render session.
    LoadTerrain { path: PathBuf },
    /// Unload the current terrain dataset.
    ClearTerrain,
    SetCamera(CameraParams),
    SetSun(SunParams),
    /// Render one frame at the requested size and ship RGBA bytes back.
    RenderFrame { width: u32, height: u32 },
    /// Render at the requested size and write a PNG to disk.
    RenderToFile {
        width: u32,
        height: u32,
        path: PathBuf,
    },
    /// Stop the worker and join the thread.
    Shutdown,
}

/// Event sent from the engine worker back to the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// GPU adapter in use, as reported by the engine's device probe.
    Ready {
        adapter: String,
        backend: String,
        software_fallback: bool,
    },
    /// Terrain was loaded; carries the camera/sun defaults chosen for it.
    TerrainLoaded {
        path: PathBuf,
        width: u32,
        height: u32,
        height_min: f32,
        height_max: f32,
        camera: CameraParams,
        sun: SunParams,
    },
    TerrainCleared,
    /// A rendered frame's RGBA bytes (row-major, 4 bytes/pixel).
    FrameReady {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        render_ms: f32,
    },
    /// An offline PNG export finished successfully.
    ExportFinished {
        path: PathBuf,
        width: u32,
        height: u32,
        render_ms: f32,
    },
    /// A PNG export failed after rendering; nothing was written.
    ExportFailed { path: PathBuf, message: String },
    /// A user-facing operation failed. `context` says which operation.
    Error { context: String, message: String },
    /// A heavy operation started (`true`) or finished (`false`).
    Busy(bool),
}

/// Handle to the running engine worker.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Command>,
    join: std::sync::Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl EngineHandle {
    /// Spawn the worker thread. Python/GPU initialization happens on the
    /// worker, so this call returns immediately.
    pub fn spawn() -> (Self, Receiver<Event>) {
        let (cmd_tx, cmd_rx) = channel::<Command>();
        let (evt_tx, evt_rx) = channel::<Event>();
        // The embedded interpreter + terrain pipeline need a large stack
        // (WGSL compilation recurses deeply on the mega-shaders).
        let join = std::thread::Builder::new()
            .name("studio-engine".to_owned())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || worker_loop(evt_tx, cmd_rx))
            .expect("spawn studio-engine worker");
        (
            Self {
                tx: cmd_tx,
                join: std::sync::Arc::new(std::sync::Mutex::new(Some(join))),
            },
            evt_rx,
        )
    }

    /// Queue a command. Returns false if the worker already stopped.
    pub fn send(&self, command: Command) -> bool {
        self.tx.send(command).is_ok()
    }

    /// Stop the worker and wait for the thread to exit.
    pub fn shutdown(&self) {
        if self.send(Command::Shutdown) {
            if let Ok(mut slot) = self.join.lock() {
                if let Some(handle) = slot.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Embedded Python bootstrap
// ---------------------------------------------------------------------------

/// The Python bridge executed inside the embedded interpreter. It wraps the
/// certified upstream terrain flow and keeps one persistent session across
/// frames.
const BRIDGE_SOURCE: &str = r##"
import numpy as np
import forge3d as f3d
from forge3d.terrain_params import make_terrain_params_config

_SOFTWARE_TOKENS = ("lavapipe", "llvmpipe", "swiftshader", "warp", "basic render driver")
_CMAP_STOPS = [
    (0.0, "#18391f"),
    (0.40, "#4e7c35"),
    (0.72, "#9a8552"),
    (1.0, "#f3f5f9"),
]

def studio_probe():
    probe = f3d.device_probe()
    name = str(probe.get("name", ""))
    return {
        "ok": str(probe.get("status")) == "ok",
        "name": name,
        "backend": str(probe.get("backend", "")),
        "device_type": str(probe.get("device_type", "")),
        "software": any(t in name.lower() for t in _SOFTWARE_TOKENS),
    }

def studio_make_session(hdr_path):
    global _session, _renderer, _material, _ibl, _overlay
    _session = f3d.Session(window=False)
    _renderer = f3d.TerrainRenderer(_session)
    _material = f3d.MaterialSet.terrain_default()
    _ibl = f3d.IBL.from_hdr(str(hdr_path), intensity=1.0)
    cmap = f3d.Colormap1D.from_stops(stops=_CMAP_STOPS, domain=(0.0, 1.0))
    _overlay = f3d.OverlayLayer.from_colormap1d(cmap, strength=1.0)

def studio_render(heightmap, out_png, width, height, camera, sun, terrain_span, domain):
    config = make_terrain_params_config(
        size_px=(int(width), int(height)),
        render_scale=1.0,
        terrain_span=float(terrain_span),
        msaa_samples=1,
        z_scale=1.35,
        exposure=1.0,
        domain=(float(domain[0]), float(domain[1])),
        albedo_mode="colormap",
        colormap_strength=1.0,
        ibl_enabled=True,
        light_azimuth_deg=float(sun["azimuth_deg"]),
        light_elevation_deg=float(sun["elevation_deg"]),
        sun_intensity=float(sun["intensity"]),
        cam_radius=float(camera["radius"]),
        cam_phi_deg=float(camera["phi_deg"]),
        cam_theta_deg=float(camera["theta_deg"]),
        fov_y_deg=float(camera["fov_deg"]),
        camera_mode="screen",
        overlays=[_overlay],
    )
    frame = _renderer.render_terrain_pbr_pom(
        material_set=_material,
        env_maps=_ibl,
        params=f3d.TerrainRenderParams(config),
        heightmap=heightmap,
        target=None,
        water_mask=None,
    )
    frame.save(str(out_png))
"##;

/// Minimal Radiance HDR feeding `IBL.from_hdr` — an 8x4 gradient is enough
/// for image-based lighting without shipping an HDR asset.
const PROBE_HDR_LINES: &[&str] = &["#?RADIANCE", "FORMAT=32-bit_rle_rgbe", "", "-Y 4 +X 8"];

fn write_probe_hdr(path: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)
        .map_err(|err| format!("creating {}: {err}", path.display()))?;
    for line in PROBE_HDR_LINES {
        writeln!(file, "{line}").map_err(|err| err.to_string())?;
    }
    for y in 0..4u32 {
        for x in 0..8u32 {
            let r = (x * 255 / 7) as u8;
            let g = (y * 255 / 3) as u8;
            // Radiance unpacked RGBE: 3 RGB bytes + exponent byte.
            file.write_all(&[r, g, 164, 128])
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

/// Locate the Python environment for the embedded interpreter:
/// `(python_home, site_packages)`.
///
/// Resolution order:
/// 1. `FORGE3D_PYTHON_HOME` / `FORGE3D_PYTHON_SITE` env overrides,
/// 2. the engine checkout's `.venv` (pyvenv.cfg → base prefix).
fn locate_python_env() -> Result<(PathBuf, PathBuf), String> {
    if let (Ok(home), Ok(site)) = (
        std::env::var("FORGE3D_PYTHON_HOME"),
        std::env::var("FORGE3D_PYTHON_SITE"),
    ) {
        return Ok((PathBuf::from(home), PathBuf::from(site)));
    }

    let venv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.venv");
    let pyvenv_cfg = venv.join("pyvenv.cfg");
    let cfg = std::fs::read_to_string(&pyvenv_cfg)
        .map_err(|err| format!("reading {}: {err}", pyvenv_cfg.display()))?;
    let home_line = cfg
        .lines()
        .find(|l| l.trim_start().starts_with("home"))
        .ok_or_else(|| format!("no 'home' entry in {}", pyvenv_cfg.display()))?;
    let raw = home_line
        .split('=')
        .nth(1)
        .ok_or_else(|| format!("malformed 'home' entry in {}", pyvenv_cfg.display()))?;
    let base = PathBuf::from(raw.trim());
    // pyvenv.cfg 'home' points at the base prefix's bin directory on POSIX.
    let base = if base.ends_with("bin") {
        base.parent().unwrap_or(&base).to_path_buf()
    } else {
        base
    };

    let lib_dir = venv.join("lib");
    let mut site_packages = None;
    if let Ok(entries) = std::fs::read_dir(&lib_dir) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("site-packages");
            if candidate.is_dir() {
                site_packages = Some(candidate);
                break;
            }
        }
    }
    let site_packages =
        site_packages.ok_or_else(|| format!("no site-packages under {}", lib_dir.display()))?;

    // Editable installs (forge3d is one) register themselves through .pth
    // files, which the embedded interpreter will not process for
    // PYTHONPATH-provided directories. Expand them here.
    let mut python_path = site_packages.clone().into_os_string();
    if let Ok(entries) = std::fs::read_dir(&site_packages) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("pth") {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with("import ") {
                    continue;
                }
                python_path.push(":");
                python_path.push(line);
            }
        }
    }

    Ok((base, python_path.into()))
}

struct PyEngine {
    bridge: Py<PyDict>,
    heightmap: Option<Py<pyo3::types::PyAny>>,
    camera: CameraParams,
    sun: SunParams,
    terrain_visible: bool,
    dirty: bool,
    domain: (f32, f32),
    terrain_span: f32,
    scratch_png: PathBuf,
}

fn decode_geotiff(path: &Path) -> Result<(Vec<f32>, u32, u32, (f32, f32)), String> {
    let file =
        std::fs::File::open(path).map_err(|err| format!("opening {}: {err}", path.display()))?;
    let mut decoder = tiff::decoder::Decoder::new(file)
        .map_err(|err| format!("decoding {}: {err}", path.display()))?;
    let (width, height) = decoder
        .dimensions()
        .map_err(|err| format!("reading dimensions of {}: {err}", path.display()))?;
    let image = decoder
        .read_image()
        .map_err(|err| format!("reading pixels of {}: {err}", path.display()))?;
    let data: Vec<f32> = match image {
        tiff::decoder::DecodingResult::F32(data) => data,
        tiff::decoder::DecodingResult::F64(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::U8(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::U16(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::U32(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::I8(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::I16(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::I32(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::U64(data) => data.iter().map(|&v| v as f32).collect(),
        tiff::decoder::DecodingResult::I64(data) => data.iter().map(|&v| v as f32).collect(),
    };
    if data.len() != (width as usize) * (height as usize) {
        return Err(format!(
            "{}: expected {} pixels, decoded {}",
            path.display(),
            width as usize * height as usize,
            data.len()
        ));
    }
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    for &v in &data {
        if v.is_finite() {
            min = min.min(v);
            max = max.max(v);
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return Err(format!("{}: no finite height samples", path.display()));
    }
    // Nodata holes are flattened to the minimum so shading stays defined.
    let data: Vec<f32> = data
        .iter()
        .map(|&v| if v.is_finite() { v } else { min })
        .collect();
    Ok((data, width, height, (min, max)))
}

fn worker_loop(events: Sender<Event>, commands: Receiver<Command>) {
    // Environment must be configured before the interpreter starts.
    let (py_home, py_site) = match locate_python_env() {
        Ok(env) => env,
        Err(err) => {
            let _ = events.send(Event::Error {
                context: "Python environment".to_owned(),
                message: format!(
                    "{err}. Set FORGE3D_PYTHON_HOME and FORGE3D_PYTHON_SITE to a Python \
                     environment with forge3d + numpy installed (see studio/README.md)."
                ),
            });
            return;
        }
    };
    std::env::set_var("PYTHONHOME", &py_home);
    std::env::set_var("PYTHONPATH", &py_site);

    // Embed CPython. The process is already linked against libpython
    // (build.rs); pyo3's extension-module configuration means we must
    // initialize the interpreter manually.
    unsafe {
        pyo3::ffi::Py_InitializeEx(0);
        pyo3::ffi::PyEval_SaveThread();
    }

    if let Err(err) = Python::with_gil(|py| run_worker(py, &events, &commands)) {
        let _ = events.send(Event::Error {
            context: "Engine worker".to_owned(),
            message: err,
        });
    }
}

fn call_bridge<'py>(
    _py: Python<'py>,
    bridge: &Bound<'py, PyDict>,
    name: &str,
    kwargs: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, pyo3::types::PyAny>> {
    let func = bridge
        .get_item(name)?
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err(format!("missing {name}")))?;
    func.call((), Some(kwargs))
}

fn run_worker(
    py: Python<'_>,
    events: &Sender<Event>,
    commands: &Receiver<Command>,
) -> Result<(), String> {
    // Import the bridge: a namespace dict holding the certified upstream
    // terrain flow (see BRIDGE_SOURCE).
    let bridge = PyDict::new_bound(py);
    bridge
        .set_item("__name__", "studio_bridge")
        .map_err(|err| format!("bridge setup: {err}"))?;
    py.run_bound(BRIDGE_SOURCE, Some(&bridge), None)
        .map_err(|err| format!("loading engine bridge: {err}"))?;

    let probe = call_bridge(py, &bridge, "studio_probe", &PyDict::new_bound(py))
        .map_err(|err| format!("device probe: {err}"))?;
    let get_str = |key: &str| -> String {
        probe
            .get_item(key)
            .ok()
            .and_then(|v| v.extract::<String>().ok())
            .unwrap_or_default()
    };
    let get_bool = |key: &str| -> bool {
        probe
            .get_item(key)
            .ok()
            .and_then(|v| v.extract::<bool>().ok())
            .unwrap_or(false)
    };
    if !get_bool("ok") {
        return Err(format!(
            "device probe failed: adapter '{}' backend '{}' is not usable",
            get_str("name"),
            get_str("backend")
        ));
    }
    let _ = events.send(Event::Ready {
        adapter: get_str("name"),
        backend: get_str("backend"),
        software_fallback: get_bool("software"),
    });

    // Persistent IBL environment: a tiny procedural HDR is enough for
    // image-based lighting without shipping an HDR asset.
    let hdr_path = std::env::temp_dir().join("forge3d-studio-ibl.hdr");
    write_probe_hdr(&hdr_path)?;
    let session_kwargs = PyDict::new_bound(py);
    session_kwargs
        .set_item("hdr_path", hdr_path.to_string_lossy().into_owned())
        .unwrap();
    call_bridge(py, &bridge, "studio_make_session", &session_kwargs)
        .map_err(|err| format!("creating terrain session: {err}"))?;

    let mut engine = PyEngine {
        bridge: bridge.clone().unbind(),
        heightmap: None,
        camera: CameraParams::default(),
        sun: SunParams::default(),
        terrain_visible: true,
        dirty: true,
        domain: (0.0, 1.0),
        terrain_span: 3.0,
        scratch_png: std::env::temp_dir().join("forge3d-studio-frame.png"),
    };

    while let Ok(command) = commands.recv() {
        match command {
            Command::Shutdown => break,
            Command::LoadTerrain { path } => {
                let _ = events.send(Event::Busy(true));
                match load_terrain(py, &mut engine, &path) {
                    Ok(event) => {
                        engine.dirty = true;
                        let _ = events.send(event);
                    }
                    Err(message) => {
                        let _ = events.send(Event::Error {
                            context: format!("Loading terrain {}", path.display()),
                            message,
                        });
                    }
                }
                let _ = events.send(Event::Busy(false));
            }
            Command::ClearTerrain => {
                engine.heightmap = None;
                engine.dirty = true;
                let _ = events.send(Event::TerrainCleared);
            }
            Command::SetCamera(camera) => {
                engine.camera = camera;
                engine.dirty = true;
            }
            Command::SetSun(sun) => {
                engine.sun = sun;
                engine.dirty = true;
            }
            Command::RenderFrame { width, height } => {
                if engine.dirty {
                    engine.dirty = false;
                    render_and_decode(py, &mut engine, events, width, height, None);
                }
            }
            Command::RenderToFile {
                width,
                height,
                path,
            } => {
                let _ = events.send(Event::Busy(true));
                let started = Instant::now();
                let width = width.max(1);
                let height = height.max(1);
                match render_via_bridge(py, &mut engine, width, height, &path) {
                    Ok(()) => {
                        let _ = events.send(Event::ExportFinished {
                            path,
                            width,
                            height,
                            render_ms: started.elapsed().as_secs_f32() * 1000.0,
                        });
                    }
                    Err(message) => {
                        let _ = events.send(Event::ExportFailed { path, message });
                    }
                }
                let _ = events.send(Event::Busy(false));
            }
        }
    }
    Ok(())
}

fn load_terrain(py: Python<'_>, engine: &mut PyEngine, path: &Path) -> Result<Event, String> {
    let (data, width, height, domain) = decode_geotiff(path)?;
    let array = ndarray::Array2::from_shape_vec((height as usize, width as usize), data)
        .map_err(|err| format!("reshaping heightmap: {err}"))?;
    let py_array = numpy::PyArray2::from_owned_array_bound(py, array);

    // World-scale defaults: the terrain shader spans `terrain_span` world
    // units horizontally; a relief-to-span ratio of 1:6 frames a typical DEM
    // without georeferencing metadata. Users adjust from the properties panel.
    let domain_range = (domain.1 - domain.0).max(1.0);
    let terrain_span = domain_range * 6.0;
    let default_camera = CameraParams {
        phi_deg: 135.0,
        theta_deg: 58.0,
        radius: terrain_span * 1.65,
        fov_deg: 54.0,
    };
    engine.heightmap = Some(py_array.into_any().unbind());
    engine.domain = domain;
    engine.terrain_span = terrain_span;
    engine.camera = default_camera;

    Ok(Event::TerrainLoaded {
        path: path.to_path_buf(),
        width,
        height,
        height_min: domain.0,
        height_max: domain.1,
        camera: default_camera,
        sun: engine.sun,
    })
}

/// Render via the bridge (engine state), then decode the output PNG into
/// RGBA bytes and ship a `FrameReady` event. `force_out` is used by exports;
/// viewport renders reuse a scratch file.
fn render_and_decode(
    py: Python<'_>,
    engine: &mut PyEngine,
    events: &Sender<Event>,
    width: u32,
    height: u32,
    force_out: Option<PathBuf>,
) {
    if engine.heightmap.is_none() {
        return;
    }
    let out = force_out.clone().unwrap_or_else(|| engine.scratch_png.clone());
    let started = Instant::now();
    let result = render_via_bridge(py, engine, width, height, &out).and_then(|()| {
        image::open(&out)
            .map_err(|err| format!("decoding rendered PNG: {err}"))
            .map(|img| img.to_rgba8().into_raw())
    });
    match result {
        Ok(rgba) => {
            let _ = events.send(Event::FrameReady {
                width,
                height,
                rgba,
                render_ms: started.elapsed().as_secs_f32() * 1000.0,
            });
        }
        Err(message) => {
            let _ = events.send(Event::Error {
                context: format!("Frame render ({width}x{height})"),
                message,
            });
        }
    }
}

fn render_via_bridge(
    py: Python<'_>,
    engine: &mut PyEngine,
    width: u32,
    height: u32,
    out: &Path,
) -> Result<(), String> {
    let Some(heightmap) = engine.heightmap.as_ref() else {
        return Err("no terrain loaded".to_owned());
    };
    if !engine.terrain_visible {
        return Err("terrain layer is hidden".to_owned());
    }

    let camera = PyDict::new_bound(py);
    camera.set_item("phi_deg", engine.camera.phi_deg).unwrap();
    camera.set_item("theta_deg", engine.camera.theta_deg).unwrap();
    camera.set_item("radius", engine.camera.radius).unwrap();
    camera.set_item("fov_deg", engine.camera.fov_deg).unwrap();
    let sun = PyDict::new_bound(py);
    sun.set_item("azimuth_deg", engine.sun.azimuth_deg).unwrap();
    sun.set_item("elevation_deg", engine.sun.elevation_deg).unwrap();
    sun.set_item("intensity", engine.sun.intensity).unwrap();

    let bridge = engine.bridge.bind(py);
    let kwargs = PyDict::new_bound(py);
    kwargs.set_item("heightmap", heightmap.bind(py)).unwrap();
    kwargs
        .set_item("out_png", out.to_string_lossy().into_owned())
        .unwrap();
    kwargs.set_item("width", width).unwrap();
    kwargs.set_item("height", height).unwrap();
    kwargs.set_item("camera", camera).unwrap();
    kwargs.set_item("sun", sun).unwrap();
    kwargs.set_item("terrain_span", engine.terrain_span).unwrap();
    kwargs
        .set_item("domain", vec![engine.domain.0, engine.domain.1])
        .unwrap();

    call_bridge(py, &bridge, "studio_render", &kwargs)
        .map(|_| ())
        .map_err(|err| format!("engine render failed: {err}"))
}
