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
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

const SERVICE_SCRIPT: &str = include_str!("studio_render.py");
const READY_TIMEOUT: Duration = Duration::from_secs(240);
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(180);
/// A viewport frame is a preview: a wedged one must not block the queue behind
/// it for the still timeout.
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);
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

/// State the reader thread and the waiting command share.
///
/// The condition variable is the point: a render waits for its own reply
/// instead of waking every 40 ms, so a stopped render returns at once and a
/// finished one comes back without a poll interval of latency.
struct Shared {
    replies: Mutex<HashMap<u64, serde_json::Value>>,
    /// Readiness for the current session, tagged with its generation so a
    /// reader from a previous session cannot satisfy (or fail) a new one.
    ready: Mutex<Option<(u64, Result<(), String>)>>,
    log: Mutex<VecDeque<String>>,
    changed: Condvar,
}

impl Shared {
    fn new() -> Self {
        Self {
            replies: Mutex::new(HashMap::new()),
            ready: Mutex::new(None),
            log: Mutex::new(VecDeque::new()),
            changed: Condvar::new(),
        }
    }
}

pub struct RenderState {
    session: Mutex<Option<Session>>,
    shared: Arc<Shared>,
    /// One request at a time. The service is a single-threaded stdio loop, and
    /// a viewport frame must not race a still (or a session restart).
    request: Mutex<()>,
    /// Bumped by [`RenderState::stop`]; a waiter compares it and returns
    /// instead of waiting out its own timeout.
    generation: AtomicU64,
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
            shared: Arc::new(Shared::new()),
            request: Mutex::new(()),
            generation: AtomicU64::new(0),
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
                .shared
                .log
                .lock()
                .map(|log| log.iter().cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// Render one frame for `dem`, starting the service on first use.
    ///
    /// Blocks. Every caller runs on a worker thread (the commands are `async`
    /// and hand the body to the runtime), never on the UI thread.
    pub fn render(
        &self,
        python: &str,
        dem: &str,
        request: serde_json::Value,
        timeout: Duration,
    ) -> Result<RenderFrame, String> {
        // Serialise first, then start the clock: a viewport frame queued behind
        // a still must not spend the still's time against its own deadline.
        let _request = self.request.lock().map_err(lock_error)?;
        self.ensure_session(python, dem)?;
        let generation = self.generation.load(Ordering::SeqCst);

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

        let reply = match wait_for_reply(&self.shared, &self.generation, generation, id, timeout) {
            WaitOutcome::Reply(reply) => reply,
            WaitOutcome::Stopped => return Err("the render was stopped".to_owned()),
            WaitOutcome::TimedOut => {
                // A hung render must not wedge the session.
                self.stop();
                return Err("the renderer did not answer in time".to_owned());
            }
        };

        if reply.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
            return Err(reply
                .get("error")
                .and_then(|error| error.as_str())
                .unwrap_or("the renderer reported a failure")
                .to_owned());
        }
        Ok(RenderFrame {
            width: reply.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            height: reply.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            ms: reply.get("ms").and_then(|v| v.as_f64()).unwrap_or(0.0),
            png: reply
                .get("png")
                .and_then(|png| png.as_str())
                .unwrap_or_default()
                .to_owned(),
            path: None,
        })
    }

