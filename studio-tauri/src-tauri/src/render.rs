//! Embedded offscreen renderer for the Studio viewport's Rendered mode.
//!
//! This is the plan's "Rendered" viewport, not the separate viewer window: a
//! persistent Python process drives the certified `render_terrain_pbr_pom`
//! pipeline and returns PNG frames to the shell, which the viewport displays.
//!
//! Same engine, same shaders, same settings as the golden-tested renders; what
//! differs is the transport (frames over stdio) and the fact that there is no
//! window to manage.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

const SERVICE_SCRIPT: &str = include_str!("studio_render.py");
const READY_TIMEOUT: Duration = Duration::from_secs(240);
const FRAME_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_LOG: usize = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderFrame {
    pub width: u32,
    pub height: u32,
    pub ms: f64,
    /// PNG bytes, base64: the viewport shows it directly.
    pub png: String,
    /// Where a still was written, when it was written to disk.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderStatus {
    pub running: bool,
    pub dem: Option<String>,
    pub diagnostics: Vec<String>,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    dem: String,
}

pub struct RenderState {
    session: Mutex<Option<Session>>,
    replies: Arc<Mutex<HashMap<u64, serde_json::Value>>>,
    log: Arc<Mutex<VecDeque<String>>>,
    next_id: AtomicU64,
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            replies: Arc::new(Mutex::new(HashMap::new())),
            log: Arc::new(Mutex::new(VecDeque::new())),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn status(&self) -> RenderStatus {
        let (running, dem) = match self.session.lock() {
            Ok(guard) => (
                guard.is_some(),
                guard.as_ref().map(|session| session.dem.clone()),
            ),
            Err(_) => (false, None),
        };
        RenderStatus {
            running,
            dem,
            diagnostics: self
                .log
                .lock()
                .map(|log| log.iter().cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// Render one frame for `dem`, starting the service on first use.
    pub fn render(
        &self,
        python: &str,
        dem: &str,
        request: serde_json::Value,
    ) -> Result<RenderFrame, String> {
        self.ensure_session(python, dem)?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut payload = request;
        if let Some(object) = payload.as_object_mut() {
            object.insert("id".into(), json!(id));
        }

        {
            let mut guard = self.session.lock().map_err(lock_error)?;
            let session = guard
                .as_mut()
                .ok_or_else(|| "the renderer is not running".to_owned())?;
            writeln!(session.stdin, "{payload}")
                .and_then(|_| session.stdin.flush())
                .map_err(|error| format!("lost the renderer: {error}"))?;
        }

        let deadline = Instant::now() + FRAME_TIMEOUT;
        loop {
            if let Ok(mut replies) = self.replies.lock() {
                if let Some(reply) = replies.remove(&id) {
                    if reply.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
                        return Err(reply
                            .get("error")
                            .and_then(|error| error.as_str())
                            .unwrap_or("the renderer reported a failure")
                            .to_owned());
                    }
                    return Ok(RenderFrame {
                        width: reply.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                        height: reply.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                        ms: reply.get("ms").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        png: reply
                            .get("png")
                            .and_then(|png| png.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        path: None,
                    });
                }
            }
            if Instant::now() > deadline {
                // A hung render must not wedge the session.
                self.stop();
                return Err("the renderer did not answer in time".to_owned());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    pub fn stop(&self) {
        let Ok(mut guard) = self.session.lock() else {
            return;
        };
        if let Some(mut session) = guard.take() {
            let _ = writeln!(session.stdin, "{}", json!({"id": 0, "cmd": "close"}));
            let _ = session.stdin.flush();
            for _ in 0..40 {
                match session.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
        push_log(&self.log, "renderer stopped".to_owned());
    }

    fn ensure_session(&self, python: &str, dem: &str) -> Result<(), String> {
        {
            let guard = self.session.lock().map_err(lock_error)?;
            if let Some(session) = guard.as_ref() {
                if session.dem == dem {
                    return Ok(());
                }
            }
        }
        self.stop();

        let directory = std::env::temp_dir().join("forge3d-studio");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let script = directory.join("studio_render.py");
        std::fs::write(&script, SERVICE_SCRIPT)
            .map_err(|error| format!("could not write {}: {error}", script.display()))?;

        push_log(&self.log, format!("starting renderer for {dem}"));
        let mut child = Command::new(python)
            .arg("-u")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start {python}: {error}"))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "the renderer has no stdin".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "the renderer has no stdout".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "the renderer has no stderr".to_owned())?;

        writeln!(stdin, "{}", json!({ "dem": dem, "zScale": 1.0 }))
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("could not configure the renderer: {error}"))?;

        let ready = Arc::new(Mutex::new(None::<Result<(), String>>));
        {
            let mut guard = self.session.lock().map_err(lock_error)?;
            *guard = Some(Session {
                child,
                stdin,
                dem: dem.to_owned(),
            });
        }

        spawn_reader(
            stdout,
            Arc::clone(&self.replies),
            Arc::clone(&self.log),
            Arc::clone(&ready),
        );
        spawn_stderr(stderr, Arc::clone(&self.log));

        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(mut state) = ready.lock() {
                if let Some(result) = state.take() {
                    return result;
                }
            }
            if Instant::now() > deadline {
                self.stop();
                return Err("the renderer did not become ready in time".to_owned());
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }
}

fn lock_error<T>(_: T) -> String {
    "the renderer is unavailable after an internal error".to_owned()
}

fn push_log(log: &Arc<Mutex<VecDeque<String>>>, line: String) {
    if let Ok(mut log) = log.lock() {
        log.push_back(line);
        while log.len() > MAX_LOG {
            log.pop_front();
        }
    }
}

fn spawn_reader(
    stdout: impl std::io::Read + Send + 'static,
    replies: Arc<Mutex<HashMap<u64, serde_json::Value>>>,
    log: Arc<Mutex<VecDeque<String>>>,
    ready: Arc<Mutex<Option<Result<(), String>>>>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let trimmed = line.trim();
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                push_log(&log, trimmed.to_owned());
                continue;
            };
            match value.get("event").and_then(|event| event.as_str()) {
                Some("ready") => {
                    push_log(&log, "renderer ready".to_owned());
                    if let Ok(mut state) = ready.lock() {
                        *state = Some(Ok(()));
                    }
                }
                Some("error") => {
                    let message = value
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("the renderer failed to start")
                        .to_owned();
                    push_log(&log, message.clone());
                    if let Ok(mut state) = ready.lock() {
                        *state = Some(Err(message));
                    }
                }
                _ => {
                    if let Some(id) = value.get("id").and_then(|id| id.as_u64()) {
                        if let Ok(mut replies) = replies.lock() {
                            replies.insert(id, value);
                        }
                    }
                }
            }
        }
        if let Ok(mut state) = ready.lock() {
            if state.is_none() {
                *state = Some(Err("the renderer exited unexpectedly".to_owned()));
            }
        }
        push_log(&log, "renderer closed".to_owned());
    });
}

fn spawn_stderr(stderr: impl std::io::Read + Send + 'static, log: Arc<Mutex<VecDeque<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            push_log(&log, format!("stderr: {line}"));
        }
    });
}

/// Camera override from the Studio viewport, so Rendered frames look like what
/// the solid view shows instead of a different framing. The type lives in the
/// core so the animation export and the viewport agree on it by construction.
pub use studio_core::workspace::RenderView;

/// Everything the shell needs for one frame, derived from the project.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub width: u32,
    pub height: u32,
    pub dem: String,
    pub phi_deg: f64,
    pub theta_deg: f64,
    pub radius: f64,
    pub target: [f64; 3],
    pub fov_deg: f64,
    pub sun_azimuth_deg: f64,
    pub sun_elevation_deg: f64,
    pub sun_intensity: f64,
    /// Linear RGB colour of the scene's directional light.
    pub sun_color: [f64; 3],
    pub z_scale: f64,
    pub overlay: Option<OverlayRequest>,
    pub exposure: f64,
    pub shadows: bool,
    pub ambient_occlusion: bool,
    /// Render the sky background; the project can turn it off for Rendered.
    pub sky: bool,
    /// Environment/IBL intensity multiplier (0 = environment light off).
    pub ibl_intensity: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayRequest {
    pub path: String,
    pub opacity: f32,
    pub preserve_colors: bool,
}

impl RenderRequest {
    pub fn with_settings(mut self, settings: &studio_core::RenderSettings) -> Self {
        self.exposure = f64::from(settings.exposure);
        self.shadows = settings.shadows;
        self.ambient_occlusion = settings.ambient_occlusion;
        self.sky = settings.sky;
        self.ibl_intensity = f64::from(settings.ibl_intensity);
        self
    }

