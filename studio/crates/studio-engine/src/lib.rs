//! Forge3D Studio engine integration — Bridge v2.
//!
//! The Studio worker embeds the CPython runtime (the engine checkout's
//! `.venv`) and talks to forge3d exclusively through a generic JSON-op
//! dispatcher (`studio_dispatch`) implemented in Python over the engine's
//! certified public entry points. Every capability Studio exposes is one op
//! in the dispatcher's `_OPS` table; the Rust side never calls engine
//! internals directly. This is GUI.md Part III workstream W0: the same
//! dispatcher carries terrain rendering, MapScene cartography, presets,
//! colormaps, and GIS metadata, so new features are usually one new Python
//! handler plus one UI surface.
//!
//! Why embedded Python: the maintained, upstream-verified rendering path is
//! the Python-driven one (every recipe golden and determinism test exercises
//! it). The binary links libpython anyway (see build.rs). A pure-Rust
//! viewport is GUI.md milestone M10 after the native-path repair (M9/R1).

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
    /// Load a DEM through the engine's own loader (rasterio, nodata fill).
    LoadTerrain { path: PathBuf },
    /// Unload the current terrain dataset.
    ClearTerrain,
    SetCamera(CameraParams),
    SetSun(SunParams),
    /// Render one terrain frame at the requested size and ship RGBA bytes.
    RenderFrame { width: u32, height: u32 },
    /// Render terrain at the requested size and write a PNG to disk.
    RenderToFile {
        width: u32,
        height: u32,
        path: PathBuf,
    },
    /// Run a generic engine op through the Bridge v2 dispatcher.
    ///
    /// Ops are stable string tags (see `_OPS` in BRIDGE_SOURCE); params are
    /// JSON, results come back as JSON in `Event::JobDone`. The worker
    /// serializes jobs; there is no concurrency inside the engine.
    Call {
        job: u64,
        op: String,
        params: serde_json::Value,
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
    /// A rendered terrain frame's RGBA bytes (row-major, 4 bytes/pixel).
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
    /// A generic engine op completed. `result` is the op's JSON result.
    JobDone {
        job: u64,
        op: String,
        result: serde_json::Value,
    },
    /// A generic engine op failed.
    JobFailed {
        job: u64,
        op: String,
        message: String,
    },
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
    next_job: std::sync::Arc<std::sync::atomic::AtomicU64>,
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
                next_job: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
            },
            evt_rx,
        )
    }

    /// Queue a command. Returns false if the worker already stopped.
    pub fn send(&self, command: Command) -> bool {
        self.tx.send(command).is_ok()
    }

    /// Queue a generic engine op and return its job id. Results arrive as
    /// [`Event::JobDone`] / [`Event::JobFailed`].
    pub fn call(&self, op: &str, params: serde_json::Value) -> u64 {
        let job = self
            .next_job
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.send(Command::Call {
            job,
            op: op.to_owned(),
            params,
        });
        job
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

/// The Bridge v2 dispatcher executed inside the embedded interpreter.
///
/// `_OPS` maps stable op tags to handlers over forge3d's public API. Handlers
/// return plain JSON-serializable dicts. `studio_dispatch` catches everything
/// (returning the traceback text) so worker failures are always structured.
const BRIDGE_SOURCE: &str = r##"
import json
import os
import sys
import traceback

import numpy as np
import forge3d as f3d
from forge3d.terrain_params import make_terrain_params_config

# Embedded-interpreter repair: forge3d's terrain runtime gate proves the
# runtime by spawning `sys.executable` as a subprocess. An embedded
# interpreter reports whatever interpreter pyo3 linked against — which does
# not own the forge3d environment — so always point it at the host Python
# injected by the Rust worker (the venv that owns site-packages).
_host = globals().get("_studio_host_python")
if _host and os.path.exists(_host):
    sys.executable = _host

_SOFTWARE_TOKENS = ("lavapipe", "llvmpipe", "swiftshader", "warp", "basic render driver")
_DEFAULT_CMAP_STOPS = [
    (0.0, "#18391f"),
    (0.40, "#4e7c35"),
    (0.72, "#9a8552"),
    (1.0, "#f3f5f9"),
]

# --- persistent engine state (one session across all ops) -------------------
_session = None
_renderer = None
_material = None
_ibl = None
_overlay = None
_overlay_cmap_key = None
_heightmap = None  # np.ndarray float32 [H, W]
_domain = (0.0, 1.0)


def studio_probe(p=None):
    probe = f3d.device_probe()
    name = str(probe.get("name", ""))
    return {
        "ok": str(probe.get("status")) == "ok",
        "name": name,
        "backend": str(probe.get("backend", "")),
        "device_type": str(probe.get("device_type", "")),
        "software": any(t in name.lower() for t in _SOFTWARE_TOKENS),
    }


def _ensure_session(hdr_path):
    """Create the persistent terrain render session on first use."""
    global _session, _renderer, _material, _ibl
    if _renderer is not None:
        return
    _session = f3d.Session(window=False)
    _renderer = f3d.TerrainRenderer(_session)
    _material = f3d.MaterialSet.terrain_default()
    _ibl = f3d.IBL.from_hdr(str(hdr_path), intensity=1.0)


def _ensure_overlay(cmap_name):
    """Build (and memoize) the colormap overlay used for terrain frames."""
    global _overlay, _overlay_cmap_key
    key = cmap_name or "__default__"
    if _overlay is not None and _overlay_cmap_key == key:
        return _overlay
    if cmap_name:
        cmap = f3d.get_colormap(cmap_name if ":" in cmap_name else "forge3d:" + cmap_name)
        rows = np.clip((cmap.rgba * 255.0) + 0.5, 0, 255).astype(np.uint8)
        stops = [
            (i / (len(rows) - 1), "#{:02x}{:02x}{:02x}".format(r, g, b))
            for i, (r, g, b, _a) in enumerate(rows)
        ]
    else:
        stops = _DEFAULT_CMAP_STOPS
    cmap1d = f3d.Colormap1D.from_stops(stops=stops, domain=(0.0, 1.0))
    _overlay = f3d.OverlayLayer.from_colormap1d(cmap1d, strength=1.0)
    _overlay_cmap_key = key
    return _overlay


# --- ops: terrain ------------------------------------------------------------

def _op_terrain_load(p):
    global _heightmap, _domain
    dem = f3d.io.load_dem(p["path"])
    _heightmap = np.ascontiguousarray(dem.data, dtype=np.float32)
    _domain = (float(dem.domain[0]), float(dem.domain[1]))
    return {
        "width": int(_heightmap.shape[1]),
        "height": int(_heightmap.shape[0]),
        "domain": [_domain[0], _domain[1]],
        "crs": dem.crs,
        "bounds": list(dem.bounds) if dem.bounds else None,
        "resolution": list(dem.resolution) if dem.resolution else None,
        "nodata_value": dem.nodata_value,
        "stats": {k: float(v) for k, v in (dem.stats or {}).items()},
    }


def _op_terrain_info(p):
    dem = f3d.io.load_dem(p["path"])
    return {
        "width": int(dem.data.shape[1]),
        "height": int(dem.data.shape[0]),
        "domain": [float(dem.domain[0]), float(dem.domain[1])],
        "crs": dem.crs,
        "bounds": list(dem.bounds) if dem.bounds else None,
        "nodata_value": dem.nodata_value,
    }


def _op_terrain_render(p):
    if _heightmap is None:
        raise RuntimeError("no terrain loaded")
    _ensure_session(p["hdr_path"])
    overlay = _ensure_overlay(p.get("colormap"))
    camera = p["camera"]
    sun = p["sun"]
    domain = p.get("domain") or [_domain[0], _domain[1]]
    span = float(p.get("terrain_span") or max(domain[1] - domain[0], 1.0) * 6.0)
    config = make_terrain_params_config(
        size_px=(int(p["width"]), int(p["height"])),
        render_scale=1.0,
        terrain_span=span,
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
        overlays=[overlay],
    )
    frame = _renderer.render_terrain_pbr_pom(
        material_set=_material,
        env_maps=_ibl,
        params=f3d.TerrainRenderParams(config),
        heightmap=_heightmap,
        target=None,
        water_mask=None,
    )
    frame.save(str(p["out_png"]))
    return {"png": str(p["out_png"]), "terrain_span": span}


def _op_terrain_clear(p):
    global _heightmap
    _heightmap = None
    return {"cleared": True}


# --- ops: MapScene cartography ------------------------------------------------

def _op_mapscene_templates(p):
    return {"templates": ["terrain", "terrain_raster", "terrain_vector_labels"]}


def _op_mapscene_template(p):
    """Build a recipe the same way the canonical examples do, and serialize it.

    The construction mirrors examples/mapscene_terrain_raster.py and
    examples/mapscene_vector_labels.py (the golden-baseline call sequences),
    so recipes produced here are the example workflows by construction.
    """
    name = p["template"]
    width = int(p.get("width", 640))
    height = int(p.get("height", 420))
    dem_path = p.get("dem_path")
    if not dem_path:
        raise RuntimeError("template requires dem_path")

    # Terrain world metadata. GeoTIFF goes through the engine's own loader;
    # .npy terrain sources (which MapScene accepts natively) load via numpy.
    try:
        dem = f3d.io.load_dem(dem_path)
        tw, th = int(dem.data.shape[1]), int(dem.data.shape[0])
        crs_default = dem.crs
    except Exception:
        arr = np.load(dem_path)
        tw, th = int(arr.shape[1]), int(arr.shape[0])
        crs_default = None
    # CRS comes from the dataset's own georeferencing; an explicit override
    # wins. When neither exists the engine's validation reports missing_crs —
    # Studio surfaces that honestly instead of inventing a CRS.
    crs = p.get("crs") or crs_default
    terrain = f3d.TerrainSource(
        path=str(dem_path),
        crs=crs,
        metadata={"width": tw, "height": th, "source_id": "studio-terrain"},
    )
    camera = f3d.OrbitCamera(
        target=(0.0, 0.0, 0.0),
        distance=float(p.get("distance", 1.2)),
        azimuth_deg=float(p.get("azimuth_deg", 25.0)),
        elevation_deg=float(p.get("elevation_deg", 45.0)),
        fov_deg=float(p.get("fov_deg", 45.0)),
    )
    output = f3d.OutputSpec(width=width, height=height, format="png")

    if name == "terrain":
        layers = []
    elif name == "terrain_raster":
        raster_path = p.get("raster_path")
        if not raster_path:
            raise RuntimeError("terrain_raster template requires raster_path")
        layers = [
            f3d.RasterOverlay(
                layer_id="ortho",
                path=str(raster_path),
                crs=crs,
                opacity=float(p.get("opacity", 0.8)),
                metadata={"source_id": "studio-raster-overlay"},
            )
        ]
    elif name == "terrain_vector_labels":
        features = p.get("features") or [
            {
                "id": "line-1",
                "geometry": {"type": "LineString", "coordinates": [[0.1, 0.2], [0.9, 0.75]]},
                "properties": {"class": "primary"},
            }
        ]
        labels = p.get("labels") or [
            {
                "id": "label-1",
                "kind": "point",
                "text": str(p.get("label_text", "Label")),
                "geometry": {"type": "Point", "coordinates": [tw * 0.5, th * 0.5, 0.0]},
                "priority_class": "cities",
            }
        ]
        layers = [
            f3d.VectorOverlay(
                layer_id="lines",
                crs=crs,
                features=features,
                style={"version": 8, "layers": [
                    {"id": "lines", "type": "line", "paint": {"line-color": "#ffffff"}}
                ]},
                metadata={"source_id": "studio-vector-overlay"},
            ),
            f3d.LabelLayer(layer_id="labels", labels=labels),
        ]
    else:
        raise RuntimeError("unknown template: " + str(name))

    scene = f3d.MapScene(
        terrain=terrain,
        camera=camera,
        lighting=f3d.LightingPreset(name="daylight", intensity=1.1),
        output=output,
        layers=layers,
        reproducibility_profile=f3d.ReproducibilityProfile(seed=404),
    )
    return {"recipe": scene.to_dict()["recipe"]}


def _op_mapscene_render(p):
    """Render a recipe JSON through MapScene.render (the certified path)."""
    recipe_data = json.loads(p["recipe_json"])
    recipe = f3d.MapScene._recipe_from_dict(recipe_data)
    scene = f3d.MapScene(recipe=recipe)
    try:
        report = scene.render(
            path=str(p["out_png"]),
            certificate=bool(p.get("certificate", False)),
        )
    except Exception as exc:
        # Surface the engine's validation report with the failure: the UI
        # shows these codes next to the recipe.
        dbg = {"error": str(exc)}
        try:
            vr = scene.validate()
            dbg["validation_status"] = str(vr.status)
            dbg["validation"] = [
                f"{getattr(d, 'severity', '?')}/{getattr(d, 'code', '?')}: "
                + str(getattr(d, 'message', ''))[:160]
                for d in vr.diagnostics
            ][:8]
        except Exception as v_err:
            dbg["validation_err"] = str(v_err)
        raise RuntimeError(json.dumps(dbg, default=str)) from exc
    diagnostics = []
    try:
        diagnostics = [str(getattr(d, "code", d)) for d in report.diagnostics]
    except Exception:
        pass
    metadata = dict(scene.last_render_metadata or {})
    # Drop volatile timing keys so results are comparable across runs.
    for key in ("offline_accumulation_ms", "timing_source"):
        metadata.pop(key, None)
    return {
        "png": str(p["out_png"]),
        "status": str(report.status),
        "backend": str(scene.last_render_backend),
        "diagnostics": diagnostics,
        "metadata": metadata,
    }


def _op_mapscene_validate(p):
    recipe_data = json.loads(p["recipe_json"])
    recipe = f3d.MapScene._recipe_from_dict(recipe_data)
    scene = f3d.MapScene(recipe=recipe)
    report = scene.validate()
    return {"status": str(report.status)}


# --- ops: catalog data --------------------------------------------------------

def _op_colormaps_list(p):
    return {"colormaps": sorted(f3d.colormaps.available())}


def _op_presets_list(p):
    import forge3d.presets as presets

    return {"presets": presets.available()}


def _op_datasets_list(p):
    """Enumerate bundled sample datasets without downloading anything."""
    entries = []
    try:
        for entry in f3d.datasets.list_datasets():
            entries.append(str(entry if isinstance(entry, str) else getattr(entry, "name", entry)))
    except Exception:
        pass
    return {"datasets": entries}


_OPS = {
    "probe": studio_probe,
    "terrain.load": _op_terrain_load,
    "terrain.info": _op_terrain_info,
    "terrain.render": _op_terrain_render,
    "terrain.clear": _op_terrain_clear,
    "mapscene.templates": _op_mapscene_templates,
    "mapscene.template": _op_mapscene_template,
    "mapscene.render": _op_mapscene_render,
    "mapscene.validate": _op_mapscene_validate,
    "colormaps.list": _op_colormaps_list,
    "presets.list": _op_presets_list,
    "datasets.list": _op_datasets_list,
}


def studio_dispatch(op, params_json):
    """Run one engine op. Always returns a JSON string:
    {"ok": true, "result": ...} or {"ok": false, "error": ..., "traceback": ...}.
    """
    try:
        params = json.loads(params_json) if params_json else {}
        handler = _OPS.get(op)
        if handler is None:
            raise RuntimeError("unknown op: " + str(op))
        result = handler(params)
        return json.dumps({"ok": True, "result": result}, default=str)
    except Exception as exc:  # noqa: BLE001 - the boundary converts everything
        return json.dumps({
            "ok": False,
            "error": str(exc),
            "traceback": traceback.format_exc(),
        })
"##;

/// Locate the Python environment for the embedded interpreter:
/// `(python_home, python_path)` where `python_path` is a colon-joined
/// PYTHONPATH value (site-packages plus expanded .pth entries).
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

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// The venv interpreter behind a site-packages path:
/// `<site>/../../bin/python3`. Used to repair `sys.executable` inside the
/// embedded interpreter so forge3d's subprocess-based runtime gates work.
fn locate_host_python(site_packages: &Path) -> PathBuf {
    site_packages
        .parent() // lib/python3.X
        .and_then(|p| p.parent()) // venv/lib
        .and_then(|p| p.parent()) // venv root
        .map(|venv| venv.join("bin").join("python3"))
        .unwrap_or_else(|| PathBuf::from("python3"))
}

struct WorkerState {
    bridge: Py<PyDict>,
    hdr_path: PathBuf,
    scratch_png: PathBuf,
    camera: CameraParams,
    sun: SunParams,
    terrain_visible: bool,
    dirty: bool,
    has_terrain: bool,
    domain: (f32, f32),
}

impl WorkerState {
    fn default_camera(domain: (f32, f32)) -> CameraParams {
        // World-scale defaults: the terrain shader spans `terrain_span` world
        // units horizontally; a relief-to-span ratio of 1:6 frames a typical
        // DEM without georeferencing metadata. Users adjust from the panel.
        let span = (domain.1 - domain.0).max(1.0) * 6.0;
        CameraParams {
            phi_deg: 135.0,
            theta_deg: 58.0,
            radius: span * 1.65,
            fov_deg: 54.0,
        }
    }
}

/// Run one dispatcher op and parse its JSON envelope.
fn dispatch(
    py: Python<'_>,
    bridge: &Bound<'_, PyDict>,
    op: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let kwargs = PyDict::new_bound(py);
    kwargs.set_item("op", op).map_err(|e| e.to_string())?;
    kwargs
        .set_item(
            "params_json",
            serde_json::to_string(params).unwrap_or_else(|_| "{}".into()),
        )
        .map_err(|e| e.to_string())?;
    let raw = call_bridge(py, bridge, "studio_dispatch", &kwargs)
        .map_err(|e| format!("dispatch call: {e}"))?;
    let text: String =
        raw.extract().map_err(|e| format!("dispatch returned non-string: {e}"))?;
    let envelope: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("dispatch envelope parse: {e}"))?;
    if envelope.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        Ok(envelope.get("result").cloned().unwrap_or(serde_json::Value::Null))
    } else {
        let mut message = envelope
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown dispatcher error")
            .to_owned();
        if let Some(tb) = envelope.get("traceback").and_then(|v| v.as_str()) {
            // Keep the tail of the traceback: the failing engine frames.
            let tail: Vec<&str> = tb.lines().rev().take(8).collect();
            message.push_str("\n--- engine traceback (tail) ---\n");
            for line in tail.into_iter().rev() {
                message.push_str(line);
                message.push('\n');
            }
        }
        Err(message)
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
    // initialize the interpreter manually. The init is process-wide: multiple
    // worker threads (tests spawn several) share the one interpreter, while
    // each worker executes its own bridge namespace dict for its state.
    static PY_INIT: std::sync::Once = std::sync::Once::new();
    PY_INIT.call_once(|| unsafe {
        pyo3::ffi::Py_InitializeEx(0);
        pyo3::ffi::PyEval_SaveThread();
    });

    let host_python = locate_host_python(Path::new(
        py_site.to_string_lossy().split(':').next().unwrap_or(""),
    ));
    if let Err(err) = Python::with_gil(|py| run_worker(py, &events, &commands, &host_python)) {
        let _ = events.send(Event::Error {
            context: "Engine worker".to_owned(),
            message: err,
        });
    }
}

