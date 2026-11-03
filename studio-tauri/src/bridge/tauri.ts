import { invoke } from "@tauri-apps/api/core";

import type { StudioBridge } from "./index";
import type {
  AutosaveStatus,
  EditResult,
  EngineStatus,
  MeshFilePayload,
  RecoveryEntry,
  RenderFrame,
  RenderStatus,
  ScenePreset,
  SessionState,
  ShellInfo,
  TerrainPreview,
} from "./types";

/** The real bridge: Tauri IPC. Argument names match the Rust command params. */
export const tauriBridge: StudioBridge = {
  kind: "tauri",

  info: () => invoke<ShellInfo>("shell_info"),

  state: () => invoke<SessionState | null>("project_state"),

  newProject: (name, preset) =>
    invoke<SessionState>("new_project", { name: name ?? null, preset: preset ?? null }),

  scenePresets: () => invoke<ScenePreset[]>("scene_presets"),

  openProject: (path) =>
    invoke<SessionState>("open_project", { path }),

  saveProject: (path) =>
    invoke<SessionState>("save_project", { path: path ?? null }),

  closeProject: () => invoke<null>("close_project"),

  applyCommand: (command) =>
    invoke<EditResult>("apply_command", { command }),

  undo: () => invoke<EditResult>("undo"),

  redo: () => invoke<EditResult>("redo"),

  setSelection: (ids) => invoke<SessionState>("set_selection", { ids }),

  autosaveNow: () => invoke("autosave_now"),

  autosaveStatus: () => invoke<AutosaveStatus>("autosave_status"),

  listRecovery: () => invoke<RecoveryEntry[]>("list_recovery"),

  restoreRecovery: (id) =>
    invoke<SessionState>("restore_recovery", { id }),

  discardRecovery: (id) => invoke<void>("discard_recovery", { id }),

  readMeshFile: (path) => invoke<MeshFilePayload>("read_mesh_file", { path }),

  readImageAsset: (path) => invoke<MeshFilePayload>("read_image_asset", { path }),

  terrainPreview: (path, resolution) =>
    invoke<TerrainPreview>("terrain_preview", { path, resolution: resolution ?? null }),

  importTerrainDem: (path) => invoke<SessionState>("import_terrain_dem", { path }),

  setTerrainOverlay: (path, opacity) =>
    invoke<SessionState>("set_terrain_overlay", { path, opacity: opacity ?? null }),

  forge3dProbe: (force) => invoke<EngineStatus>("forge3d_probe", { force: force ?? false }),

  renderFrame: (width, height, quality, view) =>
    invoke<RenderFrame>("render_frame", {
      width,
      height,
      quality: quality ?? "final",
      view: view ?? null,
    }),

  renderStill: (path) => invoke<RenderFrame>("render_still", { path: path ?? null }),

  renderStatus: () => invoke<RenderStatus>("render_status"),

  renderStop: () => invoke<RenderStatus>("render_stop"),
};
