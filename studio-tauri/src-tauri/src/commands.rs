//! Tauri commands: the only way the UI can touch project state.
//!
//! Every command returns the full [`SessionState`] snapshot rather than a delta.
//! Phase 1 scenes are small and this removes any chance of the window holding a
//! divergent copy of the scene.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;
use studio_core::autosave::RecoveryEntry;
use studio_core::command::Command;
use studio_core::editor::{Applied, SessionState};
use studio_core::ids::ObjectId;
use studio_core::project::AssetKind;
use tauri::State;

use studio_core::workspace::Handoff;
use studio_core::{AutosaveReport, Session, Workspace, AUTOSAVE_INTERVAL_SECONDS};

use crate::forge3d::{EngineStatus, Forge3dState};
use crate::render::{RenderFrame, RenderRequest, RenderState, RenderStatus, RenderView};
use crate::session::AppState;

/// Edit result: what the command did, plus the new snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditResult {
    pub applied: Applied,
    pub state: SessionState,
}

/// Autosave health, shown in the status bar instead of failing silently.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutosaveStatus {
    pub interval_seconds: u64,
    pub last_write_unix_ms: Option<u64>,
    pub last_error: Option<String>,
    pub directory: String,
}

fn lock<'a>(state: &'a State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Session>, String> {
    state
        .session
        .lock()
        .map_err(|_| "the project session is unavailable after an internal error".to_owned())
}

#[tauri::command]
pub fn project_state(state: State<'_, AppState>) -> Result<Option<SessionState>, String> {
    Ok(lock(&state)?.state())
}

