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

/// Starter scenes offered by the New Project dialog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenePreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub dem_name: Option<String>,
    pub grid: Option<[u32; 2]>,
    pub elevation_range: Option<[f32; 2]>,
    pub world_span: Option<[f64; 2]>,
    /// False when the DEM is missing from this checkout; the UI disables it.
    pub available: bool,
}

#[tauri::command]
pub fn scene_presets(forge: State<'_, Forge3dState>) -> Vec<ScenePreset> {
    let mut presets = Vec::new();
    for (id, name, dem_file, description) in crate::forge3d::SCENE_PRESETS {
        let seed = forge.terrain_seed(Some(id));
        presets.push(ScenePreset {
            id: id.to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
            dem_name: seed.as_ref().map(|seed| seed.name.clone()),
            grid: seed.as_ref().and_then(|seed| seed.dimensions),
            elevation_range: seed.as_ref().and_then(|seed| seed.elevation_range),
            world_span: seed.as_ref().and_then(|seed| seed.world_span_xz),
            available: seed.is_some(),
        });
        let _ = dem_file;
    }
    presets
}

#[tauri::command]
pub fn new_project(
    forge: State<'_, Forge3dState>,
    state: State<'_, AppState>,
    name: Option<String>,
    preset: Option<String>,
) -> Result<SessionState, String> {
    // A new project starts from the chosen bundled DEM (Bryce by default); the
    // core falls back to the primitive starter when none can be found.
    let seed = match preset.as_deref() {
        // The dialog's "Empty scene" really is empty: no DEM.
        Some("empty") => None,
        other => forge.terrain_seed(other),
    };
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

/// Move the playhead and evaluate the timeline into the project (view state:
/// not undoable, no autosave). Returns the snapshot so the UI re-renders with
/// the evaluated transforms, plus the clamped frame that was applied.
#[tauri::command]
pub fn set_playhead(state: State<'_, AppState>, frame: f32) -> Result<PlayheadResult, String> {
    let (frame, project_state) = lock(&state)?
        .set_playhead(frame)
        .map_err(|error| error.to_string())?;
    Ok(PlayheadResult {
        frame,
        state: project_state,
    })
}

/// What `set_playhead` returns: the new snapshot plus the clamped frame.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayheadResult {
    pub frame: f32,
    pub state: SessionState,
}

/// Check every referenced asset against the filesystem.
///
/// Read-only: the UI shows "missing"/"changed" next to each asset and offers
/// Relink; nothing here modifies the project. The pure half lives in
/// `studio_core::integrity`, so the rules are testable without disk.
#[tauri::command]
pub fn check_assets(
    state: State<'_, AppState>,
) -> Result<Vec<studio_core::AssetIntegrity>, String> {
    let session = lock(&state)?;
    let snapshot = session
        .state()
        .ok_or_else(|| "no project is open".to_owned())?;
    Ok(studio_core::asset_integrity(
        &snapshot.project,
        |path| match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
            Ok(_) => Err(format!("{path} is not a file")),
            Err(error) => Err(format!("{path}: {error}")),
        },
    ))
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
/// Cap on an HDRI pulled across IPC for the Material preview (base64 adds a
/// third, so this is a real memory bound).
const MAX_HDR_BYTES: u64 = 64 * 1024 * 1024;

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
    // `.hdr` is an asset kind too (the environment map); it is not a mesh.
    if !matches!(kind, AssetKind::Gltf | AssetKind::Glb) {
        return Err(format!(
            "{} is not a glTF file; import .gltf or .glb",
            path.display()
        ));
    }

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

