//! Forge3D engine discovery and DEM access.
//!
//! Studio no longer opens a separate viewer window: rendering happens in the
//! viewport's Rendered mode (`render.rs`). What remains here is what that path
//! and the terrain preview need — finding the interpreter that can import
//! `forge3d`, resolving the default DEM, and measuring it.
//!
//! Process control is deliberately honest: if the engine cannot be imported,
//! the probe says exactly what is missing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Serialize;

/// Bundled DEMs offered as starter scenes. `id` is what the UI sends back.
pub const SCENE_PRESETS: [(&str, &str, &str, &str); 2] = [
    (
        "bryce",
        "Bryce Canyon",
        "Bryce_Canyon_1024.tif",
        "4 x 3.8 km · 1024² DEM · hoodoo amphitheatre",
    ),
    (
        "swiss",
        "Swiss Alps",
        "switzerland_dem.tif",
        "353 x 226 km · national DEM · Alps to Jura",
    ),
];

/// Bundled DEM used as the default scene. Overridable with `FORGE3D_STUDIO_DEM`.
const DEFAULT_DEM_RELATIVE: &str = "assets/tif/Bryce_Canyon_1024.tif";
/// NAIP orthophoto draped on the Bryce starter (the Bryce "material").
const BRYCE_NAIP_RELATIVE: &str = "assets/tif/Bryce_Canyon_naip.tif";

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Probe program: reports the package version. The viewer binary is optional
/// diagnostics only — rendering runs in-process (`studio_render.py`), so a
/// missing `interactive_viewer` must not gate Rendered mode.
const PROBE_SCRIPT: &str = r#"
import json

def main():
    try:
        import forge3d
    except Exception as exc:
        print(json.dumps({"ok": False, "detail": "import forge3d failed: %s" % (exc,)}))
        return
    version = getattr(forge3d, "__version__", "unknown")
    backend = adapter = None
    software = False
    try:
        # `device_probe()` reports the adapter as "name"; accepting the other
        # spellings keeps an older engine usable, and the explicit software
        # flag means a CPU fallback can never be mistaken for a GPU render.
        probe = forge3d.device_probe()
        backend = probe.get("backend")
        adapter = (
            probe.get("name")
            or probe.get("adapter_name")
            or probe.get("adapter")
            or probe.get("device_type")
        )
        software = bool(probe.get("software_fallback"))
    except Exception:
        pass
    try:
        from forge3d.denoise_oidn import oidn_available
        denoise = bool(oidn_available())
    except Exception:
        denoise = False
    features = None
    try:
        # What the device actually granted — the fact every later "needs X"
        # decision reads, instead of guessing from the adapter name.
        granted = forge3d.capabilities().get("granted")
        if isinstance(granted, list):
            features = sorted(str(name) for name in granted)
    except Exception:
        pass
    try:
        from forge3d._viewer_binary import find_viewer_binary
        import forge3d.viewer as viewer
        binary = find_viewer_binary(viewer.__file__)
        detail = "engine ready (viewer binary present)"
    except Exception as exc:
        binary = None
        detail = "engine ready (viewer binary not found — optional: %s)" % (exc,)
    print(json.dumps({
        "ok": True,
        "version": version,
        "viewer": binary,
        "detail": detail,
        "backend": backend,
        "adapter": adapter,
        "software": software,
        "denoise": denoise,
        "features": features,
    }))

main()
"#;

/// Sampling script for the Studio heightfield preview.
///
/// Reading, nodata masking and the metric footprint are the engine's
/// (`forge3d.io.load_dem`, `forge3d.gis`), so a preview and a render can never
/// disagree about a DEM. Only the down-sample and the JSON shape are the
/// shell's.
const PREVIEW_SCRIPT: &str = r#"
import json, sys

try:
    import numpy as np
    import forge3d.io as fio
    from forge3d import gis
except Exception as exc:
    print(json.dumps({"ok": False, "detail": "the engine is unavailable: %s" % (exc,)}))
    raise SystemExit(0)

path = sys.argv[1]
resolution = max(8, min(256, int(sys.argv[2])))
try:
    dem = fio.load_dem(path, fill_nodata_values=False)
except Exception as exc:
    print(json.dumps({"ok": False, "detail": "the DEM could not be read: %s" % (exc,)}))
    raise SystemExit(0)

