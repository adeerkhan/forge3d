import { useCallback, useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { TransformControls } from "three/addons/controls/TransformControls.js";
import { HDRLoader } from "three/addons/loaders/HDRLoader.js";

import { bridge } from "../bridge/env";
import type {
  CameraObject,
  LightObject,
  Primitive,
  SceneObject,
  Transform,
  ViewportMode,
} from "../bridge/types";
import { ViewportToolbar } from "../components/ViewportToolbar";
import { useSession } from "../state/session-context";
import { AxisGizmo, type Axis } from "./AxisGizmo";
import type { Material, TerrainPreview } from "../bridge/types";
import { base64ToArrayBuffer, disposeTree, loadAssetScene } from "./assets";
import { viewStore } from "./viewStore";
import { TERRAIN_VIEW_SIZE, computeRenderView } from "./viewMath";

const DEG = Math.PI / 180;
const ACCENT = 0xf0a94a;
/**
 * Solid is a Blender-style clay view: no albedo, no textures, no material
 * colour. Shape comes from the scene light only.
 */
const CLAY_COLOUR = 0x8b95a4;
const CAMERA_COLOUR = 0x6fa8ff;
const SOLID_BACKGROUND = 0x101318;
/** Material preview backdrop: the studio HDRI provides the light, this the
 *  neutral surround, so nothing hides the albedo. */
const MATERIAL_BACKGROUND = 0x1c2129;
const SOLID_BG = new THREE.Color(SOLID_BACKGROUND);
const MATERIAL_BG = new THREE.Color(MATERIAL_BACKGROUND);
/** Built-in studio sky — the same gradient the render service writes as its
 *  studio HDR, so the preview and Rendered frames share one environment. */
const SKY_ZENITH = [0.25, 0.40, 0.75] as const;
const SKY_HORIZON = [0.85, 0.72, 0.55] as const;
const SKY_GROUND = [0.10, 0.09, 0.08] as const;
const SKY_GAIN = 1.6;

type Quaternion = [number, number, number, number];

/** A project HDRI decoded into an equirect source plus its prefiltered PMREM. */
interface LoadedEnvironment {
  path: string;
  /** Equirectangular half-float texture — the PMREM input. */
  source: THREE.Texture;
  /** Display-referred 8-bit copy used as the backdrop when the GPU cannot
   *  filter half-float textures (some WebKit/ANGLE webviews). */
  background: THREE.Texture;
  /** PMREM output: what `scene.environment` wants. */
  environment: THREE.Texture;
  target: THREE.WebGLRenderTarget;
}

interface Runtime {
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  orbit: OrbitControls;
  gizmo: TransformControls;
  sun: THREE.DirectionalLight;
  hemi: THREE.HemisphereLight;
  grid: THREE.GridHelper;
  objects: Map<string, THREE.Group>;
  helpers: THREE.BoxHelper[];
  /** Selection + camera-view identity the current helpers were built for. */
  helperKey: string;
  /** Built-in sky gradient, used until a project HDRI replaces it. */
  studioEnvironment: THREE.Texture | null;
  /** Environment lighting the scene right now (PMREM output). */
  environment: THREE.Texture | null;
  /** HDRI source texture for the Material background, when one is loaded. */
  environmentBackground: THREE.Texture | null;
  pmrem: THREE.PMREMGenerator | null;
  /** GPU capability probe: 0 = full quality, 1 = hard shadows, 2 = no IBL,
   *  3 = no shadows. Raised one step at a time when `gl.getError()` reports a
   *  rejected draw (WebKit/ANGLE rejects three's shadow depth textures or
   *  half-float filtering). */
  glStep: number;
  /** False until the probe has seen a clean frame after the last downgrade. */
  glProbed: boolean;
  /** Latest selection, so the canvas click handler is not a stale closure. */
  selection: string[];
}

/** Per-mesh shading state: what the built content looks like, and its clay. */
interface ShadingRecord {
  studioMaterial?: THREE.Material | THREE.Material[];
  clayMaterial?: THREE.Material | THREE.Material[];
  /** Terrain only: decoded albedo drape. */
  drape?: THREE.Texture | null;
}

/** Clamp half-float RGBA data to 8-bit linear values for a safe backdrop. */
function clampHalfFloatToByte(
  data: Uint16Array,
  width: number,
  height: number,
): Uint8Array {
  const out = new Uint8Array(width * height * 4);
  for (let index = 0; index < width * height; index += 1) {
    for (let channel = 0; channel < 4; channel += 1) {
      const value = THREE.DataUtils.fromHalfFloat(data[index * 4 + channel]);
      const clamped = Math.min(1, Math.max(0, value));
      out[index * 4 + channel] = Math.round(clamped * 255);
    }
  }
  return out;
}

/**
 * The engine's built-in sky as a linear equirect texture.
 *
 * `write_sky_hdr` (studio_render.py) bakes the same gradient into the studio
 * HDR the renderer uses when no project environment is set. Keeping one source
 * of truth in both places is what makes Material's orbit and the Rendered frame
 * share a light environment instead of a studio room vs. a sky.
 */
function studioSkyTexture(width = 128, height = 64): THREE.DataTexture {
  const data = new Float32Array(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    const v = y / (height - 1);
    // 0 = zenith, 0.5 = horizon, 1 = nadir.
    const t = Math.min(1, Math.max(0, (0.5 - v) / 0.5));
    for (let x = 0; x < width; x += 1) {
      const offset = (y * width + x) * 4;
      for (let channel = 0; channel < 3; channel += 1) {
        const sky = SKY_HORIZON[channel] * (1 - t) + SKY_ZENITH[channel] * t;
        const value = v < 0.5 ? sky : SKY_GROUND[channel];
        data[offset + channel] = value * SKY_GAIN;
      }
      data[offset + 3] = 1;
    }
  }
  const texture = new THREE.DataTexture(data, width, height, THREE.RGBAFormat, THREE.FloatType);
  // Radiance, not an sRGB image: keep the values the engine would decode.
  texture.colorSpace = THREE.LinearSRGBColorSpace;
  texture.mapping = THREE.EquirectangularReflectionMapping;
  texture.needsUpdate = true;
  return texture;
}

function clayMaterial(side: THREE.Side = THREE.FrontSide): THREE.MeshStandardMaterial {
  return new THREE.MeshStandardMaterial({
    color: CLAY_COLOUR,
    roughness: 0.62,
    metalness: 0.0,
    side,
  });
}

function buildPrimitive(primitive: Primitive, material?: Material): THREE.Object3D {
  let geometry: THREE.BufferGeometry;
  switch (primitive.type) {
    case "cube":
      geometry = new THREE.BoxGeometry(primitive.size, primitive.size, primitive.size);
      break;
    case "sphere":
      geometry = new THREE.SphereGeometry(primitive.radius, 32, 24);
      break;
    case "plane":
      geometry = new THREE.PlaneGeometry(primitive.size, primitive.size);
      break;
  }
  const side = primitive.type === "plane" ? THREE.DoubleSide : THREE.FrontSide;
  const standard = new THREE.MeshStandardMaterial({
    color: material
      ? new THREE.Color(material.baseColor[0], material.baseColor[1], material.baseColor[2])
      : CLAY_COLOUR,
    roughness: material?.roughness ?? 0.62,
    metalness: material?.metallic ?? 0.04,
    side,
  });
  const mesh = new THREE.Mesh(geometry, standard);
  // Both materials live on the mesh: Solid swaps in the clay, Material swaps
  // the authored material back. Neither is rebuilt when the mode changes.
  mesh.userData.owned = true;
  mesh.userData.studioMaterial = standard;
  mesh.userData.clayMaterial = clayMaterial(side);
  mesh.castShadow = true;
  mesh.receiveShadow = true;
  if (primitive.type === "plane") mesh.rotation.x = -Math.PI / 2;
  return mesh;
}

/**
 * Swap a subtree between its authored materials and Solid's clay.
 *
 * Imported glTF scenes share materials with the asset cache, so the original
 * is only referenced (never disposed) and restored verbatim. Terrain has its
 * own drape-aware path ([`applyTerrainShading`]).
 */
function setSolidShading(root: THREE.Object3D, solid: boolean): void {
  root.traverse((node) => {
    const mesh = node as THREE.Mesh;
    if (!(mesh as { isMesh?: boolean }).isMesh) return;
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    const record = node.userData as ShadingRecord;
    if (!record.studioMaterial || !record.clayMaterial) {
      record.studioMaterial = mesh.material;
      record.clayMaterial = clayMaterial(THREE.DoubleSide);
    }
    mesh.material = solid ? record.clayMaterial : record.studioMaterial;
  });
}

/** Solid is clay; Material shows the albedo drape (or clay without one).
 *
 * The drape is drawn opaque. It used to inherit the overlay's strength as
 * material opacity, which made the terrain 82% opaque and blended it with the
 * dark backdrop — the preview then read darker than the same frame from the
 * engine, which composites the overlay onto a lit material instead. Neither the
 * sun nor the exposure may change with a slider, so the preview keeps the
 * surface opaque; the still honours the opacity in its overlay pass.
 */
function applyTerrainShading(mesh: THREE.Mesh, solid: boolean): void {
  const material = mesh.material as THREE.MeshStandardMaterial;
  const { drape = null } = mesh.userData as ShadingRecord;
  const draped = !solid && drape !== null;
  const map = draped ? drape : null;
  const colour = draped ? 0xffffff : CLAY_COLOUR;
  // Playback re-applies shading on every snapshot; only touch the material
  // when the mode (or the drape) actually changed, so `needsUpdate` does not
  // rebuild the program every frame.
  let changed = false;
  if (material.map !== map) {
    material.map = map;
    changed = true;
  }
  if (material.color.getHex() !== colour) {
    material.color.setHex(colour);
    changed = true;
  }
  if (material.transparent) {
    material.transparent = false;
    changed = true;
  }
  if (material.opacity !== 1) {
    material.opacity = 1;
    changed = true;
  }
  if (changed) material.needsUpdate = true;
}

/** Re-apply the mode's shading to one object's built content. */
function applyShading(
  runtime: Runtime,
  object: SceneObject,
  mode: ViewportMode,
): void {
  const group = runtime.objects.get(object.id);
  if (!group) return;
  const solid = mode === "solid";
  if (object.kind.type === "terrain") {
    for (const child of group.children) {
      if (child instanceof THREE.Mesh) applyTerrainShading(child, solid);
    }
    return;
  }
  if (object.kind.type === "mesh") setSolidShading(group, solid);
}

/**
 * Environment lighting and backdrop for the current mode.
 *
 * Solid stays neutral: no IBL and no HDRI sky (the clay view is for shape).
 * Material uses the project HDRI — including as the background, like Blender's
 * Material preview — or the built-in studio sky. The project's IBL intensity
 * scales the light; the fallback is additionally dimmed so it reads as fill.
 */
function recompileMaterials(runtime: Runtime): void {
  for (const group of runtime.objects.values()) {
    group.traverse((node) => {
      const mesh = node as THREE.Mesh;
      const material = mesh.material as THREE.Material | THREE.Material[] | undefined;
      if (Array.isArray(material)) material.forEach((entry) => (entry.needsUpdate = true));
      else if (material) material.needsUpdate = true;
    });
  }
}

/**
 * The GPU rejected a draw: three's shadow depth texture with a comparison
 * sampler, or half-float filtering (WebKit/ANGLE). Degrade one step at a time
 * and say so in the viewport instead of spamming INVALID_OPERATION forever.
 * Returns the note to show.
 */
function applyGpuStep(runtime: Runtime): string {
  runtime.glStep += 1;
  const step = runtime.glStep;
  if (step >= 1) {
    runtime.renderer.shadowMap.type = THREE.BasicShadowMap;
    recompileMaterials(runtime);
  }
  if (step >= 2) {
    // Keep the 8-bit backdrop; drop only the IBL, which needs the half-float
    // PMREM output the platform cannot sample. Every mode now needs the same
    // fallback fill the engine's environment would have provided.
    runtime.environment = null;
    runtime.scene.environment = null;
    runtime.hemi.intensity = 0.25;
  }
  if (step >= 3) {
    runtime.sun.castShadow = false;
    for (const group of runtime.objects.values()) {
      group.traverse((node) => {
        const mesh = node as THREE.Mesh;
        if (mesh.isMesh) mesh.castShadow = false;
      });
    }
  }
  return step === 1
    ? "GPU limit: hard shadows"
    : step === 2
      ? "GPU limit: no image-based lighting"
      : "GPU limit: shadows off";
}

/**
 * Environment lighting and backdrop for the current mode.
 *
 * Lighting is identical in every mode: one sun, one environment. Solid and
 * Material differ only in the surface they show (clay vs. albedo) and in the
 * backdrop (neutral vs. the project HDRI). The project's IBL intensity scales
 * the environment exactly as it scales the engine's.
 */
function applyEnvironment(
  runtime: Runtime,
  mode: "solid" | "material",
  iblIntensity: number,
  backgroundIntensity: number,
): void {
  const solid = mode === "solid";
  const noIbl = runtime.glStep >= 2;
  runtime.scene.environment = noIbl ? null : runtime.environment;
  runtime.scene.environmentIntensity = noIbl ? 0 : iblIntensity;
  // The engine applies no ambient beyond the environment image, so neither does
  // the preview. Only a device that cannot sample the environment (glStep 2)
  // gets the fallback fill, and then every mode gets it.
  runtime.hemi.intensity = noIbl ? 0.25 : 0;
  const backdrop = runtime.environmentBackground;
  if (solid || !backdrop || backgroundIntensity <= 0.001) {
    // Solid stays neutral; 0 on the slider hides the HDRI and leaves the
    // neutral studio backdrop rather than fading to black.
    runtime.scene.background = solid ? SOLID_BG : MATERIAL_BG;
    runtime.scene.backgroundIntensity = 1;
  } else {
    runtime.scene.background = backdrop;
    runtime.scene.backgroundIntensity = backgroundIntensity;
  }
}

/** A small frustum, drawn where the project's active camera sits. */
function buildCameraGizmo(camera: CameraObject): THREE.LineSegments {
  const forward = 1.15;
  const halfHeight = Math.tan((camera.fovDeg * Math.PI) / 360) * forward;
  const halfWidth = halfHeight * 1.45;
  const apex: [number, number, number] = [0, 0, 0];
  const corners: Array<[number, number, number]> = [
    [-halfWidth, -halfHeight, -forward],
    [halfWidth, -halfHeight, -forward],
    [halfWidth, halfHeight, -forward],
    [-halfWidth, halfHeight, -forward],
  ];

  const points: number[] = [];
  corners.forEach((corner, index) => {
    points.push(...apex, ...corner);
    points.push(...corner, ...corners[(index + 1) % corners.length]);
  });
  points.push(0, 0, 0, 0, 0.24, 0, 0, 0, 0, 0.24, 0, 0);

  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.Float32BufferAttribute(points, 3));
  const lines = new THREE.LineSegments(
    geometry,
    new THREE.LineBasicMaterial({ color: CAMERA_COLOUR }),
  );
  lines.userData.owned = true;
  return lines;
}

