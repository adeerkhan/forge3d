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
    try:
        from forge3d._viewer_binary import find_viewer_binary
        import forge3d.viewer as viewer
        binary = find_viewer_binary(viewer.__file__)
        detail = "engine ready (viewer binary present)"
    except Exception as exc:
        binary = None
        detail = "engine ready (viewer binary not found — optional: %s)" % (exc,)
    print(json.dumps({"ok": True, "version": version, "viewer": binary, "detail": detail}))

main()
"#;

/// Sampling script for the Studio heightfield preview (rasterio is already a
/// dependency of the engine's terrain workflow).
const PREVIEW_SCRIPT: &str = r#"
import json, sys
import numpy as np

path = sys.argv[1]
resolution = max(8, min(256, int(sys.argv[2])))
try:
    import rasterio
    from rasterio.warp import transform_bounds
except Exception as exc:
    print(json.dumps({"ok": False, "detail": "rasterio is unavailable: %s" % (exc,)}))
    raise SystemExit(0)

try:
    with rasterio.open(path) as src:
        # Masked read honours the file's nodata value. A sentinel such as
        # -3.4e38 would otherwise become the elevation range and turn the mesh
        # into spikes.
        masked = src.read(1, masked=True)
        raw = np.asarray(masked.filled(np.nan), dtype="float64")
        valid = np.isfinite(raw)
        if not valid.any():
            print(json.dumps({"ok": False, "detail": "the DEM contains no valid elevation samples"}))
            raise SystemExit(0)
        # A robust range: single corrupted samples (one 11 km pixel in the Swiss
        # DEM) would otherwise define the display range and flatten everything.
        valid_values = raw[valid]
        low, high = np.percentile(valid_values, [0.01, 99.99])
        min_elevation = float(low)
        max_elevation = float(high)
        outliers = int(np.count_nonzero((valid_values < low) | (valid_values > high)))

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

        # World footprint. Geographic rasters (degrees) are converted to metres;
        # treating degrees as metres is what made a 353 km DEM report 0.00 km.
        crs = src.crs
        bounds = src.bounds
        origin_xz = [0.0, 0.0]
        if crs is None:
            span_xz = [float(width), float(height)]
        elif crs.is_geographic:
            lon = (bounds.left + bounds.right) * 0.5
            lat = (bounds.top + bounds.bottom) * 0.5
            zone = int((lon + 180.0) // 6.0) + 1
            epsg = (32600 if lat >= 0 else 32700) + zone
            left, bottom, right, top = transform_bounds(crs, "EPSG:%d" % epsg, *bounds)
            span_xz = [abs(right - left), abs(top - bottom)]
        else:
            span_xz = [abs(float(src.transform.a)) * width, abs(float(src.transform.e)) * height]
            origin_xz = [float(src.transform.c), float(-src.transform.f)]

        payload = {
            "ok": True,
            "width": int(width),
            "height": int(height),
            "minElevation": min_elevation,
            "maxElevation": max_elevation,
            "extentMeters": span_xz,
            "originXz": origin_xz,
            "spanXz": span_xz,
            "geographic": bool(crs is not None and crs.is_geographic),
            "outliers": outliers,
            "resolution": resolution,
            "heights": [round(float(value), 2) for value in out.ravel()],
        }
except Exception as exc:
    print(json.dumps({"ok": False, "detail": str(exc)}))
    raise SystemExit(0)

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

/// Sample a DEM into a heightfield for the Studio viewport.
pub fn sample_terrain(
    python: &str,
    path: &str,
    resolution: u32,
) -> Result<serde_json::Value, String> {
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(PREVIEW_SCRIPT)
        .arg(path)
        .arg(resolution.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(extra) = python_path_entries() {
        command.env("PYTHONPATH", extra);
    }

    let output = command
        .output()
        .map_err(|error| format!("could not run {python}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| {
            format!(
                "the DEM sampler produced no output: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
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

    Ok(EngineStatus {
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
        detail: value
            .get("detail")
            .and_then(|detail| detail.as_str())
            .unwrap_or("the engine probe returned no detail")
            .to_owned(),
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}
