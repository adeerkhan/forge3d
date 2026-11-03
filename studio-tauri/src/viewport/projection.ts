/**
 * Projection math for the Phase 1 solid preview.
 *
 * This is not a renderer: it draws wireframes from the canonical model so the
 * scene graph, transforms, and selection are visible while editing. Phase 2
 * replaces it with a Three.js viewport that renders meshes for real; the orbit
 * camera here is a viewport camera, deliberately independent of the scene's
 * active camera (which is drawn as a gizmo).
 */

import type { Vec3 } from "../bridge/types";

export const DEG = Math.PI / 180;

export interface OrbitCamera {
  yawDeg: number;
  pitchDeg: number;
  distance: number;
  target: Vec3;
}

export const DEFAULT_ORBIT: OrbitCamera = {
  yawDeg: 42,
  pitchDeg: 24,
  distance: 14,
  target: { x: 0, y: 0.8, z: 0 },
};

export const add = (a: Vec3, b: Vec3): Vec3 => ({
  x: a.x + b.x,
  y: a.y + b.y,
  z: a.z + b.z,
});

export const sub = (a: Vec3, b: Vec3): Vec3 => ({
  x: a.x - b.x,
  y: a.y - b.y,
  z: a.z - b.z,
});

export const scale = (a: Vec3, factor: number): Vec3 => ({
  x: a.x * factor,
  y: a.y * factor,
  z: a.z * factor,
});

export const dot = (a: Vec3, b: Vec3): number =>
  a.x * b.x + a.y * b.y + a.z * b.z;

export const cross = (a: Vec3, b: Vec3): Vec3 => ({
  x: a.y * b.z - a.z * b.y,
  y: a.z * b.x - a.x * b.z,
  z: a.x * b.y - a.y * b.x,
});

export const length = (a: Vec3): number => Math.sqrt(dot(a, a));

export const normalize = (a: Vec3): Vec3 => {
  const len = length(a);
  return len > 1e-6 ? scale(a, 1 / len) : { x: 0, y: 0, z: 0 };
};

export const lerp = (a: Vec3, b: Vec3, t: number): Vec3 => ({
  x: a.x + (b.x - a.x) * t,
  y: a.y + (b.y - a.y) * t,
  z: a.z + (b.z - a.z) * t,
});

/**
 * Rotate by Euler degrees in X, then Y, then Z (Rz·Ry·Rx), matching
 * `crates/studio-core/src/math.rs`.
 */
export function rotateEuler(vector: Vec3, rotationDeg: Vec3): Vec3 {
  const { x: rx, y: ry, z: rz } = rotationDeg;
  const [sinX, cosX] = [Math.sin(rx * DEG), Math.cos(rx * DEG)];
  const [sinY, cosY] = [Math.sin(ry * DEG), Math.cos(ry * DEG)];
  const [sinZ, cosZ] = [Math.sin(rz * DEG), Math.cos(rz * DEG)];

  // Rx
  let y = vector.y * cosX - vector.z * sinX;
  let z = vector.y * sinX + vector.z * cosX;
  let x = vector.x;

  // Ry
  const x2 = x * cosY + z * sinY;
  z = -x * sinY + z * cosY;
  x = x2;

  // Rz
  const x3 = x * cosZ - y * sinZ;
  y = x * sinZ + y * cosZ;
  x = x3;

  return { x, y, z };
}

export interface ScreenPoint {
  x: number;
  y: number;
}

export interface Projection {
  eye: Vec3;
  right: Vec3;
  up: Vec3;
  forward: Vec3;
  focal: number;
  center: ScreenPoint;
  /** Project a world point; `null` when it is behind the eye. */
  project(point: Vec3): ScreenPoint | null;
  /** Depth along the view direction, for painter's-order hints. */
  depth(point: Vec3): number;
}

export function createProjection(
  camera: OrbitCamera,
  width: number,
  height: number,
): Projection {
  const yaw = camera.yawDeg * DEG;
  const pitch = camera.pitchDeg * DEG;
  const offset: Vec3 = {
    x: Math.cos(pitch) * Math.sin(yaw),
    y: Math.sin(pitch),
    z: Math.cos(pitch) * Math.cos(yaw),
  };
  const eye = add(camera.target, scale(offset, camera.distance));
  const forward = normalize(sub(camera.target, eye));
  const right = normalize(cross(forward, { x: 0, y: 1, z: 0 }));
  const up = normalize(cross(right, forward));
  const focal = (height / 2) / Math.tan(22.5 * DEG);
  const center = { x: width / 2, y: height / 2 };

  const depth = (point: Vec3) => dot(sub(point, eye), forward);

  return {
    eye,
    right,
    up,
    forward,
    focal,
    center,
    depth,
    project(point) {
      const relative = sub(point, eye);
      const d = dot(relative, forward);
      if (d <= 0.05) return null;
      return {
        x: center.x + (dot(relative, right) / d) * focal,
        y: center.y - (dot(relative, up) / d) * focal,
      };
    },
  };
}

/** Clip a world-space segment against the camera plane and project it. */
export function projectSegment(
  projection: Projection,
  from: Vec3,
  to: Vec3,
): [ScreenPoint, ScreenPoint] | null {
  const near = 0.06;
  const fromDepth = projection.depth(from);
  const toDepth = projection.depth(to);
  if (fromDepth <= near && toDepth <= near) return null;
  if (fromDepth <= near) {
    const t = (near - fromDepth) / (toDepth - fromDepth);
    from = lerp(from, to, t);
  } else if (toDepth <= near) {
    const t = (near - toDepth) / (fromDepth - toDepth);
    to = lerp(to, from, t);
  }
  const a = projection.project(from);
  const b = projection.project(to);
  return a && b ? [a, b] : null;
}

export function pointsToPath(points: ScreenPoint[]): string {
  return points
    .map((point, index) => `${index === 0 ? "M" : "L"}${point.x.toFixed(2)} ${point.y.toFixed(2)}`)
    .join(" ");
}
