import { createContext, useContext } from "react";

import type {
  Applied,
  AutosaveStatus,
  Command,
  EngineStatus,
  PrimitiveShape,
  RecoveryEntry,
  RenderFrame,
  ScenePreset,
  SessionState,
  ShellInfo,
  Tool,
  ToolSpace,
  ViewportMode,
} from "../bridge/types";

export interface SessionApi {
  bridgeKind: "tauri" | "mock";
  info: ShellInfo | null;
  state: SessionState | null;
  autosave: AutosaveStatus | null;
  recovery: RecoveryEntry[];
  error: string | null;
  notice: string | null;
  busy: boolean;
  /** Active manipulation tool. UI state, not project state. */
  tool: Tool;
  setTool(tool: Tool): void;
  /** Gizmo orientation. UI state, not project state. */
  space: ToolSpace;
  setSpace(space: ToolSpace): void;
  /** Look through the scene's active camera instead of the free view. */
  cameraView: boolean;
  setCameraView(enabled: boolean): void;
  /** Forge3D engine discovery (renderer + probe). */
  engine: EngineStatus | null;
  /** Heightfield resolution used by the Solid terrain preview. */
  terrainResolution: number;
  setTerrainResolution(resolution: number): void;
  /** Last embedded-renderer frame (Rendered viewport mode). */
  render: {
    frame: RenderFrame | null;
    busy: boolean;
    error: string | null;
  };
  /** Starter scenes for the New Project dialog (empty until probed). */
  presets: ScenePreset[];
  newProjectOpen: boolean;
  actions: {
    requestNewProject(): void;
    dismissNewProject(): void;
    createProject(presetId: string | null): Promise<void>;
    newProject(): Promise<void>;
    openProject(): Promise<void>;
    saveProject(saveAs?: boolean): Promise<void>;
    closeProject(): Promise<void>;
    apply(command: Command): Promise<Applied | null>;
    undo(): Promise<void>;
    redo(): Promise<void>;
    select(ids: string[]): Promise<void>;
    addPrimitive(shape: PrimitiveShape): Promise<void>;
    duplicateSelection(): Promise<void>;
    deleteSelection(): Promise<void>;
    importMesh(): Promise<void>;
    refreshEngine(force?: boolean): Promise<void>;
    setViewportMode(mode: ViewportMode): Promise<void>;
    renderViewport(
      width: number,
      height: number,
      quality?: "preview" | "material" | "final",
    ): Promise<void>;
    /** Pick and import a DEM, replacing the terrain's source. */
    importTerrainDem(): Promise<void>;
    /** Pick a raster overlay for the terrain. */
    importTerrainOverlay(): Promise<void>;
    /** Change or clear the overlay's opacity (0..1). */
    setTerrainOverlayOpacity(opacity: number): Promise<void>;
    clearTerrainOverlay(): Promise<void>;
    /** Render a full-resolution still into the project's renders/ directory. */
    renderStill(): Promise<void>;
    restoreRecovery(id: string): Promise<void>;
    discardRecovery(id: string): Promise<void>;
    dismissError(): void;
  };
}

export const SessionContext = createContext<SessionApi | null>(null);



/** Access the session. Lives in its own module so `session.tsx` only exports a
 * component (Vite's Fast Refresh refuses modules that mix the two). */
export function useSession(): SessionApi {
  const value = useContext(SessionContext);
  if (!value) throw new Error("useSession must be used inside SessionProvider");
  return value;
}