    /// Stop the renderer. A command waiting on a reply wakes immediately
    /// (`"the render was stopped"`) instead of waiting out its frame timeout.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.changed.notify_all();
        let Ok(mut guard) = self.session.lock() else {
            return;
        };
        if let Some(mut session) = guard.take() {
            let _ = writeln!(session.stdin, "{}", json!({"id": 0, "cmd": "close"}));
            let _ = session.stdin.flush();
            // The service exits on `close` immediately; do not spend a second
            // of a worker's time waiting for a graceful exit that will not come.
            for _ in 0..10 {
                match session.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
        push_log(&self.shared, "renderer stopped".to_owned());
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
        if let Ok(mut ready) = self.shared.ready.lock() {
            *ready = None;
        }

        let directory = std::env::temp_dir().join("forge3d-studio");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let script = directory.join("studio_render.py");
        std::fs::write(&script, SERVICE_SCRIPT)
            .map_err(|error| format!("could not write {}: {error}", script.display()))?;

        push_log(&self.shared, format!("starting renderer for {dem}"));
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

        {
            let mut guard = self.session.lock().map_err(lock_error)?;
            *guard = Some(Session {
                child,
                stdin,
                dem: dem.to_owned(),
            });
        }

        // `stop()` above bumped the generation; readiness belongs to this one.
        let generation = self.generation.load(Ordering::SeqCst);
        spawn_reader(stdout, Arc::clone(&self.shared), generation);
        spawn_stderr(stderr, Arc::clone(&self.shared));

        let mut ready = self.shared.ready.lock().map_err(lock_error)?;
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Some((tag, result)) = ready.take() {
                if tag == generation {
                    return result;
                }
                // A stale reader finished after its session was replaced.
            }
            if self.generation.load(Ordering::SeqCst) != generation {
                return Err("the renderer was stopped".to_owned());
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                drop(ready);
                self.stop();
                return Err("the renderer did not become ready in time".to_owned());
            };
            let (guard, _) = self
                .shared
                .changed
                .wait_timeout(ready, remaining)
                .map_err(lock_error)?;
            ready = guard;
        }
    }
}

/// What a waiting command saw.
#[derive(Debug)]
enum WaitOutcome {
    Reply(serde_json::Value),
    Stopped,
    TimedOut,
}

/// Block until `id`'s reply arrives, the renderer is stopped, or `timeout`
/// elapses. Split out so the wake/stop/timeout behaviour is testable without a
/// Python process.
fn wait_for_reply(
    shared: &Shared,
    generation: &AtomicU64,
    expected: u64,
    id: u64,
    timeout: Duration,
) -> WaitOutcome {
    let deadline = Instant::now() + timeout;
    let mut replies = match shared.replies.lock() {
        Ok(replies) => replies,
        Err(poisoned) => poisoned.into_inner(),
    };
    loop {
        if let Some(reply) = replies.remove(&id) {
            return WaitOutcome::Reply(reply);
        }
        if generation.load(Ordering::SeqCst) != expected {
            return WaitOutcome::Stopped;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return WaitOutcome::TimedOut;
        };
        replies = match shared.changed.wait_timeout(replies, remaining) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

fn lock_error<T>(_: T) -> String {
    "the renderer is unavailable after an internal error".to_owned()
}

fn push_log(shared: &Shared, line: String) {
    if let Ok(mut log) = shared.log.lock() {
        log.push_back(line);
        while log.len() > MAX_LOG {
            log.pop_front();
        }
    }
}

fn spawn_reader(stdout: impl std::io::Read + Send + 'static, shared: Arc<Shared>, generation: u64) {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let trimmed = line.trim();
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                push_log(&shared, trimmed.to_owned());
                continue;
            };
            match value.get("event").and_then(|event| event.as_str()) {
                Some("ready") => {
                    push_log(&shared, "renderer ready".to_owned());
                    if let Ok(mut state) = shared.ready.lock() {
                        *state = Some((generation, Ok(())));
                    }
                    shared.changed.notify_all();
                }
                Some("error") => {
                    let message = value
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("the renderer failed to start")
                        .to_owned();
                    push_log(&shared, message.clone());
                    if let Ok(mut state) = shared.ready.lock() {
                        *state = Some((generation, Err(message)));
                    }
                    shared.changed.notify_all();
                }
                _ => {
                    if let Some(id) = value.get("id").and_then(|id| id.as_u64()) {
                        if let Ok(mut replies) = shared.replies.lock() {
                            replies.insert(id, value);
                        }
                        shared.changed.notify_all();
                    }
                }
            }
        }
        if let Ok(mut state) = shared.ready.lock() {
            if state.is_none() {
                *state = Some((
                    generation,
                    Err("the renderer exited unexpectedly".to_owned()),
                ));
            }
        }
        shared.changed.notify_all();
        push_log(&shared, "renderer closed".to_owned());
    });
}

