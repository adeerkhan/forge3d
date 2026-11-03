import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { bridge } from "../bridge/env";
import { base64ToArrayBuffer, inspectGltf, putAssetBytes } from "../viewport/assets";
import type {
  Applied,
  AutosaveStatus,
  Command,
  EngineStatus,
  Forge3dStatus,
  Handoff,
  PrimitiveShape,
  RecoveryEntry,
  SessionState,
  ShellInfo,
  Tool,
  ToolSpace,
  Workspace,
} from "../bridge/types";
import { DEFAULT_PRIMITIVES } from "../bridge/types";

const PROJECT_FILTER = {
  name: "Forge3D Studio project",
  extensions: ["studio3d"],
};

const MESH_FILTER = {
  name: "glTF / GLB",
  extensions: ["gltf", "glb"],
};

interface SessionApi {
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
  /** Forge3D workspace adapter state. */
  engine: EngineStatus | null;
  forgeStatus: Forge3dStatus | null;
  handoff: Handoff | null;
  actions: {
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
    setWorkspace(workspace: Workspace): Promise<void>;
    refreshForge3d(force?: boolean): Promise<void>;
    openForge3d(): Promise<void>;
    syncForge3dCamera(): Promise<void>;
    stopForge3d(): Promise<void>;
    restoreRecovery(id: string): Promise<void>;
    discardRecovery(id: string): Promise<void>;
    dismissError(): void;
  };
}