    pub fn with_view(mut self, view: Option<&RenderView>) -> Self {
        if let Some(view) = view {
            if view.radius.is_finite() && view.radius > 0.0 {
                self.phi_deg = view.phi_deg;
                self.theta_deg = view.theta_deg;
                self.radius = view.radius;
            }
            if let Some(target) = view.target {
                if target.iter().all(|component| component.is_finite()) {
                    self.target = target;
                }
            }
            if let Some(fov_deg) = view.fov_deg {
                if fov_deg.is_finite() && (1.0..=179.0).contains(&fov_deg) {
                    self.fov_deg = fov_deg;
                }
            }
        }
        self
    }

    pub fn to_payload(&self, quality: &str, output: Option<&str>) -> serde_json::Value {
        let final_quality = quality == "final";
        json!({
            "width": self.width,
            "height": self.height,
            "zScale": self.z_scale,
            "camera": {
                "phiDeg": self.phi_deg,
                "thetaDeg": self.theta_deg,
                "radius": self.radius,
                "target": self.target,
                "fovDeg": self.fov_deg,
            },
            "sun": {
                "azimuthDeg": self.sun_azimuth_deg,
                "elevationDeg": self.sun_elevation_deg,
                "intensity": self.sun_intensity,
                "color": self.sun_color,
            },
            "quality": {
                "samples": 1,
                // Quality presets: interactive previews skip POM and any AO;
                // finals honour the project's settings. Shadows and sky follow
                // the project in both.
                "shadows": self.shadows,
                "pom": final_quality,
                "sky": self.sky,
                "ambientOcclusion": final_quality && self.ambient_occlusion,
                "heightAo": final_quality && self.ambient_occlusion,
            },
            "overlay": self.overlay,
            "exposure": self.exposure,
            "shadows": self.shadows,
            "ambientOcclusion": self.ambient_occlusion,
            "iblIntensity": self.ibl_intensity,
            "output": output,
        })
    }