fn run_worker(
    py: Python<'_>,
    events: &Sender<Event>,
    commands: &Receiver<Command>,
    host_python: &Path,
) -> Result<(), String> {
    // Import the bridge: a namespace dict holding the dispatcher (see
    // BRIDGE_SOURCE). The dict must be exec globals so module-level imports
    // resolve inside the defined functions.
    let bridge = PyDict::new_bound(py);
    bridge
        .set_item("__name__", "studio_bridge")
        .map_err(|err| format!("bridge setup: {err}"))?;
    bridge
        .set_item("_studio_host_python", host_python.to_string_lossy().into_owned())
        .map_err(|err| format!("bridge setup: {err}"))?;
    py.run_bound(BRIDGE_SOURCE, Some(&bridge), None)
        .map_err(|err| format!("loading engine bridge: {err}"))?;

    let probe = dispatch(py, &bridge, "probe", &serde_json::json!({}))
        .map_err(|err| format!("device probe: {err}"))?;
    let _ = events.send(Event::Ready {
        adapter: probe
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown adapter")
            .to_owned(),
        backend: probe
            .get("backend")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        software_fallback: probe
            .get("software")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    });

    let hdr_path = std::env::temp_dir().join("forge3d-studio-ibl.hdr");
    write_probe_hdr(&hdr_path)?;

    let mut engine = WorkerState {
        bridge: bridge.clone().unbind(),
        hdr_path,
        scratch_png: std::env::temp_dir().join("forge3d-studio-frame.png"),
        camera: CameraParams::default(),
        sun: SunParams::default(),
        terrain_visible: true,
        dirty: true,
        has_terrain: false,
        domain: (0.0, 1.0),
    };

    while let Ok(command) = commands.recv() {
        match command {
            Command::Shutdown => break,
            Command::LoadTerrain { path } => {
                let _ = events.send(Event::Busy(true));
                match dispatch(
                    py,
                    &engine.bridge.bind(py),
                    "terrain.load",
                    &serde_json::json!({ "path": path.to_string_lossy() }),
                ) {
                    Ok(meta) => {
                        let width = meta.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let height =
                            meta.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let dmin = meta
                            .pointer("/domain/0")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0) as f32;
                        let dmax = meta
                            .pointer("/domain/1")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0) as f32;
                        engine.domain = (dmin, dmax);
                        engine.camera = WorkerState::default_camera(engine.domain);
                        engine.has_terrain = true;
                        engine.dirty = true;
                        let _ = events.send(Event::TerrainLoaded {
                            path: path.clone(),
                            width,
                            height,
                            height_min: dmin,
                            height_max: dmax,
                            camera: engine.camera,
                            sun: engine.sun,
                        });
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
                let _ = dispatch(
                    py,
                    &engine.bridge.bind(py),
                    "terrain.clear",
                    &serde_json::json!({}),
                );
                engine.has_terrain = false;
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
                if engine.dirty && engine.has_terrain {
                    engine.dirty = false;
                    render_terrain_frame(py, &mut engine, events, width, height, None);
                }
            }
            Command::RenderToFile {
                width,
                height,
                path,
            } => {
                let _ = events.send(Event::Busy(true));
                let started = Instant::now();
                if !engine.has_terrain {
                    let _ = events.send(Event::ExportFailed {
                        path,
                        message: "no terrain loaded".to_owned(),
                    });
                } else {
                    match render_terrain_png(py, &mut engine, width, height, &path) {
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
                }
                let _ = events.send(Event::Busy(false));
            }
            Command::Call { job, op, params } => {
                let _ = events.send(Event::Busy(true));
                match dispatch(py, &engine.bridge.bind(py), &op, &params) {
                    Ok(result) => {
                        let _ = events.send(Event::JobDone { job, op, result });
                    }
                    Err(message) => {
                        let _ = events.send(Event::JobFailed { job, op, message });
                    }
                }
                let _ = events.send(Event::Busy(false));
            }
        }
    }
    Ok(())
}

fn render_terrain_png(
    py: Python<'_>,
    engine: &mut WorkerState,
    width: u32,
    height: u32,
    out: &Path,
) -> Result<(), String> {
    if !engine.has_terrain {
        return Err("no terrain loaded".to_owned());
    }
    if !engine.terrain_visible {
        return Err("terrain layer is hidden".to_owned());
    }
    dispatch(
        py,
        &engine.bridge.bind(py),
        "terrain.render",
        &serde_json::json!({
            "width": width,
            "height": height,
            "camera": {
                "phi_deg": engine.camera.phi_deg,
                "theta_deg": engine.camera.theta_deg,
                "radius": engine.camera.radius,
                "fov_deg": engine.camera.fov_deg,
            },
            "sun": {
                "azimuth_deg": engine.sun.azimuth_deg,
                "elevation_deg": engine.sun.elevation_deg,
                "intensity": engine.sun.intensity,
            },
            "domain": [engine.domain.0, engine.domain.1],
            "hdr_path": engine.hdr_path.to_string_lossy(),
            "out_png": out.to_string_lossy(),
        }),
    )
    .map(|_| ())
}

/// Render via the dispatcher, then decode the output PNG into RGBA bytes and
/// ship a `FrameReady` event. `force_out` is used by exports; viewport
/// renders reuse a scratch file.
fn render_terrain_frame(
    py: Python<'_>,
    engine: &mut WorkerState,
    events: &Sender<Event>,
    width: u32,
    height: u32,
    force_out: Option<PathBuf>,
) {
    let out = force_out.unwrap_or_else(|| engine.scratch_png.clone());
    let started = Instant::now();
    let result = render_terrain_png(py, engine, width, height, &out).and_then(|()| {
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

/// Minimal Radiance HDR feeding `IBL.from_hdr` — an 8x4 gradient is enough
/// for image-based lighting without shipping an HDR asset.
fn write_probe_hdr(path: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)
        .map_err(|err| format!("creating {}: {err}", path.display()))?;
    for line in ["#?RADIANCE", "FORMAT=32-bit_rle_rgbe", "", "-Y 4 +X 8"] {
        writeln!(file, "{line}").map_err(|err| err.to_string())?;
    }
    for _y in 0..4u32 {
        for _x in 0..8u32 {
            // Radiance unpacked RGBE; exponent 136 keeps values ~1.0.
            file.write_all(&[200, 210, 235, 136])
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}
