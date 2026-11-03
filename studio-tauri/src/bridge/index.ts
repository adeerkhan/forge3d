/**
 * The bridge between the React UI and the Rust shell.
 *
 * Inside Tauri this is `invoke`. In a browser (`npm run dev` without Tauri, or
 * the visual harness) it falls back to an in-memory mock so the UI can be
 * developed and reviewed without a webkit runtime. The mock is a preview
 * harness, never the source of truth: it is selected only when the Tauri
 * internals are absent.
 */

import type {
  AutosaveStatus,
  Command,
  EditResult,
  EngineStatus,
  Forge3dStatus,
  Handoff,
  Project,
  RecoveryEntry,
  SessionState,
  MeshFilePayload,
  RenderFrame,
  RenderStatus,
  ShellInfo,
  TerrainPreview,
} from "./types";

export interface StudioBridge {
  readonly kind: "tauri" | "mock";
  info(): Promise<ShellInfo>;
  state(): Promise<SessionState | null>;
  newProject(name?: string | null): Promise<SessionState>;
  openProject(path: string): Promise<SessionState>;
  saveProject(path?: string | null): Promise<SessionState>;
  closeProject(): Promise<null>;
  applyCommand(command: Command): Promise<EditResult>;
  undo(): Promise<EditResult>;
  redo(): Promise<EditResult>;
  setSelection(ids: string[]): Promise<SessionState>;
  autosaveNow(): Promise<unknown>;
  autosaveStatus(): Promise<AutosaveStatus>;
  listRecovery(): Promise<RecoveryEntry[]>;
  restoreRecovery(id: string): Promise<SessionState>;
  discardRecovery(id: string): Promise<void>;
  /** Read a `.gltf`/`.glb` file for import. Desktop shell only. */
  readMeshFile(path: string): Promise<MeshFilePayload>;
  /** Sample a DEM into a heightfield for the Studio viewport. */
  terrainPreview(path: string, resolution?: number): Promise<TerrainPreview>;

  // Forge3D workspace adapter
  forge3dProbe(force?: boolean): Promise<EngineStatus>;
  forge3dStatus(): Promise<Forge3dStatus>;
  forge3dHandoff(): Promise<Handoff>;
  forge3dLaunch(width?: number, height?: number): Promise<Forge3dStatus>;
  forge3dSyncCamera(): Promise<Forge3dStatus>;
  forge3dStop(): Promise<Forge3dStatus>;

  // Embedded renderer (Rendered viewport mode)
  renderFrame(width: number, height: number): Promise<RenderFrame>;
  renderStatus(): Promise<RenderStatus>;
  renderStop(): Promise<RenderStatus>;
}

export interface OpenResult {
  path: string | null;
}

export type { Project };
