import { useEffect, useState } from "react";
import type {
  Asset,
  CapabilityLevel,
  CapabilityTable,
  Material,
  ParameterCapability,
  CameraObject,
  LightKind,
  LightObject,
  Primitive,
  SceneObject,
  Transform,
} from "../bridge/types";
import { objectKindLabel } from "../bridge/types";
import { bridge } from "../bridge/env";
import { base64ToArrayBuffer, listGltfNodes } from "../viewport/assets";
import { useSession } from "../state/session-context";
import { NumberField, RangeField, Section, TextField, Toggle } from "./fields";

// ---------------------------------------------------------------- capability

let capabilityTablePromise: Promise<CapabilityTable | null> | null = null;

/** The shell's static capability table, fetched once per app run. */
function useCapabilities(): CapabilityTable | null {
  const [table, setTable] = useState<CapabilityTable | null>(null);
  useEffect(() => {
    capabilityTablePromise ??= bridge.capabilityTable().catch(() => null);
    let live = true;
    void capabilityTablePromise.then((value) => {
      if (live) setTable(value);
    });
    return () => {
      live = false;
    };
  }, []);
  return table;
}

const LEVEL_GLYPH: Record<CapabilityLevel, string> = {
  full: "✓",
  partial: "~",
  unsupported: "✗",
};

/** Per-parameter support badges: glyph = level, hover = what it drives.
 *  Phase 5's "capability-tagged parameters", straight from studio-core. */
function CapabilityBadges({ rows }: { rows: ParameterCapability[] | undefined }) {
  if (!rows || rows.length === 0) return null;
  return (
    <div className="capability-badges">
      {rows.map((row) => (
        <span
          key={row.parameter}
          className="badge"
          title={`${LEVEL_GLYPH[row.level]} ${row.level} — ${row.detail}`}
        >
          {LEVEL_GLYPH[row.level]} {row.parameter}
        </span>
      ))}
    </div>
  );
}

function TransformEditor({
  object,
  transform,
  disabled,
}: {
  object: SceneObject;
  transform: Transform;
  disabled: boolean;
}) {
  const { actions } = useSession();

  const commit = (next: Transform) => {
    void actions.apply({ type: "setTransform", id: object.id, transform: next });
  };

  const axis = (
    key: "translation" | "rotationDeg" | "scale",
    field: "x" | "y" | "z",
  ) => (value: number) =>
    commit({ ...transform, [key]: { ...transform[key], [field]: value } });

  return (
    <>
      <Section title="Transform">
        <div className="triple">
          <NumberField label="X" value={transform.translation.x} disabled={disabled} onCommit={axis("translation", "x")} />
          <NumberField label="Y" value={transform.translation.y} disabled={disabled} onCommit={axis("translation", "y")} />
          <NumberField label="Z" value={transform.translation.z} disabled={disabled} onCommit={axis("translation", "z")} />
        </div>
        <span className="group-label">Translation</span>

        <div className="triple">
          <NumberField label="X" value={transform.rotationDeg.x} disabled={disabled} onCommit={axis("rotationDeg", "x")} />
          <NumberField label="Y" value={transform.rotationDeg.y} disabled={disabled} onCommit={axis("rotationDeg", "y")} />
          <NumberField label="Z" value={transform.rotationDeg.z} disabled={disabled} onCommit={axis("rotationDeg", "z")} />
        </div>
        <span className="group-label">Rotation (degrees)</span>

        <div className="triple">
          <NumberField label="X" value={transform.scale.x} step={0.05} disabled={disabled} onCommit={axis("scale", "x")} />
          <NumberField label="Y" value={transform.scale.y} step={0.05} disabled={disabled} onCommit={axis("scale", "y")} />
          <NumberField label="Z" value={transform.scale.z} step={0.05} disabled={disabled} onCommit={axis("scale", "z")} />
        </div>
        <span className="group-label">Scale</span>
        {object.kind.type === "terrain" && (
          <p className="hint">
            Terrain is georeferenced — its position and rotation are fixed so
            the Solid view and the renderer frame the same ground. Use Vertical
            scale (Terrain section) for relief.
          </p>
        )}
      </Section>

      <Section title="Object">
        <Toggle
          label="Visible"
          checked={object.visible}
          onChange={(visible) =>
            void actions.apply({ type: "setVisibility", id: object.id, visible })
          }
        />
        <Toggle
          label="Locked"
          checked={object.locked}
          onChange={(locked) =>
            void actions.apply({ type: "setLocked", id: object.id, locked })
          }
        />
      </Section>
    </>
  );
}

