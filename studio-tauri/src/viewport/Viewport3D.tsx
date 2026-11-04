import { useCallback, useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { TransformControls } from "three/addons/controls/TransformControls.js";

import { bridge } from "../bridge/env";
import type {
  CameraObject,
  LightObject,
  Primitive,
  SceneObject,
  Transform,
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
const MESH_COLOUR = 0x9fb0c8;
const CAMERA_COLOUR = 0x6fa8ff;

type Quaternion = [number, number, number, number];

interface Runtime {
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  orbit: OrbitControls;
  gizmo: TransformControls;
  sun: THREE.DirectionalLight;
  grid: THREE.GridHelper;
  objects: Map<string, THREE.Group>;
  helpers: THREE.BoxHelper[];
  /** Latest selection, so the canvas click handler is not a stale closure. */
  selection: string[];
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
  const standard = new THREE.MeshStandardMaterial({
    color: material
      ? new THREE.Color(material.baseColor[0], material.baseColor[1], material.baseColor[2])
      : MESH_COLOUR,
    roughness: material?.roughness ?? 0.62,
    metalness: material?.metallic ?? 0.04,
    side: primitive.type === "plane" ? THREE.DoubleSide : THREE.FrontSide,
  });
  const mesh = new THREE.Mesh(geometry, standard);
  if (primitive.type === "plane") mesh.rotation.x = -Math.PI / 2;
  mesh.userData.owned = true;
  return mesh;
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
 */
function buildTerrainMesh(preview: TerrainPreview, zScale: number): THREE.Mesh {
  const resolution = preview.resolution;
  const geometry = new THREE.PlaneGeometry(1, 1, resolution - 1, resolution - 1);
  geometry.rotateX(-Math.PI / 2);

  const [spanX, spanZ] = preview.spanXz;
  const horizontal = TERRAIN_VIEW_SIZE / Math.max(spanX, spanZ);
  const [minElevation, maxElevation] = [preview.minElevation, preview.maxElevation];
  const relief = Math.max(1e-3, maxElevation - minElevation);

  const position = geometry.attributes.position as THREE.BufferAttribute;
  const colours = new Float32Array(position.count * 3);
  const low = new THREE.Color(0x4a5545);
  const high = new THREE.Color(0xc4cbd2);

  for (let index = 0; index < position.count; index += 1) {
    const row = Math.floor(index / resolution);
    const column = index % resolution;
    const elevation = preview.heights[row * resolution + column] ?? minElevation;
    const normalized = (elevation - minElevation) / relief;
    position.setY(index, (elevation - minElevation) * zScale * horizontal);
    const colour = low.clone().lerp(high, Math.min(1, Math.max(0, normalized)));
    colours[index * 3] = colour.r;
    colours[index * 3 + 1] = colour.g;
    colours[index * 3 + 2] = colour.b;
  }

  geometry.setAttribute("color", new THREE.BufferAttribute(colours, 3));
  geometry.computeVertexNormals();
  const material = new THREE.MeshStandardMaterial({
    vertexColors: true,
    roughness: 0.95,
    metalness: 0.0,
    side: THREE.DoubleSide,
  });
  const mesh = new THREE.Mesh(geometry, material);
  // The grid is built in unit space; the footprint is what gives it metres.
  mesh.scale.set(spanX * horizontal, 1, spanZ * horizontal);
  mesh.userData.owned = true;
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
 * disposing them would break every other object using the same file.
 */
function disposeChildren(group: THREE.Group): void {
  for (const child of [...group.children]) {
    group.remove(child);
    if (!child.userData.owned) continue;
    child.traverse((node) => {
      const mesh = node as {
        geometry?: { dispose(): void };
        material?: THREE.Material | THREE.Material[];
      };
      mesh.geometry?.dispose();
      if (Array.isArray(mesh.material)) mesh.material.forEach((entry) => entry.dispose());
      else mesh.material?.dispose();
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

  const [quaternion, setQuaternion] = useState<Quaternion>([0, 0, 0, 1]);
  const [size, setSize] = useState({ width: 960, height: 600 });
  const [problem, setProblem] = useState<string | null>(null);
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

  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return;

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: false });
    } catch (cause) {
      setProblem(
        `WebGL is unavailable in this webview: ${cause instanceof Error ? cause.message : cause}`,
      );
      return;
    }
    renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    // The renderer tone-maps (ACES, exposure 1.35); without this the solid view
    // reads much brighter than the Rendered frame.
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 0.95;

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x101318);

    const camera = new THREE.PerspectiveCamera(45, 1, 0.05, 2000);
    camera.position.set(9.5, 7, 11);

    const orbit = new OrbitControls(camera, canvas);
    orbit.target.set(0, 0.8, 0);
    orbit.enableDamping = true;
    orbit.dampingFactor = 0.09;
    orbit.screenSpacePanning = false;
    orbit.minDistance = 0.8;
    orbit.maxDistance = 150;
    orbit.maxPolarAngle = Math.PI * 0.497;
    orbit.update();

    const gizmo = new TransformControls(camera, canvas);
    gizmo.setSize(0.85);
    scene.add(gizmo.getHelper());

    const hemi = new THREE.HemisphereLight(0xc8d4e8, 0x1b1f26, 0.32);
    const sun = new THREE.DirectionalLight(0xffffff, 3);
    sun.position.set(6, 9, 4);
    const fill = new THREE.DirectionalLight(0x93a8c8, 0.16);
    fill.position.set(-7, 4, -6);
    scene.add(hemi, sun, fill);

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
      grid,
      objects: new Map(),
      helpers: [],
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
      const target = requestSizeRef.current(renderQualityRef.current);
      void actionsRef.current.renderViewport(
        target.width,
        target.height,
        renderQualityRef.current,
      );
    });

    orbit.addEventListener("end", () => {
      if (!modeRef.current) return;
      const target = requestSizeRef.current(renderQualityRef.current);
      void actionsRef.current.renderViewport(
        target.width,
        target.height,
        renderQualityRef.current,
      );
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
      if (group.userData.signature === signature) continue;
      group.userData.signature = signature;
      disposeChildren(group);

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
          loadAssetScene(asset.path, readMesh)
            .then((loaded) => {
              if (group.userData.signature !== expected) {
                disposeTree(loaded);
                return;
              }
              disposeChildren(group);
              group.add(loaded);
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
            // Drape the albedo like Material/Rendered do, so all three views
            // show the same ground. The shell decodes the raster; the browser
            // preview has no file access and keeps the elevation ramp.
            if (overlay && overlayAsset?.path && bridge.kind === "tauri") {
              loadOverlayTexture(overlayAsset.path)
                .then((texture) => {
                  if (group.userData.signature !== expected) return;
                  const material = mesh.material as THREE.MeshStandardMaterial;
                  material.map = texture;
                  material.vertexColors = false;
                  material.color.setRGB(1, 1, 1);
                  material.transparent = overlay.opacity < 1;
                  material.opacity = overlay.opacity;
                  material.needsUpdate = true;
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
      runtime.sun.position.copy(direction.multiplyScalar(-12));
      const [r, g, b] = sunObject.kind.color;
      runtime.sun.color.setRGB(r, g, b, THREE.SRGBColorSpace);
      // The engine's lit-sun intensity and three's directional intensity do not
      // share a scale; 0.85 keeps the solid view close to the Rendered frame.
      runtime.sun.intensity = Math.max(0.1, sunObject.kind.intensity * 0.85);
    }

    runtime.grid.visible = project.viewport.gridVisible;

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

  const mode = state?.project.viewport.mode ?? "solid";

  /** Viewport render resolution per mode. Options: 1×/2×/4× the viewport,
   *  the DEM's own pixel size (so Material and Rendered can be compared at
   *  identical, native resolution), or the project's output size. Material
   *  keeps its own scale so the two modes can be set independently. */
  type RenderScale = "0.5" | "1" | "2" | "4" | "project" | "dem";
  const [renderScale, setRenderScale] = useState<RenderScale>("1");
  const [materialScale, setMaterialScale] = useState<RenderScale>("0.5");

  const terrainObject = state?.project.scene.objects.find((object) => object.kind.type === "terrain");
  const demDims = terrainObject?.kind.type === "terrain" ? terrainObject.kind.dimensions : null;

  /** Frame size for one quality tier: material uses its own scale selector;
   *  everything else uses the rendered-mode one. No hidden multipliers — what
   *  the selector shows is the size the engine renders. */
  const requestSize = useCallback(
    (quality: "preview" | "material" | "final") => {
      const scale: RenderScale = quality === "material" ? materialScale : renderScale;
      const base =
        scale === "project"
          ? { width: state?.project.render.width ?? 1920, height: state?.project.render.height ?? 1080 }
          : scale === "dem" && demDims
            ? { width: demDims[0], height: demDims[1] }
            : { width: Math.max(320, size.width), height: Math.max(240, size.height) };
      const factor = scale === "project" || scale === "dem" ? 1 : Number(scale);
      return {
        width: Math.min(4096, Math.max(320, Math.round(base.width * factor))),
        height: Math.min(4096, Math.max(240, Math.round(base.height * factor))),
      };
    },
    [
      demDims,
      materialScale,
      renderScale,
      size.height,
      size.width,
      state?.project.render.height,
      state?.project.render.width,
    ],
  );
  // Material preview and Rendered both show rendered frames; only the quality
  // preset differs.
  const renderedMode = mode === "rendered" || mode === "material";
  const renderQuality: "preview" | "material" | "final" =
    mode === "material" ? "material" : "preview";
  const modeRef = useRef(renderedMode);
  modeRef.current = renderedMode;
  const cameraViewRef = useRef(cameraView);
  cameraViewRef.current = cameraView;
  const renderQualityRef = useRef(renderQuality);
  renderQualityRef.current = renderQuality;
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
    const target = requestSizeRef.current(renderQualityRef.current);
    void actions.renderViewport(target.width, target.height, renderQualityRef.current);
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
      const target = requestSizeRef.current(renderQualityRef.current);
      void actionsRef.current.renderViewport(target.width, target.height, renderQualityRef.current);
    }, 350);
    return () => window.clearTimeout(timer);
  }, [revision, renderedMode]);

  // Changing either mode's render scale asks for a frame at the new size
  // right away.
  useEffect(() => {
    if (!renderedMode) return;
    const timer = window.setTimeout(() => {
      const target = requestSizeRef.current(renderQualityRef.current);
      void actionsRef.current.renderViewport(target.width, target.height, renderQualityRef.current);
    }, 50);
    return () => window.clearTimeout(timer);
    // Only the scales themselves; size/mode changes already flow through the
    // effects above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renderScale, materialScale]);

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
        <div className={`viewport-rendered ${render.frame && !render.busy ? "has-frame" : ""}`}>
          {render.frame && !render.busy && (
            <img
              src={`data:image/png;base64,${render.frame.png}`}
              alt="Forge3D render of the current scene"
            />
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
                ? "Rendering with the Forge3D terrain pipeline…"
                : render.frame
                  ? `${render.frame.width} × ${render.frame.height} · ${(render.frame.ms / 1000).toFixed(2)} s`
                  : "No frame rendered yet"}
            </span>
            <label className="viewport-rendered__scale">
              <span>Size</span>
              <select
                className="input input--select"
                value={renderQuality === "material" ? materialScale : renderScale}
                title="Material and Rendered keep independent sizes; DEM renders at the DEM's own pixels so both modes can be compared 1:1"
                onChange={(event) => {
                  const value = event.target.value as RenderScale;
                  if (renderQuality === "material") setMaterialScale(value);
                  else setRenderScale(value);
                }}
              >
                {renderQuality === "material" && <option value="0.5">½× viewport</option>}
                <option value="1">1× viewport</option>
                {renderQuality !== "material" && (
                  <>
                    <option value="2">2× viewport</option>
                    <option value="4">4× viewport</option>
                  </>
                )}
                {demDims && (
                  <option value="dem">DEM ({demDims[0]} × {demDims[1]})</option>
                )}
                <option value="project">
                  Project ({state?.project.render.width ?? 1920} × {state?.project.render.height ?? 1080})
                </option>
              </select>
            </label>
            <button
              type="button"
              className="button button--small"
              disabled={render.busy}
              onClick={() => {
                const target = requestSizeRef.current(renderQualityRef.current);
                void actions.renderViewport(target.width, target.height, renderQualityRef.current);
              }}
            >
              {render.busy ? "Rendering…" : "Re-render"}
            </button>
          </div>
        </div>
      )}
      <ViewportToolbar
        onRender={() => {
          const target = requestSizeRef.current(renderQualityRef.current);
          void actions.renderViewport(target.width, target.height, renderQualityRef.current);
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
        </span>
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
