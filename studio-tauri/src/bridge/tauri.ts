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
  SessionState,
  ShellInfo,
  TerrainPreview,
} from "./types";

/** The real bridge: Tauri IPC. Argument names match the Rust command params. */
export const tauriBridge: StudioBridge = {
  kind: "tauri",

  info: () => invoke<ShellInfo>("shell_info"),

  state: () => invoke<SessionState | null>("project_state"),

  newProject: (name) =>
    invoke<SessionState>("new_project", { name: name ?? null }),

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

  terrainPreview: (path, resolution) =>
    invoke<TerrainPreview>("terrain_preview", { path, resolution: resolution ?? null }),

  forge3dProbe: (force) => invoke<EngineStatus>("forge3d_probe", { force: force ?? false }),

  renderFrame: (width, height) =>
    invoke<RenderFrame>("render_frame", { width, height }),

  renderStatus: () => invoke<RenderStatus>("render_status"),

  renderStop: () => invoke<RenderStatus>("render_stop"),
};
