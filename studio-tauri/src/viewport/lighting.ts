/**
 * The lighting rig both renderers share.
 *
 * Solid and Material draw with three.js, Rendered draws with Forge3D; the only
 * way the three stay the same picture is that they are handed the *same*
 * numbers. This module derives those numbers from the project — one pure
 * function, no three.js, no bridge — so the viewport and the node test that
 * pins it against the renderer's payload read the same source.
 *
 * The invariants it encodes (each was a real bug once):
 *  - the sun is the scene's directional light, with its own colour and
 *    intensity — no fudge factor, no sRGB conversion (`u_shading.light_params`
 *    is `colour * intensity` in linear radiance);
 *  - the exposure is the project's, the same one the engine tone-maps with;
 *  - the environment is the project's HDRI path, or nothing for the built-in sky;
 *  - shadows are on unless the project turns them off, with the project's
 *    filter and map size.
 */
import type { Project, SceneObject, ShadowTechnique } from "../bridge/types";

export interface LightingSun {
  /** Unit direction the light travels, viewport frame (Y up, north at -Z). */
  direction: [number, number, number];
  /** Azimuth the engine receives: east → south, degrees, 0..360. */
  azimuthDeg: number;
  /** Elevation the engine receives, degrees. */
  elevationDeg: number;
  /** Linear RGB radiance multiplier, exactly the project's value. */
  intensity: number;
  /** Linear RGB colour, exactly the project's value. */
  color: [number, number, number];
}

export interface LightingShadowRig {
  enabled: boolean;
  technique: ShadowTechnique;
  resolution: number;
}

export interface LightingRig {
  /** The visible directional light, or null when the scene has none. */
  sun: LightingSun | null;
  exposure: number;
  /** HDRI asset path, or null for the built-in studio sky. */
  environment: string | null;
  shadows: LightingShadowRig;
}

/** The scene's visible directional light — the sun both views light with. */
export function directionalLight(project: Project): SceneObject | null {
  return (
    project.scene.objects.find(
      (object) =>
        object.kind.type === "light" && object.kind.kind === "directional" && object.visible,
    ) ?? null
  );
}

/**
 * Rotate a vector by Euler degrees in the model's `Rz·Ry·Rx` order.
 *
 * This is `Transform::rotate_vector` from `crates/studio-core/src/math.rs`
 * (which uses glam), restated for the test contract. The fixture test fails if
 * the two drift, which is the only reason duplicating twelve lines is safe.
 */
function rotate(rotationDeg: { x: number; y: number; z: number }, v: [number, number, number]): [number, number, number] {
  const rad = Math.PI / 180;
  const [sx, cx] = [Math.sin(rotationDeg.x * rad), Math.cos(rotationDeg.x * rad)];
  const [sy, cy] = [Math.sin(rotationDeg.y * rad), Math.cos(rotationDeg.y * rad)];
  const [sz, cz] = [Math.sin(rotationDeg.z * rad), Math.cos(rotationDeg.z * rad)];

  // Rx
  const y1 = v[1] * cx - v[2] * sx;
  const z1 = v[1] * sx + v[2] * cx;
  // Ry
  const x2 = v[0] * cy + z1 * sy;
  const z2 = -v[0] * sy + z1 * cy;
  // Rz
  const x3 = x2 * cz - y1 * sz;
  const y3 = x2 * sz + y1 * cz;
  return [x3, y3, z2];
}

export function lightingRig(project: Project): LightingRig {
  const environmentId = project.render.environment;
  const environment =
    environmentId !== null
      ? project.assets.find((asset) => asset.id === environmentId)?.path ?? null
      : null;

  const light = directionalLight(project);
  let sun: LightingSun | null = null;
  if (light && light.kind.type === "light") {
    // The light's local -Z is where it travels (`Transform::forward`).
    const direction = rotate(light.transform.rotationDeg, [0, 0, -1]);
    const toward: [number, number, number] = [-direction[0], -direction[1], -direction[2]];
    const elevationDeg = (Math.asin(Math.max(-1, Math.min(1, toward[1]))) * 180) / Math.PI;
    let azimuthDeg = (Math.atan2(toward[2], toward[0]) * 180) / Math.PI;
    if (azimuthDeg < 0) azimuthDeg += 360;
    sun = {
      direction,
      azimuthDeg,
      elevationDeg,
      intensity: Math.max(0, light.kind.intensity),
      color: [...light.kind.color] as [number, number, number],
    };
  }

  return {
    sun,
    exposure: project.render.exposure,
    environment,
    shadows: {
      enabled: project.render.shadows,
      technique: project.render.shadowTechnique,
      resolution: project.render.shadowResolution,
    },
  };
}