raw = np.asarray(dem.data, dtype="float64")
if dem.nodata_value is not None:
    # A sentinel such as -3.4e38 would otherwise define the range and turn the
    # mesh into spikes.
    raw = np.where(np.isclose(raw, float(dem.nodata_value)), np.nan, raw)
valid = np.isfinite(raw)
if not valid.any():
    print(json.dumps({"ok": False, "detail": "the DEM contains no valid elevation samples"}))
    raise SystemExit(0)

# A robust range: single corrupted samples (one 11 km pixel in the Swiss DEM)
# would otherwise define the display range and flatten everything.
values = raw[valid]
low, high = np.percentile(values, [0.01, 99.99])
min_elevation = float(low)
max_elevation = float(high)
outliers = int(np.count_nonzero((values < low) | (values > high)))

height, width = raw.shape
ys = np.linspace(0, height, resolution + 1).astype(int)
xs = np.linspace(0, width, resolution + 1).astype(int)
out = np.empty((resolution, resolution), dtype="float64")
for j in range(resolution):
    for i in range(resolution):
        block = raw[ys[j]:max(ys[j] + 1, ys[j + 1]), xs[i]:max(xs[i] + 1, xs[i + 1])]
        good = block[np.isfinite(block)]
        if good.size:
            out[j, i] = float(np.clip(good.mean(), min_elevation, max_elevation))
        else:
            out[j, i] = min_elevation

# World footprint in metres. Geographic rasters (degrees) are reprojected to
# their local UTM zone; treating degrees as metres reported a 353 km DEM as
# 0.00 km.
crs = str(dem.crs or "")
geographic = "4326" in crs or "wgs84" in crs.lower()
origin_xz = [0.0, 0.0]
if not crs:
    span_xz = [float(width), float(height)]
elif geographic and dem.bounds is not None:
    local = gis.estimate_local_utm(dem.bounds)["crs"]
    left, bottom, right, top = gis.transform_bounds(dem.crs, local, dem.bounds, densify=21)
    span_xz = [abs(right - left), abs(top - bottom)]
else:
    res = dem.resolution or (1.0, 1.0)
    span_xz = [abs(float(res[0])) * width, abs(float(res[1])) * height]
    transform = dem.transform
    if transform is not None:
        origin_xz = [
            float(getattr(transform, "c", 0.0)),
            float(-getattr(transform, "f", 0.0)),
        ]

payload = {
    "ok": True,
    "width": int(width),
    "height": int(height),
    "minElevation": min_elevation,
    "maxElevation": max_elevation,
    "extentMeters": span_xz,
    "originXz": origin_xz,
    "spanXz": span_xz,
    "geographic": bool(geographic),
    "outliers": outliers,
    "resolution": resolution,
    "heights": [round(float(value), 2) for value in out.ravel()],
}
print(json.dumps(payload))
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub available: bool,
    /// Interpreter that was probed (the one Rendered mode will use).
    pub python: String,
    pub package_version: Option<String>,
    pub viewer_binary: Option<String>,
    /// wgpu backend the engine selected, when the probe could reach the GPU.
    pub backend: Option<String>,
    /// Adapter name — the GPU actually rendering.
    pub adapter: Option<String>,
    /// True when the interpreter can denoise with OIDN (the `denoise` extra).
    pub denoise: Option<bool>,
    /// True when the engine is running on a software (CPU) adapter — the
    /// status bar must not present that as GPU rendering.
    pub software: Option<bool>,
    /// Device features the adapter actually granted (`forge3d.capabilities()`
    /// `granted`), sorted. `None` when the probe could not ask.
    pub features: Option<Vec<String>>,
    /// Always present: what is missing, or what is ready.
    pub detail: String,
}

pub struct Forge3dState {
    engine: std::sync::Mutex<Option<EngineStatus>>,
    /// Resolved default DEM plus its measured metadata (sampling is not free).
    terrain_cache: std::sync::Mutex<Option<(String, studio_core::TerrainSeed)>>,
}

impl Default for Forge3dState {
    fn default() -> Self {
        Self::new()
    }
}

impl Forge3dState {
    pub fn new() -> Self {
        Self {
            engine: std::sync::Mutex::new(None),
            terrain_cache: std::sync::Mutex::new(None),
        }
    }

