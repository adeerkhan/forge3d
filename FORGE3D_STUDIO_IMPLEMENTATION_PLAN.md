# Forge3D Studio — Implementation Plan

> **Goal:** Build a polished, Blender-inspired 3D desktop application that can move naturally between Forge3D and Studio while preserving the same project, scene, assets, camera, animation, and render configuration.
>
> **Status:** Architecture and implementation plan. This document is based on Forge3D's public README, architecture/viewer documentation, rendering and analysis guide, data and scene workflows, and examples catalog. It is not a line-by-line source audit of every Rust/Python file. Before implementation, verify each proposed integration against the checked-out commit and add a source/API compatibility matrix.

---

## 1. Product vision

Forge3D and Studio should feel like two workspaces for one 3D project:

- **Forge3D workspace:** focused on inspecting, navigating, preparing, and rendering Forge3D-native scenes and data.
- **Studio workspace:** focused on interactive editing, scene organization, object transforms, materials, animation, and visual authoring.
- **Rendered workspace:** uses Forge3D's rendering capabilities where the current scene/features are supported.
- **Switching workspaces:** is a reversible session handoff, not a destructive conversion or export/import workflow.

A user should be able to:

1. Open a project in Forge3D.
2. Add or inspect scene content.
3. Switch to Studio.
4. Continue editing the same scene.
5. Switch back to Forge3D.
6. Render or export without losing supported edits.

### Non-goals for the first release

Do not attempt to recreate all of Blender. Initially exclude sculpting, advanced topology tools, a full modifier stack, simulation systems, a universal shader-node graph, rigging, and production-grade character animation. Keep these as future extensions.

---

## 2. Repository and Forge3D capability review

Forge3D describes itself as a Rust-first, cross-platform `wgpu`/WebGPU renderer exposed to Python. Its current public product is terrain-centric, with GIS/cartography and scene rendering capabilities. Its public documentation identifies two main execution paths and a shared data model.

### 2.1 Relevant existing Forge3D subsystems

| Forge3D subsystem | Publicly documented capability | Studio integration role | Caveat / validation |
|---|---|---|---|
| Native runtime (`src/`, Rust + wgpu) | GPU resources, terrain renderer, point-cloud support, screen-space effects, viewer executable | Rendering backend and potential future native bridge | Confirm supported scene primitives, lifecycle, and API stability at pinned commit |
| Interactive viewer | `open_viewer_async()`, `ViewerHandle`, `interactive_viewer` | Forge3D workspace / viewer session | Viewer is a separate process; not automatically an embedded Studio viewport |
| Viewer IPC | NDJSON commands over TCP; convenience methods plus `send_ipc()` | Command bridge for supported viewer operations | Sequential socket calls; discover and document commands and events; avoid treating private commands as stable |
| Native/offscreen rendering | `Scene`, `Session`, `TerrainRenderer`, `TerrainRenderParams`, offline accumulation, denoising | Render service / offline output | Documented stack is terrain-focused; prove general mesh scene support before depending on it |
| Terrain configuration | Lighting, shadows, fog, reflections, bloom, AO, terrain materials, LOD, sampling, denoise settings | Render settings UI and presets | Preserve Forge3D-specific settings without pretending they are universal material properties |
| Geometry helpers | Mesh generation, extrusion, subdivision, displacement, tangents, transforms, validation | Import/preprocessing and selected modeling utilities | Helpers do not equal a complete editable mesh modeler |
| Materials/textures/lighting | Material and texture helpers; IBL and terrain material paths; specialized path-tracing/SDF/lighting utilities | Shared material/lighting representation where compatible | Verify supported material types and shader semantics; define explicit unsupported-feature diagnostics |
| Point clouds | LAZ/LAS/COPC/EPT-related workflows and viewer rendering | Point-cloud objects/layers | Treat source data and LOD/decoding limitations explicitly |
| Buildings / 3D Tiles | Building import helpers, CityJSON, 3D Tiles parsing and traversal | GIS/BIM scene import | Not full Cesium runtime parity; feature and render support varies |
| Raster / vector / GIS | GeoTIFF/COG, CRS helpers, overlays, vectors, labels, styles | GIS-oriented scene layers and asset loading | Keep coordinate systems and data provenance in project metadata |
| Animation | `forge3d.animation`, camera rigs, camera animation, frame/snapshot loops | Camera animation and render automation | Public examples establish camera animation; do not assume a general object keyframe system |
| Scene bundles | `.forge3d` bundle save/load; viewer bundle loading; typed MapScene recipes | Existing interoperability and portability path | Bundle is not assumed to be the canonical Studio editable-project format |
| Export / diagnostics | Snapshots, PNG/EXR/AOV paths, export helpers, diagnostics, support matrices | Render output, validation, troubleshooting | Export capabilities and Pro boundaries must remain visible to users |