const SessionContext = createContext<SessionApi | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<ShellInfo | null>(null);
  const [state, setState] = useState<SessionState | null>(null);
  const [autosave, setAutosave] = useState<AutosaveStatus | null>(null);
  const [recovery, setRecovery] = useState<RecoveryEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tool, setTool] = useState<Tool>("select");
  const [space, setSpace] = useState<ToolSpace>("world");
  const [cameraView, setCameraView] = useState(false);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [forgeStatus, setForgeStatus] = useState<Forge3dStatus | null>(null);
  const [handoff, setHandoff] = useState<Handoff | null>(null);
  const noticeTimer = useRef<number | null>(null);

  const flash = useCallback((message: string) => {
    setNotice(message);
    if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 2600);
  }, []);

  const fail = useCallback((cause: unknown) => {
    setError(cause instanceof Error ? cause.message : String(cause));
  }, []);

  const refreshRecovery = useCallback(async () => {
    try {
      setRecovery(await bridge.listRecovery());
    } catch (cause) {
      fail(cause);
    }
  }, [fail]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [shellInfo, snapshot, autosaveStatus, entries] = await Promise.all([
          bridge.info(),
          bridge.state(),
          bridge.autosaveStatus(),
          bridge.listRecovery(),
        ]);
        if (cancelled) return;
        setInfo(shellInfo);
        setState(snapshot);
        setAutosave(autosaveStatus);
        setRecovery(entries);
      } catch (cause) {
        if (!cancelled) fail(cause);
      }
    })();
    return () => {
      cancelled = true;
      if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
    };
  }, [fail]);

  // Keep "autosaved 14:03" honest without a websocket: one cheap poll.
  useEffect(() => {
    if (bridge.kind !== "tauri") return;
    const timer = window.setInterval(() => {
      bridge
        .autosaveStatus()
        .then(setAutosave)
        .catch(() => undefined);
    }, 10_000);
    return () => window.clearInterval(timer);
  }, []);

  const guardUnsaved = useCallback(
    (action: string): boolean => {
      if (!state?.dirty) return true;
      return window.confirm(
        `"${state.project.name}" has unsaved changes. Continue and ${action}?`,
      );
    },
    [state],
  );

  const applyCommand = useCallback(
    async (command: Command): Promise<Applied | null> => {
      try {
        const result = await bridge.applyCommand(command);
        setState(result.state);
        return result.applied;
      } catch (cause) {
        fail(cause);
        return null;
      }
    },
    [fail],
  );

  const select = useCallback(async (ids: string[]) => {
    try {
      setState(await bridge.setSelection(ids));
    } catch (cause) {
      fail(cause);
    }
  }, [fail]);

  const addPrimitive = useCallback(
    async (shape: PrimitiveShape) => {
      const applied = await applyCommand({
        type: "createPrimitive",
        primitive: DEFAULT_PRIMITIVES[shape],
      });
      const id = applied?.affected[0];
      if (id) await select([id]);
    },
    [applyCommand, select],
  );

  const duplicateSelection = useCallback(async () => {
    const selection = state?.project.selection ?? [];
    if (selection.length === 0) return;
    const applied = await applyCommand({
      type: "batch",
      commands: selection.map((id) => ({ type: "duplicateObject" as const, id })),
    });
    if (applied && applied.affected.length > 0) {
      await select(applied.affected);
      flash(
        applied.affected.length === 1
          ? "Duplicated object"
          : `Duplicated ${applied.affected.length} objects`,
      );
    }
  }, [applyCommand, flash, select, state]);

  const deleteSelection = useCallback(async () => {
    const scene = state?.project.scene;
    const selection = state?.project.selection ?? [];
    if (!scene || selection.length === 0) return;

    // Children before parents, so deleting a subtree in one batch is legal.
    const ordered = [...selection].sort(
      (a, b) =>
        scene.objects.findIndex((object) => object.id === b) -
        scene.objects.findIndex((object) => object.id === a),
    );
    const applied = await applyCommand({
      type: "batch",
      commands: ordered.map((id) => ({ type: "deleteObject" as const, id })),
    });
    if (applied) {
      await select([]);
      flash(
        applied.affected.length === 1
          ? "Deleted object"
          : `Deleted ${applied.affected.length} objects`,
      );
    }
  }, [applyCommand, flash, select, state]);

  const importMesh = useCallback(async () => {
    if (bridge.kind !== "tauri") {
      fail(new Error("glTF import needs the desktop shell: the browser preview cannot read files"));
      return;
    }
    try {
      const picked = await openDialog({ multiple: false, filters: [MESH_FILTER] });
      const path = typeof picked === "string" ? picked : null;
      if (!path) return;

      setBusy(true);
      const payload = await bridge.readMeshFile(path);
      const buffer = base64ToArrayBuffer(payload.base64);
      // Parse before registering: a corrupt file must not reach the project.
      const { meshes } = await inspectGltf(buffer, payload.name);
      putAssetBytes(payload.path, buffer);

      const applied = await applyCommand({
        type: "importGltf",
        name: payload.name,
        path: payload.path,
        kind: payload.kind,
        byteSize: payload.byteSize,
        node: null,
      });
      const id = applied?.affected[0];
      if (id) await select([id]);
      flash(`Imported ${payload.name} (${meshes} mesh${meshes === 1 ? "" : "es"})`);
    } catch (cause) {
      fail(cause);
    } finally {
      setBusy(false);
    }
  }, [applyCommand, fail, flash, select]);

  const refreshForge3d = useCallback(
    async (force = false) => {
      try {
        setEngine(await bridge.forge3dProbe(force));
        setForgeStatus(await bridge.forge3dStatus());
        try {
          setHandoff(await bridge.forge3dHandoff());
        } catch {
          // No project open: the panel shows the engine state only.
          setHandoff(null);
        }
      } catch (cause) {
        fail(cause);
      }
    },
    [fail],
  );

  const openForge3d = useCallback(async () => {
    try {
      setForgeStatus(await bridge.forge3dLaunch());
      setHandoff(await bridge.forge3dHandoff().catch(() => null));
      flash("Forge3D viewer started");
    } catch (cause) {
      fail(cause);
    }
  }, [fail, flash]);

  const syncForge3dCamera = useCallback(async () => {
    try {
      setForgeStatus(await bridge.forge3dSyncCamera());
      flash("Camera and sun sent to Forge3D");
    } catch (cause) {
      fail(cause);
    }
  }, [fail, flash]);

  const stopForge3d = useCallback(async () => {
    try {
      setForgeStatus(await bridge.forge3dStop());
      flash("Forge3D viewer stopped");
    } catch (cause) {
      fail(cause);
    }
  }, [fail, flash]);

  // While the viewer runs, keep diagnostics and status fresh.
  useEffect(() => {
    if (!forgeStatus?.running || bridge.kind !== "tauri") return;
    const timer = window.setInterval(() => {
      bridge
        .forge3dStatus()
        .then(setForgeStatus)
        .catch(() => undefined);
    }, 2000);
    return () => window.clearInterval(timer);
  }, [forgeStatus?.running]);

  const actions = useMemo<SessionApi["actions"]>(
    () => ({
      async newProject() {
        if (!guardUnsaved("start a new project")) return;
        setBusy(true);
        try {
          setState(await bridge.newProject());
          await refreshRecovery();
          flash("New project");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async openProject() {
        if (!guardUnsaved("open another project")) return;
        try {
          let path: string | null = null;
          if (bridge.kind === "tauri") {
            const picked = await openDialog({
              multiple: false,
              filters: [PROJECT_FILTER],
            });
            path = typeof picked === "string" ? picked : null;
          } else {
            path = "/preview/example.studio3d";
          }
          if (!path) return;
          setBusy(true);
          setState(await bridge.openProject(path));
          await refreshRecovery();
          flash("Project opened");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async saveProject(saveAs = false) {
        if (!state) return;
        try {
          let path: string | null = null;
          if (saveAs || !state.path) {
            if (bridge.kind === "tauri") {
              const picked = await saveDialog({
                defaultPath: `${state.project.name || "untitled"}.studio3d`,
                filters: [PROJECT_FILTER],
              });
              path = picked ?? null;
            } else {
              path = "/preview/untitled.studio3d";
            }
            if (!path) return;
          }
          setBusy(true);
          const saved = await bridge.saveProject(path);
          setState(saved);
          await refreshRecovery();
          flash(`Saved ${saved.path ?? "project"}`);
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async closeProject() {
        if (!guardUnsaved("close the project")) return;
        setBusy(true);
        try {
          setState(await bridge.closeProject());
          flash("Project closed");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      apply: applyCommand,

      async undo() {
        try {
          setState((await bridge.undo()).state);
        } catch (cause) {
          fail(cause);
        }
      },

      async redo() {
        try {
          setState((await bridge.redo()).state);
        } catch (cause) {
          fail(cause);
        }
      },

      select,
      addPrimitive,
      duplicateSelection,
      deleteSelection,
      importMesh,

      async setWorkspace(workspace) {
        await applyCommand({ type: "setWorkspace", workspace });
        if (workspace === "forge3d") await refreshForge3d();
      },

      refreshForge3d,
      openForge3d,
      syncForge3dCamera,
      stopForge3d,

      async restoreRecovery(id) {
        setBusy(true);
        try {
          setState(await bridge.restoreRecovery(id));
          await refreshRecovery();
          flash("Recovered unsaved work");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async discardRecovery(id) {
        try {
          await bridge.discardRecovery(id);
          await refreshRecovery();
          flash("Recovery snapshot discarded");
        } catch (cause) {
          fail(cause);
        }
      },

      dismissError() {
        setError(null);
      },
    }),
    [
      addPrimitive,
      applyCommand,
      deleteSelection,
      duplicateSelection,
      fail,
      flash,
      guardUnsaved,
      importMesh,
      openForge3d,
      refreshForge3d,
      refreshRecovery,
      select,
      state,
      stopForge3d,
      syncForge3dCamera,
    ],
  );

  const value: SessionApi = {
    bridgeKind: bridge.kind,
    info,
    state,
    autosave,
    recovery,
    error,
    notice,
    busy,
    tool,
    setTool,
    space,
    setSpace,
    cameraView,
    setCameraView,
    engine,
    forgeStatus,
    handoff,
    actions,
  };

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

export function useSession(): SessionApi {
  const value = useContext(SessionContext);
  if (!value) throw new Error("useSession must be used inside SessionProvider");
  return value;
}