/// Per-parameter capability table: what each material/light/camera parameter
/// actually reaches in the current renderers (studio-core is the one source).
#[tauri::command]
pub fn capability_table() -> studio_core::CapabilityTable {
    studio_core::parameter_capabilities()
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

// ---------------------------------------------------------------------- render

/// Render the current scene at `width` × `height` through the embedded
/// Forge3D pipeline (the same one the golden tests certify).
/// Replace the scene terrain's DEM with a raster the user picked; on an empty
/// scene, create the terrain (so the Add menu can start from a DEM too).
#[tauri::command]
pub fn import_terrain_dem(
    forge: State<'_, Forge3dState>,
    state: State<'_, AppState>,
    path: String,
) -> Result<SessionState, String> {
    let python = forge.probe(false).python;
    let info = crate::forge3d::sample_terrain(&python, &path, 48)?;
    let metadata = std::fs::metadata(&path).map_err(|error| format!("{path}: {error}"))?;

    let mut session = lock(&state)?;
    let snapshot = session
        .state()
        .ok_or_else(|| "no project is open".to_owned())?;

    let pair_f64 = |key: &str| -> Option<[f64; 2]> {
        let values = info.get(key)?.as_array()?;
        Some([values.first()?.as_f64()?, values.get(1)?.as_f64()?])
    };
    let dimensions = info
        .get("width")
        .and_then(|value| value.as_u64())
        .zip(info.get("height").and_then(|value| value.as_u64()))
        .map(|(width, height)| [width as u32, height as u32]);
    let elevation_range = info
        .get("minElevation")
        .and_then(|value| value.as_f64())
        .zip(info.get("maxElevation").and_then(|value| value.as_f64()))
        .map(|(low, high)| [low as f32, high as f32]);
    let asset = studio_core::Asset {
        id: studio_core::ids::new_id(),
        name: std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "terrain.tif".to_owned()),
        path: path.clone(),
        kind: studio_core::AssetKind::Dem,
        byte_size: metadata.len(),
    };

    let mut commands = vec![Command::AddAsset {
        asset: asset.clone(),
    }];
    match snapshot.project.terrain_object() {
        Some(terrain) => commands.push(Command::SetTerrainDem {
            id: terrain.id,
            asset: asset.id,
            dimensions,
            elevation_range,
            world_origin_xz: pair_f64("originXz"),
            world_span_xz: pair_f64("spanXz"),
        }),
        None => {
            let mut terrain = studio_core::TerrainObject::new(asset.id);
            if let (Some(dimensions), Some(elevation_range)) = (dimensions, elevation_range) {
                terrain = terrain.with_dem_info(
                    dimensions,
                    elevation_range,
                    pair_f64("originXz"),
                    pair_f64("spanXz"),
                );
            }
            let name = std::path::Path::new(&path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .filter(|stem| !stem.trim().is_empty())
                .unwrap_or_else(|| "Terrain".to_owned());
            commands.push(Command::CreateObject {
                object: studio_core::SceneObject {
                    id: studio_core::ids::new_id(),
                    name,
                    parent: None,
                    transform: studio_core::Transform::default(),
                    visible: true,
                    locked: false,
                    kind: studio_core::ObjectKind::Terrain(terrain),
                },
                index: Some(0),
            });
        }
    };

    session
        .apply(Command::Batch { commands })
        .map_err(|error| error.to_string())?;
    session
        .state()
        .ok_or_else(|| "no project is open".to_owned())
}

/// Size of a file on disk. The webview cannot stat, and asset registration
/// validates that files are non-empty (`byte_size == 0` reads as "empty file").
#[tauri::command]
pub fn file_size(path: String) -> Result<u64, String> {
    let metadata = std::fs::metadata(&path).map_err(|error| format!("{path}: {error}"))?;
    if metadata.is_dir() {
        return Err(format!("{path} is a directory, not a file"));
    }
    Ok(metadata.len())
}

/// Attach (or clear) a raster overlay on the scene terrain.
#[tauri::command]
pub fn set_terrain_overlay(
    state: State<'_, AppState>,
    path: Option<String>,
    opacity: Option<f32>,
) -> Result<SessionState, String> {
    let mut session = lock(&state)?;
    let snapshot = session
        .state()
        .ok_or_else(|| "no project is open".to_owned())?;
    let terrain = snapshot
        .project
        .terrain_object()
        .ok_or_else(|| "the scene has no terrain object".to_owned())?;
    let opacity = opacity.unwrap_or(0.82).clamp(0.0, 1.0);

    let mut commands = Vec::new();
    let overlay = match path {
        None => None,
        Some(path) => {
            let metadata = std::fs::metadata(&path).map_err(|error| format!("{path}: {error}"))?;
            let existing = snapshot
                .project
                .assets
                .iter()
                .find(|asset| asset.path == path)
                .cloned();
            let asset = existing.unwrap_or_else(|| studio_core::Asset {
                id: studio_core::ids::new_id(),
                name: std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "overlay.tif".to_owned()),
                path: path.clone(),
                kind: studio_core::AssetKind::Orthophoto,
                byte_size: metadata.len(),
            });
            if !snapshot
                .project
                .assets
                .iter()
                .any(|entry| entry.id == asset.id)
            {
                commands.push(Command::AddAsset {
                    asset: asset.clone(),
                });
            }
            Some(studio_core::scene::TerrainOverlay {
                asset: asset.id,
                opacity,
                preserve_colors: true,
            })
        }
    };
    commands.push(Command::SetTerrainOverlay {
        id: terrain.id,
        overlay,
    });

    session
        .apply(Command::Batch { commands })
        .map_err(|error| error.to_string())?;
    session
        .state()
        .ok_or_else(|| "no project is open".to_owned())
}