### 2.2 Public entry points and documentation to inspect in the checkout

Start source review at:

- `python/forge3d/__init__.py` — package public surface
- `python/forge3d/viewer.py`
- `python/forge3d/viewer_ipc.py`
- `python/forge3d/animation.py`
- `python/forge3d/bundle.py`
- `python/forge3d/geometry.py`
- `python/forge3d/materials.py` and `python/forge3d/textures.py`
- `python/forge3d/terrain_params.py`, `python/forge3d/presets.py`
- `python/forge3d/map_scene.py`, `python/forge3d/buildings.py`, `python/forge3d/tiles3d.py`
- `src/` — Rust runtime and renderer
- `examples/` and `docs/examples/index.md` — runnable workflow evidence
- `docs/api/api_reference.rst` — public API reference
- support matrices, diagnostics guides, and relevant tests

### 2.3 Example workflows to use as integration tests

Use the repo's examples as executable capability evidence:

- `terrain_viewer_interactive.py` — viewer baseline
- `pointcloud_viewer_interactive.py` — point cloud
- `buildings_viewer_interactive.py` — building import
- `picking_demo.py` — selection, lasso, highlight, pick events
- `camera_animation_demo.py` — camera keyframes and frame export
- `terrain_camera_rigs_demo.py` — orbit/rail/follow camera rigs
- `terrain_demo.py` — bundle-aware scene workflow
- `terrain_atmosphere_path_demo.py` — native/offline rendering path
- overlay, label, style, COG, and 3D Tiles examples — GIS scene content

**Repository-audit deliverable:** a checked-in `docs/forge3d-capability-matrix.md` with columns for feature, public API, source location, test/example evidence, Studio integration, limitations, license/Pro gate, and status (`supported`, `adapter required`, `experimental`, `unsupported`, `needs verification`).

---

## 3. Core architecture

### 3.1 Single source of truth

Create a canonical **Studio Project Model**. The UI, Forge3D viewer, and render pipeline must all consume the same project identity and scene state.

```text
Studio Desktop (Tauri)
├── React + TypeScript UI
│   ├── Forge3D workspace
│   ├── Studio workspace
│   ├── shared scene hierarchy / inspector / timeline
│   └── viewport host
├── Application Core (Rust)
│   ├── Project service
│   ├── Canonical scene model
│   ├── Command / undo-redo service
│   ├── Asset registry
│   ├── Session and workspace handoff
│   ├── Renderer capability registry
│   └── Render job manager
├── Viewport adapters
│   ├── Studio interactive viewport (initially Three.js)
│   └── Forge3D viewer adapter (Python IPC initially)
└── Forge3D
    ├── Interactive viewer process
    └── Native/offscreen rendering process or bridge
```

### 3.2 Important renderer decision

Do not assume Three.js and Forge3D can render every object identically. Forge3D's current public runtime is a terrain/GIS-oriented renderer. The Studio viewport may support general meshes and editing that Forge3D does not render through its existing public scene path.

Implement a **capability-aware renderer interface**:

