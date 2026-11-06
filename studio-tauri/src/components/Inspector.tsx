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
  RenderSettings,
  SceneObject,
  SolarResult,
  Transform,
} from "../bridge/types";
import { objectKindLabel } from "../bridge/types";
import { bridge } from "../bridge/env";
import { base64ToArrayBuffer, listGltfNodes } from "../viewport/assets";
import { useSession } from "../state/session-context";
import { ChoiceField, NumberField, RangeField, Section, TextField, Toggle } from "./fields";

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

function localDateTimeInput(): string {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}T${pad(
    now.getHours(),
  )}:${pad(now.getMinutes())}`;
}

/**
 * Bakes the engine's solar ephemeris into the light's rotation. The datetime
 * input is local; the browser converts it to the UTC string the engine wants.
 * Nothing is stored: re-applying a different time is the way to change it.
 */
function SolarPositionEditor({ object }: { object: SceneObject }) {
  const { actions, bridgeKind } = useSession();
  const [latitude, setLatitude] = useState(0);
  const [longitude, setLongitude] = useState(0);
  const [when, setWhen] = useState(localDateTimeInput);
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<SolarResult | null>(null);

  const apply = async () => {
    const local = new Date(when);
    if (Number.isNaN(local.getTime())) {
      actions.notify("Enter a valid date and time");
      return;
    }
    setPending(true);
    try {
      const utc = local.toISOString();
      const applied = await actions.setSunFromSolar(
        object.id,
        latitude,
        longitude,
        utc,
      );
      if (applied) setResult(applied);
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="solar-position">
      <span className="group-label">Sun from date &amp; place</span>
      <div className="pair">
        <NumberField
          label="Latitude"
          value={latitude}
          step={0.1}
          min={-90}
          max={90}
          onCommit={setLatitude}
        />
        <NumberField
          label="Longitude"
          value={longitude}
          step={0.1}
          min={-180}
          max={180}
          onCommit={setLongitude}
        />
      </div>
      <label className="field">
        <span className="field-label">Local date &amp; time</span>
        <input
          className="input"
          type="datetime-local"
          value={when}
          disabled={bridgeKind !== "tauri"}
          onChange={(event) => setWhen(event.target.value)}
        />
      </label>
      <button
        type="button"
        className="button button--small"
        disabled={bridgeKind !== "tauri" || pending || !when}
        title={
          bridgeKind === "tauri"
            ? "Rotate the sun to this place and time — one undo step"
            : "Solar placement needs the desktop shell"
        }
        onClick={() => void apply()}
      >
        {pending ? "Locating…" : "Set sun"}
      </button>
      {result && (
        <p className="hint">
          Azimuth {result.azimuthDeg.toFixed(1)}° · elevation{" "}
          {result.elevationDeg.toFixed(1)}°
          {!result.daytime && " · below the horizon"}
        </p>
      )}
      {bridgeKind !== "tauri" && (
        <p className="hint">Solar placement needs the desktop shell.</p>
      )}
    </div>
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

  const rotation = object.transform.rotationDeg;
  const setRotation = (axis: "x" | "y" | "z", value: number) =>
    void actions.apply({
      type: "setTransform",
      id: object.id,
      transform: {
        ...object.transform,
        rotationDeg: { ...rotation, [axis]: value },
      },
    });

  return (
    <Section title="Sun" id="light-sun">
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
      <div className="triple">
        <NumberField label="Pitch" value={rotation.x} step={1} min={-90} max={90} disabled={light.kind !== "directional"} onCommit={(value) => setRotation("x", value)} />
        <NumberField label="Yaw" value={rotation.y} step={1} min={-180} max={180} disabled={light.kind !== "directional"} onCommit={(value) => setRotation("y", value)} />
        <NumberField label="Roll" value={rotation.z} step={1} min={-180} max={180} disabled={light.kind !== "directional"} onCommit={(value) => setRotation("z", value)} />
      </div>
      <span className="group-label">Direction (degrees)</span>
      {light.kind === "directional" && <SolarPositionEditor object={object} />}
      <CapabilityBadges rows={caps?.light} />
    </Section>
  );
}

export function AssetsPanel() {
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

/** The project's render settings plus a whole-object commit (every settings
 *  edit is one command, like `setChannelKeys`). */
function useRenderSettings() {
  const { state, actions } = useSession();
  const render = state?.project.render;
  if (!render || !state) return null;
  const commit = (patch: Partial<RenderSettings>) => {
    void actions.apply({
      type: "setRenderSettings",
      render: { ...render, ...patch },
    });
  };
  return { project: state.project, render, commit };
}

/** Render tab: what the still and the Rendered frame are made of. */
export function RenderSettingsEditor() {
  const { engine } = useSession();
  const settings = useRenderSettings();
  if (!settings) return null;
  const { render, commit } = settings;

  return (
    <>
      <Section title="Output" id="render-output">
        <div className="pair">
          <NumberField label="Width" value={render.width} step={16} min={64} max={16384} onCommit={(width) => commit({ width: Math.round(width) })} />
          <NumberField label="Height" value={render.height} step={16} min={64} max={16384} onCommit={(height) => commit({ height: Math.round(height) })} />
        </div>
        <NumberField label="Exposure" value={render.exposure} step={0.05} min={0.05} max={8} onCommit={(exposure) => commit({ exposure })} />
        <p className="hint">
          The renderer tone-maps with ACES at this exposure; the previews use
          the same value, so a still matches what you were looking at.
        </p>
      </Section>

      <Section title="Shadows" id="render-shadows">
        <Toggle label="Sun shadows" checked={render.shadows} onChange={(shadows) => commit({ shadows })} />
        <ChoiceField
          label="Filter"
          value={render.shadowTechnique}
          options={[
            { value: "pcf", label: "PCF", title: "Fixed-width filter: crisp and cheap" },
            { value: "pcss", label: "PCSS", title: "Blocker search: softer contact shadows, roughly 2× the cost" },
          ]}
          onChange={(shadowTechnique) => commit({ shadowTechnique })}
        />
        <ChoiceField
          label="Map size"
          value={String(render.shadowResolution)}
          options={[512, 1024, 2048, 4096].map((size) => ({
            value: String(size),
            label: String(size),
            title: `${size}² shadow map: ${size < 2048 ? "faster, chunkier edges" : "crisper, more memory and fill"}`,
          }))}
          onChange={(value) => commit({ shadowResolution: Number(value) })}
          hint="Applies to previews too: the Rendered viewport re-renders on orbit, so a big map costs frame time."
        />
      </Section>

      <Section title="Quality" id="render-quality">
        <Toggle
          label="Ambient occlusion (final, ~5 s)"
          checked={render.ambientOcclusion}
          onChange={(ambientOcclusion) => commit({ ambientOcclusion })}
        />
        <p className="hint">
          SSAO is final-only: it costs seconds per frame, and nobody watches a
          preview's occlusion pass.
        </p>
      </Section>

      <Section title="Stills" id="render-stills">
        <Toggle
          label="AOVs next to the still (albedo · normal · depth)"
          checked={render.stillAov.albedo || render.stillAov.normal || render.stillAov.depth}
          onChange={(value) =>
            commit({ stillAov: { albedo: value, normal: value, depth: value } })
          }
        />
        <Toggle
          label="Denoise the still with OIDN"
          checked={render.stillDenoise}
          onChange={(stillDenoise) => commit({ stillDenoise })}
        />
        {render.stillDenoise && engine?.available && engine.denoise === false && (
          <p className="hint hint--boxed">
            The engine's interpreter has no OIDN runtime, so a denoised still
            fails instead of rendering. Install it with{" "}
            <code>pip install -e .[denoise]</code> for{" "}
            <code>{engine.python}</code>.
          </p>
        )}
        <p className="hint">
          Both apply to sequence frames as well (they are final renders) and to
          viewport previews never.
        </p>
      </Section>
    </>
  );
}

/** Light tab: the scene's environment — the map that lights everything. */
export function EnvironmentSettings() {
  const { actions, bridgeKind } = useSession();
  const caps = useCapabilities();
  const settings = useRenderSettings();
  if (!settings) return null;
  const { project, render, commit } = settings;
  const environment = render.environment
    ? project.assets.find((asset) => asset.id === render.environment)
    : undefined;

  return (
    <Section title="Environment" id="light-environment">
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
      <div className="readout readout--compact">
        <dt>Map</dt>
        <dd>{environment?.name ?? "built-in studio sky"}</dd>
      </div>
      <div className="inline-actions">
        <button
          type="button"
          className="button button--small"
          disabled={bridgeKind !== "tauri"}
          title="Pick a Radiance .hdr for image-based lighting; both Material and Rendered use it"
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
      {!environment ? (
        <p className="hint">
          The built-in sky gives the terrain its blue fill and the viewport its
          neutral backdrop. Add an HDRI to light and backdrop your own
          environment, and to control the backdrop brightness here.
        </p>
      ) : (
        <p className="hint">
          Rendered draws this map as its sky; Material uses it behind the scene
          and as the light. Background intensity moves only the backdrop.
        </p>
      )}
      <CapabilityBadges rows={caps?.environment} />
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

/**
 * Object tab: the selected object's own properties. Scene-wide and
 * light-rig settings live in their own tabs, so this stays about the thing
 * that is selected.
 */
export function ObjectProperties() {
  const { state, actions } = useSession();
  if (!state) return null;

  const { project } = state;
  const selectedObjects = project.scene.objects.filter((object) =>
    project.selection.includes(object.id),
  );

  if (selectedObjects.length === 0) {
    return (
      <Section title="Object" id="object-none">
        <p className="hint">
          Nothing is selected. Pick an object in the scene tree (or the
          viewport) to edit its transform, material and kind-specific settings.
        </p>
        <p className="hint">
          Render output and shadows are in the <strong>Render</strong> tab, the
          sun and environment in <strong>Lighting</strong>, and the project
          itself in <strong>Scene</strong>.
        </p>
      </Section>
    );
  }

  if (selectedObjects.length > 1) {
    return (
      <Section title="Object" id="object-multi">
        <p className="hint">
          {selectedObjects.length} objects selected. Transform editing is
          one object at a time for now.
        </p>
        <ul className="multi-list">
          {selectedObjects.map((object) => (
            <li key={object.id}>
              <span>{object.name}</span>
              <span className="mono">{objectKindLabel(object.kind)}</span>
            </li>
          ))}
        </ul>
      </Section>
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
    <>
      <Section title="Identity" id="object-identity">
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
      {object.kind.type === "light" && (
        <Section title="Light" id="object-light">
          <p className="hint">
            Intensity, colour, direction and solar placement are the scene's
            lighting rig — they live in the <strong>Lighting</strong> tab so the
            same sun is edited in one place.
          </p>
        </Section>
      )}
    </>
  );
}

/** Scene tab: the project and scene facts, and the asset list. */
export function ProjectFacts() {
  const { state } = useSession();
  const project = state?.project;
  if (!state || !project) return null;

  return (
    <>
      <Section title="Project" id="scene-project">
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
          <dt>Saved</dt>
          <dd>{state.dirty ? "unsaved changes" : "saved"}</dd>
        </div>
      </Section>
      <Section title="Scene" id="scene-facts">
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
      <AssetsPanel />
    </>
  );
}

/** Light tab: the scene's sun, edited wherever the selection happens to be. */
export function LightRig() {
  const { state } = useSession();
  const sun = state?.project.scene.objects.find(
    (object) =>
      object.kind.type === "light" && object.kind.kind === "directional" && object.visible,
  );

  if (!sun || sun.kind.type !== "light") {
    return (
      <Section title="Sun" id="light-sun">
        <p className="hint">
          The scene has no visible directional light. Both the preview and the
          renderer need one: add it from the scene tree, or undo the change that
          hid it.
        </p>
      </Section>
    );
  }

  return (
    <>
      <LightEditor object={sun} light={sun.kind} />
      <p className="hint">
        This is the same sun Solid, Material and Rendered light with. Solar
        placement writes one rotation, so a single undo puts the old sun back.
      </p>
    </>
  );
}