    /// Probe the engine once and cache the result.
    pub fn probe(&self, force: bool) -> EngineStatus {
        if !force {
            if let Some(cached) = self.engine.lock().ok().and_then(|guard| guard.clone()) {
                return cached;
            }
        }

        // Keep the *most informative* failure, not the last one: an interpreter
        // that imports forge3d but lacks the viewer binary is far more useful
        // than "python3: no module named forge3d".
        let mut best: Option<EngineStatus> = None;

        for python in interpreter_candidates() {
            match probe_interpreter(&python) {
                Ok(status) => {
                    if status.available {
                        if let Ok(mut guard) = self.engine.lock() {
                            *guard = Some(status.clone());
                        }
                        return status;
                    }
                    let better = status.package_version.is_some();
                    let current = best
                        .as_ref()
                        .map(|entry| entry.package_version.is_some())
                        .unwrap_or(false);
                    if best.is_none() || (better && !current) {
                        best = Some(status);
                    }
                }
                Err(detail) => {
                    if best.is_none() {
                        best = Some(EngineStatus {
                            available: false,
                            python: python.clone(),
                            package_version: None,
                            viewer_binary: None,
                            backend: None,
                            adapter: None,
                            denoise: None,
                            software: None,
                            features: None,
                            detail,
                        });
                    }
                }
            }
        }

        let last = best.unwrap_or(EngineStatus {
            available: false,
            python: "python3".to_owned(),
            package_version: None,
            viewer_binary: None,
            backend: None,
            adapter: None,
            denoise: None,
            software: None,
            features: None,
            detail: "no Python interpreter was found".to_owned(),
        });
        if let Ok(mut guard) = self.engine.lock() {
            *guard = Some(last.clone());
        }
        last
    }

    /// A starter DEM by preset id ("bryce" | "swiss"), else the default.
    pub fn terrain_seed(&self, preset: Option<&str>) -> Option<studio_core::TerrainSeed> {
        let relative = preset
            .and_then(|id| SCENE_PRESETS.iter().find(|(key, ..)| *key == id))
            .map(|(_, _, file, _)| format!("assets/tif/{file}"))
            .unwrap_or_else(|| DEFAULT_DEM_RELATIVE.to_owned());
        self.seed_for_relative(&relative)
    }

    fn seed_for_relative(&self, relative: &str) -> Option<studio_core::TerrainSeed> {
        let path = resolve_dem(relative)?;
        let path_text = path.to_string_lossy().into_owned();

        if let Ok(cache) = self.terrain_cache.lock() {
            if let Some((cached_path, seed)) = cache.as_ref() {
                if cached_path == &path_text {
                    return Some(seed.clone());
                }
            }
        }

        let metadata = std::fs::metadata(&path).ok()?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "terrain.tif".to_owned());
        let title = SCENE_PRESETS
            .iter()
            .find(|(_, _, file, _)| format!("assets/tif/{file}") == relative)
            .map(|(_, name, _, _)| (*name).to_owned());
        let mut seed = studio_core::TerrainSeed {
            title,
            name,
            path: path_text.clone(),
            byte_size: metadata.len(),
            dimensions: None,
            elevation_range: None,
            world_origin_xz: None,
            world_span_xz: None,
            overlay: bryce_naip_overlay(relative),
        };

        let python = self.probe(false).python;
        match sample_terrain(&python, &path_text, 48) {
            Ok(value) => {
                let width = value.get("width").and_then(|v| v.as_u64());
                let height = value.get("height").and_then(|v| v.as_u64());
                if let (Some(width), Some(height)) = (width, height) {
                    seed.dimensions = Some([width as u32, height as u32]);
                }
                let low = value.get("minElevation").and_then(|v| v.as_f64());
                let high = value.get("maxElevation").and_then(|v| v.as_f64());
                if let (Some(low), Some(high)) = (low, high) {
                    seed.elevation_range = Some([low as f32, high as f32]);
                }
                let origin = value.get("originXz").and_then(|v| v.as_array());
                let span = value.get("spanXz").and_then(|v| v.as_array());
                if let (Some(origin), Some(span)) = (origin, span) {
                    let pair = |values: &Vec<serde_json::Value>| -> Option<[f64; 2]> {
                        Some([values.first()?.as_f64()?, values.get(1)?.as_f64()?])
                    };
                    seed.world_origin_xz = pair(origin);
                    seed.world_span_xz = pair(span);
                }
            }
            Err(error) => log::warn!(
                "DEM metadata unavailable ({error}); the renderer will use its own framing"
            ),
        }