```ts
interface RendererAdapter {
  id: "studio-viewport" | "forge3d";
  capabilities(): RendererCapabilities;
  attach(project: ProjectSnapshot): Promise<void>;
  applyPatch(patch: ScenePatch): Promise<ApplyResult>;
  setCamera(camera: CameraState): Promise<void>;
  render(request: RenderRequest): Promise<RenderResult>;
  detach(): Promise<void>;
}
```

Every scene feature must declare whether it is:
- editable and renderable in both workspaces;
- editable in Studio but only previewable/unsupported in Forge3D;
- Forge3D-native (e.g. certain terrain/GIS layers);
- render-only or external;
- unsupported with a clear diagnostic.

Never silently drop unsupported content during workspace switching.

---

## 4. Canonical project and scene model

Create a versioned project format, e.g. `.studio3d` (name provisional), with:

```text
project/
├── project.json
├── scene/
│   ├── scene.json
│   ├── objects/
│   └── animations/
├── assets/
│   ├── meshes/
│   ├── textures/
│   ├── environments/
│   └── source-data/
├── forge3d/
│   ├── bundle-reference.json
│   └── renderer-settings.json
├── previews/
└── renders/
```

The project model should include:

- Stable UUIDs for project, scene, object, asset, material, camera, and animation.
- Object transforms and hierarchy.
- Mesh references and geometry data.
- Material references and texture slots.
- Lights and cameras.
- GIS/terrain layer references, CRS, source metadata, and Forge3D-specific settings.
- Animation tracks, keyframes, interpolation, and frame rate.
- Active workspace, selection, camera/view state, and render configuration.
- Asset hashes, relative paths, and missing-asset diagnostics.
- Schema version and migration metadata.

Use an explicit **feature/renderer capability declaration** per object or scene layer. Do not make the project file dependent on transient GPU resources, viewer process IDs, or renderer-specific object handles.

### Project persistence strategy

- Canonical Studio project is the source of truth for cross-workspace edits.
- Forge3D `.forge3d` bundles remain supported as import/export/interchange and native scene payloads.
- Preserve bundle references and original source assets where feasible.
- Round-trip tests must prove no loss for the declared supported feature subset.
- Save atomically using temporary files plus rename; maintain autosave/recovery.

---

## 5. Natural Forge3D ↔ Studio handoff

Implement a session handoff service, not a file conversion button.

### Switch from Forge3D to Studio

1. Ask Forge3D viewer adapter for current supported scene state and camera/view state.
2. Flush pending commands and verify the viewer response.
3. Reconcile changes into the canonical project model.
4. Preserve Forge3D-native terrain/GIS layers and their original source references.
5. Start or activate Studio viewport with the same project ID and scene revision.
6. Restore camera, selection where available, workspace layout, and render settings.
7. Show unsupported or uneditable content explicitly.

### Switch from Studio to Forge3D

1. Flush Studio commands and commit the latest canonical scene revision.
2. Build a Forge3D-compatible scene payload from the shared model.
3. Preserve native Forge3D assets/settings instead of reconstructing them unnecessarily.
4. Validate compatibility and report any unsupported objects/materials/features.
5. Update or launch the Forge3D viewer with the payload.
6. Restore camera and scene state as closely as the adapter permits.
7. Keep the Studio session alive or recoverable; do not close it before successful handoff.

### Handoff contract

```ts
interface WorkspaceHandoff {
  projectId: string;
  sceneId: string;
  sourceWorkspace: "forge3d" | "studio";
  targetWorkspace: "forge3d" | "studio";
  sourceRevision: number;
  cameraState: CameraState;
  selection: string[];
  capabilityReport: CapabilityReport;
  diagnostics: Diagnostic[];
}
```

### Reliability rules

- Handoff must be idempotent.
- Handoff must never delete source data.
- Failed handoff leaves the current workspace usable.
- No implicit lossy conversion.
- Use revision IDs and conflict checks.
- Show a compatibility summary before switching if features will be degraded.
- Record all adapter transformations in diagnostics.

