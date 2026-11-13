/**
 * Cartography overlays for the Solid/Material viewport: the geometry the
 * still draws, in the same 0..1 terrain lattice the terrain preview uses,
 * lifted onto the mesh so the lines sit on the ground.
 *
 * Pure math, no three.js — the meshes are assembled by the caller, the tests
 * pin the numbers.
 */
import { TERRAIN_VIEW_SIZE } from "./viewMath.ts";

export interface HeightPreview {
  resolution: number;
  /** Row-major heights; row 0 is the north edge (the raster's top). */
  heights: number[];
  minElevation: number;
  maxElevation: number;
}

/**
 * Bilinear sample of the preview's height grid at a terrain UV.
 * u runs west→east, v runs north→south — the same lattice the vector
 * preview normalises onto.
 */
export function sampleHeight(preview: HeightPreview, u: number, v: number): number {
  const res = preview.resolution;
  const gx = Math.min(Math.max(u, 0), 1) * (res - 1);
  const gy = Math.min(Math.max(v, 0), 1) * (res - 1);
  const x0 = Math.floor(gx);
  const y0 = Math.floor(gy);
  const x1 = Math.min(x0 + 1, res - 1);
  const y1 = Math.min(y0 + 1, res - 1);
  const tx = gx - x0;
  const ty = gy - y0;
  const at = (col: number, row: number): number => {
    const value = preview.heights[row * res + col];
    return Number.isFinite(value) ? value : preview.minElevation;
  };
  const south = at(x0, y0) * (1 - tx) + at(x1, y0) * tx;
  const north = at(x0, y1) * (1 - tx) + at(x1, y1) * tx;
  return south * (1 - ty) + north * ty;
}

/**
 * The world-space lift a drape needs to clear the mesh: one percent of the
 * relief in viewer units, with a floor so a nearly flat DEM still clears
 * z-fighting on the surface.
 */
export function drapeLift(
  spanXz: [number, number],
  elevationRange: [number, number],
  zScale: number,
): number {
  const horizontal = TERRAIN_VIEW_SIZE / Math.max(spanXz[0], spanXz[1]);
  const relief = (elevationRange[1] - elevationRange[0]) * zScale * horizontal;
  return Math.max(0.02, relief * 0.008);
}

/**
 * The world position of a terrain UV, on the mesh surface plus `lift`.
 * Mirrors `buildTerrainMesh`: the grid spans the unit plane, scaled to the
 * footprint, with elevation relative to the preview's minimum.
 */
export function worldPosition(
  preview: HeightPreview,
  spanXz: [number, number],
  zScale: number,
  u: number,
  v: number,
  lift: number,
): [number, number, number] {
  const horizontal = TERRAIN_VIEW_SIZE / Math.max(spanXz[0], spanXz[1]);
  const elevation = sampleHeight(preview, u, v) - preview.minElevation;
  return [
    (u - 0.5) * spanXz[0] * horizontal,
    elevation * zScale * horizontal + lift,
    (v - 0.5) * spanXz[1] * horizontal,
  ];
}

/**
 * Drape a batch of polylines on the terrain and flatten them into a segment
 * soup (one `LineSegments2` draw call per layer).
 *
 * Returns flat vertex positions (xyz per vertex) and `pairs`, the indices of
 * each segment — consecutive vertices of every polyline.
 */
export function drapePolylines(
  lines: number[][][],
  preview: HeightPreview,
  spanXz: [number, number],
  zScale: number,
  lift: number,
): { positions: Float32Array; pairs: Uint32Array } {
  const vertices: number[] = [];
  const segments: number[] = [];
  for (const line of lines) {
    const base = vertices.length / 3;
    for (const point of line) {
      if (point.length < 2) continue;
      const [x, y, z] = worldPosition(preview, spanXz, zScale, point[0], point[1], lift);
      vertices.push(x, y, z);
    }
    const count = vertices.length / 3 - base;
    for (let index = 0; index + 1 < count; index += 1) {
      segments.push(base + index, base + index + 1);
    }
  }
  return {
    positions: new Float32Array(vertices),
    pairs: new Uint32Array(segments),
  };
}

/** A grid annotation anchored on the ground: its world position, ready to project. */
export function anchorWorld(
  preview: HeightPreview,
  spanXz: [number, number],
  zScale: number,
  u: number,
  v: number,
): [number, number, number] {
  // A small lift so the label's ray does not clip the mesh at the anchor.
  return worldPosition(preview, spanXz, zScale, u, v, 0.02);
}

/**
 * Expand the `pairs` index list to the flat endpoint-position layout
 * `LineSegmentsGeometry.setPositions` expects (six floats per segment).
 */
export function expandPairs(positions: Float32Array, pairs: Uint32Array): Float32Array {
  const out = new Float32Array(pairs.length * 3);
  for (let index = 0; index < pairs.length; index += 1) {
    const vertex = pairs[index] * 3;
    out[index * 3] = positions[vertex];
    out[index * 3 + 1] = positions[vertex + 1];
    out[index * 3 + 2] = positions[vertex + 2];
  }
  return out;
}
