/**
 * Browser-only preview harness.
 *
 * `npm run dev` without Tauri (no Rust side) gets an in-memory session with the
 * same command semantics as `studio-core`, so the UI can be built and reviewed.
 * It is deliberately not a second implementation of the product: no file I/O,
 * no recovery, no glTF import, and no Forge3D.
 *
 * Any behaviour here that matters is mirrored by a Rust test — this file is
 * part of why `schema_contract.rs` and `phase1.rs`/`phase2.rs` exist.
 */

import type { StudioBridge } from "./index";
import type {
  Asset,
  AutosaveStatus,
  CameraObject,
  Command,
  EditResult,
  LightObject,
  MeshFilePayload,
  ObjectKind,
  RenderFrame,
  RenderStatus,
  RenderView,
  Primitive,
  Project,
  SceneObject,
  SessionState,
  TerrainPreview,
  Transform,
} from "./types";

const newId = () => crypto.randomUUID();

const identity = (): Transform => ({
  translation: { x: 0, y: 0, z: 0 },
  rotationDeg: { x: 0, y: 0, z: 0 },
  scale: { x: 1, y: 1, z: 1 },
});

function primitiveObject(
  name: string,
  primitive: Primitive,
  transform: Transform = identity(),
): SceneObject {
  return {
    id: newId(),
    name,
    parent: null,
    transform,
    visible: true,
    locked: false,
    kind: { type: "mesh", source: { type: "primitive", primitive } },
  };
}

function starterProject(name: string): Project {
  const now = Date.now();
  // The preview harness mirrors the desktop starter: a DEM, not a cube.
  const demAsset = {
    id: newId(),
    name: "Bryce_Canyon_1024.tif",
    path: "/preview/Bryce_Canyon_1024.tif",
    kind: "dem" as const,
    byteSize: 3_321_356,
  };
  const terrain: SceneObject = {
    id: newId(),
    name: "Bryce Canyon",
    parent: null,
    transform: identity(),
    visible: true,
    locked: false,
    kind: {
      type: "terrain",
      asset: demAsset.id,
      zScale: 1,
      albedo: null,
      dimensions: [1024, 1024],
      worldOriginXz: [395_998.5, -4_165_574.5],
      worldSpanXz: [4000, 3800],
      elevationRange: [2099.23, 2553.58],
    },
  };
  const camera: SceneObject = {
    id: newId(),
    name: "Camera",
    parent: null,
    transform: {
      translation: { x: 6, y: 4, z: 8 },
      rotationDeg: { x: -22, y: 37, z: 0 },
      scale: { x: 1, y: 1, z: 1 },
    },
    visible: true,
    locked: false,
    kind: { type: "camera", fovDeg: 45, near: 0.1, far: 1000 },
  };
  const light: SceneObject = {
    id: newId(),
    name: "Sun",
    parent: null,
    transform: {
      translation: { x: 4, y: 6, z: 3 },
      rotationDeg: { x: -50, y: 30, z: 0 },
      scale: { x: 1, y: 1, z: 1 },
    },
    visible: true,
    locked: false,
    kind: {
      type: "light",
      kind: "directional",
      color: [1, 0.97, 0.92],
      intensity: 3,
    },
  };
  return {
    format: "studio3d",
    schemaVersion: 3,
    id: newId(),
    name,
    createdUnixMs: now,
    updatedUnixMs: now,
    scene: {
      id: newId(),
      name: "Scene",
      objects: [terrain, camera, light],
      activeCamera: camera.id,
    },
    assets: [demAsset],
    selection: [],
    workspace: "studio",
    viewport: { mode: "solid", gridVisible: true },
    render: { width: 1920, height: 1080 },
  };
}

interface Snapshot {
  label: string;
  project: Project;
}