        if let Ok(mut cache) = self.terrain_cache.lock() {
            *cache = Some((path_text, seed.clone()));
        }
        Some(seed)
    }
}

/// The Bryce starter ships its NAIP orthophoto as the terrain's albedo. Only
/// the untouched default Bryce scene gets it: an explicit `FORGE3D_STUDIO_DEM`
/// override or another preset means the user brought their own DEM, and
/// draping Bryce imagery on it would be wrong.
fn bryce_naip_overlay(dem_relative: &str) -> Option<studio_core::TerrainSeedOverlay> {
    if dem_relative != DEFAULT_DEM_RELATIVE {
        return None;
    }
    if std::env::var_os("FORGE3D_STUDIO_DEM").is_some() {
        return None;
    }
    let path = resolve_bundled(BRYCE_NAIP_RELATIVE)?;
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() == 0 {
        return None;
    }
    Some(studio_core::TerrainSeedOverlay {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Bryce_Canyon_naip.tif".to_owned()),
        path: path.to_string_lossy().into_owned(),
        byte_size: metadata.len(),
        opacity: 0.82,
    })
}

/// Resolve a bundled file: `FORGE3D_STUDIO_DEM` overrides the default sample,
/// otherwise the named file is searched from the executable and the working
/// directory.
fn resolve_dem(relative: &str) -> Option<PathBuf> {
    if relative == DEFAULT_DEM_RELATIVE {
        if let Some(path) = std::env::var_os("FORGE3D_STUDIO_DEM") {
            let candidate = PathBuf::from(path);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    resolve_bundled(relative)
}

/// Search the executable's and the working directory's ancestors for a file
/// relative to the repo root (`assets/…`).
fn resolve_bundled(relative: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().take(6).map(Path::to_path_buf));
    }
    if let Ok(current) = std::env::current_dir() {
        roots.extend(current.ancestors().take(6).map(Path::to_path_buf));
    }
    for root in roots {
        candidates.push(root.join(relative));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Run a one-shot Python script with a deadline.
///
/// `Command::output()` has no timeout, so a hung interpreter leaves the caller
/// pending forever. The pipes are drained on threads, otherwise a large payload
/// (a DEM preview, a base64 PNG) can fill the pipe buffer and stall the child
/// before it exits.
fn run_script(
    python: &str,
    script: &str,
    args: &[String],
    timeout: Duration,
) -> Result<(String, String), String> {
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(extra) = python_path_entries() {
        command.env("PYTHONPATH", extra);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run {python}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "the script has no stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "the script has no stderr".to_owned())?;
    let out_reader = std::thread::spawn(move || read_all(stdout));
    let err_reader = std::thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_reader.join().unwrap_or_default();
                let stderr = err_reader.join().unwrap_or_default();
                if !status.success() && stdout.trim().is_empty() {
                    return Err(format!("{python} exited with {status}: {}", stderr.trim()));
                }
                return Ok((stdout, stderr));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(format!(
                    "{python} did not answer within {}s",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(format!("could not wait for {python}: {error}")),
        }
    }
}

fn read_all(mut reader: impl std::io::Read) -> String {
    let mut text = String::new();
    let _ = reader.read_to_string(&mut text);
    text
}

/// Sample a DEM into a heightfield for the Studio viewport.
pub fn sample_terrain(
    python: &str,
    path: &str,
    resolution: u32,
) -> Result<serde_json::Value, String> {
    let (stdout, stderr) = run_script(
        python,
        PREVIEW_SCRIPT,
        &[path.to_owned(), resolution.to_string()],
        SCRIPT_TIMEOUT,
    )?;
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| format!("the DEM sampler produced no output: {}", stderr.trim()))?;
    let value: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("the DEM sampler returned invalid JSON: {error}"))?;
    if value.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
        return Err(value
            .get("detail")
            .and_then(|detail| detail.as_str())
            .unwrap_or("the DEM could not be sampled")
            .to_owned());
    }
    Ok(value)
}

/// Solar position from the engine's SIDERA ephemeris.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolarPosition {
    /// Azimuth from north toward east, degrees (the engine's convention).
    pub azimuth_deg: f64,
    /// Degrees above the horizon.
    pub elevation_deg: f64,
    /// Direction toward the sun in MapScene axes: +X east, +Y up, +Z north.
    pub direction: [f32; 3],
    pub daytime: bool,
}