function MeshEditor({ object, primitive }: { object: SceneObject; primitive: Primitive }) {
  const { actions } = useSession();
  const extent = primitive.type === "sphere" ? primitive.radius : primitive.size;

  const setShape = (shape: string) => {
    const withShape: Primitive =
      shape === "sphere"
        ? { type: "sphere", radius: extent }
        : shape === "plane"
          ? { type: "plane", size: extent }
          : { type: "cube", size: extent };
    void actions.apply({ type: "setPrimitive", id: object.id, primitive: withShape });
  };

  const setExtent = (value: number) => {
    const next: Primitive =
      primitive.type === "sphere"
        ? { type: "sphere", radius: value }
        : primitive.type === "plane"
          ? { type: "plane", size: value }
          : { type: "cube", size: value };
    void actions.apply({ type: "setPrimitive", id: object.id, primitive: next });
  };

  return (
    <Section title="Mesh">
      <label className="field">
        <span className="field-label">Primitive</span>
        <select
          className="input input--select"
          value={primitive.type}
          onChange={(event) => setShape(event.target.value)}
        >
          <option value="cube">Cube</option>
          <option value="sphere">Sphere</option>
          <option value="plane">Plane</option>
        </select>
      </label>
      <NumberField
        label={primitive.type === "sphere" ? "Radius" : "Size"}
        value={extent}
        step={0.1}
        min={0.01}
        onCommit={setExtent}
      />
    </Section>
  );
}

function CameraEditor({
  object,
  camera,
  active,
}: {
  object: SceneObject;
  camera: CameraObject;
  active: boolean;
}) {
  const { actions } = useSession();
  const caps = useCapabilities();
  const commit = (next: CameraObject) =>
    void actions.apply({ type: "setCamera", id: object.id, camera: next });

  return (
    <Section
      title="Camera"
      actions={
        active ? (
          <span className="badge badge--active">active</span>
        ) : (
          <button
            type="button"
            className="text-button"
            onClick={() => void actions.apply({ type: "setActiveCamera", id: object.id })}
          >
            Set active
          </button>
        )
      }
    >
      <NumberField
        label="Field of view"
        value={camera.fovDeg}
        step={1}
        min={1}
        max={179}
        onCommit={(fovDeg) => commit({ ...camera, fovDeg })}
      />
      <div className="pair">
        <NumberField
          label="Near"
          value={camera.near}
          step={0.05}
          min={0.001}
          onCommit={(near) => commit({ ...camera, near })}
        />
        <NumberField
          label="Far"
          value={camera.far}
          step={10}
          onCommit={(far) => commit({ ...camera, far })}
        />
      </div>
      <CapabilityBadges rows={caps?.camera} />
    </Section>
  );
}

function LightEditor({ object, light }: { object: SceneObject; light: LightObject }) {
  const { actions } = useSession();
  const caps = useCapabilities();
  const commit = (next: LightObject) =>
    void actions.apply({ type: "setLight", id: object.id, light: next });

  const channel = (index: 0 | 1 | 2) => (value: number) => {
    const color: [number, number, number] = [...light.color] as [number, number, number];
    color[index] = value;
    commit({ ...light, color });
  };

  return (
    <Section title="Light">
      <label className="field">
        <span className="field-label">Type</span>
        <select
          className="input input--select"
          value={light.kind}
          title="Both views light with a directional sun; point lights are not implemented in either view"
          onChange={(event) => commit({ ...light, kind: event.target.value as LightKind })}
        >
          <option value="directional">Directional</option>
          {/* Neither the Solid view nor the renderer implements point lights;
              creating one would silently diverge the two (renders fall back
              to the default sun). Kept selectable *from* for legacy scenes. */}
          <option value="point" disabled>Point — unsupported</option>
        </select>
      </label>
      <NumberField
        label="Intensity"
        value={light.intensity}
        step={0.1}
        min={0}
        onCommit={(intensity) => commit({ ...light, intensity })}
      />
      <div className="triple">
        <NumberField label="R" value={light.color[0]} step={0.02} min={0} max={1} onCommit={channel(0)} />
        <NumberField label="G" value={light.color[1]} step={0.02} min={0} max={1} onCommit={channel(1)} />
        <NumberField label="B" value={light.color[2]} step={0.02} min={0} max={1} onCommit={channel(2)} />
      </div>
      <span className="group-label">Colour (linear RGB)</span>
      <CapabilityBadges rows={caps?.light} />
    </Section>
  );
}