/// Read an image asset (PNG/JPEG) for the Solid viewport's textures.
#[tauri::command]
pub fn read_image_asset(path: String) -> Result<MeshFilePayload, String> {
    let lower = path.to_ascii_lowercase();
    if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
        return Err(format!(
            "{path} is not a PNG/JPEG image; GeoTIFF textures are not yet decoded in the viewport"
        ));
    }
    let kind = studio_core::AssetKind::Orthophoto;
    let bytes = std::fs::read(&path).map_err(|error| format!("could not read {path}: {error}"))?;
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "texture".to_owned());
    Ok(MeshFilePayload {
        path,
        name,
        kind,
        byte_size: bytes.len() as u64,
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

/// Raw bytes of a Radiance `.hdr` environment for the Material preview.
///
/// The webview cannot display RGBE with an `<img>`, so the shell reads the file
/// and the viewport parses it with three's `HDRLoader` before pre-filtering it
/// with PMREM. Rendered frames do not go through here: the engine reads the
/// path itself (`IBL.from_hdr`).
#[tauri::command]
pub fn read_hdr(path: String) -> Result<MeshFilePayload, String> {
    if AssetKind::from_path(std::path::Path::new(&path)) != Some(AssetKind::Hdr) {
        return Err(format!(
            "{path} is not a Radiance .hdr environment — export the HDRI as .hdr"
        ));
    }
    let bytes = std::fs::read(&path).map_err(|error| format!("could not read {path}: {error}"))?;
    if bytes.is_empty() {
        return Err(format!("{path} is empty"));
    }
    if bytes.len() as u64 > MAX_HDR_BYTES {
        return Err(format!(
            "{path} is {:.1} MB; the environment map limit is {} MB",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_HDR_BYTES / (1024 * 1024)
        ));
    }
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "environment.hdr".to_owned());
    Ok(MeshFilePayload {
        path,
        name,
        kind: AssetKind::Hdr,
        byte_size: bytes.len() as u64,
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

/// Decoded, downsampled PNG of a raster overlay for the Material preview's
/// terrain drape — the webview cannot decode GeoTIFF, so the engine python
/// does it and hands back a PNG the canvas can display.
#[tauri::command]
pub fn raster_preview(
    forge: State<'_, Forge3dState>,
    path: String,
    width: Option<u32>,
) -> Result<MeshFilePayload, String> {
    let python = forge.probe(false).python;
    let width = width.unwrap_or(2048).clamp(64, 4096);
    let bytes = crate::forge3d::raster_preview_png(&python, &path, width)?;
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "overlay".to_owned());
    Ok(MeshFilePayload {
        path,
        name,
        kind: studio_core::AssetKind::Orthophoto,
        byte_size: bytes.len() as u64,
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

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
    let (handoff, settings) = {
        let session = lock(&state)?;
        let snapshot = session
            .state()
            .ok_or_else(|| "no project is open".to_owned())?;
        (
            studio_core::workspace::handoff(
                &snapshot.project,
                snapshot.revision,
                Workspace::Forge3d,
            ),
            snapshot.project.render,
        )
    };
    let request = RenderRequest::from_handoff(
        &handoff,
        width.unwrap_or(960).clamp(64, 4096),
        height.unwrap_or(600).clamp(64, 4096),
    )?
    .with_view(view.as_ref())
    .with_settings(&settings);
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
    path: Option<String>,
    view: Option<RenderView>,
) -> Result<RenderFrame, String> {
    let (handoff, settings, project_name, project_path) = {
        let session = lock(&state)?;
        let snapshot = session
            .state()
            .ok_or_else(|| "no project is open".to_owned())?;
        (
            studio_core::workspace::handoff(
                &snapshot.project,
                snapshot.revision,
                Workspace::Forge3d,
            ),
            snapshot.project.render,
            snapshot.project.name.clone(),
            snapshot.path.clone(),
        )
    };

    // Default output lives with the project, under renders/, and carries a
    // sidecar describing exactly what produced it.
    let target = match path {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let base = project_path
                .as_deref()
                .map(std::path::Path::new)
                .and_then(|path| path.parent())
                .map(|dir| dir.join("renders"))
                .unwrap_or_else(|| std::env::temp_dir().join("forge3d-studio/renders"));
            let stamp = studio_core::now_unix_ms();
            let safe: String = project_name
                .chars()
                .map(|ch| {
                    if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                        ch
                    } else {
                        '-'
                    }
                })
                .collect();
            base.join(format!("{safe}-{stamp}.png"))
        }
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }

    let request = RenderRequest::from_handoff(&handoff, settings.width, settings.height)?
        .with_settings(&settings)
        .with_view(view.as_ref());
    let python = forge.probe(false).python;
    let mut frame = render.render(
        &python,
        &request.dem,
        request.to_payload("final", Some(&target.to_string_lossy())),
    )?;
    frame.path = Some(target.display().to_string());

    let sidecar = target.with_extension("json");
    let description = serde_json::json!({
        "renderer": "forge3d.render_terrain_pbr_pom",
        "width": request.width,
        "height": request.height,
        "exposure": request.exposure,
        "shadows": request.shadows,
        "ambientOcclusion": request.ambient_occlusion,
        "sky": request.sky,
        "iblIntensity": request.ibl_intensity,
        "environment": request.environment,
        "environmentBackgroundIntensity": request.environment_background_intensity,
        "camera": {
            "phiDeg": request.phi_deg,
            "thetaDeg": request.theta_deg,
            "radius": request.radius,
            "target": request.target,
            "fovDeg": request.fov_deg,
        },
        "sun": {
            "azimuthDeg": request.sun_azimuth_deg,
            "elevationDeg": request.sun_elevation_deg,
            "intensity": request.sun_intensity,
            "color": request.sun_color,
        },
        "terrain": {
            "dem": request.dem,
            "zScale": request.z_scale,
            "overlay": request.overlay.as_ref().map(|overlay| overlay.path.clone()),
        },
        "frameMs": frame.ms,
        "revision": handoff.revision,
        "handoffKey": handoff.idempotency_key,
    });
    if let Err(error) = std::fs::write(
        &sidecar,
        serde_json::to_vec_pretty(&description).unwrap_or_default(),
    ) {
        log::warn!("could not write {}: {error}", sidecar.display());
    }

    Ok(frame)
}

#[tauri::command]
pub fn render_status(render: State<'_, RenderState>) -> RenderStatus {
    render.status()
}

/// One frame of an offline sequence export: what the frontend's per-frame
/// loop calls. The shell evaluates the timeline on a **clone** at `frame`,
/// hands the evaluated camera to Forge3D, renders, and writes `output`.
///
/// The clone matters: exporting must not move the session's playhead or
/// dirty the project. Evaluation runs through the same
/// `studio_core::timeline::evaluate` the playhead uses, so the sequence is
/// byte-for-byte the state you scrubbed (the Phase 6 exit criterion).
///
/// One frame per IPC call keeps the command short-lived: the UI gets
/// progress and a working Stop for free, instead of one long blocking call.
// Tauri commands take their IPC fields as arguments; the wire schema is the
// point, so the argument count is not a code smell here.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn render_animation_frame(
    render: State<'_, RenderState>,
    forge: State<'_, Forge3dState>,
    state: State<'_, AppState>,
    frame: f32,
    width: u32,
    height: u32,
    quality: Option<String>,
    output: String,
) -> Result<WrittenFrame, String> {
    // Evaluate a clone: the session's own playhead stays where the user left it.
    let (mut project, revision) = {
        let session = lock(&state)?;
        let snapshot = session
            .state()
            .ok_or_else(|| "no project is open".to_owned())?;
        (snapshot.project, snapshot.revision)
    };
    let frame = studio_core::timeline::evaluate(&mut project, frame);

    let handoff = studio_core::workspace::handoff(&project, revision, Workspace::Forge3d);
    // The evaluated camera's own view, not the viewport's live orbit: a frame
    // must match a camera-view still of the same object (the export used to
    // fall back to `terrain_camera`, which ignores the camera's rotation and
    // distance). `with_view` also carries the FOV and the terrain-centred aim.
    let view = studio_core::workspace::camera_render_view(&project);
    let request =
        RenderRequest::from_handoff(&handoff, width.clamp(64, 4096), height.clamp(64, 4096))?
            .with_view(view.as_ref())
            .with_settings(&project.render);

    let target = std::path::PathBuf::from(&output);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }

    let python = forge.probe(false).python;
    let quality = quality.unwrap_or_else(|| "final".to_owned());
    let mut written = render.render(
        &python,
        &request.dem,
        request.to_payload(&quality, Some(&target.to_string_lossy())),
    )?;
    written.path = Some(target.display().to_string());

    Ok(WrittenFrame {
        path: target.display().to_string(),
        ms: written.ms,
        evaluated_frame: frame,
    })
}

/// One rendered frame of a sequence: where it landed and how long it took.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenFrame {
    pub path: String,
    pub ms: f64,
    /// The clamped timeline frame actually rendered (differs from the
    /// request only when the request was out of range).
    pub evaluated_frame: f32,
}

#[tauri::command]
pub fn render_stop(render: State<'_, RenderState>) -> RenderStatus {
    render.stop();
    render.status()
}
