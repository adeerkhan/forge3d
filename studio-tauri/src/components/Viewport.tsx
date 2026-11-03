import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from "react";

import type { SceneObject, Transform, Vec3 } from "../bridge/types";
import { useSession } from "../state/session";
import {
  add,
  createProjection,
  DEFAULT_ORBIT,
  normalize,
  projectSegment,
  rotateEuler,
  scale,
  type OrbitCamera,
  type Projection,
  type ScreenPoint,
} from "../viewport/projection";
import { FrameIcon, GridIcon } from "./icons";

interface Size {
  width: number;
  height: number;
}

function worldOf(transform: Transform, local: Vec3): Vec3 {
  const rotated = rotateEuler(
    { x: local.x * transform.scale.x, y: local.y * transform.scale.y, z: local.z * transform.scale.z },
    transform.rotationDeg,
  );
  return add(transform.translation, rotated);
}

const CUBE_CORNERS: Vec3[] = [
  [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5],
  [-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5],
].map(([x, y, z]) => ({ x, y, z }));

const CUBE_EDGES: Array<[number, number]> = [
  [0, 1], [1, 2], [2, 3], [3, 0],
  [4, 5], [5, 6], [6, 7], [7, 4],
  [0, 4], [1, 5], [2, 6], [3, 7],
];

function meshSegments(object: SceneObject, projection: Projection): Array<[ScreenPoint, ScreenPoint]> {
  if (object.kind.type !== "mesh") return [];
  const { primitive } = object.kind;
  const transform = object.transform;
  const segments: Array<[ScreenPoint, ScreenPoint]> = [];

  const push = (a: Vec3, b: Vec3) => {
    const segment = projectSegment(projection, worldOf(transform, a), worldOf(transform, b));
    if (segment) segments.push(segment);
  };

  if (primitive.type === "cube") {
    for (const [from, to] of CUBE_EDGES) {
      push(scale(CUBE_CORNERS[from], primitive.size), scale(CUBE_CORNERS[to], primitive.size));
    }
  } else if (primitive.type === "plane") {
    const h = primitive.size / 2;
    push({ x: -h, y: 0, z: -h }, { x: h, y: 0, z: -h });
    push({ x: h, y: 0, z: -h }, { x: h, y: 0, z: h });
    push({ x: h, y: 0, z: h }, { x: -h, y: 0, z: h });
    push({ x: -h, y: 0, z: h }, { x: -h, y: 0, z: -h });
    push({ x: -h, y: 0, z: 0 }, { x: h, y: 0, z: 0 });
    push({ x: 0, y: 0, z: -h }, { x: 0, y: 0, z: h });
  } else {
    // Three great circles read as a sphere without shading.
    const radius = primitive.radius;
    const points = 28;
    for (let circle = 0; circle < 3; circle += 1) {
      for (let i = 0; i < points; i += 1) {
        const a = (i / points) * Math.PI * 2;
        const b = ((i + 1) / points) * Math.PI * 2;
        const at = (angle: number): Vec3 => {
          const u = Math.cos(angle) * radius;
          const v = Math.sin(angle) * radius;
          if (circle === 0) return { x: u, y: v, z: 0 };
          if (circle === 1) return { x: u, y: 0, z: v };
          return { x: 0, y: u, z: v };
        };
        push(at(a), at(b));
      }
    }
  }
  return segments;
}

function cameraSegments(object: SceneObject, projection: Projection): Array<[ScreenPoint, ScreenPoint]> {
  if (object.kind.type !== "camera") return [];
  const transform = object.transform;
  // Kept small on purpose: the gizmo marks where the render camera sits, it is
  // not a preview of the rendered frame.
  const forward = 1.15;
  const halfHeight = Math.tan((object.kind.fovDeg * Math.PI) / 360) * forward;
  const halfWidth = halfHeight * 1.5;

  const apex: Vec3 = { x: 0, y: 0, z: 0 };
  const corners: Vec3[] = [
    { x: -halfWidth, y: -halfHeight, z: -forward },
    { x: halfWidth, y: -halfHeight, z: -forward },
    { x: halfWidth, y: halfHeight, z: -forward },
    { x: -halfWidth, y: halfHeight, z: -forward },
  ];
  const segments: Array<[ScreenPoint, ScreenPoint]> = [];
  const push = (a: Vec3, b: Vec3) => {
    const segment = projectSegment(projection, worldOf(transform, a), worldOf(transform, b));
    if (segment) segments.push(segment);
  };
  for (let i = 0; i < corners.length; i += 1) {
    push(apex, corners[i]);
    push(corners[i], corners[(i + 1) % corners.length]);
  }
  push(apex, { x: 0, y: 0.22, z: 0 });
  push(apex, { x: 0.22, y: 0, z: 0 });
  return segments;
}