function AssetsPanel() {
  const { state, actions, assetIntegrity } = useSession();
  const project = state?.project;
  if (!project || project.assets.length === 0) return null;
  const integrity = new Map(assetIntegrity.map((entry) => [entry.id, entry]));
  const problems = assetIntegrity.filter(
    (entry) => entry.actualBytes === null || entry.actualBytes !== entry.expectedBytes,
  );
  return (
    <Section title="Assets">
      {problems.length > 0 && (
        <p className="hint hint--boxed">
          {problems.length === 1
            ? "1 referenced file needs attention."
            : `${problems.length} referenced files need attention.`}{" "}
          Missing files keep their path; relink them before rendering.
        </p>
      )}
      <ul className="multi-list">
        {project.assets.map((asset) => {
          const usage =
            project.scene.objects.filter(
              (object) =>
                (object.kind.type === "mesh" &&
                  (object.kind.source.type === "gltf"
                    ? object.kind.source.asset === asset.id
                    : object.kind.material.texture === asset.id)) ||
                (object.kind.type === "terrain" &&
                  (object.kind.asset === asset.id ||
                    object.kind.overlay?.asset === asset.id)),
            ).length + (project.render.environment === asset.id ? 1 : 0);
          const check = integrity.get(asset.id);
          const exists = check ? check.actualBytes !== null : true;
          const sizeMatches = check
            ? check.actualBytes === check.expectedBytes
            : true;
          return (
            <li key={asset.id}>
              <span title={check?.error ?? asset.path}>{asset.name}</span>
              {check && !exists && (
                <span className="badge badge--missing" title={check.error ?? undefined}>
                  missing
                </span>
              )}
              {check && exists && !sizeMatches && (
                <span
                  className="badge badge--changed"
                  title={`recorded ${check.expectedBytes} bytes, found ${check.actualBytes}`}
                >
                  changed
                </span>
              )}
              <span className="mono">
                {usage} use{usage === 1 ? "" : "s"}
              </span>
              <button
                type="button"
                className="text-button"
                onClick={() => void actions.relinkAsset(asset.id)}
              >
                Relink…
              </button>
            </li>
          );
        })}
      </ul>
      <p className="hint">
        Files are checked when the project opens and whenever an asset is
        imported or relinked. Relinking points the asset at a new path without
        touching scene references.
      </p>
    </Section>
  );
}