fn spawn_stderr(stderr: impl std::io::Read + Send + 'static, shared: Arc<Shared>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            push_log(&shared, format!("stderr: {line}"));
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
    /// Radiance `.hdr` environment for image-based lighting, when the project
    /// sets one. `None` makes the service use its built-in studio sky.
    pub environment: Option<String>,
    /// Backdrop brightness of the environment map in Rendered frames, 0..=1.
    /// Independent of `ibl_intensity`, which scales the lighting.
    pub environment_background_intensity: f64,
    /// Final-quality AOV export (see `studio_core::RenderSettings::still_aov`).
    pub aov_albedo: bool,
    pub aov_normal: bool,
    pub aov_depth: bool,
    /// Final-quality OIDN denoise of the beauty pass.
    pub still_denoise: bool,
    /// Shadow filter (`PCF` | `PCSS`) and shadow-map edge length in texels.
    pub shadow_technique: String,
    pub shadow_resolution: u32,
    /// Analytic sky model token (`hosek` | `preetham` | `aether`) and its
    /// backdrop parameters. Used only when the project has no HDRI.
    pub sky_model: String,
    pub sky_turbidity: f32,
    pub sky_ground_albedo: f32,
    pub sky_exposure: f32,
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
        self.environment_background_intensity =
            f64::from(settings.environment_background_intensity);
        self.aov_albedo = settings.still_aov.albedo;
        self.aov_normal = settings.still_aov.normal;
        self.aov_depth = settings.still_aov.depth;
        self.still_denoise = settings.still_denoise;
        self.shadow_technique = settings.shadow_technique.engine_name().to_owned();
        self.shadow_resolution = settings.shadow_resolution;
        self.sky_model = settings.sky_model.token().to_owned();
        self.sky_turbidity = settings.sky_turbidity;
        self.sky_ground_albedo = settings.sky_ground_albedo;
        self.sky_exposure = settings.sky_exposure;
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
                // Final stills (and the frames of a sequence) may ask for AOVs
                // and an OIDN pass; previews never pay for either.
                "aov": {
                    "albedo": final_quality && self.aov_albedo,
                    "normal": final_quality && self.aov_normal,
                    "depth": final_quality && self.aov_depth,
                },
                "denoise": final_quality && self.still_denoise,
                "shadowTechnique": self.shadow_technique,
                "shadowResolution": self.shadow_resolution,
                "skyModel": self.sky_model,
                "skyTurbidity": self.sky_turbidity,
                "skyGroundAlbedo": self.sky_ground_albedo,
                "skyExposure": self.sky_exposure,
            },
            "overlay": self.overlay,
            "exposure": self.exposure,
            "shadows": self.shadows,
            "ambientOcclusion": self.ambient_occlusion,
            "iblIntensity": self.ibl_intensity,
            "environment": self.environment,
            "environmentBackgroundIntensity": self.environment_background_intensity,
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
            environment: handoff
                .environment
                .as_ref()
                .map(|environment| environment.path.clone()),
            environment_background_intensity: 1.0,
            // Off until `with_settings` copies the project's choice in.
            aov_albedo: false,
            aov_normal: false,
            aov_depth: false,
            still_denoise: false,
            shadow_technique: "PCF".to_owned(),
            shadow_resolution: 4096,
            sky_model: "hosek".to_owned(),
            sky_turbidity: 2.0,
            sky_ground_albedo: 0.3,
            sky_exposure: 0.08,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> RenderRequest {
        RenderRequest {
            width: 960,
            height: 600,
            dem: "/data/dem.tif".to_owned(),
            phi_deg: 310.0,
            theta_deg: 68.0,
            radius: 6100.0,
            target: [0.0, 0.0, 39.5],
            fov_deg: 45.0,
            sun_azimuth_deg: 60.0,
            sun_elevation_deg: 50.0,
            sun_intensity: 3.0,
            sun_color: [1.0, 0.97, 0.92],
            z_scale: 1.0,
            overlay: None,
            exposure: 1.35,
            shadows: true,
            ambient_occlusion: false,
            sky: true,
            ibl_intensity: 1.0,
            environment: None,
            environment_background_intensity: 1.0,
            aov_albedo: false,
            aov_normal: false,
            aov_depth: false,
            still_denoise: false,
            shadow_technique: "PCF".to_owned(),
            shadow_resolution: 4096,
            sky_model: "hosek".to_owned(),
            sky_turbidity: 2.0,
            sky_ground_albedo: 0.3,
            sky_exposure: 0.08,
        }
    }

    #[test]
    fn payload_carries_the_project_hdri() {
        let mut frame = request();
        frame.environment = Some("/hdri/studio_sunrise.hdr".to_owned());
        let payload = frame.to_payload("final", Some("/out/frame.png"));
        assert_eq!(payload["environment"], "/hdri/studio_sunrise.hdr");
        assert_eq!(payload["iblIntensity"], 1.0);
        assert_eq!(payload["environmentBackgroundIntensity"], 1.0);
        assert_eq!(payload["output"], "/out/frame.png");

        // The built-in sky is requested by *not* sending a path, and a preview
        // frame skips POM and AO while a final honours them.
        let preview = request().to_payload("preview", None);
        assert!(preview["environment"].is_null());
        assert_eq!(preview["quality"]["pom"], false);
        assert_eq!(preview["quality"]["ambientOcclusion"], false);
        assert_eq!(preview["quality"]["aov"]["albedo"], false);
        assert_eq!(preview["quality"]["denoise"], false);
    }

    #[test]
    fn payload_emits_aov_and_denoise_only_for_finals() {
        let settings = studio_core::RenderSettings {
            still_aov: studio_core::StillAov {
                albedo: true,
                normal: false,
                depth: true,
            },
            still_denoise: true,
            ..studio_core::RenderSettings::default()
        };
        let enabled = request().with_settings(&settings);
        let final_payload = enabled.to_payload("final", Some("/out/still.png"));
        assert_eq!(final_payload["quality"]["aov"]["albedo"], true);
        assert_eq!(final_payload["quality"]["aov"]["normal"], false);
        assert_eq!(final_payload["quality"]["aov"]["depth"], true);
        assert_eq!(final_payload["quality"]["denoise"], true);

        // A preview drops both — the interactive frame pays neither.
        let preview = enabled.to_payload("preview", None);
        assert_eq!(preview["quality"]["aov"]["albedo"], false);
        assert_eq!(preview["quality"]["aov"]["depth"], false);
        assert_eq!(preview["quality"]["denoise"], false);
    }

    #[test]
    fn payload_carries_the_shadow_filter_and_map_size() {
        let settings = studio_core::RenderSettings {
            shadow_technique: studio_core::ShadowTechnique::Pcss,
            shadow_resolution: 2048,
            ..studio_core::RenderSettings::default()
        };
        let payload = request()
            .with_settings(&settings)
            .to_payload("preview", None);
        assert_eq!(payload["quality"]["shadowTechnique"], "PCSS");
        assert_eq!(payload["quality"]["shadowResolution"], 2048);
    }

    /// The fixture `studio-tauri/tests/fixtures/lighting-rig.json` is read by
    /// this test and by `src/viewport/lighting.test.ts`. This half asserts the
    /// payload the renderer receives; the node half asserts the numbers the
    /// preview hands to three.js. A drift in either fails, instead of showing
    /// up as "the render looks different from the preview".
    /// End-to-end through the real service: spawn the engine Python, render one
    /// frame over stdio, and stop. Ignored by default because it needs the
    /// checkout's `.venv` and a GPU; run with
    /// `cargo test -p forge3d-studio-shell the_session_renders -- --ignored`.
    #[test]
    #[ignore = "spawns the engine Python session and the GPU"]
    fn the_session_renders_a_frame_and_stops_cleanly() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let python = root.join(".venv/bin/python");
        let dem = root.join("assets/tif/Bryce_Canyon_1024.tif");
        if !python.exists() || !dem.exists() {
            eprintln!("skipping: this checkout has no .venv/engine DEM pair");
            return;
        }

        let state = RenderState::new();
        let payload = json!({
            "width": 96,
            "height": 64,
            "camera": {"phiDeg": 310.0, "thetaDeg": 40.0, "radius": 6000.0, "target": [0, 0, 0], "fovDeg": 45.0},
            "sun": {"azimuthDeg": 135.0, "elevationDeg": 40.0, "intensity": 3.0, "color": [1.0, 0.97, 0.92]},
            "quality": {"aov": {}, "sky": true, "skyModel": "hosek", "skyTurbidity": 2.0, "skyGroundAlbedo": 0.3, "skyExposure": 0.08},
            "exposure": 1.35,
            "iblIntensity": 1.0,
            "environmentBackgroundIntensity": 1.0
        });
        let frame = state
            .render(
                python.to_str().expect("utf-8 python path"),
                dem.to_str().expect("utf-8 dem path"),
                payload,
                FRAME_TIMEOUT,
            )
            .expect("one frame from the service");
        // The service clamps each side to at least 64 px.
        assert_eq!((frame.width, frame.height), (96, 64));
        assert!(frame.png.len() > 1000, "a PNG came back");
        assert!(state.status().running);

        state.stop();
        assert!(!state.status().running);
    }

    #[test]
    fn reply_wait_returns_the_reply_without_a_poll_interval() {
        let state = RenderState::new();
        state
            .shared
            .replies
            .lock()
            .expect("replies lock")
            .insert(7, json!({"ok": true, "width": 4}));
        match wait_for_reply(
            &state.shared,
            &state.generation,
            0,
            7,
            Duration::from_secs(1),
        ) {
            WaitOutcome::Reply(reply) => assert_eq!(reply["width"], 4),
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    #[test]
    fn stopping_wakes_a_waiting_render_instead_of_waiting_for_the_timeout() {
        let state = Arc::new(RenderState::new());
        let started = Instant::now();
        let waiter = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                wait_for_reply(
                    &state.shared,
                    &state.generation,
                    0,
                    1,
                    Duration::from_secs(60),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(80));
        state.stop();
        let outcome = waiter.join().expect("waiter finished");
        assert!(matches!(outcome, WaitOutcome::Stopped), "got {outcome:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stop must wake the waiter, not wait out the frame timeout"
        );
    }

    #[test]
    fn the_reply_wait_honours_its_timeout() {
        let state = RenderState::new();
        let started = Instant::now();
        let outcome = wait_for_reply(
            &state.shared,
            &state.generation,
            0,
            2,
            Duration::from_millis(60),
        );
        assert!(matches!(outcome, WaitOutcome::TimedOut), "got {outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn payload_matches_the_shared_lighting_rig_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/lighting-rig.json"))
                .expect("the lighting-rig fixture parses");
        let project = rig_project(&fixture);
        let handoff = studio_core::workspace::handoff(&project, 1, studio_core::Workspace::Forge3d);
        let payload = RenderRequest::from_handoff(&handoff, 640, 400)
            .expect("the fixture scene has terrain")
            .with_settings(&project.render)
            .to_payload("final", None);

        let expected = &fixture["expected"];
        near_json(
            "sun.azimuthDeg",
            &payload["sun"]["azimuthDeg"],
            &expected["sun"]["azimuthDeg"],
        );
        near_json(
            "sun.elevationDeg",
            &payload["sun"]["elevationDeg"],
            &expected["sun"]["elevationDeg"],
        );
        near_json(
            "sun.intensity",
            &payload["sun"]["intensity"],
            &expected["sun"]["intensity"],
        );
        for (index, channel) in ["r", "g", "b"].iter().enumerate() {
            near_json(
                &format!("sun.color.{channel}"),
                &payload["sun"]["color"][index],
                &expected["sun"]["color"][index],
            );
        }
        near_json("exposure", &payload["exposure"], &expected["exposure"]);
        assert_eq!(
            payload["environment"], expected["environment"],
            "environment must be the project's HDRI path or null"
        );
        assert_eq!(
            payload["quality"]["shadows"],
            expected["shadows"]["enabled"]
        );
        assert_eq!(
            payload["quality"]["shadowTechnique"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase(),
            expected["shadows"]["technique"]
                .as_str()
                .unwrap_or_default(),
            "payload carries the engine name; the fixture stores the model token"
        );
        assert_eq!(
            payload["quality"]["shadowResolution"],
            expected["shadows"]["resolution"]
        );

        // Render-only: the analytic sky draws the backdrop and is not part of
        // the preview rig (the environment map lights the scene).
        let sky = &expected["renderOnly"]["sky"];
        assert_eq!(payload["quality"]["sky"], sky["enabled"]);
        assert_eq!(payload["quality"]["skyModel"], sky["model"]);
        near_json(
            "quality.skyTurbidity",
            &payload["quality"]["skyTurbidity"],
            &sky["turbidity"],
        );
        near_json(
            "quality.skyGroundAlbedo",
            &payload["quality"]["skyGroundAlbedo"],
            &sky["groundAlbedo"],
        );
        near_json(
            "quality.skyExposure",
            &payload["quality"]["skyExposure"],
            &sky["exposure"],
        );
    }

    fn near_json(label: &str, actual: &serde_json::Value, expected: &serde_json::Value) {
        let actual = actual
            .as_f64()
            .unwrap_or_else(|| panic!("{label} is not a number"));
        let expected = expected
            .as_f64()
            .unwrap_or_else(|| panic!("{label} fixture is not a number"));
        assert!(
            (actual - expected).abs() <= 1e-4,
            "{label}: payload {actual} != fixture {expected}"
        );
    }

    /// A starter scene with the fixture's sun and render settings.
    fn rig_project(fixture: &serde_json::Value) -> studio_core::Project {
        let seed = studio_core::TerrainSeed {
            title: None,
            name: "dem.tif".to_owned(),
            path: "/data/dem.tif".to_owned(),
            byte_size: 1,
            dimensions: Some([64, 64]),
            elevation_range: Some([0.0, 100.0]),
            world_origin_xz: Some([0.0, 0.0]),
            world_span_xz: Some([640.0, 640.0]),
            overlay: None,
        };
        let mut project = studio_core::Project::starter("Rig", Some(seed));

        let rotation = &fixture["project"]["sunRotationDeg"];
        let color = &fixture["project"]["sunColor"];
        let sun = project
            .scene
            .objects
            .iter_mut()
            .find(|object| {
                matches!(
                    &object.kind,
                    studio_core::ObjectKind::Light(light)
                        if light.kind == studio_core::LightKind::Directional
                )
            })
            .expect("the starter scene has a sun");
        sun.transform.rotation_deg = studio_core::Vec3::new(
            rotation["x"].as_f64().unwrap_or(0.0) as f32,
            rotation["y"].as_f64().unwrap_or(0.0) as f32,
            rotation["z"].as_f64().unwrap_or(0.0) as f32,
        );
        if let studio_core::ObjectKind::Light(light) = &mut sun.kind {
            light.color = [
                color[0].as_f64().unwrap_or(0.0) as f32,
                color[1].as_f64().unwrap_or(0.0) as f32,
                color[2].as_f64().unwrap_or(0.0) as f32,
            ];
            light.intensity = fixture["project"]["sunIntensity"].as_f64().unwrap_or(0.0) as f32;
        }

        let render = &fixture["project"]["render"];
        project.render.exposure = render["exposure"].as_f64().unwrap_or(1.35) as f32;
        project.render.shadows = render["shadows"].as_bool().unwrap_or(true);
        project.render.shadow_technique = match render["shadowTechnique"].as_str().unwrap_or("pcf")
        {
            "pcss" => studio_core::ShadowTechnique::Pcss,
            _ => studio_core::ShadowTechnique::Pcf,
        };
        project.render.shadow_resolution =
            render["shadowResolution"].as_u64().unwrap_or(4096) as u32;
        project.render.environment = None;
        project
    }

    #[test]
    fn view_override_still_wins_over_the_handoff_camera() {
        let mut frame = request();
        frame.phi_deg = 10.0;
        frame.target = [1.0, 2.0, 3.0];
        let view = RenderView {
            phi_deg: 200.0,
            theta_deg: 55.0,
            radius: 900.0,
            target: Some([4.0, 5.0, 6.0]),
            fov_deg: Some(30.0),
        };
        let frame = frame.with_view(Some(&view));
        assert_eq!(frame.phi_deg, 200.0);
        assert_eq!(frame.theta_deg, 55.0);
        assert_eq!(frame.radius, 900.0);
        assert_eq!(frame.target, [4.0, 5.0, 6.0]);
        assert_eq!(frame.fov_deg, 30.0);
    }
}