    /// Resolve the renderer's camera from a handoff: the renderer measures the
    /// DEM in its own metres with Z up, so only the direction transfers.
    pub fn from_handoff(
        handoff: &studio_core::Handoff,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let terrain = handoff
            .terrain
            .as_ref()
            .ok_or_else(|| "the scene has no terrain to render".to_owned())?;
        let camera = handoff
            .terrain_camera
            .ok_or_else(|| "the scene has no camera".to_owned())?;

        let span = terrain
            .world_span_xz
            .ok_or_else(|| "the DEM footprint is unknown (open the project again)".to_owned())?;

        Ok(Self {
            width,
            height,
            dem: terrain.path.clone(),
            phi_deg: f64::from(camera.phi_deg),
            theta_deg: f64::from(camera.theta_deg),
            // Use the radius the camera's distance implies, so the frame matches
            // the viewport; fall back to the viewer's framing only when the
            // project cannot say (no camera distance available).
            radius: camera.radius.unwrap_or_else(|| span[0].max(span[1]) * 1.5),
            // The terrain mesh is vertically centred on Z=0 in the render world
            // (the shader centres it "so the camera at origin can see it"), so
            // the fallback aim is the mesh centre — aiming at absolute
            // mid-elevation framed empty sky for high-altitude DEMs.
            target: [0.0, 0.0, 0.0],
            fov_deg: f64::from(camera.fov_deg),
            sun_azimuth_deg: handoff
                .sun
                .map(|sun| f64::from(sun.azimuth_deg))
                .unwrap_or(135.0),
            sun_elevation_deg: handoff
                .sun
                .map(|sun| f64::from(sun.elevation_deg))
                .unwrap_or(35.0),
            sun_intensity: handoff
                .sun
                .map(|sun| f64::from(sun.intensity))
                .unwrap_or(3.0),
            sun_color: handoff
                .sun
                .map(|sun| {
                    [
                        f64::from(sun.color[0]),
                        f64::from(sun.color[1]),
                        f64::from(sun.color[2]),
                    ]
                })
                .unwrap_or([1.0, 0.97, 0.92]),
            z_scale: f64::from(terrain.z_scale),
            overlay: terrain.overlay.as_ref().map(|overlay| OverlayRequest {
                path: overlay.path.clone(),
                opacity: overlay.opacity,
                preserve_colors: overlay.preserve_colors,
            }),
            exposure: 1.35,
            shadows: true,
            ambient_occlusion: false,
            sky: true,
            ibl_intensity: 1.0,
        })
    }
}