function RenderSettingsEditor() {
  const { state, actions, bridgeKind } = useSession();
  const caps = useCapabilities();
  const render = state?.project.render;
  if (!render) return null;
  const environment = render.environment
    ? state?.project.assets.find((asset) => asset.id === render.environment)
    : undefined;
  const commit = (patch: Partial<typeof render>) => {
    void actions.apply({ type: "setRenderSettings", render: { ...render, ...patch } });
  };
  return (
    <Section title="Render output">
      <div className="pair">
        <NumberField label="Width" value={render.width} step={16} min={64} max={16384} onCommit={(width) => commit({ width: Math.round(width) })} />
        <NumberField label="Height" value={render.height} step={16} min={64} max={16384} onCommit={(height) => commit({ height: Math.round(height) })} />
      </div>
      <NumberField label="Exposure" value={render.exposure} step={0.05} min={0.05} max={8} onCommit={(exposure) => commit({ exposure })} />
      <Toggle label="Shadows" checked={render.shadows} onChange={(shadows) => commit({ shadows })} />
      <Toggle
        label="Ambient occlusion (final, ~5 s)"
        checked={render.ambientOcclusion}
        onChange={(ambientOcclusion) => commit({ ambientOcclusion })}
      />
      <Toggle
        label="Sky background"
        checked={render.sky}
        onChange={(sky) => commit({ sky })}
      />
      <NumberField
        label="Environment / IBL intensity"
        value={render.iblIntensity}
        step={0.05}
        min={0}
        max={4}
        onCommit={(iblIntensity) => commit({ iblIntensity })}
      />
      <span className="group-label">Environment (HDRI)</span>
      <div className="readout readout--compact">
        <dt>Map</dt>
        <dd>{environment?.name ?? "built-in studio sky"}</dd>
      </div>
      <div className="inline-actions">
        <button
          type="button"
          className="button button--small"
          disabled={bridgeKind !== "tauri"}
          title={
            bridgeKind === "tauri"
              ? "Pick a Radiance .hdr for image-based lighting; both Material and Rendered use it"
              : "HDRI environments need the desktop shell"
          }
          onClick={() => void actions.importEnvironment()}
        >
          {environment ? "Replace HDRI…" : "Add HDRI…"}
        </button>
        {environment && (
          <button
            type="button"
            className="button button--small"
            onClick={() => void actions.clearEnvironment()}
          >
            Remove
          </button>
        )}
      </div>
      <RangeField
        label="Background intensity"
        value={render.environmentBackgroundIntensity}
        min={0}
        max={1}
        step={0.01}
        disabled={!environment}
        onCommit={(environmentBackgroundIntensity) =>
          commit({ environmentBackgroundIntensity })
        }
      />
      {!environment && (
        <p className="hint">
          Add an HDRI to control its backdrop brightness here.
        </p>
      )}
      <CapabilityBadges rows={caps?.environment} />
      <p className="hint">
        Settings are stored with the project and applied to both the Rendered
        viewport and the Render button. Solid is a neutral clay view; the
        Material preview lights the scene with this HDRI (or the built-in studio
        sky) plus soft sun shadows and never renders through the engine.
        Rendered uses the HDRI for lighting but draws the engine's procedural
        sky, so the backdrop slider affects the Material preview only.
      </p>
    </Section>
  );
}

function MaterialEditor({ object, material }: { object: SceneObject; material: Material }) {
  const { actions, state } = useSession();
  const caps = useCapabilities();
  const texture = material.texture
    ? state?.project.assets.find((asset) => asset.id === material.texture)
    : undefined;
  const commit = (next: Material) =>
    void actions.apply({ type: "setMeshMaterial", id: object.id, material: next });
  const channel = (index: 0 | 1 | 2) => (value: number) => {
    const baseColor: [number, number, number] = [...material.baseColor] as [number, number, number];
    baseColor[index] = value;
    commit({ ...material, baseColor });
  };
  return (
    <Section title="Material">
      <div className="triple">
        <NumberField label="R" value={material.baseColor[0]} step={0.02} min={0} max={1} onCommit={channel(0)} />
        <NumberField label="G" value={material.baseColor[1]} step={0.02} min={0} max={1} onCommit={channel(1)} />
        <NumberField label="B" value={material.baseColor[2]} step={0.02} min={0} max={1} onCommit={channel(2)} />
      </div>
      <span className="group-label">Base colour (linear RGB)</span>
      <div className="pair">
        <NumberField label="Roughness" value={material.roughness} step={0.05} min={0} max={1} onCommit={(roughness) => commit({ ...material, roughness })} />
        <NumberField label="Metallic" value={material.metallic} step={0.05} min={0} max={1} onCommit={(metallic) => commit({ ...material, metallic })} />
      </div>
      <div className="readout readout--compact">
        <dt>Texture</dt>
        <dd>{texture?.name ?? "none"}</dd>
      </div>
      <div className="inline-actions">
        <button
          type="button"
          className="button button--small"
          onClick={() => void actions.importMaterialTexture()}
        >
          {texture ? "Replace texture…" : "Add texture…"}
        </button>
        {texture && (
          <button
            type="button"
            className="button button--small"
            onClick={() => void actions.clearMaterialTexture()}
          >
            Remove
          </button>
        )}
      </div>
      <CapabilityBadges rows={caps?.meshMaterial} />
      <p className="hint">
        Studio renders this material; the Forge3D terrain pipeline does not
        consume it, and the capability report says so.
      </p>
    </Section>
  );
}