function buildLightGizmo(light: LightObject): THREE.Group {
  const group = new THREE.Group();
  const colour = new THREE.Color(light.color[0], light.color[1], light.color[2]);

  const bulb = new THREE.Mesh(
    new THREE.SphereGeometry(0.09, 14, 10),
    new THREE.MeshBasicMaterial({ color: colour }),
  );
  bulb.userData.owned = true;
  group.add(bulb);

  if (light.kind === "directional") {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute(
      "position",
      new THREE.Float32BufferAttribute([0, 0, 0, 0, 0, -1.6], 3),
    );
    const ray = new THREE.Line(
      geometry,
      new THREE.LineBasicMaterial({ color: colour, transparent: true, opacity: 0.75 }),
    );
    ray.userData.owned = true;
    group.add(ray);
  } else {
    for (let index = 0; index < 8; index += 1) {
      const angle = (index / 8) * Math.PI * 2;
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute(
        "position",
        new THREE.Float32BufferAttribute(
          [0, 0, 0, Math.cos(angle) * 0.42, Math.sin(angle) * 0.42, 0],
          3,
        ),
      );
      const spike = new THREE.Line(
        geometry,
        new THREE.LineBasicMaterial({ color: colour, transparent: true, opacity: 0.6 }),
      );
      spike.userData.owned = true;
      group.add(spike);
    }
  }
  group.userData.owned = true;
  return group;
}

