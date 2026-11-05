import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { viewStore } from "../viewport/viewStore";
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
  RenderView,
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
  const [exporting, setExporting] = useState<{ done: number; total: number } | null>(null);
  const exportCancelled = useRef(false);
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

  const setPlayhead = useCallback(
    async (frame: number) => {
      try {
        const result = await bridge.setPlayhead(frame);
        setState(result.state);
      } catch (cause) {
        fail(cause);
      }
    },
    [fail],
  );

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
    quality: "preview" | "final";
    /** The viewport camera at the moment the frame was requested. */
    view: RenderView | undefined;
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
          const frame = await bridge.renderFrame(request.width, request.height, request.quality, request.view);
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
    async (width: number, height: number, quality: "preview" | "final" = "preview") => {
      pendingRender.current = { width, height, quality, view: viewStore.current ?? undefined };
      await pumpRender();
    },
    [pumpRender],
  );

  const selectedMesh = useCallback(() => {
    const project = state?.project;
    const object = project?.scene.objects.find((entry) =>
      project.selection.includes(entry.id),
    );
    return object && object.kind.type === "mesh" ? object : null;
  }, [state]);

  const importMaterialTexture = useCallback(async () => {
    const mesh = selectedMesh();
    if (!mesh || mesh.kind.type !== "mesh") return;
    try {
      const picked = await openDialog({
        multiple: false,
        filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg"] }],
      });
      const path = typeof picked === "string" ? picked : null;
      if (!path) return;
      // Asset registration rejects empty files (`byte_size == 0` reads as
      // "empty"), so stat the real size up front — the webview cannot.
      const byteSize = await bridge.fileSize(path);
      const asset = {
        id: crypto.randomUUID(),
        name: path.split(/[\\/]/).pop() ?? "texture.png",
        path,
        kind: "orthophoto" as const,
        byteSize,
      };
      const applied = await applyCommand({
        type: "batch",
        commands: [
          { type: "addAsset", asset },
          {
            type: "setMeshMaterial",
            id: mesh.id,
            material: { ...mesh.kind.material, texture: asset.id },
          },
        ],
      });
      if (applied) flash("Texture attached");
    } catch (cause) {
      fail(cause);
    }
  }, [applyCommand, fail, flash, selectedMesh]);

  const clearMaterialTexture = useCallback(async () => {
    const mesh = selectedMesh();
    if (!mesh || mesh.kind.type !== "mesh") return;
    await applyCommand({
      type: "setMeshMaterial",
      id: mesh.id,
      material: { ...mesh.kind.material, texture: null },
    });
    flash("Texture removed");
  }, [applyCommand, flash, selectedMesh]);

  /**
   * Pick a Radiance `.hdr` and make it the project's environment. Registered
   * as an asset first (same batch), so the render settings never reference an
   * id the project does not carry.
   */
  const importEnvironment = useCallback(async () => {
    if (bridge.kind !== "tauri") {
      fail(new Error("HDRI environments need the desktop shell"));
      return;
    }
    const project = state?.project;
    if (!project) return;
    try {
      const picked = await openDialog({
        multiple: false,
        filters: [{ name: "Radiance HDR environment", extensions: ["hdr"] }],
      });
      const path = typeof picked === "string" ? picked : null;
      if (!path) return;
      const byteSize = await bridge.fileSize(path);
      const asset = {
        id: crypto.randomUUID(),
        name: path.split(/[\\/]/).pop() ?? "environment.hdr",
        path,
        kind: "hdr" as const,
        byteSize,
      };
      const applied = await applyCommand({
        type: "batch",
        commands: [
          { type: "addAsset", asset },
          {
            type: "setRenderSettings",
            render: { ...project.render, environment: asset.id },
          },
        ],
      });
      if (applied) flash(`Environment set to ${asset.name}`);
    } catch (cause) {
      fail(cause);
    }
  }, [applyCommand, fail, flash, state]);

  const clearEnvironment = useCallback(async () => {
    const project = state?.project;
    if (!project) return;
    await applyCommand({
      type: "setRenderSettings",
      render: { ...project.render, environment: null },
    });
    flash("Environment cleared");
  }, [applyCommand, flash, state]);

  const relinkAsset = useCallback(
    async (assetId: string) => {
      try {
        const picked = await openDialog({ multiple: false });
        const path = typeof picked === "string" ? picked : null;
        if (!path) return;
        // Same non-empty invariant as AddAsset: stat, don't guess.
        await applyCommand({
          type: "setAssetPath",
          id: assetId,
          path,
          byteSize: await bridge.fileSize(path),
        });
        flash("Asset relinked");
      } catch (cause) {
        fail(cause);
      }
    },
    [applyCommand, fail, flash],
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
      flash(`Terrain DEM set to ${path.split(/[\\/]/).pop()}`);
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
      const frame = await bridge.renderStill(path, viewStore.current ?? undefined);
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

  /**
   * Render the whole timeline as a PNG sequence, one IPC per frame so progress
   * is real and Stop works between frames. The shell evaluates a clone per
   * frame — the same studio-core `evaluate` the playhead uses — so the
   * session's playhead and dirty state never move during export.
   */
  const exportFrames = useCallback(
    async (directory: string) => {
      const snapshot = state;
      if (!snapshot || bridge.kind !== "tauri") return;
      const { timeline, render: renderSettings } = snapshot.project;
      const total = timeline.length + 1;
      exportCancelled.current = false;
      setExporting({ done: 0, total });
      try {
        for (let index = 0; index <= timeline.length; index += 1) {
          if (exportCancelled.current) {
            flash(`Export stopped at frame ${index} of ${total}`);
            return;
          }
          const output = `${directory}/frame_${String(index).padStart(4, "0")}.png`;
          await bridge.renderAnimationFrame({
            frame: index,
            width: renderSettings.width,
            height: renderSettings.height,
            quality: "final",
            output,
          });
          setExporting({ done: index + 1, total });
        }
        flash(`Exported ${total} frames to ${directory}`);
      } catch (cause) {
        fail(cause);
      } finally {
        setExporting(null);
      }
    },
    [state, flash, fail],
  );

  const cancelExport = useCallback(() => {
    exportCancelled.current = true;
  }, []);

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
        // The still goes where the user points the save dialog; the folder
        // defaults to <project>/renders/<name>-<timestamp>.png so a render
        // can always be traced to its settings (JSON sidecar next to it).
        if (!state) return;
        if (bridge.kind !== "tauri") {
          await renderStillTo(null);
          return;
        }
        const name = (state.project.name || "untitled").replace(/[^A-Za-z0-9-_]+/g, "-");
        const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
        const slash = Math.max(
          state.path?.lastIndexOf("/") ?? -1,
          state.path?.lastIndexOf("\\") ?? -1,
        );
        const dir = state.path && slash > 0 ? state.path.slice(0, slash) : null;
        const picked = await saveDialog({
          defaultPath: dir
            ? `${dir}/renders/${name}-${stamp}.png`
            : `${name}-${stamp}.png`,
          filters: [{ name: "PNG image", extensions: ["png"] }],
        });
        if (!picked) return;
        await renderStillTo(picked);
      },

      exportFrames,
      cancelExport,

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
      setPlayhead,
      addPrimitive,
      duplicateSelection,
      deleteSelection,
      importMesh,

      refreshEngine,
      importMaterialTexture,
      clearMaterialTexture,
      importEnvironment,
      clearEnvironment,
      relinkAsset,
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

      notify(message) {
        flash(message);
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
      importMaterialTexture,
      clearMaterialTexture,
      importEnvironment,
      clearEnvironment,
      relinkAsset,
      importTerrainDem,
      importTerrainOverlay,
      clearTerrainOverlay,
      refreshEngine,
      refreshRecovery,
      renderStillTo,
      renderViewport,
      exportFrames,
      cancelExport,
      select,
      setPlayhead,
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
    exporting,
    terrainResolution,
    setTerrainResolution,
    actions,
  };

  return <SessionContext.Provider value={value}>{children}</SessionContext.Provider>;
}

