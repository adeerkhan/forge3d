import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { bridge } from "../bridge/env";
import { SessionContext, type SessionApi } from "./session-context";
import { base64ToArrayBuffer, inspectGltf, putAssetBytes } from "../viewport/assets";
import type {
  Applied,
  AutosaveStatus,
  Command,
  EngineStatus,
  RenderFrame,
  ScenePreset,
  ViewportMode,
  PrimitiveShape,
  RecoveryEntry,
  SessionState,
  ShellInfo,
  Tool,
  ToolSpace,
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
  const [terrainResolution, setTerrainResolution] = useState(96);
  const [presets, setPresets] = useState<ScenePreset[]>([]);
  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const [render, setRender] = useState<{ frame: RenderFrame | null; busy: boolean; error: string | null }>({
    frame: null,
    busy: false,
    error: null,
  });
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

  // One engine probe per session: the status bar reports whether the renderer
  // can run, without a whole workspace for it.
  useEffect(() => {
    void refreshEngine();
    if (bridge.kind === "tauri") {
      bridge
        .scenePresets()
        .then(setPresets)
        .catch(() => undefined);
    } else {
      bridge
        .scenePresets()
        .then(setPresets)
        .catch(() => undefined);
    }
    // Mount-time probe only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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

  const refreshEngine = useCallback(
    async (force = false) => {
      if (bridge.kind !== "tauri") return;
      try {
        setEngine(await bridge.forge3dProbe(force));
      } catch {
        // A missing engine is reported through the status bar, not a failure.
      }
    },
    [],
  );

  // The renderer costs seconds per frame, so requests are serialised and only
  // the newest queued view matters: dragging the camera must not build a queue.
  const pendingRender = useRef<{
    width: number;
    height: number;
    quality: "preview" | "material" | "final";
  } | null>(null);
  const renderInFlight = useRef(false);

  const pumpRender = useCallback(async () => {
    if (renderInFlight.current) return;
    renderInFlight.current = true;
    try {
      while (pendingRender.current) {
        const request = pendingRender.current;
        pendingRender.current = null;
        setRender((current) => ({ ...current, busy: true, error: null }));
        try {
          const frame = await bridge.renderFrame(request.width, request.height, request.quality);
          setRender({ frame, busy: false, error: null });
        } catch (cause) {
          setRender({
            frame: null,
            busy: false,
            error: cause instanceof Error ? cause.message : String(cause),
          });
        }
      }
    } finally {
      renderInFlight.current = false;
    }
  }, []);

  const renderViewport = useCallback(
    async (width: number, height: number, quality: "preview" | "material" | "final" = "preview") => {
      pendingRender.current = { width, height, quality };
      await pumpRender();
    },
    [pumpRender],
  );

  const importTerrainDem = useCallback(async () => {
    try {
      const picked = await openDialog({
        multiple: false,
        filters: [{ name: "GeoTIFF DEM", extensions: ["tif", "tiff"] }],
      });
      const path = typeof picked === "string" ? picked : null;
      if (!path) return;
      setBusy(true);
      setState(await bridge.importTerrainDem(path));
      flash(`DEM replaced with ${path.split(/[\\/]/).pop()}`);
    } catch (cause) {
      fail(cause);
    } finally {
      setBusy(false);
    }
  }, [fail, flash]);

  const importTerrainOverlay = useCallback(async () => {
    try {
      const picked = await openDialog({
        multiple: false,
        filters: [{ name: "Raster overlay", extensions: ["tif", "tiff", "png"] }],
      });
      const path = typeof picked === "string" ? picked : null;
      if (!path) return;
      setBusy(true);
      setState(await bridge.setTerrainOverlay(path, 0.82));
      flash("Overlay attached");
    } catch (cause) {
      fail(cause);
    } finally {
      setBusy(false);
    }
  }, [fail, flash]);

  const setTerrainOverlayOpacity = useCallback(
    async (opacity: number) => {
      const project = state?.project;
      const terrain = project?.scene.objects.find((object) => object.kind.type === "terrain");
      if (!project || !terrain || terrain.kind.type !== "terrain" || !terrain.kind.overlay) {
        return;
      }
      const overlayAsset = project.assets.find(
        (entry) => entry.id === (terrain.kind.type === "terrain" ? terrain.kind.overlay?.asset : null),
      );
      if (!overlayAsset) return;
      try {
        setBusy(true);
        setState(await bridge.setTerrainOverlay(overlayAsset.path, opacity));
      } catch (cause) {
        fail(cause);
      } finally {
        setBusy(false);
      }
    },
    [bridge, fail, state],
  );

  const clearTerrainOverlay = useCallback(async () => {
    try {
      setBusy(true);
      setState(await bridge.setTerrainOverlay(null));
      flash("Overlay removed");
    } catch (cause) {
      fail(cause);
    } finally {
      setBusy(false);
    }
  }, [fail, flash]);

  const renderStillTo = useCallback(async (path: string | null) => {
    setRender((current) => ({ ...current, busy: true, error: null }));
    try {
      const frame = await bridge.renderStill(path);
      setRender({ frame, busy: false, error: null });
      flash(frame.path ? `Saved ${frame.path}` : "Rendered");
    } catch (cause) {
      setRender({
        frame: null,
        busy: false,
        error: cause instanceof Error ? cause.message : String(cause),
      });
    }
  }, [flash]);

  const setViewportMode = useCallback(
    async (mode: ViewportMode) => {
      const project = state?.project;
      if (!project) return;
      await applyCommand({
        type: "setViewport",
        viewport: { ...project.viewport, mode },
      });
    },
    [applyCommand, state],
  );

  const actions = useMemo<SessionApi["actions"]>(
    () => ({
      async newProject() {
        // Kept for programmatic use (keyboard shortcut opens the dialog instead).
        await actions.createProject(null);
      },

      requestNewProject() {
        if (!guardUnsaved("start a new project")) return;
        if (presets.length === 0) {
          bridge
            .scenePresets()
            .then(setPresets)
            .catch(() => undefined);
        }
        setNewProjectOpen(true);
      },

      dismissNewProject() {
        setNewProjectOpen(false);
      },

      async createProject(presetId) {
        setBusy(true);
        setNewProjectOpen(false);
        try {
          const snapshot = await bridge.newProject(null, presetId);
          setState(snapshot);
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

      async renderStill() {
        // Default destination: <project>/renders/<name>-<timestamp>.png with a
        // JSON sidecar, so a render can always be traced to its settings.
        await renderStillTo(null);
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

      refreshEngine,
      importTerrainDem,
      importTerrainOverlay,
      setTerrainOverlayOpacity,
      clearTerrainOverlay,
      setViewportMode,
      renderViewport,

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
      importTerrainDem,
      importTerrainOverlay,
      clearTerrainOverlay,
      refreshEngine,
      refreshRecovery,
      renderStillTo,
      renderViewport,
      select,
      setTerrainOverlayOpacity,
      setViewportMode,
      state,
    ],
  );

  const value: SessionApi = {
    bridgeKind: bridge.kind,
    presets,
    newProjectOpen,
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
    render,
    terrainResolution,
    setTerrainResolution,
    actions,
  };

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