function TerrainEditor({
  object,
  terrain,
  asset,
}: {
  object: SceneObject;
  terrain: Extract<SceneObject["kind"], { type: "terrain" }>;
  asset: Asset | undefined;
}) {
  const { actions, terrainResolution, setTerrainResolution, state } = useSession();
  const caps = useCapabilities();
  const elevation =
    terrain.elevationRange === null
      ? "—"
      : `${terrain.elevationRange[0].toFixed(0)}–${terrain.elevationRange[1].toFixed(0)} m`;
  const footprint =
    terrain.worldSpanXz === null
      ? "—"
      : terrain.worldSpanXz.map((value) => `${(value / 1000).toFixed(2)} km`).join(" × ");
  const pixels =
    terrain.dimensions === null ? "—" : `${terrain.dimensions[0]} × ${terrain.dimensions[1]} px`;

  const overlayAsset =
    terrain.overlay
      ? state?.project.assets.find((entry) => entry.id === terrain.overlay?.asset)
      : undefined;

  return (
    <Section title="Terrain">
      <div className="readout">
        <dt>DEM</dt>
        <dd>{asset?.name ?? "missing asset"}</dd>
        <dt>Grid</dt>
        <dd className="mono">{pixels}</dd>
        <dt>Footprint</dt>
        <dd className="mono">{footprint}</dd>
        <dt>Elevation</dt>
        <dd className="mono">{elevation}</dd>
      </div>

      <NumberField
        label="Vertical scale"
        value={terrain.zScale}
        step={0.1}
        min={0.05}
        max={20}
        onCommit={(zScale) =>
          void actions.apply({ type: "setTerrainZScale", id: object.id, zScale })
        }
      />
      <label className="field">
        <span className="field-label">
          Solid mesh · {terrainResolution} × {terrainResolution}
        </span>
        <input
          className="slider"
          type="range"
          min={32}
          max={512}
          step={32}
          value={terrainResolution}
          onChange={(event) => setTerrainResolution(Number(event.target.value))}
        />
      </label>

      <div className="inline-actions">
        <button
          type="button"
          className="button button--small"
          onClick={() => void actions.importTerrainDem()}
        >
          Change DEM…
        </button>
      </div>

      <Section title="Material">
        <div className="readout readout--compact">
          <dt>Engine set</dt>
          <dd>terrain_default()</dd>
          <dt>Layers</dt>
          <dd>rock · grass · dirt · snow</dd>
          <dt>Texture maps</dt>
          <dd>none found — flat layer colours</dd>
        </div>
        <p className="hint">
          The engine shades terrain from four triplanar PBR layers and resolves
          their rock/grass/dirt/snow maps from{" "}
          <span className="mono">assets/materials/</span> (or{" "}
          <span className="mono">FORGE3D_MATERIAL_DIR</span>). This checkout
          ships none, so the default renders flat. Attach an albedo raster to
          give the surface imagery — new Bryce projects come with the NAIP
          orthophoto already attached.
        </p>
        <div className="readout readout--compact">
          <dt>Albedo raster</dt>
          <dd>{overlayAsset?.name ?? "none — engine default"}</dd>
        </div>
        {terrain.overlay && (
          <label className="field">
            <span className="field-label">
              Opacity · {Math.round(terrain.overlay.opacity * 100)}%
            </span>
            <input
              className="slider"
              type="range"
              min={0}
              max={100}
              step={5}
              value={Math.round(terrain.overlay.opacity * 100)}
              onChange={(event) =>
                void actions.setTerrainOverlayOpacity(Number(event.target.value) / 100)
              }
            />
          </label>
        )}
        <div className="inline-actions">
          <button
            type="button"
            className="button button--small"
            onClick={() => void actions.importTerrainOverlay()}
          >
            {terrain.overlay ? "Replace albedo raster…" : "Add albedo raster…"}
          </button>
          {terrain.overlay && (
            <button
              type="button"
              className="button button--small"
              onClick={() => void actions.clearTerrainOverlay()}
            >
              Remove
            </button>
          )}
        </div>
        <CapabilityBadges rows={caps?.terrainMaterial} />
        <p className="hint">
          The albedo raster is draped over the DEM and composited after
          lighting, so categorical palettes survive. This is also what the
          Forge3D renderer consumes; the engine has no other material
          override for terrain.
        </p>
      </Section>

      <p className="hint">
        The slider is the heightfield detail the Solid view builds from this DEM
        (32 = blocky, 512 = close to the source grid). The renderer always reads
        the original GeoTIFF, so Rendered is unaffected by this setting — and
        every render uses the camera you are looking through, so orbit and press
        Re-render to see it from there.
      </p>
    </Section>
  );
}