/**
 * A displaced grid from a sampled DEM.
 *
 * The Studio viewport is a preview: this is real elevation data at reduced
 * resolution, scaled so the scene fits the viewport. Forge3D still receives the
 * original GeoTIFF and renders the full-resolution surface.
 *
 * The surface is neutral clay here. Elevation shading used to double as a fake
 * albedo ramp; Solid must show no colour at all, and Material supplies the real
 * drape ([`applyTerrainShading`]).
 */
function buildTerrainMesh(preview: TerrainPreview, zScale: number): THREE.Mesh {
  const resolution = preview.resolution;
  const geometry = new THREE.PlaneGeometry(1, 1, resolution - 1, resolution - 1);
  geometry.rotateX(-Math.PI / 2);

  const [spanX, spanZ] = preview.spanXz;
  const horizontal = TERRAIN_VIEW_SIZE / Math.max(spanX, spanZ);
  const [minElevation] = [preview.minElevation];

  const position = geometry.attributes.position as THREE.BufferAttribute;
  for (let index = 0; index < position.count; index += 1) {
    const row = Math.floor(index / resolution);
    const column = index % resolution;
    const elevation = preview.heights[row * resolution + column] ?? minElevation;
    position.setY(index, (elevation - minElevation) * zScale * horizontal);
  }

  geometry.computeVertexNormals();
  const material = clayMaterial(THREE.DoubleSide);
  material.roughness = 0.95;
  const mesh = new THREE.Mesh(geometry, material);
  mesh.castShadow = true;
  mesh.receiveShadow = true;
  // The grid is built in unit space; the footprint is what gives it metres.
  mesh.scale.set(spanX * horizontal, 1, spanZ * horizontal);
  mesh.userData.owned = true;
  mesh.userData.studioMaterial = mesh.material;
  mesh.userData.drape = null;
  return mesh;
}

function contentSignature(object: SceneObject, terrainResolution: number): string {
  switch (object.kind.type) {
    case "mesh":
      return `mesh:${JSON.stringify(object.kind.source)}:${JSON.stringify(object.kind.material)}`;
    case "terrain":
      // Preview resolution AND the draped albedo are part of the identity:
      // changing either must rebuild (and re-drape) the mesh, which is exactly
      // what used to be ignored.
      return `terrain:${object.kind.asset}:${object.kind.zScale}:${terrainResolution}:${object.kind.overlay?.asset ?? "none"}:${object.kind.overlay?.opacity ?? ""}`;
    case "camera":
      return `camera:${object.kind.fovDeg}`;
    case "light":
      return `light:${object.kind.kind}:${object.kind.color.join(",")}`;
  }
}

function applyTransform(group: THREE.Object3D, transform: Transform): void {
  group.position.set(
    transform.translation.x,
    transform.translation.y,
    transform.translation.z,
  );
  // Studio's model applies X, then Y, then Z (`Rz * Ry * Rx`), which is three's
  // "ZYX" order — see crates/studio-core/src/math.rs.
  group.rotation.set(
    transform.rotationDeg.x * DEG,
    transform.rotationDeg.y * DEG,
    transform.rotationDeg.z * DEG,
    "ZYX",
  );
  group.scale.set(transform.scale.x, transform.scale.y, transform.scale.z);
}

function readTransform(object: THREE.Object3D): Transform {
  const euler = new THREE.Euler().setFromQuaternion(object.quaternion, "ZYX");
  return {
    translation: { x: object.position.x, y: object.position.y, z: object.position.z },
    rotationDeg: {
      x: euler.x / DEG,
      y: euler.y / DEG,
      z: euler.z / DEG,
    },
    scale: { x: object.scale.x, y: object.scale.y, z: object.scale.z },
  };
}

/**
 * Remove a group's contents. Only geometry this crate created is disposed:
 * imported glTF scenes share geometry and materials with the asset cache, and
 * disposing them would break every other object using the same file. The one
 * material we always own is the Solid clay we put on that clone.
 */
function disposeChildren(group: THREE.Group): void {
  for (const child of [...group.children]) {
    group.remove(child);
    const owned = Boolean(child.userData.owned);
    const seen = new Set<THREE.Material>();
    const disposeMaterial = (material: unknown) => {
      if (!material) return;
      const entries = Array.isArray(material) ? material : [material];
      for (const entry of entries as THREE.Material[]) {
        if (seen.has(entry)) continue;
        seen.add(entry);
        entry.dispose();
      }
    };
    child.traverse((node) => {
      const mesh = node as {
        geometry?: { dispose(): void };
        material?: THREE.Material | THREE.Material[];
        userData?: ShadingRecord;
      };
      if (owned) {
        mesh.geometry?.dispose();
        disposeMaterial(mesh.material);
        disposeMaterial(mesh.userData?.studioMaterial);
      }
      disposeMaterial(mesh.userData?.clayMaterial);
    });
  }
}

