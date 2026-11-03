//! Forge3D workspace adapter: engine probe, viewer process, and diagnostics.
//!
//! The viewer is a **separate process with a window**, per the plan — it is not
//! embedded in the Studio webview. Studio keeps the canonical project; this
//! module only moves supported state into that process and reports what could
//! not be moved.
//!
//! Process control is deliberately honest: if the engine or the viewer binary
//! is missing, the probe says exactly what is missing and what to build. Nothing
//! here fakes a running viewer.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use studio_core::workspace::Handoff;
use studio_core::AssetKind;

/// Embedded so the installed app does not need to ship a separate Python file.
const HELPER_SCRIPT: &str = include_str!("studio_bridge.py");

const MAX_DIAGNOSTICS: usize = 300;
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const READY_TIMEOUT: Duration = Duration::from_secs(45);

/// Probe program: reports the package version and whether the viewer binary
/// resolves, without opening a window.
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub available: bool,
    /// Interpreter that was probed successfully (or the last one tried).
    pub python: String,
    pub package_version: Option<String>,
    pub viewer_binary: Option<String>,
    /// Always present: what is missing, or what is ready.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forge3dStatus {
    pub running: bool,
    pub ready: bool,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub last_handoff_key: Option<String>,
    pub payload_path: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
struct Inner {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pid: Option<u32>,
    ready: bool,
    error: Option<String>,
    last_handoff_key: Option<String>,
    payload_path: Option<PathBuf>,
    next_id: u64,
}

pub struct Forge3dState {
    engine: Mutex<Option<EngineStatus>>,
    inner: Arc<Mutex<Inner>>,
    log: Arc<Mutex<VecDeque<String>>>,
}

impl Default for Forge3dState {
    fn default() -> Self {
        Self::new()
    }
}

impl Forge3dState {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(None),
            inner: Arc::new(Mutex::new(Inner::default())),
            log: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Probe the engine once and cache the result.
    pub fn probe(&self, force: bool) -> EngineStatus {
        if !force {
            if let Some(cached) = self.engine.lock().ok().and_then(|guard| guard.clone()) {
                return cached;
            }
        }

        let mut last = EngineStatus {
            available: false,
            python: "python3".to_owned(),
            package_version: None,
            viewer_binary: None,
            detail: "no Python interpreter was found".to_owned(),
        };

        for python in interpreter_candidates() {
            match probe_interpreter(&python) {
                Ok(status) if status.available => {
                    if let Ok(mut guard) = self.engine.lock() {
                        *guard = Some(status.clone());
                    }
                    return status;
                }
                Ok(status) => last = status,
                Err(detail) => {
                    last = EngineStatus {
                        python: python.clone(),
                        detail,
                        ..last
                    }
                }
            }
        }

        push_log(&self.log, format!("engine probe: {}", last.detail));
        if let Ok(mut guard) = self.engine.lock() {
            *guard = Some(last.clone());
        }
        last
    }

