/**
 * TypeScript mirror of the Rust wire format in `crates/studio-core`.
 *
 * Hand-written on purpose for Phase 1. `crates/studio-core/tests/schema_contract.rs`
 * pins the Rust side of these shapes, so a rename there fails a test instead of
 * silently producing `undefined` in the inspector.
 */

export interface Vec3 {
  x: number;
  y: number;
  z: number;
}

export interface Transform {
  translation: Vec3;
  rotationDeg: Vec3;
  scale: Vec3;
}

export type Primitive =
  | { type: "cube"; size: number }
  | { type: "sphere"; radius: number }
  | { type: "plane"; size: number };

export interface CameraObject {
  fovDeg: number;
  near: number;
  far: number;
}

export type LightKind = "directional" | "point";

export interface LightObject {
  kind: LightKind;
  /** Linear RGB, 0..1. */
  color: [number, number, number];
  intensity: number;
}

/** Internally tagged enum: the payload is flattened next to `type`. */
export type ObjectKind =
  | { type: "mesh"; primitive: Primitive }
  | ({ type: "camera" } & CameraObject)
  | ({ type: "light" } & LightObject);

export interface SceneObject {
  id: string;
  name: string;
  parent: string | null;
  transform: Transform;
  visible: boolean;
  locked: boolean;
  kind: ObjectKind;
}

export interface Scene {
  id: string;
  name: string;
  objects: SceneObject[];
  activeCamera: string | null;
}

export type Workspace = "studio" | "forge3d";
export type ViewportMode = "solid" | "rendered";

export interface ViewportState {
  mode: ViewportMode;
  gridVisible: boolean;
}

export interface RenderSettings {
  width: number;
  height: number;
}

export interface Project {
  format: string;
  schemaVersion: number;
  id: string;
  name: string;
  createdUnixMs: number;
  updatedUnixMs: number;
  scene: Scene;
  selection: string[];
  workspace: Workspace;
  viewport: ViewportState;
  render: RenderSettings;
}

export interface SessionState {
  project: Project;
  revision: number;
  dirty: boolean;
  path: string | null;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  historyLen: number;
}

export interface Applied {
  label: string;
  revision: number;
  affected: string[];
}

export interface EditResult {
  applied: Applied;
  state: SessionState;
}

export interface RecoveryEntry {
  id: string;
  writtenUnixMs: number;
  sourcePath: string | null;
  projectId: string;
  projectName: string;
  objectCount: number;
  path: string;
}

export interface AutosaveStatus {
  intervalSeconds: number;
  lastWriteUnixMs: number | null;
  lastError: string | null;
  directory: string;
}

export interface ShellInfo {
  name: string;
  version: string;
  phase: string;
}

export type Command =
  | { type: "createObject"; object: SceneObject; index?: number | null }
  | { type: "deleteObject"; id: string }
  | { type: "setTransform"; id: string; transform: Transform }
  | { type: "renameObject"; id: string; name: string }
  | { type: "setVisibility"; id: string; visible: boolean }
  | { type: "setLocked"; id: string; locked: boolean }
  | { type: "setCamera"; id: string; camera: CameraObject }
  | { type: "setLight"; id: string; light: LightObject }
  | { type: "setPrimitive"; id: string; primitive: Primitive }
  | { type: "setActiveCamera"; id: string | null }
  | { type: "setViewport"; viewport: ViewportState }
  | { type: "batch"; commands: Command[] };

export const IDENTITY_TRANSFORM: Transform = {
  translation: { x: 0, y: 0, z: 0 },
  rotationDeg: { x: 0, y: 0, z: 0 },
  scale: { x: 1, y: 1, z: 1 },
};

export function objectKindLabel(kind: ObjectKind): string {
  switch (kind.type) {
    case "mesh":
      return kind.primitive.type;
    case "camera":
      return "camera";
    case "light":
      return `${kind.kind} light`;
  }
}

export function isCamera(object: SceneObject): boolean {
  return object.kind.type === "camera";
}
