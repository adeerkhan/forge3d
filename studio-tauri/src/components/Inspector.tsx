import type {
  Asset,
  CameraObject,
  LightKind,
  LightObject,
  Primitive,
  SceneObject,
  Transform,
} from "../bridge/types";
import { objectKindLabel } from "../bridge/types";
import { useSession } from "../state/session-context";
import { NumberField, Section, TextField, Toggle } from "./fields";

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
    </Section>
  );
}

function LightEditor({ object, light }: { object: SceneObject; light: LightObject }) {
  const { actions } = useSession();
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
          onChange={(event) => commit({ ...light, kind: event.target.value as LightKind })}
        >
          <option value="directional">Directional</option>
          <option value="point">Point</option>
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
  const { actions, terrainResolution, setTerrainResolution } = useSession();
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

      <p className="hint">
        The slider is the heightfield detail the Solid view builds from this DEM
        (32 = blocky, 512 = close to the source grid). The renderer always reads
        the original GeoTIFF, so Rendered is unaffected by this setting — press
        End to put the renderer's camera where your viewport is looking.
      </p>
    </Section>
  );
}

function ImportedMesh({ asset }: { object: SceneObject; asset: Asset | undefined }) {
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
      <p className="hint">
        Imported meshes are moved, rotated, and scaled like any other object.
        Material overrides and asset relinking arrive in Phase 5.
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
          <Section title="Render output">
            <div className="readout readout--compact">
              <dt>Resolution</dt>
              <dd className="mono">
                {project.render.width} × {project.render.height}
              </dd>
            </div>
            <p className="hint">
              Stored with the project. Phase 4 exposes the Forge3D render
              controls that consume it.
            </p>
          </Section>
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

        <TransformEditor object={object} transform={object.transform} disabled={locked} />

        {meshSource?.type === "primitive" && (
          <MeshEditor object={object} primitive={meshSource.primitive} />
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