---

## 6. UI and UX

### Main layout

- Top bar: project, workspace switcher, undo/redo, save, render.
- Left: scene hierarchy and asset browser.
- Center: viewport.
- Right: inspector/properties.
- Bottom: timeline (collapsible).
- Optional panels: console, render queue, material editor, diagnostics.

### Viewport modes

- **Solid:** fast interactive editing, selection, transforms, grid, axes, object outlines.
- **Material Preview:** optional intermediate mode; only implement if material parity is meaningful.
- **Rendered:** Forge3D-backed rendering for supported scene features.
- **Forge3D workspace:** native viewer and GIS/terrain controls.

Do not promise a Blender-like rendered viewport until a scene synchronization loop and renderer compatibility layer have been validated.

### Visual design

Use a dark, restrained, high-contrast design. Prioritize viewport area, low-noise panels, clear selection state, keyboard shortcuts, and responsive dock panels. Avoid building a custom UI framework before the project/viewport loop works.

---

## 7. Phased implementation

### Phase 0 — Repository audit and feasibility spike

**Deliverables**
- `docs/forge3d-capability-matrix.md`
- Architecture decision records (ADRs)
- Pinned Forge3D revision and dependency/license review
- Working Forge3D viewer launch and controlled shutdown
- Test proving current scene/camera state can be queried and changed
- Test proving one supported scene payload can be loaded into both workspace paths

**Exit criteria**
- We know which features can round-trip, which require an adapter, and which cannot be supported without Forge3D changes.
- No claims about arbitrary mesh/path-tracing support remain unverified.

### Phase 1 — Desktop shell and project model

**Deliverables**
- Tauri app shell
- React/TypeScript UI
- Rust project service
- Versioned project schema
- Open/save/autosave/recovery
- Scene tree with a single cube, camera, and light
- Shared command model and undo/redo

**Exit criteria**
- A project opens, saves, closes, and reopens with stable IDs and transforms.

### Phase 2 — Studio solid viewport

**Deliverables**
- Three.js viewport
- Orbit/pan/zoom
- Object selection
- Move/rotate/scale gizmos
- Scene hierarchy selection synchronization
- Inspector for transforms
- Add/delete/duplicate objects
- Basic mesh import (start with glTF/GLB)

**Exit criteria**
- Editing the scene changes the canonical model, not a detached viewport-only copy.
- Reloading the project preserves all edits.

### Phase 3 — Forge3D workspace adapter

**Deliverables**
- Forge3D process lifecycle management
- Python bridge and IPC adapter
- Camera synchronization
- Bundle import/load
- Viewer command queue and diagnostics
- Capability report UI

**Exit criteria**
- User can switch Forge3D → Studio → Forge3D without losing the supported scene subset.
- Viewer crashes or unsupported commands do not corrupt the project.

### Phase 4 — Rendered mode

**Deliverables**
- Render job manager
- Forge3D render adapter for proven compatible scene types
- Render settings UI for exposed Forge3D controls
- Still-image output and render status/progress
- Clear unsupported feature warnings
- Comparison tests for scene/camera/settings consistency

**Exit criteria**
- Rendered mode produces correct output for a documented set of supported scenes.
- Unsupported geometry/materials are reported, not silently omitted.

### Phase 5 — Materials, lights, and assets

**Deliverables**
- Shared material schema with capability-tagged parameters
- Material inspector
- Texture asset management
- Environment/IBL controls where supported
- Basic lights and camera properties
- Asset relinking and dependency collection

**Exit criteria**
- Materials and assets persist and have predictable behavior in each renderer.
- Feature parity limitations are visible.

### Phase 6 — Animation

**Deliverables**
- Timeline, playback, frame rate, current frame
- Keyframes for transforms and camera
- Interpolation modes
- Animation evaluation in canonical scene model
- Forge3D camera animation/render loop adapter
- Frame sequence export