function ImportedMesh({ object, asset }: { object: SceneObject; asset: Asset | undefined }) {
  const { actions } = useSession();
  const [nodes, setNodes] = useState<string[] | null>(null);
  const [nodeError, setNodeError] = useState<string | null>(null);
  const source = object.kind.type === "mesh" ? object.kind.source : null;
  const current = source?.type === "gltf" ? source.node : null;

  // Node names come from the file itself (parse through the shell, cached by
  // path in assets.ts) — they are derived data, never stored in the project.
  useEffect(() => {
    if (bridge.kind !== "tauri" || !asset?.path) {
      setNodes(null);
      return;
    }
    let live = true;
    setNodeError(null);
    listGltfNodes(asset.path, (path) =>
      bridge.readMeshFile(path).then((payload) => base64ToArrayBuffer(payload.base64)),
    )
      .then((names) => {
        if (live) setNodes(names);
      })
      .catch((cause) => {
        if (live) {
          setNodes([]);
          setNodeError(cause instanceof Error ? cause.message : String(cause));
        }
      });
    return () => {
      live = false;
    };
  }, [asset?.path]);

  return (
    <Section title="Mesh">
      <div className="readout">
        <dt>Source</dt>
        <dd>glTF/GLB asset</dd>
        <dt>Asset</dt>
        <dd>{asset?.name ?? "missing asset"}</dd>
        <dt>File</dt>
        <dd className="mono" title={asset?.path}>
          {asset?.path ?? "—"}
        </dd>
        <dt>Size</dt>
        <dd className="mono">
          {asset ? `${(asset.byteSize / 1024).toFixed(1)} KB` : "—"}
        </dd>
      </div>
      {nodes && nodes.length > 0 && (
        <label className="field">
          <span className="field-label">Node</span>
          <select
            className="input input--select"
            value={current ?? ""}
            title="Draw the whole file, or only the selected node's subtree"
            onChange={(event) =>
              void actions.apply({
                type: "setGltfNode",
                id: object.id,
                node: event.target.value || null,
              })
            }
          >
            <option value="">Whole file</option>
            {current && !nodes.includes(current) && (
              <option value={current}>{current} (missing)</option>
            )}
            {nodes.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
      )}
      {nodeError && <p className="hint">{nodeError}</p>}
      <p className="hint">
        Imported meshes are moved, rotated, and scaled like any other object;
        Solid draws the file's own materials — Node picks which part of the
        file this object shows. A Studio-side material override for glTF
        assets is not implemented yet; asset relinking lives in the Assets
        panel.
      </p>
    </Section>
  );
}

export function Inspector() {
  const { state, actions } = useSession();
  if (!state) return null;

  const { project } = state;
  const selectedObjects = project.scene.objects.filter((object) =>
    project.selection.includes(object.id),
  );

  if (selectedObjects.length === 0) {
    return (
      <div className="panel">
        <header className="panel__header">
          <h2>Project</h2>
        </header>
        <div className="inspector-body">
          <Section title="Project">
            <div className="readout">
              <dt>Name</dt>
              <dd>{project.name}</dd>
              <dt>Format</dt>
              <dd className="mono">
                {project.format} v{project.schemaVersion}
              </dd>
              <dt>Project ID</dt>
              <dd className="mono">{project.id.slice(0, 8)}</dd>
              <dt>Scene ID</dt>
              <dd className="mono">{project.scene.id.slice(0, 8)}</dd>
              <dt>Revision</dt>
              <dd className="mono">{state.revision}</dd>
            </div>
          </Section>
          <Section title="Scene">
            <div className="readout">
              <dt>Objects</dt>
              <dd>{project.scene.objects.length}</dd>
              <dt>Meshes</dt>
              <dd>{project.scene.objects.filter((o) => o.kind.type === "mesh").length}</dd>
              <dt>Terrains</dt>
              <dd>{project.scene.objects.filter((o) => o.kind.type === "terrain").length}</dd>
              <dt>Cameras</dt>
              <dd>{project.scene.objects.filter((o) => o.kind.type === "camera").length}</dd>
              <dt>Lights</dt>
              <dd>{project.scene.objects.filter((o) => o.kind.type === "light").length}</dd>
            </div>
          </Section>
          <RenderSettingsEditor />
          <AssetsPanel />
          <p className="hint">Select an object to edit its properties.</p>
        </div>
      </div>
    );
  }

  if (selectedObjects.length > 1) {
    return (
      <div className="panel">
        <header className="panel__header">
          <h2>Inspector</h2>
          <span className="panel__meta">{selectedObjects.length} selected</span>
        </header>
        <div className="inspector-body">
          <p className="hint">
            Multi-object editing arrives with the solid viewport in Phase 2.
          </p>
          <ul className="multi-list">
            {selectedObjects.map((object) => (
              <li key={object.id}>
                <span>{object.name}</span>
                <span className="mono">{objectKindLabel(object.kind)}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    );
  }

  const object = selectedObjects[0];
  const locked = object.locked;
  const terrainKind = object.kind.type === "terrain" ? object.kind : null;
  const terrainAsset = terrainKind
    ? project.assets.find((asset) => asset.id === terrainKind.asset)
    : undefined;
  const meshSource = object.kind.type === "mesh" ? object.kind.source : null;
  const meshAsset =
    meshSource?.type === "gltf"
      ? project.assets.find((asset) => asset.id === meshSource.asset)
      : undefined;

  return (
    <div className="panel">
      <header className="panel__header">
        <h2>Inspector</h2>
        <span className="panel__meta">{objectKindLabel(object.kind)}</span>
      </header>
      <div className="inspector-body">
        <Section title="Identity">
          <TextField
            label="Name"
            value={object.name}
            onCommit={(name) =>
              void actions.apply({ type: "renameObject", id: object.id, name })
            }
          />
          <div className="readout readout--compact">
            <dt>ID</dt>
            <dd className="mono">{object.id.slice(0, 8)}</dd>
            <dt>Parent</dt>
            <dd className="mono">
              {object.parent
                ? project.scene.objects.find((entry) => entry.id === object.parent)?.name ??
                  "missing"
                : "scene root"}
            </dd>
          </div>
        </Section>

        <TransformEditor
          object={object}
          transform={object.transform}
          disabled={locked || object.kind.type === "terrain"}
        />

        {meshSource?.type === "primitive" && (
          <MeshEditor object={object} primitive={meshSource.primitive} />
        )}
        {/* A Studio material only reaches primitives today (the glTF loader
            draws the file's own materials), so showing the editor for glTF
            assets would be a control that does nothing — the shared material
            library owns it (README "not yet implemented"). */}
        {object.kind.type === "mesh" && meshSource?.type === "primitive" && (
          <MaterialEditor object={object} material={object.kind.material} />
        )}
        {meshSource?.type === "gltf" && (
          <ImportedMesh object={object} asset={meshAsset} />
        )}
        {object.kind.type === "terrain" && (
          <TerrainEditor object={object} terrain={object.kind} asset={terrainAsset} />
        )}
        {object.kind.type === "camera" && (
          <CameraEditor
            object={object}
            camera={object.kind}
            active={project.scene.activeCamera === object.id}
          />
        )}
        {object.kind.type === "light" && <LightEditor object={object} light={object.kind} />}
      </div>
    </div>
  );
}