function lightSegments(object: SceneObject, projection: Projection): Array<[ScreenPoint, ScreenPoint]> {
  if (object.kind.type !== "light") return [];
  const transform = object.transform;
  const direction = rotateEuler({ x: 0, y: 0, z: -1 }, transform.rotationDeg);
  const right = normalize({ x: direction.z, y: 0, z: -direction.x });
  const up = normalize({ x: 0, y: 1, z: 0 });
  const segments: Array<[ScreenPoint, ScreenPoint]> = [];
  const push = (a: Vec3, b: Vec3) => {
    const segment = projectSegment(
      projection,
      add(transform.translation, a),
      add(transform.translation, b),
    );
    if (segment) segments.push(segment);
  };

  if (object.kind.kind === "directional") {
    for (const offset of [-1.4, 0, 1.4]) {
      const base = add(scale(right, offset), scale(up, Math.abs(offset) * 0.4));
      push(base, add(base, scale(direction, 2.2)));
    }
  } else {
    for (let i = 0; i < 8; i += 1) {
      const angle = (i / 8) * Math.PI * 2;
      const dir = { x: Math.cos(angle) * 0.35, y: Math.sin(angle) * 0.35, z: 0 };
      push({ x: 0, y: 0, z: 0 }, dir);
    }
  }
  return segments;
}

function Grid({ projection, extent }: { projection: Projection; extent: number }) {
  const lines: ReactElement[] = [];
  for (let i = -extent; i <= extent; i += 1) {
    if (i === 0) continue;
    const major = i % 5 === 0;
    const stroke = major ? "var(--vp-grid-major)" : "var(--vp-grid)";
    const a = projectSegment(projection, { x: i, y: 0, z: -extent }, { x: i, y: 0, z: extent });
    const b = projectSegment(projection, { x: -extent, y: 0, z: i }, { x: extent, y: 0, z: i });
    for (const [key, segment] of [["z", a], ["x", b]] as const) {
      if (!segment) continue;
      const [from, to] = segment;
      lines.push(
        <line
          key={`${key}${i}`}
          x1={from.x}
          y1={from.y}
          x2={to.x}
          y2={to.y}
          stroke={stroke}
          strokeWidth={1}
        />,
      );
    }
  }
  return <g>{lines}</g>;
}

