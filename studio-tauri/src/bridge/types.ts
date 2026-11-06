/**
 * TypeScript mirror of the Rust wire format in `crates/studio-core`.
 *
 * Hand-written on purpose. `crates/studio-core/tests/schema_contract.rs` pins
 * the Rust side of these shapes, so a rename there fails a test instead of
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

export type PrimitiveShape = Primitive["type"];

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

export type AssetKind = "gltf" | "glb" | "dem" | "orthophoto" | "hdr";

/** An imported source file. Stored relative to the project where possible. */
export interface Asset {
  id: string;
  name: string;
  path: string;
  kind: AssetKind;
  byteSize: number;
}

/** One asset's on-disk state, checked at open and after relink. */
export interface AssetIntegrity {
  id: string;
  name: string;
  path: string;
  kind: AssetKind;
  /** Size recorded when the asset was imported or relinked. */
  expectedBytes: number;
  /** Size on disk, when the file could be read. */
  actualBytes: number | null;
  /** Why it could not be read (missing, permissions), when it could not. */
  error: string | null;
}

export type MeshSource =
  | { type: "primitive"; primitive: Primitive }
  | { type: "gltf"; asset: string; node: string | null };

/** A DEM-backed terrain object (Forge3D-native). */
export interface TerrainObject {
  asset: string;
  zScale: number;
  albedo: string | null;
  dimensions: [number, number] | null;
  /** Viewer-basis world origin `[mapX, -mapY]`, metres. */
  worldOriginXz: [number, number] | null;
  /** Viewer-basis world span, metres (the viewer's terrain world units). */
  worldSpanXz: [number, number] | null;
  elevationRange: [number, number] | null;
  overlay: TerrainOverlay | null;
}

export interface TerrainOverlay {
  asset: string;
  opacity: number;
  preserveColors: boolean;
}

/** Internally tagged enum: the payload is flattened next to `type`. */
export type ObjectKind =
  | { type: "mesh"; source: MeshSource; material: Material }
  | ({ type: "terrain" } & TerrainObject)
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
export type ViewportMode = "solid" | "rendered" | "material";

/** Renderer quality preset: preview/material are interactive, final is a still. */
export type RenderQuality = "preview" | "final";

export interface ViewportState {
  mode: ViewportMode;
  gridVisible: boolean;
}

export interface RenderSettings {
  width: number;
  height: number;
  exposure: number;
  shadows: boolean;
  ambientOcclusion: boolean;
  /** Sky background in Rendered preview and still frames. */
  sky: boolean;
  /** Environment/IBL intensity multiplier; 0 turns the environment light off. */
  iblIntensity: number;
  /** Radiance .hdr environment asset for image-based lighting, or null for
   *  the built-in studio sky. Shared by the Material preview and Rendered. */
  environment: string | null;
  /** HDRI backdrop brightness (0..1): the Material preview's backdrop and the
   *  Rendered sky exposure. 0 hides it. */
  environmentBackgroundIntensity: number;
}

/** Studio-side material. The terrain renderer does not consume it. */
export interface Material {
  baseColor: [number, number, number];
  roughness: number;
  metallic: number;
  texture: string | null;
}

export interface Project {
  format: string;
  schemaVersion: number;
  id: string;
  name: string;
  createdUnixMs: number;
  updatedUnixMs: number;
  scene: Scene;
  assets: Asset[];
  selection: string[];
  workspace: Workspace;
  viewport: ViewportState;
  render: RenderSettings;
  timeline: Timeline;
}

// ---------------------------------------------------------------- timeline

/** How the value moves between two keyframes. */
export type Interpolation = "linear" | "constant" | "smooth";

/** One animatable scalar: the nine transform components + camera FOV. */
export type Channel =
  | "translationX"
  | "translationY"
  | "translationZ"
  | "rotationX"
  | "rotationY"
  | "rotationZ"
  | "scaleX"
  | "scaleY"
  | "scaleZ"
  | "fovDeg";

export interface Keyframe {
  frame: number;
  value: number;
  interpolation: Interpolation;
}

export interface ChannelKeys {
  channel: Channel;
  /** Sorted by frame, strictly increasing. */
  keys: Keyframe[];
}

export interface ObjectTrack {
  objectId: string;
  channels: ChannelKeys[];
}

