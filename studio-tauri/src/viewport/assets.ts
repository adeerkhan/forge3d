/**
 * glTF/GLB loading for the Studio viewport.
 *
 * Bytes come from the shell (`read_mesh_file`) or from an in-flight import.
 * Parsed scenes are cached per path: a project can reference the same asset
 * from several objects, and re-parsing a GLB on every state change would make
 * the viewport unusable.
 */

import type { Group, Material } from "three";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";

const bytesByPath = new Map<string, ArrayBuffer>();
const scenesByPath = new Map<string, Promise<Group>>();

/** Decode the base64 payload the shell returns for a mesh file. */
export function base64ToArrayBuffer(base64: string): ArrayBuffer {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

export function putAssetBytes(path: string, buffer: ArrayBuffer): void {
  bytesByPath.set(path, buffer);
  scenesByPath.delete(path);
}

export function cachedAssetBytes(path: string): ArrayBuffer | undefined {
  return bytesByPath.get(path);
}

export function parseGltf(buffer: ArrayBuffer, path: string): Promise<Group> {
  return new Promise((resolve, reject) => {
    const loader = new GLTFLoader();
    // GLB is self-contained. A .gltf with external buffers cannot be resolved
    // from the webview, so the loader fails loudly and we translate that into
    // advice rather than a stack trace.
    loader.parse(
      buffer,
      "",
      (gltf) => resolve(gltf.scene),
      (error) => {
        const message = error instanceof Error ? error.message : String(error);
        reject(
          new Error(
            `${path}: ${message}. Self-contained .glb files are supported; a .gltf that references external .bin or texture files is not yet.`,
          ),
        );
      },
    );
  });
}

/** Parse without keeping anything: used to validate a file before importing. */
export async function inspectGltf(
  buffer: ArrayBuffer,
  path: string,
): Promise<{ meshes: number }> {
  const scene = await parseGltf(buffer, path);
  let meshes = 0;
  scene.traverse((object) => {
    if ((object as { isMesh?: boolean }).isMesh) meshes += 1;
  });
  disposeTree(scene);
  if (meshes === 0) {
    throw new Error(`${path} contains no meshes to import`);
  }
  return { meshes };
}

/**
 * Load an asset, either from the import cache or through `read` (the shell).
 * Returns a clone, so two objects can use the same asset independently.
 */
export async function loadAssetScene(
  path: string,
  read: (path: string) => Promise<ArrayBuffer>,
): Promise<Group> {
  let pending = scenesByPath.get(path);
  if (!pending) {
    const bytes = bytesByPath.get(path);
    pending = bytes ? parseGltf(bytes, path) : read(path).then((buffer) => parseGltf(buffer, path));
    scenesByPath.set(path, pending);
    pending.catch(() => scenesByPath.delete(path));
  }
  const scene = await pending;
  return scene.clone(true);
}

export function disposeTree(root: Group | null): void {
  root?.traverse((object) => {
    const mesh = object as { geometry?: { dispose(): void }; material?: Material | Material[] };
    mesh.geometry?.dispose();
    const material = mesh.material;
    if (Array.isArray(material)) material.forEach((entry) => entry.dispose());
    else material?.dispose();
  });
}