export function Viewport3D() {
  const {
    state,
    actions,
    tool,
    space,
    cameraView,
    render,
    terrainResolution,
    setCameraView,
  } = useSession();
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const runtimeRef = useRef<Runtime | null>(null);
  const actionsRef = useRef(actions);
  actionsRef.current = actions;

  /** Current viewport mode: Solid is clay, Material is the lit preview, and
   *  only Rendered composes an engine frame. */
  const mode = state?.project.viewport.mode ?? "solid";
  const viewportModeRef = useRef<ViewportMode>(mode);
  viewportModeRef.current = mode;

  const [quaternion, setQuaternion] = useState<Quaternion>([0, 0, 0, 1]);
  const [size, setSize] = useState({ width: 960, height: 600 });
  const [problem, setProblem] = useState<string | null>(null);
  /** Shown when the GPU probe had to downgrade a feature (see Runtime.glStep). */
  const [gpuNote, setGpuNote] = useState<string | null>(null);
  const savedView = useRef<{
    position: THREE.Vector3;
    quaternion: THREE.Quaternion;
    target: THREE.Vector3;
    fov: number;
  } | null>(null);
  const inCameraView = useRef(false);

  const textureCache = useRef(new Map<string, Promise<THREE.Texture>>());
  const loadTexture = useCallback((path: string): Promise<THREE.Texture> => {
    const cache = textureCache.current;
    let pending = cache.get(path);
    if (!pending) {
      pending = bridge
        .readImageAsset(path)
        .then(
          (payload) =>
            new Promise<THREE.Texture>((resolve, reject) => {
              const loader = new THREE.TextureLoader();
              loader.load(
                `data:image/${payload.kind === "orthophoto" && path.toLowerCase().endsWith(".png") ? "png" : "jpeg"};base64,${payload.base64}`,
                (texture) => {
                  texture.colorSpace = THREE.SRGBColorSpace;
                  texture.wrapS = THREE.RepeatWrapping;
                  texture.wrapT = THREE.RepeatWrapping;
                  resolve(texture);
                },
                undefined,
                reject,
              );
            }),
        );
      cache.set(path, pending);
      pending.catch(() => cache.delete(path));
    }
    return pending;
  }, []);

  /** Decoded drape texture for a terrain overlay (shell-side GeoTIFF decode;
   *  cached by path — opacity is applied on the material, not the texture). */
  const overlayTextureCache = useRef(new Map<string, Promise<THREE.Texture>>());
  const loadOverlayTexture = useCallback((path: string): Promise<THREE.Texture> => {
    const cache = overlayTextureCache.current;
    let pending = cache.get(path);
    if (!pending) {
      pending = bridge
        .rasterPreview(path, 2048)
        .then(
          (payload) =>
            new Promise<THREE.Texture>((resolve, reject) => {
              const loader = new THREE.TextureLoader();
              loader.load(
                `data:image/png;base64,${payload.base64}`,
                (texture) => {
                  texture.colorSpace = THREE.SRGBColorSpace;
                  texture.wrapS = THREE.ClampToEdgeWrapping;
                  texture.wrapT = THREE.ClampToEdgeWrapping;
                  resolve(texture);
                },
                undefined,
                reject,
              );
            }),
        );
      cache.set(path, pending);
      pending.catch(() => cache.delete(path));
    }
    return pending;
  }, []);

  const terrainCache = useRef(new Map<string, Promise<TerrainPreview>>());
  const readTerrain = useCallback(
    (path: string): Promise<TerrainPreview> => {
      const cache = terrainCache.current;
      const key = `${path}:${terrainResolution}`;
      let pending = cache.get(key);
      if (!pending) {
        pending = bridge.terrainPreview(path, terrainResolution);
        cache.set(key, pending);
        pending.catch(() => cache.delete(key));
      }
      return pending;
    },
    [terrainResolution],
  );

  const readMesh = useCallback(
    async (path: string): Promise<ArrayBuffer> => {
      if (bridge.kind !== "tauri") {
        throw new Error("glTF assets need the desktop shell");
      }
      const payload = await bridge.readMeshFile(path);
      return base64ToArrayBuffer(payload.base64);
    },
    [],
  );

  /** The project HDRI currently decoded for this viewport (one at a time). */
  const environmentRef = useRef<LoadedEnvironment | null>(null);
  const loadEnvironment = useCallback(
    async (path: string): Promise<LoadedEnvironment> => {
      const runtime = runtimeRef.current;
      if (!runtime?.pmrem) throw new Error("the viewport is not ready");
      const payload = await bridge.readHdr(path);
      const texData = new HDRLoader().parse(base64ToArrayBuffer(payload.base64));
      if (!texData.data || !texData.width || !texData.height) {
        throw new Error("the HDRI has no image data");
      }
      const source = new THREE.DataTexture(
        texData.data as unknown as Uint16Array,
        texData.width,
        texData.height,
        THREE.RGBAFormat,
        texData.type ?? THREE.HalfFloatType,
      );
      source.mapping = THREE.EquirectangularReflectionMapping;
      source.colorSpace = THREE.LinearSRGBColorSpace;
      source.minFilter = THREE.LinearFilter;
      source.magFilter = THREE.LinearFilter;
      source.generateMipmaps = false;
      source.flipY = texData.flipY ?? true;
      source.needsUpdate = true;
      // PMREM is what `scene.environment` expects; the source doubles as the
      // Material background (PMREM output cannot be drawn as a skybox).
      // A display-referred copy for the backdrop: some webviews reject a
      // half-float texture when it is filtered, and the background is a
      // fallback path anyway (values above 1 clip to white).
      const background = new THREE.DataTexture(
        // HDRLoader defaults to HalfFloatType; that is what the clamp reads.
        clampHalfFloatToByte(texData.data as Uint16Array, texData.width, texData.height),
        texData.width,
        texData.height,
        THREE.RGBAFormat,
        THREE.UnsignedByteType,
      );
      background.mapping = THREE.EquirectangularReflectionMapping;
      background.colorSpace = THREE.LinearSRGBColorSpace;
      background.minFilter = THREE.LinearFilter;
      background.magFilter = THREE.LinearFilter;
      background.generateMipmaps = false;
      background.flipY = source.flipY;
      background.needsUpdate = true;
      const target = runtime.pmrem.fromEquirectangular(source);
      return { path, source, background, environment: target.texture, target };
    },
    [],
  );

  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return;

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({
        canvas,
        antialias: true,
        alpha: false,
        // Ask for the discrete adapter on hybrid hosts instead of letting the
        // browser pick the power-saving one.
        powerPreference: "high-performance",
      });
    } catch (cause) {
      setProblem(
        `WebGL is unavailable in this webview: ${cause instanceof Error ? cause.message : cause}`,
      );
      return;
    }
    renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    // The renderer tone-maps (ACES, exposure 1.35); without this the viewport
    // reads much brighter than the Rendered frame.
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.35;
    // Material preview only, but the map must exist before any light casts.
    renderer.shadowMap.enabled = true;
    renderer.shadowMap.type = THREE.PCFShadowMap;

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(SOLID_BACKGROUND);

    const camera = new THREE.PerspectiveCamera(45, 1, 0.05, 2000);
    camera.position.set(9.5, 7, 11);

    const orbit = new OrbitControls(camera, canvas);
    orbit.target.set(0, 0.8, 0);
    orbit.enableDamping = true;
    orbit.dampingFactor = 0.09;
    orbit.screenSpacePanning = false;
    orbit.minDistance = 0.8;
    orbit.maxDistance = 150;
    // Free orbit, Blender-style: the old 89.5° cap made it impossible to look
    // at the scene from below the ground plane.
    orbit.maxPolarAngle = Math.PI;
    orbit.update();

    const gizmo = new TransformControls(camera, canvas);
    gizmo.setSize(0.85);
    scene.add(gizmo.getHelper());

    const hemi = new THREE.HemisphereLight(0xc8d4e8, 0x1b1f26, 0);
    const sun = new THREE.DirectionalLight(0xffffff, 3);
    sun.position.set(6, 9, 4);
    // One shadow rig for every mode: Solid and Material differ by the surface
    // they show, not by how they are lit.
    sun.castShadow = false;
    sun.shadow.mapSize.set(1024, 1024);
    sun.shadow.bias = -0.0008;
    sun.shadow.camera.near = 0.5;
    sun.shadow.camera.far = 80;
    const shadowExtent = TERRAIN_VIEW_SIZE;
    sun.shadow.camera.left = -shadowExtent;
    sun.shadow.camera.right = shadowExtent;
    sun.shadow.camera.top = shadowExtent;
    sun.shadow.camera.bottom = -shadowExtent;
    sun.shadow.camera.updateProjectionMatrix();
    // Sun + environment only — the engine's terrain shader has no fill light,
    // so neither does the preview. `hemi` is the fallback for a device that
    // cannot sample the environment (see `applyGpuStep`).
    scene.add(hemi, sun);

    // The built-in sky is the *same* gradient the render service writes into its
    // studio HDR (`write_sky_hdr` in studio_render.py), prefiltered once at
    // mount and swapped for the project's HDRI when one is set. A device that
    // cannot filter the float target keeps the fallback hemisphere light.
    const pmrem = new THREE.PMREMGenerator(renderer);
    let studioEnvironment: THREE.Texture | null = null;
    try {
      const sky = studioSkyTexture();
      studioEnvironment = pmrem.fromEquirectangular(sky).texture;
      sky.dispose();
    } catch {
      studioEnvironment = null;
    }

    const grid = new THREE.GridHelper(20, 40, 0x2f3744, 0x1d222a);
    const gridMaterial = grid.material as THREE.Material;
    gridMaterial.transparent = true;
    gridMaterial.opacity = 0.85;
    scene.add(grid);

    const axes = new THREE.AxesHelper(1.1);
    scene.add(axes);

    const runtime: Runtime = {
      renderer,
      scene,
      camera,
      orbit,
      gizmo,
      sun,
      hemi,
      grid,
      objects: new Map(),
      helpers: [],
      helperKey: "",
      studioEnvironment,
      environment: studioEnvironment,
      environmentBackground: null,
      pmrem,
      glStep: 0,
      glProbed: false,
      selection: [],
    };
    runtimeRef.current = runtime;

    const resize = () => {
      const width = container.clientWidth;
      const height = container.clientHeight;
      if (width === 0 || height === 0) return;
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      setSize((current) =>
        current.width === width && current.height === height
          ? current
          : { width, height },
      );
    };
    const observer = new ResizeObserver(resize);
    observer.observe(container);
    resize();

    gizmo.addEventListener("dragging-changed", (event) => {
      orbit.enabled = !(event as unknown as { value: boolean }).value;
    });
    gizmo.addEventListener("objectChange", () => {
      for (const helper of runtime.helpers) helper.update();
    });
    // Rendered mode follows the camera. Viewport frames cost ~190 ms (the SSAO
    // pass, which alone costs ~4.8 s, is reserved for final stills), so this is
    // throttled live rendering rather than one frame per drag.
    let lastRenderRequest = 0;
    orbit.addEventListener("change", () => {
      if (!modeRef.current) return;
      const now = performance.now();
      if (now - lastRenderRequest < 120) return;
      lastRenderRequest = now;
      const target = requestSizeRef.current();
      void actionsRef.current.renderViewport(target.width, target.height, "preview");
    });

    orbit.addEventListener("end", () => {
      if (!modeRef.current) return;
      const target = requestSizeRef.current();
      void actionsRef.current.renderViewport(target.width, target.height, "preview");
    });

    gizmo.addEventListener("mouseUp", () => {
      const target = gizmo.object;
      const id = target?.userData.studioId as string | undefined;
      if (!target || !id) return;
      void actionsRef.current.apply({
        type: "setTransform",
        id,
        transform: readTransform(target),
      });
    });

    // Click (not drag) selects; dragging orbits the viewport.
    let pressed: { x: number; y: number } | null = null;
    const raycaster = new THREE.Raycaster();
    const pointer = new THREE.Vector2();

    const onPointerDown = (event: PointerEvent) => {
      pressed = { x: event.clientX, y: event.clientY };
    };

    const onPointerUp = (event: PointerEvent) => {
      if (!pressed) return;
      const travelled = Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y);
      pressed = null;
      if (travelled > 4 || gizmo.dragging) return;
      // In Rendered the engine image covers this canvas; a click would select
      // whatever happens to sit under the pointer in the hidden scene. Orbit
      // and zoom still work because the overlay no longer eats them.
      if (modeRef.current) return;

      const rect = canvas.getBoundingClientRect();
      pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);

      const hits = raycaster.intersectObjects([...runtime.objects.values()], true);
      let node: THREE.Object3D | null = hits[0]?.object ?? null;
      while (node && !node.userData.studioId) node = node.parent;
      const id = node?.userData.studioId as string | undefined;

      const additive = event.shiftKey || event.ctrlKey || event.metaKey;
      if (!id) {
        if (!additive) void actionsRef.current.select([]);
        return;
      }
      if (!additive) {
        void actionsRef.current.select([id]);
        return;
      }
      const next = new Set(runtime.selection);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      void actionsRef.current.select([...next]);
    };

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointerup", onPointerUp);

    /**
     * Publish the Solid camera in the renderer's frame every frame, so a
     * render reproduces the view on screen instead of a canned framing.
     * The axis/scale/datum math lives in `viewMath` (unit-tested);
     * camera view aims 10 viewport units ahead, free view aims at the
     * orbit target — any point on the view axis frames the same picture,
     * since the mapping is a similarity transform.
     */
    const updateViewStore = () => {
      const project = stateRef.current?.project;
      const object = project?.scene.objects.find(
        (candidate) => candidate.kind.type === "terrain",
      );
      if (!object || object.kind.type !== "terrain") {
        viewStore.current = null;
        return;
      }
      const aim = cameraViewRef.current
        ? camera.position
            .clone()
            .add(
              new THREE.Vector3(0, 0, -1)
                .applyQuaternion(camera.quaternion)
                .multiplyScalar(10),
            )
        : orbit.target;
      viewStore.current = computeRenderView({
        camera: [camera.position.x, camera.position.y, camera.position.z],
        aim: [aim.x, aim.y, aim.z],
        fovDeg: camera.fov,
        spanXz: object.kind.worldSpanXz,
        dimensions: object.kind.dimensions,
        elevationRange: object.kind.elevationRange,
        zScale: object.kind.zScale,
      });
    };

    let frame = 0;
    let lastQuaternion: Quaternion = [0, 0, 0, 1];
    const tick = () => {
      frame = requestAnimationFrame(tick);
      // OrbitControls.update() ends with `camera.lookAt(target)`, which would
      // overwrite the camera-view quaternion every frame. While looking through
      // the project camera, the model owns the view.
      if (!inCameraView.current) orbit.update();
      updateViewStore();
      for (const helper of runtime.helpers) helper.update();
      const q = camera.quaternion;
      if (
        Math.abs(q.x - lastQuaternion[0]) > 1e-4 ||
        Math.abs(q.y - lastQuaternion[1]) > 1e-4 ||
        Math.abs(q.z - lastQuaternion[2]) > 1e-4 ||
        Math.abs(q.w - lastQuaternion[3]) > 1e-4
      ) {
        lastQuaternion = [q.x, q.y, q.z, q.w];
        setQuaternion(lastQuaternion);
      }
      renderer.render(scene, camera);
      // Some webviews reject the first draw of a shadow/float-texture pass.
      // Probe once per change; a rejected draw downgrades one step and the
      // next frame re-probes, so the error cannot repeat forever.
      if (!runtime.glProbed) {
        runtime.glProbed = true;
        const error = renderer.getContext().getError();
        if (error !== renderer.getContext().NO_ERROR) {
          setGpuNote(applyGpuStep(runtime));
          runtime.glProbed = false;
        }
      }
    };
    tick();

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointerup", onPointerUp);
      gizmo.detach();
      gizmo.dispose();
      orbit.dispose();
      for (const group of runtime.objects.values()) {
        disposeChildren(group);
        scene.remove(group);
      }
      for (const helper of runtime.helpers) {
        scene.remove(helper);
        helper.geometry.dispose();
        (helper.material as THREE.Material).dispose();
      }
      grid.geometry.dispose();
      (grid.material as THREE.Material).dispose();
      runtime.studioEnvironment?.dispose();
      runtime.pmrem?.dispose();
      renderer.dispose();
      runtimeRef.current = null;
    };
  }, []);

  // Project state → three.js scene. Runs on every edit; the map diff keeps it
  // to what actually changed.
  useEffect(() => {
    const runtime = runtimeRef.current;
    if (!runtime) return;
    const project = state?.project;
    if (!project) {
      for (const group of runtime.objects.values()) {
        disposeChildren(group);
        runtime.scene.remove(group);
      }
      runtime.objects.clear();
      runtime.selection = [];
      runtime.gizmo.detach();
      for (const helper of runtime.helpers) {
        runtime.scene.remove(helper);
        helper.geometry.dispose();
        (helper.material as THREE.Material).dispose();
      }
      runtime.helpers = [];
      runtime.helperKey = "";
      return;
    }

    runtime.selection = project.selection;
    const activeCameraId = project.scene.activeCamera;
    const live = new Map(project.scene.objects.map((object) => [object.id, object]));
    for (const [id, group] of runtime.objects) {
      if (live.has(id)) continue;
      disposeChildren(group);
      runtime.scene.remove(group);
      runtime.objects.delete(id);
    }

    for (const object of project.scene.objects) {
      let group = runtime.objects.get(object.id);
      if (!group) {
        group = new THREE.Group();
        group.userData.studioId = object.id;
        runtime.scene.add(group);
        runtime.objects.set(object.id, group);
      }
      group.visible = object.visible && !(cameraView && object.id === activeCameraId);
      applyTransform(group, object.transform);

      const signature = contentSignature(object, terrainResolution);
      if (group.userData.signature !== signature) {
        group.userData.signature = signature;
        disposeChildren(group);
      } else {
        // Rebuilt content keeps its geometry; a mode switch (or a material
        // edit) only changes how it is shaded.
        applyShading(runtime, object, viewportModeRef.current);
        continue;
      }

      if (object.kind.type === "mesh") {
        const meshKind = object.kind;
        const source = meshKind.source;
        if (source.type === "primitive") {
          const built = buildPrimitive(source.primitive, meshKind.material);
          group.add(built);
          const textureAsset = meshKind.material.texture
            ? project.assets.find((entry) => entry.id === meshKind.material.texture)
            : undefined;
          if (textureAsset && built instanceof THREE.Mesh) {
            const expected = signature;
            const mesh = built;
            loadTexture(textureAsset.path)
              .then((texture) => {
                if (group.userData.signature !== expected) return;
                const material = mesh.material as THREE.MeshStandardMaterial;
                material.map = texture;
                material.needsUpdate = true;
              })
              .catch(() => undefined);
          }
        } else {
          const asset = project.assets.find((entry) => entry.id === source.asset);
          if (!asset) {
            setProblem(
              `Object "${object.name}" references a missing asset — relink it in the Assets panel.`,
            );
            continue;
          }
          group.add(buildLoadingPlaceholder());
          const expected = signature;
          loadAssetScene(asset.path, readMesh, source.node)
            .then((loaded) => {
              if (group.userData.signature !== expected) {
                disposeTree(loaded);
                return;
              }
              disposeChildren(group);
              group.add(loaded);
              // glTF materials are the Material preview's albedo; Solid draws
              // the same geometry as clay.
              setSolidShading(loaded, viewportModeRef.current === "solid");
              setProblem(null);
            })
            .catch((cause) => {
              setProblem(
                `Could not load ${asset.name}: ${cause instanceof Error ? cause.message : cause}`,
              );
            });
        }
      } else if (object.kind.type === "terrain") {
        const terrainKind = object.kind;
        const asset = project.assets.find((entry) => entry.id === terrainKind.asset);
        if (!asset) {
          setProblem(
            `Object "${object.name}" references a missing DEM — relink it in the Assets panel.`,
          );
          continue;
        }
        const overlay = terrainKind.overlay ?? null;
        const overlayAsset = overlay
          ? project.assets.find((entry) => entry.id === overlay.asset)
          : undefined;
        group.add(buildLoadingPlaceholder());
        const expected = signature;
        const zScale = terrainKind.zScale;
        readTerrain(asset.path)
          .then((preview) => {
            if (group.userData.signature !== expected) return;
            disposeChildren(group);
            const mesh = buildTerrainMesh(preview, zScale);
            group.add(mesh);
            setProblem(null);
            applyTerrainShading(mesh, viewportModeRef.current === "solid");
            // The drape is the terrain's albedo: Material shows it, Solid
            // stays clay. The shell decodes the raster; the browser preview has
            // no file access and shows clay in both.
            if (overlay && overlayAsset?.path && bridge.kind === "tauri") {
              loadOverlayTexture(overlayAsset.path)
                .then((texture) => {
                  if (group.userData.signature !== expected) return;
                  mesh.userData.drape = texture;
                  applyTerrainShading(mesh, viewportModeRef.current === "solid");
                })
                .catch((cause) => {
                  setProblem(
                    `Could not drape ${overlayAsset.name}: ${cause instanceof Error ? cause.message : String(cause)}`,
                  );
                });
            }
          })
          .catch((cause) => {
            setProblem(
              `Could not sample ${asset.name}: ${cause instanceof Error ? cause.message : cause}`,
            );
          });
      } else if (object.kind.type === "camera") {
        group.add(buildCameraGizmo(object.kind));
      } else {
        group.add(buildLightGizmo(object.kind));
      }
      applyShading(runtime, object, viewportModeRef.current);
    }

    // Camera view: the render camera *is* the project's active camera, so an
    // inspector edit moves the view immediately.
    const activeCamera =
      project.scene.objects.find(
        (object) => object.id === project.scene.activeCamera && object.kind.type === "camera",
      ) ?? project.scene.objects.find((object) => object.kind.type === "camera");

    if (cameraView) {
      if (activeCamera && activeCamera.kind.type === "camera") {
        const transform = activeCamera.transform;
        runtime.camera.up.set(0, 1, 0);
        runtime.camera.position.set(
          transform.translation.x,
          transform.translation.y,
          transform.translation.z,
        );
        runtime.camera.quaternion.setFromEuler(
          new THREE.Euler(
            transform.rotationDeg.x * DEG,
            transform.rotationDeg.y * DEG,
            transform.rotationDeg.z * DEG,
            "ZYX",
          ),
        );
        runtime.camera.fov = activeCamera.kind.fovDeg;
        // Keep the projection usable even if the stored values are odd: a huge
        // near plane or a far plane behind the camera renders a black frame.
        const distance = runtime.camera.position.length();
        runtime.camera.near = Math.max(0.01, Math.min(activeCamera.kind.near, distance * 0.1));
        runtime.camera.far = Math.max(activeCamera.kind.far, distance * 8);
        runtime.camera.updateProjectionMatrix();
        runtime.orbit.enabled = false;
        // Clear a previous camera warning without wiping an asset error.
        setProblem((current) =>
          current && current.startsWith("Camera view") ? null : current,
        );
      } else {
        setProblem("Camera view needs a camera object in the scene.");
      }
    }

    // The scene's directional light drives the actual lighting.
    const solidView = viewportModeRef.current === "solid";
    const sunObject = project.scene.objects.find(
      (object) => object.kind.type === "light" && object.kind.kind === "directional" && object.visible,
    );
    if (sunObject && sunObject.kind.type === "light") {
      const direction = new THREE.Vector3(0, 0, -1).applyEuler(
        new THREE.Euler(
          sunObject.transform.rotationDeg.x * DEG,
          sunObject.transform.rotationDeg.y * DEG,
          sunObject.transform.rotationDeg.z * DEG,
          "ZYX",
        ),
      );
      // Far enough out that the shadow camera encloses the terrain.
      runtime.sun.position.copy(direction.multiplyScalar(-20));
      const [r, g, b] = sunObject.kind.color;
      // The engine multiplies `sun_color * intensity` in linear radiance
      // (`u_shading.light_params`). Matching that means no sRGB conversion
      // here and no intensity fudge: the preview and the still then evaluate
      // the same `albedo/pi * sun * n.l` term.
      runtime.sun.color.setRGB(r, g, b, THREE.LinearSRGBColorSpace);
      runtime.sun.intensity = Math.max(0, sunObject.kind.intensity);
    }

    // One rig for every mode: the scene sun plus the project environment (or
    // the built-in sky). Solid renders the same light on clay; Material on the
    // albedo drape; Rendered is the engine frame, which uses this same sun,
    // environment and exposure.
    applyEnvironment(
      runtime,
      solidView ? "solid" : "material",
      project.render.iblIntensity,
      project.render.environmentBackgroundIntensity,
    );
    // One exposure for every mode — the project's, applied by the engine in
    // Rendered; the preview now agrees instead of running 0.95 against the
    // engine's 1.35.
    runtime.renderer.toneMappingExposure = project.render.exposure;
    runtime.sun.castShadow = runtime.glStep < 3;

    runtime.grid.visible = project.viewport.gridVisible;

    // Selection boxes follow their objects every tick (playback moves them at
    // the timeline's rate), so only rebuild them when the selection or the
    // camera view changes.
    const helperKey = `${project.selection.join(",")}|${cameraView ? activeCameraId ?? "" : ""}`;
    if (runtime.helperKey === helperKey) {
      for (const helper of runtime.helpers) helper.update();
    } else {
      runtime.helperKey = helperKey;
      for (const helper of runtime.helpers) {
        runtime.scene.remove(helper);
        helper.geometry.dispose();
        (helper.material as THREE.Material).dispose();
      }
      runtime.helpers = [];
      for (const id of project.selection) {
        const group = runtime.objects.get(id);
        if (!group) continue;
        // The box around the camera we are looking through is drawn from inside
        // itself and reads as stray lines across the view.
        if (cameraView && id === activeCameraId) continue;
        const helper = new THREE.BoxHelper(group, ACCENT);
        runtime.scene.add(helper);
        runtime.helpers.push(helper);
      }
    }

    const single =
      project.selection.length === 1
        ? project.scene.objects.find((object) => object.id === project.selection[0])
        : undefined;
    // Terrain carries a measured georeferenced footprint: no gizmo (and the
    // SetTransform command rejects it anyway) — moving it would only desync
    // the Solid view from the renderer, which always draws the DEM centred.
    const editable =
      single && !single.locked && single.visible && single.kind.type !== "terrain"
        ? runtime.objects.get(single.id)
        : undefined;
    if (tool === "select" || !editable) {
      runtime.gizmo.detach();
    } else {
      runtime.gizmo.setMode(tool === "move" ? "translate" : tool === "rotate" ? "rotate" : "scale");
      runtime.gizmo.setSpace(space);
      if (runtime.gizmo.object !== editable) runtime.gizmo.attach(editable);
    }
  }, [cameraView, loadTexture, readMesh, readTerrain, space, state, terrainResolution, tool]);

  // A project HDRI replaces the studio sky for Material and Rendered. The
  // viewport decodes and pre-filters it here (Rendered receives the path and
  // lets the engine upload it); a replaced map is freed after the new one is
  // bound, and a failed load keeps whatever was lighting the scene before.
  const environmentAssetId = state?.project.render.environment ?? null;
  const environmentPath =
    environmentAssetId && state
      ? state.project.assets.find((asset) => asset.id === environmentAssetId)?.path ?? null
      : null;
  useEffect(() => {
    const runtime = runtimeRef.current;
    if (!runtime) return;
    let cancelled = false;
    const settings = stateRef.current?.project.render;
    const apply = () =>
      applyEnvironment(
        runtime,
        viewportModeRef.current === "solid" ? "solid" : "material",
        settings?.iblIntensity ?? 1,
        settings?.environmentBackgroundIntensity ?? 1,
      );

    if (!environmentPath) {
      const previous = environmentRef.current;
      environmentRef.current = null;
      previous?.source.dispose();
      previous?.background.dispose();
      previous?.target.dispose();
      runtime.environment = runtime.studioEnvironment;
      runtime.environmentBackground = null;
      apply();
      return;
    }

    void loadEnvironment(environmentPath)
      .then((loaded) => {
        if (cancelled) {
          loaded.source.dispose();
          loaded.target.dispose();
          return;
        }
        const previous = environmentRef.current;
        environmentRef.current = loaded;
        runtime.environment = loaded.environment;
        runtime.environmentBackground = loaded.background;
          // The PMREM output is new; let the probe see this texture.
        runtime.glProbed = false;
        apply();
        if (previous && previous.path !== loaded.path) {
          previous.source.dispose();
          previous.background.dispose();
          previous.target.dispose();
        }
        setProblem((current) =>
          current && current.startsWith("Could not load HDRI") ? null : current,
        );
      })
      .catch((cause) => {
        if (cancelled) return;
        setProblem(
          `Could not load HDRI ${environmentPath}: ${cause instanceof Error ? cause.message : cause}`,
        );
      });
    return () => {
      cancelled = true;
    };
  }, [environmentPath, loadEnvironment]);

  // The decoded HDRI belongs to the viewport, not the cache, so free it here.
  useEffect(
    () => () => {
      const custom = environmentRef.current;
      environmentRef.current = null;
      custom?.source.dispose();
      custom?.background.dispose();
      custom?.target.dispose();
    },
    [],
  );

  useEffect(() => {
    const runtime = runtimeRef.current;
    if (!runtime) return;

    if (cameraView && !inCameraView.current) {
      savedView.current = {
        position: runtime.camera.position.clone(),
        quaternion: runtime.camera.quaternion.clone(),
        target: runtime.orbit.target.clone(),
        fov: runtime.camera.fov,
      };
      inCameraView.current = true;
      runtime.orbit.enabled = false;
    } else if (!cameraView && inCameraView.current) {
      const saved = savedView.current;
      if (saved) {
        runtime.camera.position.copy(saved.position);
        runtime.camera.quaternion.copy(saved.quaternion);
        runtime.camera.fov = saved.fov;
        runtime.camera.updateProjectionMatrix();
        runtime.orbit.target.copy(saved.target);
      }
      inCameraView.current = false;
      runtime.orbit.enabled = true;
      runtime.orbit.update();
      setProblem((current) =>
        current && current.startsWith("Camera view") ? null : current,
      );
    }
  }, [cameraView]);

  const viewAlong = useCallback((axis: Axis) => {
    if (cameraView) return;
    const runtime = runtimeRef.current;
    if (!runtime) return;
    const direction =
      axis === "x"
        ? new THREE.Vector3(1, 0, 0)
        : axis === "y"
          ? new THREE.Vector3(0, 1, 0)
          : new THREE.Vector3(0, 0, 1);
    const distance = runtime.camera.position.distanceTo(runtime.orbit.target);
    runtime.camera.position.copy(runtime.orbit.target).add(direction.multiplyScalar(distance));
    // Looking straight down needs a different up vector or the view is degenerate.
    runtime.camera.up.set(0, axis === "y" ? 0 : 1, axis === "y" ? -1 : 0);
    runtime.camera.lookAt(runtime.orbit.target);
    runtime.orbit.update();
  }, []);

  /** Viewport render resolution for Rendered mode. Options: 1×/2×/4× the
   *  viewport, the DEM's own pixel size, or the project's output size. */
  type RenderScale = "1" | "2" | "4" | "project" | "dem";
  const [renderScale, setRenderScale] = useState<RenderScale>("1");

  const terrainObject = state?.project.scene.objects.find((object) => object.kind.type === "terrain");
  const demDims = terrainObject?.kind.type === "terrain" ? terrainObject.kind.dimensions : null;

  const renderScaleOptions: Array<{ value: RenderScale; label: string }> = [
    { value: "1", label: "1× viewport" },
    { value: "2", label: "2× viewport" },
    { value: "4", label: "4× viewport" },
    ...(demDims
      ? [{ value: "dem" as RenderScale, label: `DEM (${demDims[0]} × ${demDims[1]})` }]
      : []),
    {
      value: "project",
      label: `Project (${state?.project.render.width ?? 1920} × ${state?.project.render.height ?? 1080})`,
    },
  ];
  const activeScaleLabel =
    renderScaleOptions.find((option) => option.value === renderScale)?.label ?? "1× viewport";

  // An in-app menu — never the native `<select>` popup, which Gdk maps as a
  // separate window (the "popup with a non-topmost parent" warning, and it
  // lands behind the shell window on some WMs).
  const [sizeMenuOpen, setSizeMenuOpen] = useState(false);
  const sizeMenuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!sizeMenuOpen) return;
    const close = (event: MouseEvent) => {
      if (!sizeMenuRef.current?.contains(event.target as Node)) setSizeMenuOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSizeMenuOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", escape);
    };
  }, [sizeMenuOpen]);

  /** Frame size for a Rendered viewport frame. No hidden multipliers — what
   *  the selector shows is the size the engine renders. The 4096 cap scales
   *  the whole frame down uniformly so the aspect never changes. */
  const requestSize = useCallback(() => {
    const base =
      renderScale === "project"
        ? { width: state?.project.render.width ?? 1920, height: state?.project.render.height ?? 1080 }
        : renderScale === "dem" && demDims
          ? { width: demDims[0], height: demDims[1] }
          : { width: Math.max(320, size.width), height: Math.max(240, size.height) };
    const factor = renderScale === "project" || renderScale === "dem" ? 1 : Number(renderScale);
    const cap = 4096;
    const shrink = Math.min(1, cap / (base.width * factor), cap / (base.height * factor));
    return {
      width: Math.max(64, Math.round(base.width * factor * shrink)),
      height: Math.max(64, Math.round(base.height * factor * shrink)),
    };
  }, [demDims, renderScale, size.height, size.width, state?.project.render.height, state?.project.render.width]);
  // Only Rendered composes an engine frame. Solid and Material are the live
  // three.js scene — that is what keeps Material's orbit free and instant.
  const renderedMode = mode === "rendered";
  const modeRef = useRef(renderedMode);
  modeRef.current = renderedMode;
  const cameraViewRef = useRef(cameraView);
  cameraViewRef.current = cameraView;
  const requestSizeRef = useRef(requestSize);
  requestSizeRef.current = requestSize;
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const stateRef = useRef(state);
  stateRef.current = state;
  const setCameraViewRef = useRef(setCameraView);
  setCameraViewRef.current = setCameraView;

  /** The solid camera as a scene transform, aimed at the terrain centre. */
  const cameraTransformFromView = useCallback((): Transform | null => {
    const runtime = runtimeRef.current;
    if (!runtime) return null;
    // `Object3D.lookAt` aims +Z at the target; only cameras and lights aim -Z.
    // Using a plain Object3D made the captured camera face away from the scene,
    // which is why camera view went black.
    const aim = new THREE.PerspectiveCamera();
    aim.position.copy(runtime.camera.position);
    aim.up.set(0, 1, 0);
    aim.lookAt(0, 0, 0);
    const euler = new THREE.Euler().setFromQuaternion(aim.quaternion, "ZYX");
    return {
      translation: {
        x: runtime.camera.position.x,
        y: runtime.camera.position.y,
        z: runtime.camera.position.z,
      },
      rotationDeg: {
        x: euler.x / DEG,
        y: euler.y / DEG,
        z: euler.z / DEG,
      },
      scale: { x: 1, y: 1, z: 1 },
    };
  }, []);

  /**
   * Store the current viewport view as the scene's active camera, so the
   * renderer uses exactly what the user is looking at. Blender uses
   * Ctrl+Alt+Numpad0; this is bound to End / Numpad 1 as requested.
   */
  const captureCamera = useCallback(async () => {
    const runtime = runtimeRef.current;
    const project = stateRef.current?.project;
    if (!runtime || !project) return;
    const transform = cameraTransformFromView();
    if (!transform) return;

    const active = project.scene.objects.find(
      (object) => object.id === project.scene.activeCamera,
    );
    if (!active) {
      setProblem("There is no camera object to receive this view — add one from Add.");
      return;
    }
    await actionsRef.current.apply({
      type: "setTransform",
      id: active.id,
      transform,
    });
    await actionsRef.current.apply({ type: "setActiveCamera", id: active.id });
    // Numpad 1 should also put the viewport into camera mode.
    setCameraViewRef.current(true);
    setProblem(null);
    actionsRef.current.notify(`Camera "${active.name}" set from this view`);
  }, [cameraTransformFromView]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "End" || event.code === "Numpad1") {
        event.preventDefault();
        void captureCamera();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [captureCamera]);

  // A frame belongs to the camera and scene that produced it. Entering Rendered
  // drops the previous image so a stale view cannot cover the live scene while
  // the first engine frame is produced.
  const wasRendered = useRef(false);
  useEffect(() => {
    if (renderedMode && !wasRendered.current) {
      actionsRef.current.clearRenderFrame();
    }
    wasRendered.current = renderedMode;
  }, [renderedMode]);

  // Entering Rendered mode renders once at the current viewport size; orbiting
  // in this mode does not re-render (each frame costs seconds), so the toolbar
  // offers an explicit re-render.
  const renderedOnce = useRef(false);
  useEffect(() => {
    if (!renderedMode) {
      renderedOnce.current = false;
      return;
    }
    if (render.error) {
      // A failed frame must not latch: let the next switch/render try again.
      renderedOnce.current = false;
      return;
    }
    if (renderedOnce.current || render.busy) return;
    renderedOnce.current = true;
    const target = requestSizeRef.current();
    void actions.renderViewport(target.width, target.height, "preview");
  }, [actions, render.busy, renderedMode, render.error, size.height, size.width]);

  // Scene edits (sun, exposure, overlay, z-scale, transforms…) re-render the
  // frame after a short pause, so the Rendered view reflects the project
  // instead of a stale PNG until someone presses Re-render. Debounced and
  // serialised in the session, so dragging a slider still costs one frame.
  const revision = state?.revision ?? -1;
  const revisionRef = useRef(revision);
  useEffect(() => {
    const previous = revisionRef.current;
    revisionRef.current = revision;
    if (!renderedMode || revision === previous || previous === -1) return;
    const timer = window.setTimeout(() => {
      const target = requestSizeRef.current();
      void actionsRef.current.renderViewport(target.width, target.height, "preview");
    }, 350);
    return () => window.clearTimeout(timer);
  }, [revision, renderedMode]);

  // Changing the render scale asks for a frame at the new size right away.
  useEffect(() => {
    if (!renderedMode) return;
    const timer = window.setTimeout(() => {
      const target = requestSizeRef.current();
      void actionsRef.current.renderViewport(target.width, target.height, "preview");
    }, 50);
    return () => window.clearTimeout(timer);
    // Only the scale itself; size/mode changes already flow through the
    // effects above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renderScale]);

  const scene = state?.project.scene;
  const camera = scene?.objects.find((object) => object.id === scene.activeCamera);
  // The renderer is a terrain pipeline: say which objects it cannot draw.
  const unrendered = (scene?.objects ?? [])
    .filter((object) => object.kind.type === "terrain")
    .length === 0
    ? ["everything"]
    : (scene?.objects ?? [])
        .filter((object) => object.kind.type === "mesh" && object.visible)
        .map((object) => object.name);

  return (
    <div className="viewport" ref={containerRef}>
      <canvas ref={canvasRef} className="viewport-canvas" />
      {renderedMode && (
        <div className={`viewport-rendered ${render.frame ? "has-frame" : ""}`}>
          {render.frame && (
            <img
              src={`data:image/png;base64,${render.frame.png}`}
              alt="Forge3D render of the current scene"
              className={render.busy ? "is-updating" : undefined}
            />
          )}
          {render.busy && render.frame && (
            <div className="viewport-rendered__updating">Updating…</div>
          )}
          {unrendered.length > 0 && (
            <div className="viewport-rendered__notice">
              Rendered mode draws the terrain only — {unrendered.join(", ")}{" "}
              {unrendered.length === 1 ? "is" : "are"} not in this frame yet
              (geometry export is pending).
            </div>
          )}
          <div className="viewport-rendered__bar">
            <span>
              {render.busy
                ? render.frame
                  ? `Updating… (showing ${render.frame.width} × ${render.frame.height})`
                  : "Rendering with the Forge3D terrain pipeline…"
                : render.frame
                  ? `${render.frame.width} × ${render.frame.height} · ${(render.frame.ms / 1000).toFixed(2)} s`
                  : "No frame rendered yet"}
            </span>
            <div className="viewport-rendered__scale" ref={sizeMenuRef}>
              <span>Size</span>
              <button
                type="button"
                className="viewport-rendered__scale-button"
                aria-haspopup="menu"
                aria-expanded={sizeMenuOpen}
                title="Viewport frames render at this size; DEM renders at the DEM's own pixels"
                onClick={() => setSizeMenuOpen((open) => !open)}
              >
                {activeScaleLabel}
              </button>
              {sizeMenuOpen && (
                <div className="add-menu__dropdown viewport-rendered__menu" role="menu">
                  {renderScaleOptions.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      role="menuitemradio"
                      aria-checked={renderScale === option.value}
                      onClick={() => {
                        setSizeMenuOpen(false);
                        setRenderScale(option.value);
                      }}
                    >
                      {renderScale === option.value ? "✓ " : ""}
                      {option.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <button
              type="button"
              className="button button--small"
              disabled={render.busy}
              onClick={() => {
                const target = requestSizeRef.current();
                void actions.renderViewport(target.width, target.height, "preview");
              }}
            >
              {render.busy ? "Rendering…" : "Re-render"}
            </button>
          </div>
        </div>
      )}
      <ViewportToolbar
        onRender={() => {
          const target = requestSizeRef.current();
          void actions.renderViewport(target.width, target.height, "preview");
        }}
      />
      <AxisGizmo quaternion={quaternion} onPick={viewAlong} />
      <div className="viewport-overlay viewport-overlay--bottom">
        <span className="viewport-hint">
          {cameraView
            ? "Camera view · press 0 or the camera button to return to the free view"
            : tool === "select"
              ? "Click to select · drag to orbit · middle-drag to pan · scroll to zoom"
              : `${tool[0].toUpperCase()}${tool.slice(1)} tool · drag the gizmo · G/R/S to switch`}
        </span>
        <span className="viewport-camera">
          {camera ? `Active camera: ${camera.name}` : "No active camera"}
          {mode === "material" ? " · three.js preview" : mode === "rendered" ? " · Forge3D engine" : ""}
        </span>
        {gpuNote && <span className="viewport-gpu-note">{gpuNote}</span>}
      </div>
      {problem && !renderedMode && <div className="viewport-problem">{problem}</div>}
      {renderedMode && render.error && (
        <div className="viewport-problem">{render.error}</div>
      )}
    </div>
  );
}

function buildLoadingPlaceholder(): THREE.Object3D {
  const placeholder = new THREE.Mesh(
    new THREE.BoxGeometry(1, 1, 1),
    new THREE.MeshBasicMaterial({ color: 0x3a4150, wireframe: true }),
  );
  placeholder.userData.owned = true;
  return placeholder;
}