/// One-shot ephemeris call. No GPU session: the shell's solar button must not
/// wait for the renderer service.
const SOLAR_SCRIPT: &str = r#"
import json, sys

try:
    import forge3d
    position = forge3d.sun_position(float(sys.argv[1]), float(sys.argv[2]), sys.argv[3])
    direction = position.to_scene_direction()
    print(json.dumps({
        "ok": True,
        "azimuth": position.azimuth,
        "elevation": position.elevation,
        "direction": [direction[0], direction[1], direction[2]],
        "daytime": bool(position.is_daytime()),
    }))
except Exception as exc:
    print(json.dumps({"ok": False, "detail": str(exc)}))
"#;

/// Ask the engine where the sun is for a UTC timestamp and a place.
///
/// The engine owns the astronomy (SIDERA); Studio only turns the returned
/// direction into the light's rotation (`workspace::sun_rotation`). The
/// direction is in MapScene axes, so no azimuth conversion happens here.
pub fn solar_position(
    python: &str,
    latitude: f64,
    longitude: f64,
    datetime_utc: &str,
) -> Result<SolarPosition, String> {
    let (stdout, stderr) = run_script(
        python,
        SOLAR_SCRIPT,
        &[
            latitude.to_string(),
            longitude.to_string(),
            datetime_utc.to_owned(),
        ],
        SCRIPT_TIMEOUT,
    )?;
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| format!("the solar probe produced no output: {}", stderr.trim()))?;
    parse_solar_reply(line)
}

fn parse_solar_reply(line: &str) -> Result<SolarPosition, String> {
    let value: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("the solar probe returned invalid JSON: {error}"))?;
    if value.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
        return Err(value
            .get("detail")
            .and_then(|detail| detail.as_str())
            .unwrap_or("the engine could not compute the sun position")
            .to_owned());
    }
    let azimuth = value
        .get("azimuth")
        .and_then(|value| value.as_f64())
        .ok_or_else(|| "the solar probe returned no azimuth".to_owned())?;
    let elevation = value
        .get("elevation")
        .and_then(|value| value.as_f64())
        .ok_or_else(|| "the solar probe returned no elevation".to_owned())?;
    let direction = value
        .get("direction")
        .and_then(|value| value.as_array())
        .filter(|entries| entries.len() == 3)
        .ok_or_else(|| "the solar probe returned no direction".to_owned())?;
    let mut parsed = [0.0f32; 3];
    for (index, entry) in direction.iter().enumerate() {
        parsed[index] = entry
            .as_f64()
            .ok_or_else(|| "the solar direction is not numeric".to_owned())?
            as f32;
    }
    Ok(SolarPosition {
        azimuth_deg: azimuth,
        elevation_deg: elevation,
        direction: parsed,
        daytime: value
            .get("daytime")
            .and_then(|value| value.as_bool())
            .unwrap_or(true),
    })
}

/// Decoded raster preview for the Material preview's terrain drape: a
/// downsampled RGBA PNG, colour handling mirroring
/// `studio_render.build_overlay`'s preserve-colours path so Material and
/// Rendered drape the same pixels. The webview cannot decode GeoTIFF, hence
/// the engine-python round trip.
const RASTER_PREVIEW_SCRIPT: &str = r#"
import base64, io, json, sys

path = sys.argv[1]
width = max(64, min(4096, int(sys.argv[2])))
try:
    import numpy as np
    import rasterio
    from PIL import Image
    from rasterio.enums import Resampling
except Exception as exc:
    print(json.dumps({"ok": False, "detail": "raster preview needs the engine python: %s" % (exc,)}))
    raise SystemExit(0)