**Exit criteria**
- Studio playback and offline frame rendering use the same evaluated animation state.
- Camera animation can be handed to Forge3D where its API supports it.
- General object animation is not advertised as Forge3D-native unless implemented and tested.

### Phase 7 — Production hardening

**Deliverables**
- Crash recovery
- Asset integrity and project migration
- Large-scene performance tests
- Render queue and cancellation
- Automated workspace-switch regression suite
- Packaging for target OS
- Documentation and sample projects

**Exit criteria**
- No data loss on failed switch, renderer crash, or interrupted save.
- Repeatable project round trips for supported features.

---

## 8. Shared command model

All user actions should go through a command layer, e.g.:

- `CreateObject`
- `DeleteObject`
- `SetTransform`
- `SetMaterial`
- `SetCamera`
- `AddAsset`
- `SetAnimationKeyframe`
- `SetRenderSettings`

Each command:
- applies to the canonical model;
- emits a scene revision;
- updates the active viewport adapter;
- supports undo/redo where appropriate;
- records diagnostics if an adapter cannot apply it.

The renderer is a projection of scene state, not the owner of project truth.

---

## 9. Test strategy

### Core tests
- Project schema validation and migration.
- Stable UUIDs and asset reference integrity.
- Command apply/revert and undo/redo.
- Atomic save and recovery.

### Adapter tests
- Forge3D viewer starts, responds, and exits cleanly.
- IPC timeout/disconnect behavior.
- Scene and camera sync.
- Capability reporting.
- Native render path smoke tests.

### Round-trip tests
For every supported feature:
1. Create/edit in Studio.
2. Switch to Forge3D.
3. Change a supported property.
4. Switch back to Studio.
5. Compare canonical scene values and asset references.

### Visual tests
- Fixed scene fixtures and camera positions.
- Compare expected render outputs within documented tolerances.
- Keep renderer-specific visual differences explicit; do not use exact pixel equality where nondeterminism is expected.

### Failure tests
- Forge3D process exits during handoff.
- Missing asset.
- Unsupported object/material.
- GPU adapter unavailable.
- Project save interrupted.
- Version mismatch.
- Handoff validation fails.

---

## 10. Risks and decisions

| Risk | Mitigation |
|---|---|
| Forge3D viewer is a separate process, not an embeddable widget | Treat it as an adapter/process; use Studio viewport independently |
| Forge3D is terrain/GIS-focused, not a full Blender renderer | Establish feature matrix; do not assume arbitrary mesh/path-tracing parity |
| Different renderer material semantics | Capability-tag materials and show explicit degradation |
| Scene bundle does not capture all Studio editability | Keep canonical Studio project model; bundle is interchange/native payload |
| Python IPC can add latency and lifecycle complexity | Start with bridge; measure; consider native Rust integration later |
| User edits diverge across two workspaces | Single canonical scene model, revisions, command log, conflict checks |
| Pro-gated features | Surface license/feature gates; do not bypass them |
| Large GIS/point-cloud scenes | Preserve Forge3D-native data references and LOD workflows rather than copying all data into Three.js |

---

## 11. Immediate next tasks

1. Clone/pin Forge3D and inspect the actual current source tree.
2. Run the key examples listed in §2.3.
3. Create the capability matrix with evidence and exact source paths.
4. Inspect viewer IPC command definitions and bundle schema.
5. Prototype the canonical project model with one cube, one camera, and one Forge3D terrain scene.
6. Prove reversible workspace handoff before implementing a large UI.
7. Only then build out the full Studio interface.

## 12. Definition of success

The first meaningful success is not a beautiful interface. It is this:

> Open a project in Forge3D, switch to Studio, edit the scene, switch back to Forge3D, and continue without losing supported data or silently degrading the scene.

Once that round trip is robust, the rest of the application can be built incrementally on top of a sound architecture.
