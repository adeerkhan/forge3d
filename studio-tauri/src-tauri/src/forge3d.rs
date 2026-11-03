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

/// Bundled DEM used as the default scene. Overridable with `FORGE3D_STUDIO_DEM`.
const DEFAULT_DEM_RELATIVE: &str = "assets/tif/Bryce_Canyon_1024.tif";

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Probe program: reports the package version and whether the viewer binary
/// resolves. Opening a window is not required.
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
    except Exception as exc:
        print(json.dumps({
            "ok": False,
            "version": version,
            "detail": "viewer binary unavailable: %s. Build it with: cargo build --release --bin interactive_viewer" % (exc,),
        }))
        return
    print(json.dumps({"ok": True, "version": version, "viewer": binary, "detail": "engine and viewer present"}))

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
except Exception as exc:
    print(json.dumps({"ok": False, "detail": "rasterio is unavailable: %s" % (exc,)}))
    raise SystemExit(0)

try:
    with rasterio.open(path) as src:
        data = src.read(1).astype("float64")
        height, width = data.shape
        ys = np.linspace(0, height, resolution + 1).astype(int)
        xs = np.linspace(0, width, resolution + 1).astype(int)
        out = np.empty((resolution, resolution), dtype="float64")
        for j in range(resolution):
            for i in range(resolution):
                block = data[ys[j]:max(ys[j] + 1, ys[j + 1]), xs[i]:max(xs[i] + 1, xs[i + 1])]
                out[j, i] = float(np.nanmean(block)) if block.size else 0.0
        bounds = src.bounds
        transform = src.transform
        a, b, c, d, e, f = transform.a, transform.b, transform.c, transform.d, transform.e, transform.f
        # Mirror the viewer's footprint: origin (c, -f), span (|a|·w, |e|·h).
        if all(isinstance(v, (int, float)) and v == v for v in (a, b, c, d, e, f)):
            origin_xz = [float(c), float(-f)]
            span_xz = [abs(float(a)) * width, abs(float(e)) * height]
        else:
            origin_xz = [0.0, 0.0]
            span_xz = [float(width), float(height)]
        payload = {
            "ok": True,
            "width": int(width),
            "height": int(height),
            "minElevation": float(np.nanmin(data)),
            "maxElevation": float(np.nanmax(data)),
            "extentMeters": [float(bounds.right - bounds.left), float(bounds.top - bounds.bottom)],
            "originXz": origin_xz,
            "spanXz": span_xz,
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

    /// The default scene's DEM, with its measured dimensions and elevation
    /// range so the project can reproduce the renderer's framing without Python.
    ///
    /// Sampling costs a Python start-up; the result is cached per path.
    pub fn default_terrain_seed(&self) -> Option<studio_core::TerrainSeed> {
        let path = resolve_default_dem()?;
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
        let mut seed = studio_core::TerrainSeed {
            name,
            path: path_text.clone(),
            byte_size: metadata.len(),
            dimensions: None,
            elevation_range: None,
            world_origin_xz: None,
            world_span_xz: None,
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

/// Resolve the bundled/default DEM: `FORGE3D_STUDIO_DEM`, else the checkout's
/// Bryce Canyon sample, searched from the executable and the working directory.
fn resolve_default_dem() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("FORGE3D_STUDIO_DEM") {
        candidates.push(PathBuf::from(path));
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().take(6).map(Path::to_path_buf));
    }
    if let Ok(current) = std::env::current_dir() {
        roots.extend(current.ancestors().take(6).map(Path::to_path_buf));
    }
    for root in roots {
        candidates.push(root.join(DEFAULT_DEM_RELATIVE));
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