try:
    with rasterio.open(path) as src:
        bands = min(3, src.count)
        height = max(1, int(round(width * src.height / max(1, src.width))))
        planes = [
            src.read(b + 1, out_shape=(height, width), resampling=Resampling.average, masked=True)
            for b in range(bands)
        ]
    rgb = np.zeros((height, width, 3), dtype=np.float32)
    for index in range(bands):
        rgb[:, :, index] = np.clip(np.asarray(planes[index].filled(0.0), dtype=np.float32), 0.0, None)
    if bands == 1:
        rgb[:, :, 1] = rgb[:, :, 0]
        rgb[:, :, 2] = rgb[:, :, 0]
    # Preserve-colours contract (matches the renderer): 8-bit passes through,
    # wider integers are bit-shifted down to display range.
    peak = max((float(np.ma.max(plane)) if plane.count() else 0.0) for plane in planes)
    if peak > 255.0:
        rgb *= 255.0 / 65535.0
    valid = np.ones((height, width), dtype=bool)
    for index in range(bands):
        valid &= ~np.ma.getmaskarray(planes[index])
    rgba = np.concatenate(
        [np.clip(rgb, 0.0, 255.0).astype(np.uint8), np.where(valid, 255, 0).astype(np.uint8)[..., None]],
        axis=2,
    )
    buffer = io.BytesIO()
    Image.fromarray(rgba, "RGBA").save(buffer, format="PNG")
    print(json.dumps({"ok": True, "png": base64.b64encode(buffer.getvalue()).decode("ascii")}))
except Exception as exc:
    print(json.dumps({"ok": False, "detail": str(exc)}))
    raise SystemExit(0)
"#;

/// Decode a raster into a downsampled PNG for the Solid viewport's drape.
pub fn raster_preview_png(python: &str, path: &str, width: u32) -> Result<Vec<u8>, String> {
    let (stdout, stderr) = run_script(
        python,
        RASTER_PREVIEW_SCRIPT,
        &[path.to_owned(), width.to_string()],
        SCRIPT_TIMEOUT,
    )?;
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| format!("the raster preview produced no output: {}", stderr.trim()))?;
    let value: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("the raster preview returned invalid JSON: {error}"))?;
    if value.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
        return Err(value
            .get("detail")
            .and_then(|detail| detail.as_str())
            .unwrap_or("the raster could not be decoded")
            .to_owned());
    }
    let png = value
        .get("png")
        .and_then(|png| png.as_str())
        .ok_or_else(|| "the raster preview returned no image".to_owned())?;
    base64::engine::general_purpose::STANDARD
        .decode(png)
        .map_err(|error| format!("invalid preview image: {error}"))
}

/// Interpreter candidates, most specific first.
///
/// The app is launched by `tauri dev` (cwd = the crate or the project) or as a
/// bundle (cwd = wherever the launcher put it), so both the executable's and the
/// working directory's ancestors are searched for the repo's `.venv`.
fn interpreter_candidates() -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();

    if let Some(python) = std::env::var_os("FORGE3D_STUDIO_PYTHON") {
        candidates.push(python.to_string_lossy().into_owned());
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().take(6).map(Path::to_path_buf));
    }
    if let Ok(current) = std::env::current_dir() {
        roots.extend(current.ancestors().take(6).map(Path::to_path_buf));
    }
    for root in roots {
        let venv = root.join(".venv").join("bin").join("python");
        if venv.is_file() {
            let candidate = venv.to_string_lossy().into_owned();
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
            break;
        }
    }

    candidates.push("python3".to_owned());
    candidates.dedup();
    candidates
}

/// Directories to add to `PYTHONPATH` so a bare interpreter can import the
/// checkout's package without an editable install.
fn python_path_entries() -> Option<String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().take(6).map(Path::to_path_buf));
    }
    if let Ok(current) = std::env::current_dir() {
        roots.extend(current.ancestors().take(6).map(Path::to_path_buf));
    }
    for root in roots {
        let package = root.join("python").join("forge3d").join("__init__.py");
        if package.is_file() {
            return Some(root.join("python").to_string_lossy().into_owned());
        }
    }
    None
}

fn probe_interpreter(python: &str) -> Result<EngineStatus, String> {
    let mut command = Command::new(python);
    command.arg("-c").arg(PROBE_SCRIPT);
    if let Some(path) = python_path_entries() {
        command.env("PYTHONPATH", path);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run {python}: {error}"))?;

    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    return Err(format!(
                        "{python} did not answer the engine probe within {}s",
                        PROBE_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(format!("could not wait for {python}: {error}")),
        }
    }

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .map_err(|error| format!("could not read the probe output: {error}"))?;
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }

    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| {
            let detail = stderr.trim();
            if detail.is_empty() {
                format!("{python} produced no probe output")
            } else {
                format!("{python}: {}", first_line(detail))
            }
        })?;

    let value: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("the engine probe returned invalid JSON: {error}"))?;

    Ok(engine_status_from_probe(python, &value))
}