#[tauri::command]
pub fn new_project(
    forge: State<'_, Forge3dState>,
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<SessionState, String> {
    // A new project starts from the bundled DEM when one can be found; the core
    // falls back to the primitive starter when it cannot.
    let seed = forge.default_terrain_seed();
    Ok(lock(&state)?.new_project_seeded(name, seed))
}

#[tauri::command]
pub fn open_project(state: State<'_, AppState>, path: String) -> Result<SessionState, String> {
    lock(&state)?
        .open_project(Path::new(&path))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_project(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<SessionState, String> {
    let mut session = lock(&state)?;
    let saved = session
        .save_project(path.map(PathBuf::from))
        .map_err(|error| error.to_string())?;

    // The file on disk is now current: snapshots for it are stale.
    if let Some(saved_path) = saved.path.clone() {
        if let Err(error) = session.prune_recovery_for(&state.autosave, Path::new(&saved_path)) {
            log::warn!("failed to prune recovery snapshots: {error}");
        }
    }
    Ok(saved)
}

#[tauri::command]
pub fn close_project(state: State<'_, AppState>) -> Result<Option<SessionState>, String> {
    let mut session = lock(&state)?;
    session.close_project();
    Ok(None)
}

#[tauri::command]
pub fn apply_command(state: State<'_, AppState>, command: Command) -> Result<EditResult, String> {
    let (applied, snapshot) = lock(&state)?
        .apply(command)
        .map_err(|error| error.to_string())?;
    Ok(EditResult {
        applied,
        state: snapshot,
    })
}

#[tauri::command]
pub fn undo(state: State<'_, AppState>) -> Result<EditResult, String> {
    let (applied, snapshot) = lock(&state)?.undo().map_err(|error| error.to_string())?;
    Ok(EditResult {
        applied,
        state: snapshot,
    })
}

#[tauri::command]
pub fn redo(state: State<'_, AppState>) -> Result<EditResult, String> {
    let (applied, snapshot) = lock(&state)?.redo().map_err(|error| error.to_string())?;
    Ok(EditResult {
        applied,
        state: snapshot,
    })
}

#[tauri::command]
pub fn set_selection(
    state: State<'_, AppState>,
    ids: Vec<ObjectId>,
) -> Result<SessionState, String> {
    lock(&state)?
        .set_selection(ids)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn autosave_now(state: State<'_, AppState>) -> Result<Option<AutosaveReport>, String> {
    lock(&state)?
        .autosave_if_needed(&state.autosave)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn autosave_status(state: State<'_, AppState>) -> Result<AutosaveStatus, String> {
    let session = lock(&state)?;
    Ok(AutosaveStatus {
        interval_seconds: AUTOSAVE_INTERVAL_SECONDS,
        last_write_unix_ms: session.last_report().map(|report| report.written_unix_ms),
        last_error: session.last_error().map(str::to_owned),
        directory: state.autosave.dir().display().to_string(),
    })
}

#[tauri::command]
pub fn list_recovery(state: State<'_, AppState>) -> Result<Vec<RecoveryEntry>, String> {
    state.autosave.list().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn restore_recovery(state: State<'_, AppState>, id: String) -> Result<SessionState, String> {
    let project = state
        .autosave
        .load(&id)
        .map_err(|error| error.to_string())?;
    let mut session = lock(&state)?;
    let entries = state.autosave.list().map_err(|error| error.to_string())?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("recovery snapshot {id} no longer exists"))?;
    Ok(session.restore_recovery(&entry, project))
}

#[tauri::command]
pub fn discard_recovery(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .autosave
        .discard(&id)
        .map_err(|error| error.to_string())
}

/// Imports are read whole into memory. Phase 5 streams large assets; until
/// then the cap is what keeps a 2 GB GLB from freezing the window.
const MAX_MESH_BYTES: u64 = 32 * 1024 * 1024;

/// A mesh file the viewport can parse with GLTFLoader.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshFilePayload {
    pub path: String,
    pub name: String,
    pub kind: AssetKind,
    pub byte_size: u64,
    /// Base64 of the file bytes. IPC carries JSON, and glTF must reach the
    /// webview intact; the cap above bounds the cost.
    pub base64: String,
}

/// Read a `.gltf`/`.glb` file for import.
///
/// The UI parses the bytes before registering anything, so a corrupt file
/// never reaches the project model.
#[tauri::command]
pub fn read_mesh_file(path: String) -> Result<MeshFilePayload, String> {
    let path = PathBuf::from(path);
    let kind = AssetKind::from_path(&path).ok_or_else(|| {
        format!(
            "{} is not a glTF file; import .gltf or .glb",
            path.display()
        )
    })?;

    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("{} is empty", path.display()));
    }
    if metadata.len() > MAX_MESH_BYTES {
        return Err(format!(
            "{} is {:.1} MB; imports are limited to {} MB for now",
            path.display(),
            metadata.len() as f64 / (1024.0 * 1024.0),
            MAX_MESH_BYTES / (1024 * 1024)
        ));
    }

    let bytes = std::fs::read(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".to_owned());

    Ok(MeshFilePayload {
        path: path.to_string_lossy().into_owned(),
        name,
        kind,
        byte_size: bytes.len() as u64,
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

/// Sample a DEM into a heightfield for the Studio viewport.
///
/// The sample is a preview, not the render: Forge3D still receives the original
/// GeoTIFF. Returns the sampler's JSON so the UI reads exactly what was measured
/// (dimensions, elevation range, extent, heights).
#[tauri::command]
pub fn terrain_preview(
    forge: State<'_, Forge3dState>,
    path: String,
    resolution: Option<u32>,
) -> Result<serde_json::Value, String> {
    let python = forge.probe(false).python;
    crate::forge3d::sample_terrain(&python, &path, resolution.unwrap_or(96))
}

/// Cheap health check used by the UI on boot; also proves the bridge works.
#[tauri::command]
pub fn shell_info() -> ShellInfo {
    ShellInfo {
        name: "Forge3D Studio".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        phase: "Phase 1 — desktop shell and project model".to_owned(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellInfo {
    pub name: String,
    pub version: String,
    pub phase: String,
}

// ---------------------------------------------------------------------- Forge3D

/// Probe the Forge3D engine without opening anything.
#[tauri::command]
pub fn forge3d_probe(forge: State<'_, Forge3dState>, force: Option<bool>) -> EngineStatus {
    forge.probe(force.unwrap_or(false))
}

fn current_handoff(state: &State<'_, AppState>) -> Result<Handoff, String> {
    let session = lock(state)?;
    let snapshot = session
        .state()
        .ok_or_else(|| "no project is open".to_owned())?;
    Ok(studio_core::workspace::handoff(
        &snapshot.project,
        snapshot.revision,
        Workspace::Forge3d,
    ))
}

// ---------------------------------------------------------------------- render

/// Render the current scene at `width` × `height` through the embedded
/// Forge3D pipeline (the same one the golden tests certify).
#[tauri::command]
pub fn render_frame(
    render: State<'_, RenderState>,
    forge: State<'_, Forge3dState>,
    state: State<'_, AppState>,
    width: Option<u32>,
    height: Option<u32>,
    quality: Option<String>,
    view: Option<RenderView>,
) -> Result<RenderFrame, String> {
    let handoff = current_handoff(&state)?;
    let request = RenderRequest::from_handoff(
        &handoff,
        width.unwrap_or(960).clamp(64, 4096),
        height.unwrap_or(600).clamp(64, 4096),
    )?
    .with_view(view.as_ref());
    let python = forge.probe(false).python;
    let quality = quality.unwrap_or_else(|| "final".to_owned());
    render.render(&python, &request.dem, request.to_payload(&quality, None))
}

/// Render a final still at the project's configured resolution and write it to
/// `path` (the shell picks the path with a save dialog).
#[tauri::command]
pub fn render_still(
    render: State<'_, RenderState>,
    forge: State<'_, Forge3dState>,
    state: State<'_, AppState>,
    path: String,
) -> Result<RenderFrame, String> {
    let (handoff, width, height) = {
        let session = lock(&state)?;
        let snapshot = session
            .state()
            .ok_or_else(|| "no project is open".to_owned())?;
        let plan = studio_core::workspace::handoff(
            &snapshot.project,
            snapshot.revision,
            Workspace::Forge3d,
        );
        (
            plan,
            snapshot.project.render.width,
            snapshot.project.render.height,
        )
    };
    let request = RenderRequest::from_handoff(&handoff, width, height)?;
    let python = forge.probe(false).python;
    let frame = render.render(
        &python,
        &request.dem,
        request.to_payload("final", Some(&path)),
    )?;
    Ok(frame)
}

#[tauri::command]
pub fn render_status(render: State<'_, RenderState>) -> RenderStatus {
    render.status()
}

#[tauri::command]
pub fn render_stop(render: State<'_, RenderState>) -> RenderStatus {
    render.stop();
    render.status()
}
