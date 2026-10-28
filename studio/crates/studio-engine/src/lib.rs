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

pub mod native_viewport;

pub use native_viewport::NativeViewport;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use studio_project::{CameraParams, SunParams};

/// Command sent from the UI to the engine worker.
#[derive(Debug, Clone)]
pub enum Command {
    /// Load a DEM through the engine's own loader (rasterio, nodata fill).
    LoadTerrain {
        path: PathBuf,
    },
    /// Unload the current terrain dataset.
    ClearTerrain,
    SetCamera(CameraParams),
    SetSun(SunParams),
    /// Named colormap (forge3d registry key) for terrain rendering.
    SetColormap(Option<String>),
    /// M5: sign a RenderCertificate for every terrain render (writes the
    /// deterministic JSON next to the scratch frame).
    SetCertificate(bool),
    /// M4: project-level `TerrainRenderParams` overrides for the terrain
    /// viewport (validated by `terrain.params_validate` before they arrive).
    SetRenderParams(serde_json::Value),
    /// Render one terrain frame at the requested size and ship RGBA bytes.
    RenderFrame {
        width: u32,
        height: u32,
    },
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
    ExportFailed {
        path: PathBuf,
        message: String,
    },
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
    Error {
        context: String,
        message: String,
    },
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
import dataclasses
import inspect
import json
import os
import sys
import tempfile
import traceback

import numpy as np
import forge3d as f3d
import forge3d.terrain_params as _tp
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


def _write_default_hdr(path):
    """Minimal Radiance HDR feeding `IBL.from_hdr` (mirrors the Rust worker's
    probe HDR): an 8x4 gradient is enough for image-based lighting without
    shipping an HDR asset."""
    with open(path, "wb") as fh:
        fh.write(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 8\n")
        # Radiance unpacked RGBE; exponent 136 keeps values ~1.0.
        fh.write(bytes([200, 210, 235, 136]) * 32)


def _ensure_overlay(cmap_name, domain=None):
    """Build (and memoize) the colormap overlay used for terrain frames.

    `domain` is the terrain's raw height range, and it is not optional in
    practice: without it the overlay samples the ramp in normalised space, so
    every terrain above that range clamps to the top colour and the viewport
    renders washed out. The canonical demo passes `domain` for the same reason
    (`terrain_demo._build_params`).
    """
    global _overlay, _overlay_cmap_key
    span = None if domain is None else (float(domain[0]), float(domain[1]))
    key = (cmap_name or "__default__", span)
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
    options = {"strength": 1.0}
    if span is not None:
        options["domain"] = span
    _overlay = f3d.OverlayLayer.from_colormap1d(cmap1d, **options)
    _overlay_cmap_key = key
    return _overlay


# --- ops: terrain ------------------------------------------------------------

def _op_terrain_load(p):
    global _heightmap, _domain
    path = str(p["path"])
    name = os.path.basename(path.lower())
    crs = None
    bounds = None
    resolution = None
    nodata_value = None
    stats = None
    if name.endswith((".f3dz", ".f3z")):
        # Deterministic F3DZ DEM compression (forge3d.codec): the whole file
        # is one compressed height grid.
        from forge3d import codec

        with open(path, "rb") as fh:
            data = fh.read()
        arr, _info = codec.decompress_dem(data)
        _heightmap = np.ascontiguousarray(arr, dtype=np.float32)
    elif name.endswith(".npy"):
        arr = np.load(path)
        if getattr(arr, "ndim", 0) != 2:
            raise RuntimeError(
                f"npy terrain must be a 2D array, got shape {getattr(arr, 'shape', None)}"
            )
        _heightmap = np.ascontiguousarray(arr, dtype=np.float32)
    else:
        dem = f3d.io.load_dem(path)
        _heightmap = np.ascontiguousarray(dem.data, dtype=np.float32)
        crs = dem.crs
        bounds = list(dem.bounds) if dem.bounds else None
        resolution = list(dem.resolution) if dem.resolution else None
        nodata_value = dem.nodata_value
        stats = {k: float(v) for k, v in (dem.stats or {}).items()}
    _domain = (float(np.min(_heightmap)), float(np.max(_heightmap)))
    result = {
        "width": int(_heightmap.shape[1]),
        "height": int(_heightmap.shape[0]),
        "domain": [_domain[0], _domain[1]],
        "crs": crs,
        "bounds": bounds,
        "resolution": resolution,
        "nodata_value": nodata_value,
    }
    if stats is not None:
        result["stats"] = stats
    return result


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


# --- terrain parameter pages (M4) --------------------------------------------
# The right panel's TerrainRenderParams pages are schema-driven:
# `terrain.params_schema` introspects `make_terrain_params_config` and the
# nested settings dataclasses, and `terrain.params_validate` runs the engine's
# own constructors so its validation text reaches the UI verbatim. Studio never
# reimplements a rule: a new parameter or a changed default appears in the
# schema without a Rust edit.

# Applied when the project has not set a value, so an empty params object
# reproduces the pre-M4 viewport render exactly.
_STUDIO_RENDER_DEFAULTS = {
    "msaa_samples": 1,
    "z_scale": 1.35,
    "exposure": 1.0,
    "albedo_mode": "colormap",
    "colormap_strength": 1.0,
    "ibl_enabled": True,
}

# Never editable from the pages: the viewport pipeline, another Studio panel,
# or an engine-internal input owns them. The schema reports every one in
# `excluded` with its reason so the UI can say so honestly instead of hiding.
_EXCLUDED = {
    "size_px": "output size (viewport / export dialog)",
    "render_scale": "Studio renders at render_scale=1.0",
    "terrain_span": "derived from the dataset domain",
    "domain": "derived from the dataset domain",
    "cam_radius": "Camera panel",
    "cam_phi_deg": "Camera panel",
    "cam_theta_deg": "Camera panel",
    "fov_y_deg": "Camera panel",
    "cam_target": "viewport orbits the terrain centre",
    "camera_mode": "viewport uses the screen projection",
    "light_azimuth_deg": "Sun panel",
    "light_elevation_deg": "Sun panel",
    "sun_intensity": "Sun panel",
    "overlays": "colormap picker",
    "vt_store": "engine-returned virtual-texture store handle",
    "height_curve_lut": "256-entry LUT file (drive it from a script)",
    "aa_samples": "offline accumulation only (forge3d.offline)",
    "aa_seed": "offline accumulation only (forge3d.offline)",
    "terrain_crs": "vector reprojection input, fixed at import time",
    "terrain_data_revision": "engine cache key",
    "prefetch_horizon_ms": "virtual-texture streaming policy",
    "vt_upload_budget_bytes": "virtual-texture streaming policy",
    "vt": "requires an open virtual-texture store",
    "aov": "offline AOV export (forge3d.offline)",
    "vector_overlay": "vector overlays belong to MapScene recipes",
}

# Closed value sets the engine validates against (locked by parity tests).
# A str field absent here is free text whose engine validation the pages
# surface inline instead of pre-filtering.
_ENUM_CHOICES = {
    "albedo_mode": ["colormap", "mix", "material"],
    "culling": ["none", "frustum", "hzb_two_phase"],
    "shading": ["forward", "visibility"],
    "height_curve_mode": ["linear", "pow", "smoothstep"],
    "pom.mode": ["Occlusion", "Relief", "Parallax"],
    "clouds.quality": ["low", "medium", "high", "ultra"],
    "volumetrics.mode": ["uniform", "height", "exponential"],
    "sky.model": ["hosek-wilkie", "preetham", "approximate", "aether"],
}

_NO_DEFAULT = object()


def _strip_optional(annotation):
    """`Optional[X]` / `X | None` -> (X, nullable), else (X, False)."""
    text = str(annotation).strip()
    if text.startswith("Optional[") and text.endswith("]"):
        return text[len("Optional["):-1].strip(), True
    if text.endswith("| None"):
        return text[:-len("| None")].strip(), True
    if text.startswith("None |"):
        return text[len("None |"):].strip(), True
    return text, False


def _field_spec(base):
    """(kind, container) for a base annotation; kind None = not representable.

    The container matters: the native `TerrainRenderParams` wrapper rejects a
    list where the dataclass declares `Tuple[...]` (e.g. `clip`,
    `clamp.height_range`), and JSON has no tuple type, so it is restored here.
    """
    if base in ("bool", "int", "float", "str"):
        return base, None
    for opener in ("Tuple[", "Sequence[", "List["):
        if base.startswith(opener) and base.endswith("]"):
            parts = [
                part.strip()
                for part in base[len(opener):-1].split(",")
                if part.strip() and part.strip() != "..."
            ]
            container = tuple if opener == "Tuple[" else list
            if parts and all(part == "float" for part in parts):
                return "float[]", container
            if parts and all(part == "int" for part in parts):
                return "int[]", container
            return None, None
    return None, None


def _section_class(annotation):
    """The settings dataclass behind `Optional[FogSettings]`, else None."""
    base, _nullable = _strip_optional(annotation)
    cls = getattr(_tp, base, None)
    return cls if cls is not None and dataclasses.is_dataclass(cls) else None


def _json_safe(value):
    return json.loads(json.dumps(value, default=lambda _obj: None))


def _effective_defaults():
    """What the engine actually renders with when the project set nothing."""
    return _json_safe(dataclasses.asdict(make_terrain_params_config(
        size_px=(64, 64),
        render_scale=1.0,
        terrain_span=1.0,
        msaa_samples=1,
        z_scale=1.35,
        exposure=1.0,
        domain=(0.0, 1.0),
    )))


def _section_defaults(key, cls, eff):
    """The values a render really uses for one settings section when unset."""
    eff_value = eff.get(key)
    if isinstance(eff_value, dict):
        return eff_value
    return _json_safe(dataclasses.asdict(cls()))


def _section_schema(key, cls, eff):
    # Prefer the factory-built defaults (the values a render really uses);
    # fall back to a bare construction for sections the factory passes through.
    try:
        defaults = _section_defaults(key, cls, eff)
    except Exception as exc:
        return {
            "key": key,
            "editable": False,
            "reason": str(exc),
            "fields": [],
            "defaults": {},
        }
    fields = []
    for field in dataclasses.fields(cls):
        if not field.init:
            continue
        base, nullable = _strip_optional(field.type)
        kind, _container = _field_spec(base)
        if kind is None:
            continue  # engine-internal input (handles, nested settings)
        default = defaults.get(field.name, _NO_DEFAULT)
        if default is _NO_DEFAULT or (default is None and not nullable):
            continue
        entry = {
            "key": field.name,
            "type": kind,
            "nullable": nullable,
            "default": default,
        }
        choices = _ENUM_CHOICES.get(f"{key}.{field.name}")
        if key == "shadows" and field.name == "technique":
            choices = sorted(cls.ALL_TECHNIQUES)
        if choices:
            entry["choices"] = choices
        fields.append(entry)
    return {
        "key": key,
        "editable": True,
        "fields": fields,
        "defaults": defaults,
    }


def _build_schema():
    signature = inspect.signature(make_terrain_params_config)
    eff = _effective_defaults()
    excluded = []
    sections = []
    root_fields = []
    for name, param in signature.parameters.items():
        if name in _EXCLUDED:
            excluded.append({"key": name, "reason": _EXCLUDED[name]})
            continue
        cls = _section_class(param.annotation)
        if cls is not None:
            sections.append(_section_schema(name, cls, eff))
            continue
        base, nullable = _strip_optional(param.annotation)
        kind, _container = _field_spec(base)
        if kind is None:
            excluded.append({
                "key": name,
                "reason": "engine-internal type (not editable from Studio)",
            })
            continue
        default = _STUDIO_RENDER_DEFAULTS.get(name, _NO_DEFAULT)
        if default is _NO_DEFAULT:
            default = eff.get(name, _NO_DEFAULT)
        if default is _NO_DEFAULT:
            default = (
                param.default
                if param.default is not inspect.Parameter.empty
                else _NO_DEFAULT
            )
        if default is _NO_DEFAULT or (default is None and not nullable):
            continue
        entry = {
            "key": name,
            "type": kind,
            "nullable": nullable,
            "default": default,
        }
        choices = _ENUM_CHOICES.get(name)
        if choices:
            entry["choices"] = choices
        root_fields.append(entry)
    sections.insert(0, {
        "key": "root",
        "editable": True,
        "fields": root_fields,
        "defaults": {},
    })
    return {"sections": sections, "excluded": excluded}


def _coerce(name, kind, container, nullable, value):
    if value is None:
        if nullable:
            return None
        raise ValueError(f"{name} must not be null")
    if kind == "bool":
        if not isinstance(value, bool):
            raise ValueError(f"{name} must be true or false")
        return value
    if kind == "int":
        if isinstance(value, bool):
            raise ValueError(f"{name} must be an integer")
        if isinstance(value, float):
            if not value.is_integer():
                raise ValueError(f"{name} must be an integer")
            return int(value)
        if not isinstance(value, int):
            raise ValueError(f"{name} must be an integer")
        return value
    if kind == "float":
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(f"{name} must be a number")
        return float(value)
    if kind == "str":
        if not isinstance(value, str):
            raise ValueError(f"{name} must be a string")
        return value
    if kind in ("float[]", "int[]"):
        if not isinstance(value, (list, tuple)):
            raise ValueError(f"{name} must be an array")
        out = []
        for item in value:
            if kind == "float[]":
                if isinstance(item, bool) or not isinstance(item, (int, float)):
                    raise ValueError(f"{name} must be an array of numbers")
                out.append(float(item))
            else:
                if isinstance(item, bool) or not isinstance(item, int):
                    raise ValueError(f"{name} must be an array of integers")
                out.append(item)
        return container(out)
    raise ValueError(f"{name} has unsupported type {kind}")


def _build_section(name, cls, value, eff):
    """A section dict is a patch over the engine's effective defaults.

    The pages materialize a section before editing it, but the bridge accepts
    partial patches too (a hand-edited project, a script): settings classes
    like `ShadowSettings` have required fields, so the defaults are merged in
    rather than relying on the dataclass to fill the gaps.
    """
    if value is None:
        return None
    if not isinstance(value, dict):
        raise ValueError(f"{name} must be an object")
    try:
        merged = dict(_section_defaults(name, cls, eff))
    except Exception:
        merged = {}
    fields = {field.name: field for field in dataclasses.fields(cls) if field.init}
    for key, raw in value.items():
        field = fields.get(key)
        if field is None:
            raise ValueError(f"unknown {name} setting: {key}")
        base, nullable = _strip_optional(field.type)
        kind, container = _field_spec(base)
        if kind is None:
            # Engine-internal input (a handle, nested settings): never sent
            # from the pages, so a stale value in a project file is dropped.
            merged.pop(key, None)
            continue
        merged[key] = _coerce(f"{name}.{key}", kind, container, nullable, raw)
    return cls(**merged)


def _params_to_kwargs(params):
    """Project params -> validated `make_terrain_params_config` kwargs.

    Unknown keys are an error (a typo must never be silently dropped), keys
    the viewport pins are dropped, and everything else is coerced to the
    annotation's type so the engine's own validators see what they expect.
    """
    if params is None:
        params = {}
    if not isinstance(params, dict):
        raise ValueError("terrain params must be an object")
    signature = inspect.signature(make_terrain_params_config)
    eff = None
    kwargs = {}
    for key, value in params.items():
        param = signature.parameters.get(key)
        if param is None:
            raise ValueError(f"unknown terrain param: {key}")
        if key in _EXCLUDED:
            continue  # pinned by the viewport pipeline (see schema.excluded)
        cls = _section_class(param.annotation)
        if cls is not None:
            if eff is None:
                eff = _effective_defaults()
            kwargs[key] = _build_section(key, cls, value, eff)
            continue
        base, nullable = _strip_optional(param.annotation)
        kind, container = _field_spec(base)
        if kind is None:
            raise ValueError(f"terrain param {key} is not editable from Studio")
        kwargs[key] = _coerce(key, kind, container, nullable, value)
    return kwargs


def _op_terrain_params_schema(p):
    return _build_schema()


def _op_terrain_params_validate(p):
    """Run the engine's own constructors over the project's params.

    Raises on the first invalid value, so `studio_dispatch` returns the
    engine's message (and traceback) and the UI shows it inline.
    """
    kwargs = _params_to_kwargs(p.get("params"))
    # Required factory inputs the viewport owns; the pages never set them.
    kwargs.setdefault("size_px", (64, 64))
    kwargs.setdefault("render_scale", 1.0)
    kwargs.setdefault("terrain_span", 1.0)
    kwargs.setdefault("domain", (0.0, 1.0))
    kwargs.setdefault("cam_target", (0.0, 0.0, 0.0))
    for key, value in _STUDIO_RENDER_DEFAULTS.items():
        kwargs.setdefault(key, value)
    config = make_terrain_params_config(**kwargs)
    f3d.TerrainRenderParams(config)
    return {"valid": True, "fields": len(kwargs)}


def _op_terrain_render(p):
    if _heightmap is None:
        raise RuntimeError("no terrain loaded")
    hdr_path = p.get("hdr_path") or os.path.join(
        tempfile.gettempdir(), "forge3d-studio-ibl.hdr"
    )
    if not os.path.exists(hdr_path):
        _write_default_hdr(hdr_path)
    _ensure_session(hdr_path)
    camera = dict(p["camera"])
    sun = p["sun"]
    # Pan offset lives on the ground plane; the engine's cam_target is the
    # full 3D orbit centre (x, y, z) — keep y at 0 for screen-mode terrain.
    if "target" in camera and len(camera["target"]) == 3:
        camera["cam_target"] = camera.pop("target")
    domain = p.get("domain") or [_domain[0], _domain[1]]
    overlay = _ensure_overlay(p.get("colormap"), domain)
    span = float(p.get("terrain_span") or max(domain[1] - domain[0], 1.0) * 6.0)
    # Project params first, then the Studio defaults for anything unset, and
    # last the values the viewport pipeline owns (geometry, camera, sun,
    # colormap overlay) so no project param can override them. With an empty
    # params object this is exactly the pre-M4 call.
    kwargs = _params_to_kwargs(p.get("params"))
    for key, value in _STUDIO_RENDER_DEFAULTS.items():
        kwargs.setdefault(key, value)
    kwargs.update({
        "size_px": (int(p["width"]), int(p["height"])),
        "render_scale": 1.0,
        "terrain_span": span,
        "domain": (float(domain[0]), float(domain[1])),
        "cam_radius": float(camera["radius"]),
        "cam_phi_deg": float(camera["phi_deg"]),
        "cam_theta_deg": float(camera["theta_deg"]),
        "fov_y_deg": float(camera["fov_deg"]),
        "cam_target": tuple(camera.get("cam_target", (0.0, 0.0, 0.0))),
        "camera_mode": "screen",
        "light_azimuth_deg": float(sun["azimuth_deg"]),
        "light_elevation_deg": float(sun["elevation_deg"]),
        "sun_intensity": float(sun["intensity"]),
        "overlays": [overlay],
    })
    config = make_terrain_params_config(**kwargs)
    certificate = p.get("certificate_path")
    frame = _renderer.render_terrain_pbr_pom(
        material_set=_material,
        env_maps=_ibl,
        params=f3d.TerrainRenderParams(config),
        heightmap=_heightmap,
        target=None,
        water_mask=None,
        certificate=str(certificate) if certificate else None,
    )
    frame.save(str(p["out_png"]))
    result = {"png": str(p["out_png"]), "terrain_span": span}
    if certificate:
        result["certificate_path"] = str(certificate)
    return result


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
    bounds = None
    try:
        dem = f3d.io.load_dem(dem_path)
        tw, th = int(dem.data.shape[1]), int(dem.data.shape[0])
        crs_default = dem.crs
        bounds = getattr(dem, "bounds", None)
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
        # Vector and label geometry must be expressed in the terrain's CRS. A
        # georeferenced DEM gives its bounds; an un-georeferenced source (a raw
        # .npy grid) falls back to pixel space. Pixel coordinates on a
        # lon/lat terrain are off-terrain and every label gets rejected, which
        # is exactly the bug this branch used to have.
        if bounds is not None:
            left, bottom, right, top = (float(v) for v in bounds)
            cx = (left + right) / 2.0
            cy = (bottom + top) / 2.0
            line = [
                [left + (right - left) * 0.15, bottom + (top - bottom) * 0.25],
                [left + (right - left) * 0.85, bottom + (top - bottom) * 0.75],
            ]
        else:
            cx, cy = tw * 0.5, th * 0.5
            line = [[tw * 0.1, th * 0.2], [tw * 0.9, th * 0.75]]
        features = p.get("features") or [
            {
                "id": "line-1",
                "geometry": {"type": "LineString", "coordinates": line},
                "properties": {"class": "primary"},
            }
        ]
        labels = p.get("labels") or [
            {
                "id": "label-1",
                "kind": "point",
                "text": str(p.get("label_text", "Label")),
                "geometry": {"type": "Point", "coordinates": [cx, cy, 0.0]},
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



def _op_scene_compile(p):
    """U2: compile the Studio document (layers) into a SceneRecipe.

    The document is the source of truth in the UI; the recipe is always
    compiled from it and never hand-edited. Construction reuses the same
    engine classes the canonical examples use, per layer kind:

    - terrain          -> TerrainSource (first visible terrain layer)
    - raster_overlay   -> RasterOverlay (path, crs, layer opacity)
    - vector           -> VectorOverlay (features + Mapbox style from `style`)
    - labels           -> LabelLayer (labels from `style`)
    - group            -> recursed (visible children only)
    - other kinds      -> honest "registered, not renderable" diagnostic

    Terrain CRS resolution: style.crs override, else the dataset's own
    georeferencing via io.load_dem; layers inherit the terrain CRS.
    """
    document = p.get("document") or {}
    env = p.get("environment") or {}
    layers_flat = document.get("layers") or []
    diagnostics = []

    # Nest the flat parent-pointer list into group trees (the Rust Document
    # stores one ordered Vec with `parent` ids).
    def _nest(flat):
        by_parent = {}
        for layer in flat:
            by_parent.setdefault(layer.get("parent"), []).append(layer)
        def build(parent):
            out = []
            for layer in by_parent.get(parent, []):
                if str(layer.get("kind")) == "Group":
                    merged = dict(layer)
                    merged["children"] = build(layer.get("id"))
                    out.append(merged)
                else:
                    out.append(layer)
            return out
        return build(None)

    layers_in = _nest(layers_flat)

    dem_path = None
    for layer in layers_in:
        if str(layer.get("kind")) == "Terrain" and layer.get("visible", True) and layer.get("source"):
            dem_path = layer["source"]
            break
    if dem_path is None:
        return {
            "recipe": None,
            "diagnostics": [
                {"severity": "error", "code": "no_terrain",
                 "message": "Scene has no visible terrain layer; MapScene needs one DEM."}
            ],
        }

    bounds = None
    try:
        dem = f3d.io.load_dem(dem_path)
        tw, th = int(dem.data.shape[1]), int(dem.data.shape[0])
        crs_default = dem.crs
        bounds = getattr(dem, "bounds", None)
    except Exception:
        arr = np.load(dem_path)
        tw, th = int(arr.shape[1]), int(arr.shape[0])
        crs_default = None

    def compile_layers(items, parent_crs):
        out = []
        for layer in items:
            kind = str(layer.get("kind"))
            name = str(layer.get("name") or kind)
            if not layer.get("visible", True):
                continue
            style = layer.get("style") or {}
            if not isinstance(style, dict):
                style = {}
            if kind == "Group":
                out.extend(compile_layers(layer.get("children") or [], parent_crs))
                continue
            if kind == "Terrain":
                continue  # the terrain source is emitted once, below
            if kind == "RasterOverlay":
                out.append(f3d.RasterOverlay(
                    layer_id=name,
                    path=str(layer.get("source")),
                    crs=style.get("crs") or parent_crs,
                    opacity=float(layer.get("opacity", 1.0)),
                    metadata={"source_id": f"studio:{name}"},
                ))
            elif kind == "Vector":
                features = style.get("features")
                if not features:
                    diagnostics.append({
                        "severity": "warning", "code": "vector_no_features",
                        "message": f"Vector layer '{name}' has no features yet.",
                    })
                    continue
                out.append(f3d.VectorOverlay(
                    layer_id=name,
                    crs=style.get("crs") or parent_crs,
                    features=features,
                    style=style.get("mapbox") or {"version": 8, "layers": [
                        {"id": name, "type": "line", "paint": {"line-color": "#ffffff"}}]},
                    metadata={"source_id": f"studio:{name}"},
                ))
            elif kind == "Labels":
                labels = style.get("labels")
                if not labels:
                    diagnostics.append({
                        "severity": "warning", "code": "labels_no_candidates",
                        "message": f"Labels layer '{name}' has no labels yet.",
                    })
                    continue
                out.append(f3d.LabelLayer(
                    layer_id=name,
                    labels=labels,
                    metadata={"source_id": f"studio:{name}"},
                ))
            else:
                diagnostics.append({
                    "severity": "warning", "code": "layer_unrenderable",
                    "message": f"Layer '{name}' ({kind}) is registered but Studio cannot "
                               "render this kind yet — it is excluded from the recipe.",
                })
        return out

    layers_out = compile_layers(layers_in, crs_default)
    terrain = f3d.TerrainSource(
        path=str(dem_path),
        crs=crs_default,
        metadata={"width": tw, "height": th, "source_id": "studio-terrain"},
    )
    camera = env.get("camera") or {}
    lighting = env.get("lighting") or {}
    output = env.get("output") or {}
    scene = f3d.MapScene(
        terrain=terrain,
        camera=f3d.OrbitCamera(
            target=(0.0, 0.0, 0.0),
            distance=float(camera.get("distance", 1.2)),
            azimuth_deg=float(camera.get("azimuth_deg", 25.0)),
            elevation_deg=float(camera.get("elevation_deg", 45.0)),
            fov_deg=float(camera.get("fov_deg", 45.0)),
        ),
        lighting=f3d.LightingPreset(
            name=str(lighting.get("name", "daylight")),
            intensity=float(lighting.get("intensity", 1.1)),
        ),
        output=f3d.OutputSpec(
            width=int(output.get("width", 640)),
            height=int(output.get("height", 420)),
            format="png",
        ),
        layers=layers_out,
        reproducibility_profile=f3d.ReproducibilityProfile(seed=404),
    )
    return {
        "recipe": scene.to_dict()["recipe"],
        "diagnostics": diagnostics,
        "layer_count": len(layers_out),
    }


# --- ops: provenance, diagnostics & determinism (M5) --------------------------

def _op_certificate_verify(p):
    """Verify a signed RenderCertificate against the development signing key.

    In-process renders sign with the deterministic dev seed (certificate.py's
    documented non-production key), so verification derives the matching
    public key rather than trusting anything embedded in the file.
    """
    import hashlib
    from forge3d import _ed25519
    from forge3d.certificate import verify, payload_sha256, _dev_signing_seed
    cert_path = p["path"]
    cert = json.loads(open(cert_path, "rb").read())
    pubkey = _ed25519.public_key_from_private(_dev_signing_seed())
    valid = verify(cert, pubkey)
    return {
        "valid": bool(valid),
        # The payload digest is computed over the canonical signed view; the
        # written JSON carries it inside the signature block, not verbatim.
        "payload_sha256": payload_sha256(cert),
        "signed_with": "development key",
    }


def _op_diagnostics_snapshot(p):
    """Degradation ledger + GPU memory telemetry, exactly as recorded."""
    from forge3d import _degradation
    return {
        "degradations": _degradation.snapshot(),
        "memory": f3d.mem.memory_metrics(),
    }


def _op_determinism_check(p):
    """Render the current terrain twice with identical parameters and compare
    SHA-256 over the PNG bytes. Honest about scope: this is a two-run check
    inside one process with one backend — not a cross-machine guarantee.
    """
    import hashlib
    if _heightmap is None:
        raise RuntimeError("no terrain loaded")
    out_a = p["out_a"]
    out_b = p["out_b"]
    _op_terrain_render({"width": p.get("width", 320), "height": p.get("height", 240),
                        "camera": p["camera"], "sun": p["sun"],
                        "out_png": out_a})
    _op_terrain_render({"width": p.get("width", 320), "height": p.get("height", 240),
                        "camera": p["camera"], "sun": p["sun"],
                        "out_png": out_b})
    sha_a = hashlib.sha256(open(out_a, "rb").read()).hexdigest()
    sha_b = hashlib.sha256(open(out_b, "rb").read()).hexdigest()
    return {"sha_a": sha_a, "sha_b": sha_b, "match": sha_a == sha_b}

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


# --- ops: dataset inspection (M3 unified import) ------------------------------

def _jsonable(value):
    """JSON-roundtrip any engine metadata value (schemas, bounds, ...)."""
    return json.loads(json.dumps(value, default=str))


def _is_url(path):
    return path.startswith(("http://", "https://", "file://"))


def _inspect_geotiff(path, remote):
    from forge3d import gis as f3d_gis

    if remote:
        from forge3d import cog as f3d_cog

        ds = f3d_cog.open_cog(path)
        try:
            stats = ds.stats()
            return {
                "path": path,
                "format": "cog",
                "kind": "raster",
                "terrain_loadable": True,
                "metadata": {
                    "bounds": _jsonable(ds.bounds),
                    "overview_levels": int(ds.overview_count),
                    "cache_hit_rate_percent": float(stats.hit_rate_percent),
                },
                "notes": [
                    "remote COG; loading streams it through the engine's GDAL path"
                ],
            }
        finally:
            close = getattr(ds, "close", None)
            if callable(close):
                try:
                    close()
                except Exception:
                    pass

    def _crs_label(info):
        # RasterInfo.crs_authority is a {"code","name"} mapping (or None).
        auth = getattr(info, "crs_authority", None)
        if isinstance(auth, dict):
            name = auth.get("name") or ""
            code = auth.get("code") or ""
            return f"{name}:{code}" if code else name
        if auth:
            return str(auth)
        return str(getattr(info, "crs_wkt", "") or "")

    try:
        info = f3d_gis.read_raster_info(path)
        return {
            "path": path,
            "format": "geotiff",
            "kind": "raster",
            "terrain_loadable": True,
            "metadata": {
                "driver": str(info.driver),
                "width": int(info.width),
                "height": int(info.height),
                "band_count": int(info.band_count),
                "dtype_per_band": [str(d) for d in info.dtype_per_band],
                "crs": _crs_label(info),
                "bounds": _jsonable(info.bounds),
                "resolution": _jsonable(info.resolution),
                "nodata_per_band": [
                    None if v is None else float(v) for v in info.nodata_per_band
                ],
            },
            "notes": [],
        }
    except Exception:
        # Metadata-only rasterio fallback for TIFFs the native reader
        # rejects (same dependency io.load_dem uses for the actual load).
        import rasterio

        with rasterio.open(path) as src:
            crs = src.crs.to_string() if src.crs else ""
            b = src.bounds
            return {
                "path": path,
                "format": "geotiff",
                "kind": "raster",
                "terrain_loadable": True,
                "metadata": {
                    "driver": str(src.driver),
                    "width": int(src.width),
                    "height": int(src.height),
                    "band_count": int(src.count),
                    "dtype_per_band": [str(src.dtypes[i]) for i in range(src.count)],
                    "crs": crs,
                    "bounds": [b.left, b.bottom, b.right, b.top],
                    "resolution": [abs(src.transform.a), abs(src.transform.e)],
                    "nodata_per_band": [src.nodata],
                },
                "notes": ["inspected through the rasterio fallback reader"],
            }


def _inspect_f3dz(path):
    from forge3d import codec

    with open(path, "rb") as fh:
        data = fh.read()
    arr, info = codec.decompress_dem(data)
    return {
        "path": path,
        "format": "f3dz",
        "kind": "terrain",
        "terrain_loadable": True,
        "metadata": {
            "width": int(arr.shape[1]),
            "height": int(arr.shape[0]),
            "domain": [float(np.min(arr)), float(np.max(arr))],
            "stream": _jsonable(info),
        },
        "notes": ["F3DZ deterministic DEM compression; loading decompresses into the terrain slot"],
    }


def _inspect_npy(path):
    arr = np.load(path, mmap_mode="r")
    return {
        "path": path,
        "format": "npy",
        "kind": "terrain",
        "terrain_loadable": bool(getattr(arr, "ndim", 0) == 2),
        "metadata": {
            "shape": list(arr.shape),
            "dtype": str(arr.dtype),
        },
        "notes": ["raw numpy height grid; 2D arrays load as terrain"],
    }


def _inspect_pointcloud(path):
    from forge3d import pointcloud as f3d_pc

    ds = f3d_pc.open_pointcloud(path)
    # total_points / node_count / bounds / is_copc are properties on the
    # engine's dataset classes (not methods).
    bounds = ds.bounds
    is_copc = False
    try:
        is_copc = bool(ds.is_copc)
    except Exception:
        pass
    fmt = "ept" if os.path.basename(path.lower()) == "ept.json" else (
        "copc" if is_copc else "laz"
    )
    return {
        "path": path,
        "format": fmt,
        "kind": "pointcloud",
        "terrain_loadable": False,
        "metadata": {
            "total_points": int(ds.total_points),
            "node_count": int(ds.node_count),
            "bounds": None
            if bounds is None
            else {"center": list(bounds.center()), "size": list(bounds.size())},
        },
        "notes": [
            "point clouds are not yet a Studio scene layer; import records "
            "metadata only (GUI.md W3 ring 2)"
        ],
    }


def _inspect_tiles3d(path):
    from forge3d import tiles3d

    tileset = tiles3d.load_tileset(path)
    root = getattr(tileset, "root", None)
    volume = getattr(root, "bounding_volume", None) if root is not None else None
    return {
        "path": path,
        "format": "3dtiles",
        "kind": "tileset",
        "terrain_loadable": False,
        "metadata": {
            "tile_count": int(tileset.tile_count),
            "max_depth": int(tileset.max_depth),
            "root_bounds": None
            if volume is None
            else {"center": list(volume.center()), "radius": float(volume.radius())},
        },
        "notes": [
            "3D Tiles tilesets are not yet a Studio scene layer; import records "
            "metadata only (GUI.md W3 ring 2)"
        ],
    }


def _inspect_mesh_obj(path):
    from forge3d import io as f3d_io

    mesh, materials, _mg, _g, _o = f3d_io.load_obj(path, return_metadata=True)
    return {
        "path": path,
        "format": "obj",
        "kind": "mesh",
        "terrain_loadable": False,
        "metadata": {
            "vertex_count": int(mesh.vertex_count),
            "triangle_count": int(mesh.triangle_count),
            "material_count": len(materials),
        },
        "notes": ["mesh layers are not yet a Studio scene layer; import records metadata only"],
    }


def _inspect_mesh_gltf(path):
    from forge3d import io as f3d_io

    mesh, materials, primitive_materials = f3d_io.import_gltf(path, with_materials=True)
    return {
        "path": path,
        "format": "glTF",
        "kind": "mesh",
        "terrain_loadable": False,
        "metadata": {
            "vertex_count": int(mesh.vertex_count),
            "triangle_count": int(mesh.triangle_count),
            "material_count": len(materials),
            "primitive_materials": _jsonable(primitive_materials),
        },
        "notes": ["mesh layers are not yet a Studio scene layer; import records metadata only"],
    }


def _inspect_vector(path):
    from forge3d import gis as f3d_gis

    crs_info = f3d_gis.vector_crs(path)
    if isinstance(crs_info, dict):
        # CrsInfo mapping: prefer the authority code, else the WKT, else an
        # explicit "missing" marker (the engine's missing_crs is a hard
        # validation error for MapScene layers).
        crs = str(
            crs_info.get("authority")
            or crs_info.get("wkt")
            or ("missing" if crs_info.get("missing") else "")
        )
    else:
        crs = str(crs_info or "")
    return {
        "path": path,
        "format": "vector",
        "kind": "vector",
        "terrain_loadable": False,
        "metadata": {
            "geometry_type": str(f3d_gis.geometry_type(path)),
            "feature_count": int(f3d_gis.feature_count(path)),
            "crs": crs,
            "bounds": _jsonable(f3d_gis.vector_bounds(path)),
            "schema": _jsonable(f3d_gis.vector_schema(path)),
        },
        "notes": [
            "vector layers enter MapScene recipes as overlays (W2 ring 2); "
            "import records metadata only"
        ],
    }


def _looks_like_tileset(path):
    """Content-sniff a .json file for 3D Tiles structure.

    A tileset.json is `{asset: {version}, root: {boundingVolume, ...}}` and has
    no GeoJSON top-level `type`. Matching by name alone ("tileset*.json") misses
    real-world names like `fuji_tileset.json`, so sniff the document instead.
    """
    try:
        with open(path, "r", encoding="utf-8") as fh:
            head = fh.read(4096)
        probe = json.loads(head) if head.lstrip().startswith("{") else None
    except Exception:
        return False
    if not isinstance(probe, dict):
        return False
    if "type" in probe:  # GeoJSON (FeatureCollection/Feature/geometry)
        return False
    root = probe.get("root")
    return "asset" in probe and isinstance(root, dict) and (
        "boundingVolume" in root or "geometricError" in root
    )


def _op_dataset_inspect(p):
    """Per-format metadata preview for the unified import dialog (M3).

    Never loads pixel data for previews: rasters go through the metadata-only
    readers (`gis.read_raster_info`, COG header), point clouds/tilesets
    through their hierarchy headers, meshes through one bounded parse.
    """
    path = str(p.get("path") or "")
    if not path:
        raise RuntimeError("dataset.inspect requires path")
    name = os.path.basename(path.lower())
    remote = _is_url(path)

    if name.endswith((".tif", ".tiff")):
        return _inspect_geotiff(path, remote)
    if name.endswith((".f3dz", ".f3z")):
        return _inspect_f3dz(path)
    if name.endswith(".npy"):
        return _inspect_npy(path)
    if name.endswith((".laz", ".copc.laz")) or name == "ept.json":
        return _inspect_pointcloud(path)
    if name.endswith(".json") and not remote and _looks_like_tileset(path):
        return _inspect_tiles3d(path)
    if name.endswith(".obj"):
        return _inspect_mesh_obj(path)
    if name.endswith((".gltf", ".glb")):
        return _inspect_mesh_gltf(path)
    if name.endswith(".stl"):
        return {
            "path": path,
            "format": "stl",
            "kind": "mesh",
            "terrain_loadable": False,
            "metadata": {},
            "notes": [
                "unsupported: the engine ships STL export (io.save_stl) but no STL importer"
            ],
        }
    if name.endswith((".geojson", ".json", ".gpkg", ".shp")):
        return _inspect_vector(path)
    raise RuntimeError(
        "unsupported dataset format: "
        + name
        + " (supported: GeoTIFF/COG .tif, .f3dz, .npy, .laz/.copc.laz/ept.json, "
        "tileset.json, .obj/.gltf/.glb, .geojson/.gpkg/.shp)"
    )


# --- ops: renderer presets (W1 depth) -----------------------------------------

def _op_presets_get(p):
    import forge3d.presets as presets

    name = str(p.get("name") or "")
    return {"name": name, "preset": presets.get(name)}


# --- ops: camera rigs + animation (W1 depth) -----------------------------------

def _op_rig_bake(p):
    """Bake a camera rig into renderer-unit keyframes over the loaded DEM.

    The rigs live in scatter-source units (`TerrainScatterSource`, terrain
    width = max(DEM dimension)); Studio's viewport camera lives in renderer
    world units (terrain_span = domain * 6). Keyframes are rescaled here so
    the UI timeline replays them with no further engine math.
    """
    if _heightmap is None:
        raise RuntimeError("no terrain loaded")
    from forge3d import camera_rigs as rigs
    from forge3d.terrain_scatter import TerrainScatterSource

    kind = str(p.get("kind") or "orbit")
    params = dict(p.get("params") or {})
    sps = max(1, int(p.get("samples_per_second", 12)))
    source = TerrainScatterSource(_heightmap, z_scale=1.35)
    w, h = float(source.width), float(source.height)
    # The engine validates every sampled camera eye against the square
    # [0, terrain_width] source footprint. An orbit/follow eye sweeps
    # `radius * sin(theta)` horizontally around its target, so the default
    # radius must stay well under min(w, h)/2 to remain in bounds for the
    # full sweep (theta defaults to 45 deg, sin ~ 0.707).
    safe_radius = min(w, h) * 0.4

    if kind == "orbit":
        params.setdefault("target_xz", [w * 0.5, h * 0.5])
        params.setdefault("duration", 8.0)
        params.setdefault("radius", safe_radius)
        params.setdefault("phi_start_deg", 0.0)
        params.setdefault("phi_end_deg", 360.0)
        params.setdefault("theta_start_deg", 45.0)
        rig = rigs.TerrainOrbitRig(**params)
    elif kind == "rail":
        params.setdefault("path_xz", [[w * 0.15, h * 0.15], [w * 0.5, h * 0.5], [w * 0.85, h * 0.85]])
        params.setdefault("duration", 8.0)
        params.setdefault("camera_height_offset", max(w, h) * 0.05)
        params.setdefault("look_ahead_distance", max(w, h) * 0.08)
        rig = rigs.TerrainRailRig(**params)
    elif kind == "follow":
        params.setdefault("target_path_xz", [[w * 0.2, h * 0.8], [w * 0.5, h * 0.5], [w * 0.8, h * 0.2]])
        params.setdefault("duration", 8.0)
        params.setdefault("radius", safe_radius)
        params.setdefault("theta_deg", 45.0)
        rig = rigs.TerrainTargetFollowRig(**params)
    else:
        raise RuntimeError("unknown rig kind: " + kind + " (orbit | rail | follow)")

    animation = rig.bake(source, samples_per_second=sps)
    domain = p.get("domain") or [_domain[0], _domain[1]]
    span = float(p.get("terrain_span") or max(float(domain[1]) - float(domain[0]), 1.0) * 6.0)
    scale = span / float(source.terrain_width)
    keyframes = [
        {
            "t": float(kf.time),
            "phi_deg": float(kf.phi_deg),
            "theta_deg": float(kf.theta_deg),
            "radius": float(kf.radius) * scale,
            "fov_deg": float(kf.fov_deg),
        }
        for kf in animation.get_keyframes()
    ]
    return {
        "kind": kind,
        "duration": float(animation.duration),
        "samples_per_second": sps,
        "unit_scale": scale,
        "keyframes": keyframes,
        "notes": [
            "rig keyframes replay through the viewport orbit camera "
            "(center-locked look-at; full target-carrying playback is M4/M10)"
        ],
    }


_OPS = {
    "probe": studio_probe,
    "terrain.load": _op_terrain_load,
    "terrain.info": _op_terrain_info,
    "terrain.render": _op_terrain_render,
    "terrain.clear": _op_terrain_clear,
    "terrain.params_schema": _op_terrain_params_schema,
    "terrain.params_validate": _op_terrain_params_validate,
    "scene.compile": _op_scene_compile,
    "mapscene.templates": _op_mapscene_templates,
    "mapscene.template": _op_mapscene_template,
    "mapscene.render": _op_mapscene_render,
    "mapscene.validate": _op_mapscene_validate,
    "colormaps.list": _op_colormaps_list,
    "presets.list": _op_presets_list,
    "presets.get": _op_presets_get,
    "datasets.list": _op_datasets_list,
    "certificate.verify": _op_certificate_verify,
    "diagnostics.snapshot": _op_diagnostics_snapshot,
    "determinism.check": _op_determinism_check,
    "dataset.inspect": _op_dataset_inspect,
    "rig.bake": _op_rig_bake,
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
    colormap: Option<String>,
    request_certificate: bool,
    /// M4: project-level terrain params handed to `terrain.render`.
    render_params: serde_json::Value,
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
            target_x: 0.0,
            target_z: 0.0,
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
    let text: String = raw
        .extract()
        .map_err(|e| format!("dispatch returned non-string: {e}"))?;
    let envelope: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("dispatch envelope parse: {e}"))?;
    if envelope
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Ok(envelope
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
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
        .set_item(
            "_studio_host_python",
            host_python.to_string_lossy().into_owned(),
        )
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
        colormap: None,
        request_certificate: false,
        render_params: serde_json::Value::Object(serde_json::Map::new()),
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
                    engine.bridge.bind(py),
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
                    engine.bridge.bind(py),
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
            Command::SetColormap(colormap) => {
                engine.colormap = colormap;
                engine.dirty = true;
            }
            Command::SetCertificate(request) => {
                engine.request_certificate = request;
            }
            Command::SetRenderParams(params) => {
                engine.render_params = params;
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
                match dispatch(py, engine.bridge.bind(py), &op, &params) {
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
    // M5: a signed certificate per render lands next to the output frame.
    let certificate_path = engine.request_certificate.then(|| {
        let path = out.with_extension("cert.json");
        let _ = std::fs::remove_file(&path);
        path
    });
    dispatch(
        py,
        engine.bridge.bind(py),
        "terrain.render",
        &serde_json::json!({
            "width": width,
            "height": height,
            "camera": {
                "phi_deg": engine.camera.phi_deg,
                "theta_deg": engine.camera.theta_deg,
                "radius": engine.camera.radius,
                "fov_deg": engine.camera.fov_deg,
                "target": [engine.camera.target_x, 0.0, engine.camera.target_z],
            },
            "sun": {
                "azimuth_deg": engine.sun.azimuth_deg,
                "elevation_deg": engine.sun.elevation_deg,
                "intensity": engine.sun.intensity,
            },
            "domain": [engine.domain.0, engine.domain.1],
            "colormap": engine.colormap,
            "params": engine.render_params,
            "hdr_path": engine.hdr_path.to_string_lossy(),
            "out_png": out.to_string_lossy(),
            "certificate_path": certificate_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
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
    let mut file =
        std::fs::File::create(path).map_err(|err| format!("creating {}: {err}", path.display()))?;
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