/// Map one probe JSON object onto [`EngineStatus`].
///
/// Split out from the process plumbing so the wire shape (including a missing
/// `features` key from an older engine) is testable without Python.
fn engine_status_from_probe(python: &str, value: &serde_json::Value) -> EngineStatus {
    EngineStatus {
        available: value.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false),
        python: python.to_owned(),
        package_version: value
            .get("version")
            .and_then(|version| version.as_str())
            .map(str::to_owned),
        viewer_binary: value
            .get("viewer")
            .and_then(|viewer| viewer.as_str())
            .map(str::to_owned),
        backend: value
            .get("backend")
            .and_then(|backend| backend.as_str())
            .map(str::to_owned),
        adapter: value
            .get("adapter")
            .and_then(|adapter| adapter.as_str())
            .map(str::to_owned),
        denoise: value.get("denoise").and_then(|denoise| denoise.as_bool()),
        software: value
            .get("software")
            .and_then(|software| software.as_bool()),
        features: value.get("features").and_then(|features| {
            features.as_array().map(|list| {
                list.iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
        }),
        detail: value
            .get("detail")
            .and_then(|detail| detail.as_str())
            .unwrap_or("the engine probe returned no detail")
            .to_owned(),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::parse_solar_reply;

    #[test]
    fn solar_reply_parses_and_reports_engine_errors() {
        let ok = r#"{"ok":true,"azimuth":197.04,"elevation":66.08,"direction":[-0.118,0.914,-0.387],"daytime":true}"#;
        let position = parse_solar_reply(ok).expect("valid reply");
        assert!((position.azimuth_deg - 197.04).abs() < 1e-9);
        assert!((position.elevation_deg - 66.08).abs() < 1e-9);
        assert!((position.direction[2] + 0.387).abs() < 1e-5);
        assert!(position.daytime);

        let error =
            parse_solar_reply(r#"{"ok":false,"detail":"bad datetime"}"#).expect_err("engine error");
        assert!(error.contains("bad datetime"), "{error}");

        let missing = parse_solar_reply(r#"{"ok":true}"#).expect_err("incomplete reply");
        assert!(missing.contains("azimuth"), "{missing}");
    }
}

#[test]
fn probe_json_maps_the_engine_facts() {
    let value: serde_json::Value = serde_json::from_str(
        r#"{
                "ok": true,
                "version": "1.47.1",
                "viewer": "/bin/viewer",
                "detail": "engine ready",
                "backend": "Vulkan",
                "adapter": "NVIDIA GeForce RTX 5060 Laptop GPU",
                "software": false,
                "denoise": true,
                "features": ["texture_binding_array", "timestamp_query"]
            }"#,
    )
    .expect("probe JSON parses");

    let status = engine_status_from_probe("/venv/bin/python", &value);
    assert!(status.available);
    assert_eq!(status.python, "/venv/bin/python");
    assert_eq!(status.package_version.as_deref(), Some("1.47.1"));
    assert_eq!(status.viewer_binary.as_deref(), Some("/bin/viewer"));
    assert_eq!(status.backend.as_deref(), Some("Vulkan"));
    assert_eq!(
        status.adapter.as_deref(),
        Some("NVIDIA GeForce RTX 5060 Laptop GPU")
    );
    assert_eq!(status.denoise, Some(true));
    assert_eq!(status.software, Some(false));
    assert_eq!(
        status.features,
        Some(vec![
            "texture_binding_array".to_owned(),
            "timestamp_query".to_owned()
        ])
    );
}

#[test]
fn an_older_probe_without_features_is_not_an_error() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"ok": true, "version": "1.40.0", "detail": "engine ready"}"#)
            .expect("probe JSON parses");
    let status = engine_status_from_probe("python3", &value);
    assert_eq!(status.features, None);
    assert_eq!(status.software, None);
    assert_eq!(status.denoise, None);
    assert!(status.available);
}

#[test]
fn a_failed_probe_stays_unavailable() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"ok": false, "detail": "import forge3d failed: no module"}"#)
            .expect("probe JSON parses");
    let status = engine_status_from_probe("python3", &value);
    assert!(!status.available);
    assert_eq!(status.detail, "import forge3d failed: no module");
}