export function Viewport() {
  const { state, actions } = useSession();
  const wrapRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<Size>({ width: 900, height: 620 });
  const [orbit, setOrbit] = useState<OrbitCamera>(DEFAULT_ORBIT);
  const drag = useRef<{ x: number; y: number } | null>(null);

  useEffect(() => {
    const element = wrapRef.current;
    if (!element) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect && rect.width > 0 && rect.height > 0) {
        setSize({ width: rect.width, height: rect.height });
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const projection = useMemo(
    () => createProjection(orbit, size.width, size.height),
    [orbit, size.width, size.height],
  );

  const scene = state?.project.scene;
  const selection = useMemo(() => new Set(state?.project.selection ?? []), [state]);
  const objects = scene?.objects ?? [];

  const ordered = useMemo(() => {
    return [...objects].sort(
      (a, b) => projection.depth(b.transform.translation) - projection.depth(a.transform.translation),
    );
  }, [objects, projection]);

  const pointerDown = useCallback((event: React.PointerEvent<SVGSVGElement>) => {
    drag.current = { x: event.clientX, y: event.clientY };
    (event.target as Element).setPointerCapture?.(event.pointerId);
  }, []);

  const pointerMove = useCallback((event: React.PointerEvent<SVGSVGElement>) => {
    if (!drag.current) return;
    const dx = event.clientX - drag.current.x;
    const dy = event.clientY - drag.current.y;
    drag.current = { x: event.clientX, y: event.clientY };
    setOrbit((current) => ({
      ...current,
      yawDeg: current.yawDeg - dx * 0.35,
      pitchDeg: Math.min(78, Math.max(-6, current.pitchDeg + dy * 0.3)),
    }));
  }, []);

  const pointerUp = useCallback(() => {
    drag.current = null;
  }, []);

  const wheel = useCallback((event: React.WheelEvent<SVGSVGElement>) => {
    event.preventDefault();
    setOrbit((current) => ({
      ...current,
      distance: Math.min(80, Math.max(2.2, current.distance * (event.deltaY > 0 ? 1.08 : 0.925))),
    }));
  }, []);

  const selectObject = useCallback(
    (event: React.MouseEvent, id: string) => {
      event.stopPropagation();
      const additive = event.metaKey || event.ctrlKey || event.shiftKey;
      const current = state?.project.selection ?? [];
      if (!additive) {
        void actions.select([id]);
        return;
      }
      const next = current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id];
      void actions.select(next);
    },
    [actions, state],
  );

  const toggleGrid = useCallback(() => {
    const project = state?.project;
    if (!project) return;
    void actions.apply({
      type: "setViewport",
      viewport: { ...project.viewport, gridVisible: !project.viewport.gridVisible },
    });
  }, [actions, state]);

  const gridVisible = state?.project.viewport.gridVisible ?? true;
  const activeCameraId = scene?.activeCamera ?? null;

  return (
    <div className="viewport" ref={wrapRef}>
      <svg
        className="viewport-canvas"
        width={size.width}
        height={size.height}
        onPointerDown={pointerDown}
        onPointerMove={pointerMove}
        onPointerUp={pointerUp}
        onPointerLeave={pointerUp}
        onWheel={wheel}
        onClick={() => void actions.select([])}
        role="img"
        aria-label="Solid preview of the scene"
      >
        {gridVisible && <Grid projection={projection} extent={10} />}
        <g>
          {(() => {
            const axisX = projectSegment(projection, { x: 0, y: 0, z: 0 }, { x: 3, y: 0, z: 0 });
            const axisZ = projectSegment(projection, { x: 0, y: 0, z: 0 }, { x: 0, y: 0, z: 3 });
            return (
              <>
                {axisX && (
                  <line
                    x1={axisX[0].x}
                    y1={axisX[0].y}
                    x2={axisX[1].x}
                    y2={axisX[1].y}
                    stroke="var(--vp-axis-x)"
                    strokeWidth={1.6}
                  />
                )}
                {axisZ && (
                  <line
                    x1={axisZ[0].x}
                    y1={axisZ[0].y}
                    x2={axisZ[1].x}
                    y2={axisZ[1].y}
                    stroke="var(--vp-axis-z)"
                    strokeWidth={1.6}
                  />
                )}
              </>
            );
          })()}
        </g>

        {ordered.map((object) => {
          const selected = selection.has(object.id);
          const isActive = object.id === activeCameraId;
          const segments = [
            ...meshSegments(object, projection),
            ...cameraSegments(object, projection),
            ...lightSegments(object, projection),
          ];
          if (segments.length === 0) return null;
          const tone = selected
            ? "var(--accent)"
            : object.kind.type === "mesh"
              ? "var(--vp-mesh)"
              : object.kind.type === "camera"
                ? "var(--vp-camera)"
                : "var(--vp-light)";
          const marker = projection.project(object.transform.translation);
          return (
            <g
              key={object.id}
              className="viewport-object"
              onClick={(event) => selectObject(event, object.id)}
              style={{ cursor: "pointer", opacity: object.visible ? 1 : 0.35 }}
            >
              {segments.map(([from, to], index) => (
                <line
                  key={index}
                  x1={from.x}
                  y1={from.y}
                  x2={to.x}
                  y2={to.y}
                  stroke={tone}
                  strokeWidth={selected ? 2 : 1.3}
                  strokeLinejoin="round"
                />
              ))}
              {isActive && marker && (
                <circle
                  cx={marker.x}
                  cy={marker.y}
                  r={9}
                  fill="none"
                  stroke="var(--vp-camera)"
                  strokeDasharray="3 3"
                  strokeWidth={1.2}
                />
              )}
              {!selected && marker && (
                <circle cx={marker.x} cy={marker.y} r={2.4} fill={tone} />
              )}
            </g>
          );
        })}
      </svg>

      <div className="viewport-overlay viewport-overlay--top">
        <span className="chip chip--muted">Solid preview</span>
        <span className="viewport-hint">wireframe preview · Three.js viewport in Phase 2</span>
        <div className="viewport-tools">
          <button
            type="button"
            className={`icon-button ${gridVisible ? "is-on" : ""}`}
            title="Toggle grid"
            onClick={toggleGrid}
          >
            <GridIcon />
          </button>
          <button
            type="button"
            className="icon-button"
            title="Frame scene"
            onClick={() => setOrbit(DEFAULT_ORBIT)}
          >
            <FrameIcon />
          </button>
        </div>
      </div>

      <div className="viewport-overlay viewport-overlay--bottom">
        <span>Drag to orbit · scroll to zoom · click to select</span>
        <span className="viewport-camera">
          {(() => {
            const camera = objects.find((object) => object.id === activeCameraId);
            return camera ? `Active camera: ${camera.name}` : "No active camera";
          })()}
        </span>
      </div>
    </div>
  );
}
