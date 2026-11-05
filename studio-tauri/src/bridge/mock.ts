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
  AssetIntegrity,
  AutosaveStatus,
  CameraObject,
  Command,
  EditResult,
  LightObject,
  MeshFilePayload,
  ObjectKind,
  RenderFrame,
  RenderStatus,
  ScenePreset,
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
    kind: {
      type: "mesh",
      source: { type: "primitive", primitive },
      material: {
        baseColor: [0.62, 0.66, 0.72],
        roughness: 0.6,
        metallic: 0.04,
        texture: null,
      },
    },
  };
}

function starterProject(name: string, preset: string = "bryce"): Project {
  const now = Date.now();
  // The preview harness mirrors the desktop starter: a DEM, not a cube.
  const demAsset = {
    id: newId(),
    name: preset === "swiss" ? "switzerland_dem.tif" : "Bryce_Canyon_1024.tif",
    path:
      preset === "swiss"
        ? "/preview/switzerland_dem.tif"
        : "/preview/Bryce_Canyon_1024.tif",
    kind: "dem" as const,
    byteSize: preset === "swiss" ? 50_000_000 : 3_321_356,
  };
  // Mirrors the desktop starter: the Bryce scene ships the NAIP albedo.
  const naipAsset =
    preset === "swiss"
      ? null
      : {
          id: newId(),
          name: "Bryce_Canyon_naip.tif",
          path: "/preview/Bryce_Canyon_naip.tif",
          kind: "orthophoto" as const,
          byteSize: 28_000_000,
        };
  const cube = primitiveObject("Cube", { type: "cube", size: 2 });
  const terrain: SceneObject = {
    id: newId(),
    name: preset === "swiss" ? "Swiss Alps" : "Bryce Canyon",
    parent: null,
    transform: identity(),
    visible: true,
    locked: false,
    kind: {
      type: "terrain",
      asset: demAsset.id,
      zScale: 1,
      albedo: null,
      dimensions: preset === "swiss" ? [7294, 3200] : [1024, 1024],
      worldOriginXz: preset === "swiss" ? [0, 0] : [395_998.5, -4_165_574.5],
      worldSpanXz: preset === "swiss" ? [352_427, 225_653] : [4000, 3800],
      elevationRange: preset === "swiss" ? [189, 4220] : [2099.23, 2553.58],
      overlay: naipAsset
        ? { asset: naipAsset.id, opacity: 0.82, preserveColors: true }
        : null,
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
    schemaVersion: 4,
    id: newId(),
    name,
    createdUnixMs: now,
    updatedUnixMs: now,
    scene: {
      id: newId(),
      name: "Scene",
      objects: preset === "empty" ? [cube, camera, light] : [terrain, camera, light],
      activeCamera: camera.id,
    },
    assets:
      preset === "empty"
        ? []
        : naipAsset
          ? [demAsset, naipAsset]
          : [demAsset],
    selection: [],
    workspace: "studio",
    viewport: { mode: "solid", gridVisible: true },
    render: {
      width: 1920,
      height: 1080,
      exposure: 1.35,
      shadows: true,
      ambientOcclusion: false,
      sky: true,
      iblIntensity: 1,
      environment: null,
    },
    timeline: {
      frameRate: 24,
      length: 96,
      currentFrame: 0,
      tracks: [],
    },
  };
}

interface Snapshot {
  label: string;
  project: Project;
}

/**
 * Preview-only timeline evaluation, mirroring `studio-core/src/timeline.rs`.
 *
 * The Rust tests are the source of truth: this exists so the browser preview
 * can scrub a timeline without a shell, and it is deliberately NOT a second
 * implementation to keep in lockstep — if the two disagree, the Rust side
 * wins and this file gets corrected.
 */
function evaluateTimeline(project: Project, frame: number): number {
  const timeline = project.timeline;
  const clamped = Number.isFinite(frame)
    ? Math.min(Math.max(frame, 0), timeline.length)
    : 0;
  for (const track of timeline.tracks) {
    const object = project.scene.objects.find((entry) => entry.id === track.objectId);
    if (!object || object.kind.type === "terrain") continue;
    for (const { channel, keys } of track.channels) {
      if (keys.length === 0) continue;
      const first = keys[0];
      const last = keys[keys.length - 1];
      let value: number;
      if (clamped <= first.frame) value = first.value;
      else if (clamped >= last.frame) value = last.value;
      else {
        let left = first;
        let right = first;
        for (let i = 1; i < keys.length; i += 1) {
          if (keys[i].frame > clamped) {
            right = keys[i];
            break;
          }
          left = keys[i];
        }
        const span = right.frame - left.frame;
        let t = span <= 0 ? 0 : (clamped - left.frame) / span;
        if (left.interpolation === "constant") t = 0;
        else if (left.interpolation === "smooth") t = t * t * (3 - 2 * t);
        value = left.value + (right.value - left.value) * t;
      }
      if (channel === "fovDeg") {
        if (object.kind.type === "camera") object.kind.fovDeg = value;
        continue;
      }
      const axis = channel.endsWith("X") ? "x" : channel.endsWith("Y") ? "y" : "z";
      const target =
        channel.startsWith("translation")
          ? object.transform.translation
          : channel.startsWith("rotation")
            ? object.transform.rotationDeg
            : object.transform.scale;
      target[axis] = value;
    }
  }
  timeline.currentFrame = clamped;
  return clamped;
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
        case "setTerrainDem":
          return `Change DEM of ${find(command.id).name}`;
        case "setTerrainOverlay":
          return "Terrain overlay";
        case "setMeshMaterial":
          return `Material of ${find(command.id).name}`;
        case "setGltfNode":
          return `Select node of ${find(command.id).name}`;
        case "setChannelKeys":
          return command.channels.length === 0
            ? `Clear animation of ${find(command.id).name}`
            : `Key ${find(command.id).name}`;
        case "setTimelineConfig":
          return "Timeline settings";
        case "setRenderSettings":
          return "Render settings";
        case "setAssetPath":
          return "Relink asset";
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
      case "setTransform": {
        const object = find(command.id);
        if (object.kind.type === "terrain")
          throw new Error("terrain position is fixed (the DEM is georeferenced)");
        object.transform = structuredClone(command.transform);
        return { label, affected: [command.id] };
      }

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
        object.kind = {
          type: "mesh",
          source: { type: "primitive", primitive },
          material: object.kind.material,
        };
        return { label, affected: [command.id] };
      }

      case "createObject": {
        if (scene.objects.some((o) => o.id === command.object.id))
          throw new Error("an object with that id already exists");
        const source = command.object.kind.type === "mesh" ? command.object.kind.source : null;
        if (source?.type === "gltf" && !requireProject().assets.some((a) => a.id === source.asset))
          throw new Error("asset does not exist");
        if (command.object.kind.type === "terrain") {
          const terrainAsset = command.object.kind.asset;
          if (!requireProject().assets.some((a) => a.id === terrainAsset))
            throw new Error("asset does not exist");
        }
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
        if (source.kind.type === "terrain")
          throw new Error("a scene holds one terrain; use Change DEM to replace it");
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
        const used =
          requireProject().render.environment === command.id ||
          scene.objects.some(
            (object) =>
              (object.kind.type === "mesh" &&
                object.kind.source.type === "gltf" &&
                object.kind.source.asset === command.id) ||
              (object.kind.type === "mesh" && object.kind.material.texture === command.id) ||
              (object.kind.type === "terrain" &&
                (object.kind.asset === command.id ||
                  object.kind.overlay?.asset === command.id)),
          );
        if (used) throw new Error("asset is still used by the project");
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

      case "setMeshMaterial": {
        const object = find(command.id);
        if (object.kind.type !== "mesh") throw new Error("object is not a mesh");
        object.kind.material = structuredClone(command.material);
        return { label, affected: [command.id] };
      }

      case "setGltfNode": {
        const object = find(command.id);
        if (object.kind.type !== "mesh") throw new Error("object is not a mesh");
        if (object.kind.source.type !== "gltf")
          throw new Error("object is not an imported glTF mesh");
        if (command.node !== null && !command.node.trim())
          throw new Error("node must not be empty");
        object.kind.source.node = command.node;
        return { label, affected: [command.id] };
      }

      case "setChannelKeys": {
        const object = find(command.id);
        if (object.kind.type === "terrain")
          throw new Error("terrain position is fixed (the DEM is georeferenced)");
        for (const keys of command.channels) {
          if (keys.channel === "fovDeg" && object.kind.type !== "camera")
            throw new Error("field of view can only be keyed on a camera");
          if (keys.keys.length === 0) throw new Error("channel has no keyframes");
          let previous: number | null = null;
          for (const key of keys.keys) {
            if (!Number.isFinite(key.value)) throw new Error("keyframe must be a finite value");
            if (keys.channel === "fovDeg" && !(key.value >= 1 && key.value < 179))
              throw new Error("keyframe is outside the valid range");
            if (previous !== null && key.frame <= previous)
              throw new Error("keyframes must be strictly increasing");
            previous = key.frame;
          }
        }
        const timeline = requireProject().timeline;
        const index = timeline.tracks.findIndex((track) => track.objectId === command.id);
        if (command.channels.length === 0) {
          if (index >= 0) timeline.tracks.splice(index, 1);
        } else if (index >= 0) {
          timeline.tracks[index].channels = structuredClone(command.channels);
        } else {
          timeline.tracks.push({ objectId: command.id, channels: structuredClone(command.channels) });
        }
        return { label, affected: [command.id] };
      }

      case "setTimelineConfig": {
        if (command.frameRate < 1 || command.frameRate > 240)
          throw new Error("timeline frame rate must be between 1 and 240");
        if (command.length < 1 || command.length > 100_000)
          throw new Error("timeline length must be between 1 and 100000 frames");
        const timeline = requireProject().timeline;
        timeline.frameRate = command.frameRate;
        timeline.length = command.length;
        timeline.currentFrame = Math.min(timeline.currentFrame, timeline.length);
        return { label, affected: [] };
      }

      case "setRenderSettings":
        requireProject().render = { ...command.render };
        return { label, affected: [] };

      case "setAssetPath": {
        const asset = requireProject().assets.find((entry) => entry.id === command.id);
        if (!asset) throw new Error("asset does not exist");
        // Mirror the core: relink re-runs the non-empty invariant.
        validateAsset({ ...asset, path: command.path, byteSize: command.byteSize });
        asset.path = command.path;
        asset.byteSize = command.byteSize;
        return { label, affected: [] };
      }

      case "setTerrainDem": {
        const object = find(command.id);
        if (object.kind.type !== "terrain") throw new Error("object is not a terrain");
        object.kind.asset = command.asset;
        return { label, affected: [command.id] };
      }

      case "setTerrainOverlay": {
        const object = find(command.id);
        if (object.kind.type !== "terrain") throw new Error("object is not a terrain");
        object.kind.overlay = command.overlay;
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

    // Static table lives in studio-core (Rust-tested); the preview shows no
    // badges rather than duplicating it in TypeScript.
    capabilityTable: async () => null,

    state: async () => (project ? state() : null),

    scenePresets: async (): Promise<ScenePreset[]> => [
      {
        id: "bryce",
        name: "Bryce Canyon",
        description: "4 x 3.8 km · 1024² DEM · hoodoo amphitheatre",
        demName: "Bryce_Canyon_1024.tif",
        grid: [1024, 1024],
        elevationRange: [2099.23, 2553.58],
        worldSpan: [4000, 3800],
        available: true,
      },
      {
        id: "swiss",
        name: "Swiss Alps",
        description: "353 x 226 km · national DEM · Alps to Jura",
        demName: "switzerland_dem.tif",
        grid: [7294, 3200],
        elevationRange: [186, 4503],
        worldSpan: [352600, 225700],
        available: true,
      },
    ],

    newProject: async (name, preset) => {
      project = starterProject(name ?? "Untitled", preset ?? "bryce");
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

    setPlayhead: async (frame) => {
      const applied = evaluateTimeline(requireProject(), frame);
      return { frame: applied, state: state() };
    },

    renderAnimationFrame: async () => {
      throw new Error("frame export needs the desktop shell");
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

    readImageAsset: async (): Promise<MeshFilePayload> => {
      throw new Error("textures need the desktop shell");
    },

    rasterPreview: async (): Promise<MeshFilePayload> => {
      throw new Error("raster drapes need the desktop shell");
    },

    // The harness has no filesystem: a nonzero placeholder keeps asset
    // validation (`byte_size > 0`) usable; real sizes come from the shell.
    fileSize: async (): Promise<number> => 1,

    // The preview has no filesystem: every asset "exists" at its recorded
    // size, so the integrity badges stay quiet while editing.
    checkAssets: async (): Promise<AssetIntegrity[]> =>
      requireProject().assets.map((asset) => ({
        id: asset.id,
        name: asset.name,
        path: asset.path,
        kind: asset.kind,
        expectedBytes: asset.byteSize,
        actualBytes: asset.byteSize,
        error: null,
      })),

    readHdr: async (): Promise<MeshFilePayload> => {
      throw new Error("HDRI environments need the desktop shell");
    },

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

    importTerrainDem: async () => {
      throw new Error("DEM import needs the desktop shell");
    },

    setTerrainOverlay: async () => {
      throw new Error("overlays need the desktop shell");
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

    renderStill: async (_path?: string | null, _view?: RenderView): Promise<RenderFrame> => {
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
