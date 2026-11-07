/**
 * The preview half of the lighting-rig contract.
 *
 * `tests/fixtures/lighting-rig.json` is read by this test and by
 * `src-tauri/src/render.rs`. The Rust side asserts the payload the engine
 * receives; this side asserts the numbers the viewport hands to three.js.
 * Same file, same scene — so a fudge factor, an sRGB conversion or a dropped
 * exposure in either renderer fails here instead of showing up as "the render
 * looks different from the preview".
 *
 * Runs with zero dependencies:
 *   npm test   (node --experimental-strip-types src/viewport/lighting.test.ts)
 */
import { lightingRig } from "./lighting.ts";
import fixtureJson from "../../tests/fixtures/lighting-rig.json" with { type: "json" };
import type { Project, RenderSettings, SceneObject } from "../bridge/types";

interface Fixture {
  project: {
    sunRotationDeg: { x: number; y: number; z: number };
    sunColor: [number, number, number];
    sunIntensity: number;
    render: Pick<
      RenderSettings,
      "exposure" | "shadows" | "shadowTechnique" | "shadowResolution" | "environment"
    >;
  };
  expected: {
    sun: {
      azimuthDeg: number;
      elevationDeg: number;
      intensity: number;
      color: [number, number, number];
    };
    exposure: number;
    environment: string | null;
    shadows: { enabled: boolean; technique: string; resolution: number };
  };
}

const fixture = fixtureJson as unknown as Fixture;

let failures = 0;

function near(name: string, actual: number, expected: number, tolerance = 1e-4): void {
  if (!(Math.abs(actual - expected) <= tolerance)) {
    failures += 1;
    console.error(`FAIL ${name}: got ${actual}, want ${expected} ±${tolerance}`);
  }
}

/** The fixture scene: a starter project with the fixture's sun and render settings. */
function projectFromFixture(): Project {
  const sun: SceneObject = {
    id: "sun",
    name: "Sun",
    parent: null,
    visible: true,
    locked: false,
    transform: {
      translation: { x: 4, y: 6, z: 3 },
      rotationDeg: { ...fixture.project.sunRotationDeg },
      scale: { x: 1, y: 1, z: 1 },
    },
    kind: {
      type: "light",
      kind: "directional",
      color: [...fixture.project.sunColor],
      intensity: fixture.project.sunIntensity,
    },
  };

  return {
    format: "studio3d",
    schemaVersion: 4,
    id: "fixture",
    name: "lighting rig fixture",
    createdUnixMs: 0,
    updatedUnixMs: 0,
    assets: [],
    selection: [],
    workspace: "studio",
    scene: { id: "scene", name: "Scene", objects: [sun], activeCamera: null },
    viewport: { mode: "solid", gridVisible: true },
    render: {
      width: 640,
      height: 400,
      exposure: fixture.project.render.exposure,
      shadows: fixture.project.render.shadows,
      ambientOcclusion: false,
      sky: true,
      iblIntensity: 1,
      environment: fixture.project.render.environment,
      environmentBackgroundIntensity: 1,
      stillAov: { albedo: false, normal: false, depth: false },
      stillDenoise: false,
      shadowTechnique: fixture.project.render.shadowTechnique,
      shadowResolution: fixture.project.render.shadowResolution,
    },
    timeline: {
      length: 1,
      frameRate: 24,
      currentFrame: 0,
      tracks: [],
    },
  } as Project;
}

const rig = lightingRig(projectFromFixture());

if (!rig.sun) throw new Error("FAIL sun: the fixture scene has a directional light");
near("sun.azimuthDeg", rig.sun.azimuthDeg, fixture.expected.sun.azimuthDeg);
near("sun.elevationDeg", rig.sun.elevationDeg, fixture.expected.sun.elevationDeg);
near("sun.intensity", rig.sun.intensity, fixture.expected.sun.intensity);
near("sun.color.r", rig.sun.color[0], fixture.expected.sun.color[0]);
near("sun.color.g", rig.sun.color[1], fixture.expected.sun.color[1]);
near("sun.color.b", rig.sun.color[2], fixture.expected.sun.color[2]);
near("exposure", rig.exposure, fixture.expected.exposure);
if (rig.environment !== fixture.expected.environment) {
  failures += 1;
  console.error(`FAIL environment: got ${rig.environment}, want ${fixture.expected.environment}`);
}
if (rig.shadows.enabled !== fixture.expected.shadows.enabled) {
  failures += 1;
  console.error(`FAIL shadows.enabled: got ${rig.shadows.enabled}`);
}
if (rig.shadows.technique !== fixture.expected.shadows.technique) {
  failures += 1;
  console.error(
    `FAIL shadows.technique: got ${rig.shadows.technique}, want ${fixture.expected.shadows.technique}`,
  );
}
near("shadows.resolution", rig.shadows.resolution, fixture.expected.shadows.resolution, 0);

// The direction is the light's travel vector: unit length, pointing down.
const [dx, dy, dz] = rig.sun.direction;
near("direction.length", Math.hypot(dx, dy, dz), 1, 1e-6);
if (!(dy < 0)) {
  failures += 1;
  console.error(`FAIL direction: the sun must travel downward, got y=${dy}`);
}

if (failures > 0) {
  throw new Error(`${failures} lighting-rig assertion(s) failed`);
}
console.log("lightingRig: all assertions passed");