    pub fn status(&self) -> Forge3dStatus {
        let inner = self.inner.lock().ok();
        let diagnostics = self
            .log
            .lock()
            .map(|log| log.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        match inner {
            Some(inner) => Forge3dStatus {
                running: inner.child.is_some(),
                ready: inner.ready,
                pid: inner.pid,
                error: inner.error.clone(),
                last_handoff_key: inner.last_handoff_key.clone(),
                payload_path: inner
                    .payload_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                diagnostics,
            },
            None => Forge3dStatus {
                running: false,
                ready: false,
                pid: None,
                error: Some("the Forge3D adapter is unavailable".to_owned()),
                last_handoff_key: None,
                payload_path: None,
                diagnostics,
            },
        }
    }

    /// Start the viewer for a handoff. Idempotent: launching the same revision
    /// twice reuses the running window instead of opening a second one.
    pub fn launch(
        &self,
        handoff: &Handoff,
        app_data: &Path,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        {
            let inner = self.inner.lock().map_err(lock_error)?;
            if inner.child.is_some()
                && inner.ready
                && inner.last_handoff_key.as_deref() == Some(handoff.idempotency_key.as_str())
            {
                push_log(
                    &self.log,
                    "handoff unchanged; reusing the running viewer".to_owned(),
                );
                return Ok(());
            }
        }
        self.stop();

        let engine = self.probe(false);
        if !engine.available {
            return Err(engine.detail);
        }

        let directory = app_data.join("forge3d");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let payload_path = directory.join("handoff.json");
        let payload = serde_json::to_vec_pretty(handoff)
            .map_err(|error| format!("could not serialise the handoff: {error}"))?;
        std::fs::write(&payload_path, payload)
            .map_err(|error| format!("could not write {}: {error}", payload_path.display()))?;

        let helper_path = directory.join("studio_bridge.py");
        std::fs::write(&helper_path, HELPER_SCRIPT)
            .map_err(|error| format!("could not write {}: {error}", helper_path.display()))?;

        let gltf = handoff
            .assets
            .iter()
            .find(|asset| matches!(asset.kind, AssetKind::Gltf | AssetKind::Glb))
            .map(|asset| asset.path.clone());

        push_log(
            &self.log,
            format!(
                "starting viewer with {}",
                gltf.clone().unwrap_or_else(|| "no asset".to_owned())
            ),
        );

        let mut child = Command::new(&engine.python)
            .arg(&helper_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", engine.python))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "the bridge process has no stdin".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "the bridge process has no stdout".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "the bridge process has no stderr".to_owned())?;

        let config = json!({
            "width": width,
            "height": height,
            "fovDeg": handoff.camera.map(|camera| camera.fov_deg).unwrap_or(60.0),
            "gltfPath": gltf,
            "timeout": 30,
        });
        writeln!(stdin, "{config}")
            .map_err(|error| format!("could not configure the bridge: {error}"))?;
        stdin
            .flush()
            .map_err(|error| format!("could not configure the bridge: {error}"))?;

        {
            let mut inner = self.inner.lock().map_err(lock_error)?;
            inner.pid = Some(child.id());
            inner.child = Some(child);
            inner.stdin = Some(stdin);
            inner.ready = false;
            inner.error = None;
            inner.payload_path = Some(payload_path.clone());
        }

        spawn_stdout_reader(stdout, Arc::clone(&self.inner), Arc::clone(&self.log));
        spawn_stderr_reader(stderr, Arc::clone(&self.log));

        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            {
                let inner = self.inner.lock().map_err(lock_error)?;
                if inner.ready {
                    break;
                }
                if let Some(error) = inner.error.clone() {
                    drop(inner);
                    self.stop();
                    return Err(error);
                }
                if inner.child.is_none() {
                    return Err("the Forge3D viewer exited before it was ready".to_owned());
                }
            }
            if Instant::now() > deadline {
                self.stop();
                return Err(format!(
                    "the Forge3D viewer did not report ready within {}s",
                    READY_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(80));
        }

        let mut inner = self.inner.lock().map_err(lock_error)?;
        inner.last_handoff_key = Some(handoff.idempotency_key.clone());
        push_log(&self.log, "viewer ready".to_owned());
        Ok(())
    }

    /// Send camera/field-of-view/sun state to the running viewer.
    pub fn sync_camera(&self, handoff: &Handoff) -> Result<(), String> {
        if let Some(camera) = handoff.camera {
            self.send_payload(json!({
                "cmd": "cam_lookat",
                "eye": [camera.eye.x, camera.eye.y, camera.eye.z],
                "target": [camera.target.x, camera.target.y, camera.target.z],
                "up": [camera.up.x, camera.up.y, camera.up.z],
            }))?;
            self.send_payload(json!({ "cmd": "set_fov", "deg": camera.fov_deg }))?;
        }
        if let Some(sun) = handoff.sun {
            self.send_payload(json!({
                "cmd": "lit_sun",
                "azimuth_deg": sun.azimuth_deg,
                "elevation_deg": sun.elevation_deg,
            }))?;
        }
        Ok(())
    }

    pub fn send_payload(&self, payload: serde_json::Value) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(lock_error)?;
        inner.next_id += 1;
        let id = inner.next_id;
        let line = json!({ "id": id, "cmd": "call", "payload": payload }).to_string();
        let stdin = inner
            .stdin
            .as_mut()
            .ok_or_else(|| "the Forge3D viewer is not running".to_owned())?;
        writeln!(stdin, "{line}")
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("lost the connection to the Forge3D viewer: {error}"))
    }

    pub fn stop(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if let Some(mut stdin) = inner.stdin.take() {
            let _ = writeln!(stdin, "{}", json!({"id": 0, "cmd": "close"}));
            let _ = stdin.flush();
        }
        if let Some(mut child) = inner.child.take() {
            // Give the bridge a moment to close the viewer itself, then insist.
            for _ in 0..10 {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.ready = false;
        inner.pid = None;
        inner.last_handoff_key = None;
        inner.payload_path = None;
        drop(inner);
        push_log(&self.log, "viewer stopped".to_owned());
    }
}

fn lock_error<T>(_: T) -> String {
    "the Forge3D adapter is unavailable after an internal error".to_owned()
}

fn push_log(log: &Arc<Mutex<VecDeque<String>>>, line: String) {
    if let Ok(mut log) = log.lock() {
        log.push_back(line);
        while log.len() > MAX_DIAGNOSTICS {
            log.pop_front();
        }
    }
}

fn spawn_stdout_reader(
    stdout: impl Read + Send + 'static,
    inner: Arc<Mutex<Inner>>,
    log: Arc<Mutex<VecDeque<String>>>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            push_log(&log, line.clone());
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match value.get("event").and_then(|event| event.as_str()) {
                Some("ready") => {
                    if let Ok(mut inner) = inner.lock() {
                        inner.ready = true;
                    }
                }
                Some("error") => {
                    let message = value
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("the Forge3D viewer failed")
                        .to_owned();
                    if let Ok(mut inner) = inner.lock() {
                        inner.error = Some(message);
                    }
                }
                _ => {}
            }
        }
        if let Ok(mut inner) = inner.lock() {
            inner.ready = false;
        }
        push_log(&log, "bridge process closed".to_owned());
    });
}

fn spawn_stderr_reader(stderr: impl Read + Send + 'static, log: Arc<Mutex<VecDeque<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            push_log(&log, format!("stderr: {line}"));
        }
    });
}

/// Interpreter candidates, most specific first.
fn interpreter_candidates() -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(python) = std::env::var_os("FORGE3D_STUDIO_PYTHON") {
        candidates.push(python.to_string_lossy().into_owned());
    }

    if let Ok(current) = std::env::current_dir() {
        for ancestor in current.ancestors().take(6) {
            let venv = ancestor.join(".venv").join("bin").join("python");
            if venv.is_file() {
                candidates.push(venv.to_string_lossy().into_owned());
                break;
            }
        }
    }

    candidates.push("python3".to_owned());
    candidates.dedup();
    candidates
}

fn probe_interpreter(python: &str) -> Result<EngineStatus, String> {
    let mut child = Command::new(python)
        .arg("-c")
        .arg(PROBE_SCRIPT)
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
