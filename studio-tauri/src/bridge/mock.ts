/**
 * Browser-only preview harness.
 *
 * `npm run dev` without Tauri (no webkit runtime, no Rust side) gets an
 * in-memory session with the same command semantics as `studio-core`, so the UI
 * can be built and reviewed. It is deliberately not a second implementation of
 * the product: no file dialogs, no recovery, and no Forge3D.
 *
 * Any behaviour here that matters must be mirrored by a Rust test — this file
 * is the reason `schema_contract.rs` and `phase1.rs` exist.
 */

import type { StudioBridge } from "./index";
import type {
  AutosaveStatus,
  CameraObject,
  Command,
  EditResult,
  LightObject,
  ObjectKind,
  Primitive,
  Project,
  SceneObject,
  SessionState,
  Transform,
} from "./types";

const newId = () => crypto.randomUUID();

const identity = (): Transform => ({
  translation: { x: 0, y: 0, z: 0 },
  rotationDeg: { x: 0, y: 0, z: 0 },
  scale: { x: 1, y: 1, z: 1 },
});

function starterProject(name: string): Project {
  const now = Date.now();
  const cube: SceneObject = {
    id: newId(),
    name: "Cube",
    parent: null,
    transform: identity(),
    visible: true,
    locked: false,
    kind: { type: "mesh", primitive: { type: "cube", size: 2 } },
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
    schemaVersion: 1,
    id: newId(),
    name,
    createdUnixMs: now,
    updatedUnixMs: now,
    scene: {
      id: newId(),
      name: "Scene",
      objects: [cube, camera, light],
      activeCamera: camera.id,
    },
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
    return {
      project,
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

  const find = (id: string): SceneObject => {
    const object = project?.scene.objects.find((candidate) => candidate.id === id);
    if (!object) throw new Error(`object ${id.slice(0, 8)} does not exist`);
    return object;
  };

  const apply = (command: Command): { label: string; affected: string[] } => {
    if (!project) throw new Error("no project is open");
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
        case "createObject":
          return `Create ${command.object.name}`;
        case "deleteObject":
          return `Delete ${find(command.id).name}`;
        case "setActiveCamera":
          return "Set active camera";
        case "setViewport":
          return "Viewport settings";
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
        const extent =
          primitive.type === "sphere" ? primitive.radius : primitive.size;
        if (!(extent > 0)) throw new Error("primitive size must be positive");
        const object = find(command.id);
        if (object.kind.type !== "mesh") throw new Error("object is not a mesh");
        object.kind = { type: "mesh", primitive };
        return { label, affected: [command.id] };
      }

      case "createObject": {
        if (project.scene.objects.some((o) => o.id === command.object.id))
          throw new Error("an object with that id already exists");
        const index = command.index ?? project.scene.objects.length;
        project.scene.objects.splice(index, 0, structuredClone(command.object));
        return { label, affected: [command.object.id] };
      }

      case "deleteObject": {
        const object = find(command.id);
        if (project.scene.objects.some((o) => o.parent === object.id))
          throw new Error("object has children; delete them first");
        project.scene.objects = project.scene.objects.filter((o) => o.id !== object.id);
        project.selection = project.selection.filter((id) => id !== object.id);
        if (project.scene.activeCamera === object.id) project.scene.activeCamera = null;
        return { label, affected: [command.id] };
      }

      case "setActiveCamera": {
        if (command.id) {
          const object = find(command.id);
          if (object.kind.type !== "camera") throw new Error("object is not a camera");
        }
        project.scene.activeCamera = command.id;
        return { label, affected: command.id ? [command.id] : [] };
      }

      case "setViewport":
        project.viewport = { ...command.viewport };
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
      phase: "Phase 1 — preview harness (no Rust shell)",
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
      if (!project) throw new Error("no project is open");
      project.selection = [...new Set(ids)];
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
  };

  return bridge;
}

export type { ObjectKind };
