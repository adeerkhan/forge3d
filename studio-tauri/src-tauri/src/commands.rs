//! Tauri commands: the only way the UI can touch project state.
//!
//! Every command returns the full [`SessionState`] snapshot rather than a delta.
//! Phase 1 scenes are small and this removes any chance of the window holding a
//! divergent copy of the scene.

use std::path::{Path, PathBuf};

use serde::Serialize;
use studio_core::autosave::RecoveryEntry;
use studio_core::command::Command;
use studio_core::editor::{Applied, SessionState};
use studio_core::ids::ObjectId;
use tauri::State;

use studio_core::{AutosaveReport, Session, AUTOSAVE_INTERVAL_SECONDS};

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
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<SessionState, String> {
    Ok(lock(&state)?.new_project(name))
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
