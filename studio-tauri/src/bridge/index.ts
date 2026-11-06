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
  AssetIntegrity,
  AutosaveStatus,
  CapabilityTable,
  Command,
  EditResult,
  EngineStatus,
  PlayheadResult,
  Project,
  RecoveryEntry,
  SessionState,
  MeshFilePayload,
  RenderFrame,
  RenderStatus,
  ScenePreset,
  SolarResult,
  RenderView,
  ShellInfo,
  TerrainPreview,
  AnimationFrameRequest,
  WrittenFrame,
} from "./types";

export interface StudioBridge {
  readonly kind: "tauri" | "mock";
  info(): Promise<ShellInfo>;
  /** Per-parameter capability table; `null` in the browser preview (the mock
   *  has no shell — the Rust table is covered by studio-core tests). */
  capabilityTable(): Promise<CapabilityTable | null>;
  state(): Promise<SessionState | null>;
  newProject(name?: string | null, preset?: string | null): Promise<SessionState>;
  scenePresets(): Promise<ScenePreset[]>;
  openProject(path: string): Promise<SessionState>;
  saveProject(path?: string | null): Promise<SessionState>;
  closeProject(): Promise<null>;
  applyCommand(command: Command): Promise<EditResult>;
  undo(): Promise<EditResult>;
  redo(): Promise<EditResult>;
  setSelection(ids: string[]): Promise<SessionState>;
  /** Move the playhead and evaluate the timeline into the project (view
   *  state: not undoable, no autosave). Returns the snapshot plus the
   *  clamped frame that was applied. */
  setPlayhead(frame: number): Promise<PlayheadResult>;
  /** Place a directional light from the engine's solar ephemeris. One
   *  undoable transform; the date/place are inputs, not project state. */
  setSunFromSolar(
    id: string,
    latitude: number,
    longitude: number,
    datetimeUtc: string,
  ): Promise<SolarResult>;
  /** Render one frame of an offline sequence to `output` (PNG). The shell
   *  evaluates the timeline on a clone, so the session's playhead does not
   *  move. Desktop shell only. */
  renderAnimationFrame(request: AnimationFrameRequest): Promise<WrittenFrame>;
  autosaveNow(): Promise<unknown>;
  autosaveStatus(): Promise<AutosaveStatus>;
  listRecovery(): Promise<RecoveryEntry[]>;
  restoreRecovery(id: string): Promise<SessionState>;
  discardRecovery(id: string): Promise<void>;
  /** Read a `.gltf`/`.glb` file for import. Desktop shell only. */
  readMeshFile(path: string): Promise<MeshFilePayload>;
  /** Read a PNG/JPEG for a viewport texture. */
  readImageAsset(path: string): Promise<MeshFilePayload>;
  /** Raw Radiance `.hdr` bytes for the Material preview's PMREM. */
  readHdr(path: string): Promise<MeshFilePayload>;
  /** Decoded, downsampled PNG of a raster overlay — the Material preview
   *  drapes this on the terrain (the webview cannot decode GeoTIFF). */
  rasterPreview(path: string, width?: number): Promise<MeshFilePayload>;
  /** Size of a file on disk — asset registration validates non-empty files. */
  fileSize(path: string): Promise<number>;
  /** Existence and size of every referenced asset (read-only). */
  checkAssets(): Promise<AssetIntegrity[]>;
  /** Sample a DEM into a heightfield for the Studio viewport. */
  terrainPreview(path: string, resolution?: number): Promise<TerrainPreview>;
  /** Replace the scene terrain's DEM with a raster the user picked — or add
   *  the terrain when the scene has none yet. */
  importTerrainDem(path: string): Promise<SessionState>;
  /** Attach (`path`) or clear (`null`) a raster overlay on the terrain. */
  setTerrainOverlay(path: string | null, opacity?: number): Promise<SessionState>;

  // Forge3D workspace adapter
  forge3dProbe(force?: boolean): Promise<EngineStatus>;

  // Embedded renderer (Rendered viewport mode)
  renderFrame(
    width: number,
    height: number,
    quality?: "preview" | "final",
    view?: RenderView,
  ): Promise<RenderFrame>;
  /** Render a full-resolution still; without a path it goes to renders/. */
  renderStill(path?: string | null, view?: RenderView): Promise<RenderFrame>;
  renderStatus(): Promise<RenderStatus>;
  renderStop(): Promise<RenderStatus>;
}

export type { Project };