/** The project's animation timeline (studio-core `Timeline`). */
export interface Timeline {
  frameRate: number;
  length: number;
  /** Playhead position. View state: scrubbing is not an edit. */
  currentFrame: number;
  tracks: ObjectTrack[];
}

/** What `setSunFromSolar` returns: the ephemeris and the new snapshot. */
export interface SolarResult {
  /** Azimuth from north toward east, degrees (engine display convention). */
  azimuthDeg: number;
  elevationDeg: number;
  daytime: boolean;
  /** Rotation written to the light (degrees, ZYX). */
  rotationDeg: { x: number; y: number; z: number };
  state: SessionState;
}

/** What `setPlayhead` returns: the new snapshot plus the clamped frame. */
export interface PlayheadResult {
  frame: number;
  state: SessionState;
}

/** One frame of an offline sequence export. */
export interface AnimationFrameRequest {
  frame: number;
  width: number;
  height: number;
  quality?: RenderQuality;
  /** Absolute output path (the frontend picks the directory). */
  output: string;
}

/** One rendered frame of a sequence. */
export interface WrittenFrame {
  path: string;
  ms: number;
  /** The clamped timeline frame actually rendered. */
  evaluatedFrame: number;
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

/** A `.gltf`/`.glb` file read by the shell, base64 for the webview. */
export interface MeshFilePayload {
  path: string;
  name: string;
  kind: AssetKind;
  byteSize: number;
  base64: string;
}

// ---------------------------------------------------------------- workspaces

export type CapabilityLevel = "full" | "partial" | "unsupported";

export interface ObjectCapability {
  id: string;
  name: string;
  kind: string;
  level: CapabilityLevel;
  detail: string;
}

export interface CapabilityReport {
  target: Workspace;
  objects: ObjectCapability[];
  degradations: string[];
  supportsCameraSync: boolean;
  hasRenderableContent: boolean;
}

/** One material/light/camera parameter and what currently drives it. */
export interface ParameterCapability {
  parameter: string;
  level: CapabilityLevel;
  detail: string;
}

/** Per-parameter capability table (studio-core is the single source). */
export interface CapabilityTable {
  light: ParameterCapability[];
  environment: ParameterCapability[];
  meshMaterial: ParameterCapability[];
  terrainMaterial: ParameterCapability[];
  camera: ParameterCapability[];
}

export interface CameraSync {
  eye: Vec3;
  target: Vec3;
  up: Vec3;
  fovDeg: number;
}

export interface SunAngles {
  azimuthDeg: number;
  elevationDeg: number;
  intensity: number;
  /** Linear RGB colour of the directional light. */
  color: [number, number, number];
}

export interface ForgeAsset {
  id: string;
  name: string;
  path: string;
  kind: AssetKind;
}

/** What the Forge3D workspace receives for one project revision. */
/** A DEM sampled for the Studio viewport (heights are row-major, bottom-up). */
export interface TerrainPreview {
  ok: boolean;
  width: number;
  height: number;
  minElevation: number;
  maxElevation: number;
  extentMeters: [number, number];
  originXz: [number, number];
  spanXz: [number, number];
  resolution: number;
  heights: number[];
}

export interface TerrainHandoff {
  assetId: string;
  name: string;
  path: string;
  zScale: number;
  dimensions: [number, number] | null;
}

export interface TerrainCamera {
  phiDeg: number;
  thetaDeg: number;
  fovDeg: number;
  radius: number | null;
  target: [number, number, number] | null;
}

export interface Handoff {
  projectId: string;
  sceneId: string;
  revision: number;
  target: Workspace;
  idempotencyKey: string;
  report: CapabilityReport;
  camera: CameraSync | null;
  terrainCamera: TerrainCamera | null;
  terrain: TerrainHandoff | null;
  assets: ForgeAsset[];
  sun: SunAngles | null;
}

export interface EngineStatus {
  available: boolean;
  python: string;
  packageVersion: string | null;
  viewerBinary: string | null;
  /** wgpu backend and GPU adapter, when the probe reached the device. */
  backend: string | null;
  adapter: string | null;
  detail: string;
}

/** A starter scene offered when creating a project. */
export interface ScenePreset {
  id: string;
  name: string;
  description: string;
  demName: string | null;
  grid: [number, number] | null;
  elevationRange: [number, number] | null;
  worldSpan: [number, number] | null;
  available: boolean;
}

/** Camera override from the viewport, so frames match the solid view. */
export interface RenderView {
  phiDeg: number;
  thetaDeg: number;
  /** Orbit distance in the renderer's terrain units (metres). */
  radius: number;
  /**
   * Aim point in the renderer's mesh frame: metres, terrain centred on the
   * origin, Z up. The viewport maps its camera onto this frame before
   * rendering, so the frame matches what the user is looking at.
   */
  target: [number, number, number];
  /** Vertical field of view, degrees. */
  fovDeg: number;
}

export interface RenderFrame {
  width: number;
  height: number;
  /** Render time in milliseconds. */
  ms: number;
  /** Base64 PNG. */
  png: string;
  /** Where a still was written, when it went to disk. */
  path: string | null;
}

export interface RenderStatus {
  running: boolean;
  dem: string | null;
  diagnostics: string[];
}

export interface Forge3dStatus {
  running: boolean;
  ready: boolean;
  pid: number | null;
  error: string | null;
  lastHandoffKey: string | null;
  payloadPath: string | null;
  diagnostics: string[];
}

export type Command =
  | { type: "createObject"; object: SceneObject; index?: number | null }
  | { type: "createPrimitive"; primitive: Primitive; name?: string | null }
  | { type: "deleteObject"; id: string }
  | { type: "duplicateObject"; id: string; name?: string | null }
  | { type: "addAsset"; asset: Asset }
  | {
      type: "importGltf";
      name: string;
      path: string;
      kind: AssetKind;
      byteSize: number;
      node?: string | null;
    }
  | { type: "removeAsset"; id: string }
  | { type: "setTransform"; id: string; transform: Transform }
  | { type: "renameObject"; id: string; name: string }
  | { type: "setVisibility"; id: string; visible: boolean }
  | { type: "setLocked"; id: string; locked: boolean }
  | { type: "setCamera"; id: string; camera: CameraObject }
  | { type: "setLight"; id: string; light: LightObject }
  | { type: "setPrimitive"; id: string; primitive: Primitive }
  | { type: "setMeshMaterial"; id: string; material: Material }
  | { type: "setGltfNode"; id: string; node: string | null }
  | { type: "setChannelKeys"; id: string; channels: ChannelKeys[] }
  | { type: "setTimelineConfig"; frameRate: number; length: number }
  | { type: "setRenderSettings"; render: RenderSettings }
  | { type: "setAssetPath"; id: string; path: string; byteSize: number }
  | { type: "setTerrainZScale"; id: string; zScale: number }
  | {
      type: "setTerrainDem";
      id: string;
      asset: string;
      dimensions?: [number, number] | null;
      elevationRange?: [number, number] | null;
      worldOriginXz?: [number, number] | null;
      worldSpanXz?: [number, number] | null;
    }
  | { type: "setTerrainOverlay"; id: string; overlay: TerrainOverlay | null }
  | { type: "setActiveCamera"; id: string | null }
  | { type: "setViewport"; viewport: ViewportState }
  | { type: "setWorkspace"; workspace: Workspace }
  | { type: "batch"; commands: Command[] };

/** Active manipulation tool, Blender-style. */
export type Tool = "select" | "move" | "rotate" | "scale";

/** Gizmo orientation: world axes or the object's own axes. */
export type ToolSpace = "world" | "local";

export const IDENTITY_TRANSFORM: Transform = {
  translation: { x: 0, y: 0, z: 0 },
  rotationDeg: { x: 0, y: 0, z: 0 },
  scale: { x: 1, y: 1, z: 1 },
};

export const DEFAULT_PRIMITIVES: Record<PrimitiveShape, Primitive> = {
  cube: { type: "cube", size: 2 },
  sphere: { type: "sphere", radius: 1 },
  plane: { type: "plane", size: 2 },
};

export function objectKindLabel(kind: ObjectKind): string {
  switch (kind.type) {
    case "mesh":
      return kind.source.type === "primitive"
        ? kind.source.primitive.type
        : "imported";
    case "terrain":
      return "terrain";
    case "camera":
      return "camera";
    case "light":
      return `${kind.kind} light`;
  }
}

export function isCamera(object: SceneObject): boolean {
  return object.kind.type === "camera";
}

export function primitiveExtent(primitive: Primitive): number {
  return primitive.type === "sphere" ? primitive.radius * 2 : primitive.size;
}