export function createMockBridge(): StudioBridge {
  let project: Project | null = null;
  let revision = 0;
  let dirty = false;
  let path: string | null = null;
  let undo: Snapshot[] = [];
  let redo: Snapshot[] = [];

  const state = (): SessionState => {
    if (!project) throw new Error("no project is open");
    // Snapshot, never the live object: consumers legitimately memoise on
    // reference identity, and mutating the project in place made those memos
    // (the scene tree) go stale while the status bar updated.
    return {
      project: structuredClone(project),
      revision,
      dirty,
      path,
      canUndo: undo.length > 0,
      canRedo: redo.length > 0,
      undoLabel: undo.length ? undo[undo.length - 1].label : null,
      redoLabel: redo.length ? redo[redo.length - 1].label : null,
      historyLen: undo.length,
    };
  };

  const requireProject = (): Project => {
    if (!project) throw new Error("no project is open");
    return project;
  };

  const find = (id: string): SceneObject => {
    const object = requireProject().scene.objects.find((candidate) => candidate.id === id);
    if (!object) throw new Error(`object ${id.slice(0, 8)} does not exist`);
    return object;
  };

  const uniqueName = (base: string): string => {
    const scene = requireProject().scene;
    const taken = (candidate: string) =>
      scene.objects.some((object) => object.name.toLowerCase() === candidate.toLowerCase());
    if (!taken(base)) return base;
    for (let suffix = 1; suffix < 10_000; suffix += 1) {
      const candidate = `${base}.${String(suffix).padStart(3, "0")}`;
      if (!taken(candidate)) return candidate;
    }
    return `${base}.copy`;
  };

  const validateAsset = (asset: Asset) => {
    if (!asset.id) throw new Error("asset id must not be empty");
    if (!asset.name.trim()) throw new Error("asset name must not be empty");
    if (!asset.path.trim()) throw new Error("asset path must not be empty");
    if (asset.byteSize <= 0) throw new Error("asset file is empty");
  };

  const apply = (command: Command): { label: string; affected: string[] } => {
    const scene = requireProject().scene;

    const label = (() => {
      switch (command.type) {
        case "setTransform":
          return `Move ${find(command.id).name}`;
        case "renameObject":
          return `Rename ${find(command.id).name}`;
        case "setVisibility":
          return `${command.visible ? "Show" : "Hide"} ${find(command.id).name}`;
        case "setLocked":
          return `${command.locked ? "Lock" : "Unlock"} ${find(command.id).name}`;
        case "setCamera":
          return `Edit camera ${find(command.id).name}`;
        case "setLight":
          return `Edit light ${find(command.id).name}`;
        case "setPrimitive":
          return `Reshape ${find(command.id).name}`;
        case "setTerrainZScale":
          return `Terrain scale ${find(command.id).name}`;
        case "createObject":
          return `Create ${command.object.name}`;
        case "createPrimitive":
          return `Add ${command.primitive.type === "cube" ? "Cube" : command.primitive.type === "sphere" ? "Sphere" : "Plane"}`;
        case "deleteObject":
          return `Delete ${find(command.id).name}`;
        case "duplicateObject":
          return `Duplicate ${find(command.id).name}`;
        case "addAsset":
          return `Import ${command.asset.name}`;
        case "importGltf":
          return `Import ${command.name}`;
        case "removeAsset":
          return "Remove asset";
        case "setActiveCamera":
          return "Set active camera";
        case "setViewport":
          return "Viewport settings";
        case "setWorkspace":
          return "Switch workspace";
        case "batch":
          return command.commands.length
            ? `Edit ${command.commands.length} properties`
            : "Edit";
      }
    })();

    switch (command.type) {
      case "setTransform":
        find(command.id).transform = structuredClone(command.transform);
        return { label, affected: [command.id] };

      case "renameObject": {
        if (!command.name.trim()) throw new Error("object name must not be empty");
        find(command.id).name = command.name.trim();
        return { label, affected: [command.id] };
      }

      case "setVisibility":
        find(command.id).visible = command.visible;
        return { label, affected: [command.id] };

      case "setLocked":
        find(command.id).locked = command.locked;
        return { label, affected: [command.id] };

      case "setCamera": {
        const camera: CameraObject = command.camera;
        if (camera.fovDeg <= 1 || camera.fovDeg >= 179)
          throw new Error("camera field of view must be between 1 and 179 degrees");
        if (camera.near <= 0) throw new Error("camera near plane must be positive");
        if (camera.far <= camera.near)
          throw new Error("camera far plane must be greater than the near plane");
        const object = find(command.id);
        if (object.kind.type !== "camera") throw new Error("object is not a camera");
        object.kind = { type: "camera", ...camera };
        return { label, affected: [command.id] };
      }

      case "setLight": {
        const light: LightObject = command.light;
        if (light.intensity < 0) throw new Error("light intensity must be non-negative");
        const object = find(command.id);
        if (object.kind.type !== "light") throw new Error("object is not a light");
        object.kind = { type: "light", ...light };
        return { label, affected: [command.id] };
      }

      case "setPrimitive": {
        const primitive: Primitive = command.primitive;
        const extent = primitive.type === "sphere" ? primitive.radius : primitive.size;
        if (!(extent > 0)) throw new Error("primitive size must be positive");
        const object = find(command.id);
        if (object.kind.type !== "mesh") throw new Error("object is not a mesh");
        if (object.kind.source.type !== "primitive")
          throw new Error("object is an imported mesh, not a primitive");
        object.kind = { type: "mesh", source: { type: "primitive", primitive } };
        return { label, affected: [command.id] };
      }

      case "createObject": {
        if (scene.objects.some((o) => o.id === command.object.id))
          throw new Error("an object with that id already exists");
        const source = command.object.kind.type === "mesh" ? command.object.kind.source : null;
        if (source?.type === "gltf" && !requireProject().assets.some((a) => a.id === source.asset))
          throw new Error("asset does not exist");
        const index = command.index ?? scene.objects.length;
        scene.objects.splice(index, 0, structuredClone(command.object));
        return { label, affected: [command.object.id] };
      }

      case "createPrimitive": {
        const base =
          command.primitive.type === "cube"
            ? "Cube"
            : command.primitive.type === "sphere"
              ? "Sphere"
              : "Plane";
        const object = primitiveObject(command.name?.trim() || uniqueName(base), command.primitive);
        scene.objects.push(object);
        return { label, affected: [object.id] };
      }

      case "duplicateObject": {
        const index = scene.objects.findIndex((o) => o.id === command.id);
        if (index < 0) throw new Error("object does not exist");
        const source = scene.objects[index];
        const copy: SceneObject = {
          ...structuredClone(source),
          id: newId(),
          name: command.name?.trim() || uniqueName(source.name),
        };
        if (command.name !== undefined && command.name !== null && !command.name.trim())
          throw new Error("object name must not be empty");
        scene.objects.splice(index + 1, 0, copy);
        return { label, affected: [copy.id] };
      }

      case "deleteObject": {
        const object = find(command.id);
        if (scene.objects.some((o) => o.parent === object.id))
          throw new Error("object has children; delete them first");
        scene.objects = scene.objects.filter((o) => o.id !== object.id);
        requireProject().selection = requireProject().selection.filter((id) => id !== object.id);
        if (scene.activeCamera === object.id) scene.activeCamera = null;
        return { label, affected: [command.id] };
      }

      case "addAsset": {
        validateAsset(command.asset);
        if (requireProject().assets.some((asset) => asset.id === command.asset.id))
          throw new Error("an asset with that id already exists");
        requireProject().assets.push(structuredClone(command.asset));
        return { label, affected: [] };
      }

      case "removeAsset": {
        const found = requireProject().assets.find((asset) => asset.id === command.id);
        if (!found) throw new Error("asset does not exist");
        const used = scene.objects.some(
          (object) =>
            object.kind.type === "mesh" &&
            object.kind.source.type === "gltf" &&
            object.kind.source.asset === command.id,
        );
        if (used) throw new Error("asset is still used by an object");
        requireProject().assets = requireProject().assets.filter((a) => a.id !== command.id);
        return { label, affected: [] };
      }

      case "importGltf":
        throw new Error("glTF import needs the desktop shell");

      case "setTerrainZScale": {
        if (!(command.zScale > 0)) throw new Error("terrain vertical scale must be positive");
        const object = find(command.id);
        if (object.kind.type !== "terrain") throw new Error("object is not a terrain");
        object.kind.zScale = command.zScale;
        return { label, affected: [command.id] };
      }

      case "setActiveCamera": {
        if (command.id) {
          const object = find(command.id);
          if (object.kind.type !== "camera") throw new Error("object is not a camera");
        }
        scene.activeCamera = command.id;
        return { label, affected: command.id ? [command.id] : [] };
      }

      case "setViewport":
        requireProject().viewport = { ...command.viewport };
        return { label, affected: [] };

      case "setWorkspace":
        requireProject().workspace = command.workspace;
        return { label, affected: [] };

      case "batch":
        for (const inner of command.commands) apply(inner);
        return { label, affected: [] };
    }
  };

  const bridge: StudioBridge = {
    kind: "mock",

    info: async () => ({
      name: "Forge3D Studio",
      version: "0.1.0",
      phase: "Phase 2 — preview harness (no Rust shell)",
    }),

    state: async () => (project ? state() : null),

    newProject: async (name) => {
      project = starterProject(name ?? "Untitled");
      revision = 0;
      dirty = true;
      path = null;
      undo = [];
      redo = [];
      return state();
    },

    openProject: async (opened) => {
      project = starterProject("Reopened project");
      revision = 0;
      dirty = false;
      path = opened;
      undo = [];
      redo = [];
      return state();
    },

    saveProject: async (target) => {
      if (!project) throw new Error("no project is open");
      path = target ?? path ?? "/mock/untitled.studio3d";
      dirty = false;
      return state();
    },

    closeProject: async () => {
      project = null;
      undo = [];
      redo = [];
      return null;
    },

    applyCommand: async (command) => {
      if (!project) throw new Error("no project is open");
      const before = structuredClone(project);
      const { label, affected } = apply(structuredClone(command));
      undo.push({ label, project: before });
      if (undo.length > 256) undo.shift();
      redo = [];
      revision += 1;
      dirty = true;
      project.updatedUnixMs = Date.now();
      return {
        applied: { label, revision, affected },
        state: state(),
      } as EditResult;
    },

    undo: async () => {
      const entry = undo.pop();
      if (!entry || !project) throw new Error("nothing to undo");
      redo.push({ label: entry.label, project: structuredClone(project) });
      project = entry.project;
      revision += 1;
      dirty = true;
      return {
        applied: { label: `Undo ${entry.label}`, revision, affected: [] },
        state: state(),
      } as EditResult;
    },

    redo: async () => {
      const entry = redo.pop();
      if (!entry || !project) throw new Error("nothing to redo");
      undo.push({ label: entry.label, project: structuredClone(project) });
      project = entry.project;
      revision += 1;
      dirty = true;
      return {
        applied: { label: `Redo ${entry.label}`, revision, affected: [] },
        state: state(),
      } as EditResult;
    },

    setSelection: async (ids) => {
      const current = requireProject();
      for (const id of ids) {
        if (!current.scene.objects.some((object) => object.id === id))
          throw new Error("object does not exist");
      }
      current.selection = [...new Set(ids)];
      return state();
    },

    autosaveNow: async () => null,

    autosaveStatus: async () =>
      ({
        intervalSeconds: 30,
        lastWriteUnixMs: null,
        lastError: null,
        directory: "(browser preview: no autosave directory)",
      }) as AutosaveStatus,

    listRecovery: async () => [],

    restoreRecovery: async () => {
      throw new Error("recovery is unavailable in the browser preview");
    },

    discardRecovery: async () => {},

    readMeshFile: async (): Promise<MeshFilePayload> => {
      throw new Error(
        "glTF import needs the desktop shell: the browser preview cannot read files",
      );
    },

    // The preview harness has no engine. It reports that honestly and still
    // produces a handoff so the panel's layout can be reviewed.
    terrainPreview: async (_path, resolution = 96): Promise<TerrainPreview> => {
      // A synthetic ridge line: enough relief to review the viewport without a
      // DEM file. The desktop shell samples the real GeoTIFF.
      const size = Math.max(8, Math.min(128, resolution));
      const heights: number[] = [];
      for (let row = 0; row < size; row += 1) {
        for (let column = 0; column < size; column += 1) {
          const u = column / (size - 1);
          const v = row / (size - 1);
          const ridge =
            Math.sin(u * Math.PI * 3.1) * Math.cos(v * Math.PI * 2.3) * 0.45 +
            Math.sin((u + v) * Math.PI * 5.0) * 0.2;
          heights.push(2100 + Math.min(1, Math.max(0, ridge + 0.5)) * 450);
        }
      }
      return {
        ok: true,
        width: 1024,
        height: 1024,
        minElevation: 2100,
        maxElevation: 2550,
        extentMeters: [4000, 3800],
        originXz: [395_998.5, -4_165_574.5],
        spanXz: [4000, 3800],
        resolution: size,
        heights,
      } satisfies TerrainPreview;
    },

    forge3dProbe: async () => ({
      available: false,
      python: "python3",
      packageVersion: null,
      viewerBinary: null,
      detail:
        "The browser preview has no Forge3D engine. Run the desktop shell (npm run tauri:dev) to render.",
    }),

    renderFrame: async (_width: number, _height: number, _quality?: "preview" | "final", _view?: RenderView): Promise<RenderFrame> => {
      throw new Error(
        "Rendered mode runs the Forge3D pipeline, which needs the desktop shell",
      );
    },

    renderStill: async (): Promise<RenderFrame> => {
      throw new Error("Rendered mode needs the desktop shell");
    },

    renderStatus: async (): Promise<RenderStatus> => ({
      running: false,
      dem: null,
      diagnostics: ["preview harness: no renderer"],
    }),

    renderStop: async (): Promise<RenderStatus> => ({
      running: false,
      dem: null,
      diagnostics: ["preview harness: no renderer"],
    }),

  };

  return bridge;
}

export type { ObjectKind };
