# Bringing Forge3D into Studio — integration roadmap (Phases 8–16)

Status: **proposal**, written against the code in this checkout (engine 1.47.1,
Studio Phase 7 complete). It is a map, not a promise: each phase lists the engine
API it consumes, what changes in Studio, and what "done" means. Readiness labels
are the engine's own (`supported`, `native-required`, `underdeveloped`) where it
publishes them, so a phase cannot quietly claim more than the engine can do.

---

## 0. Where Studio is today

Studio already has the hard parts of a desktop product:

- `studio-tauri/crates/studio-core` — the canonical project (scene, commands,
  undo, timeline, assets, validation), schema v4, no GPU/Python dependency.
- `studio-tauri/src-tauri` — Tauri shell: IPC commands, autosave/recovery,
  atomic save, render service (`studio_render.py`), render queue, asset
  integrity, certificates not yet.
- `studio-tauri/src` — React UI: viewport (Solid/Material/Rendered), inspector,
  timeline, layers panel (scene tree), render queue, capability badges.

The engine boundary is currently **one purpose-built service**:
`studio_render.py` keeps a `forge3d.Session`, a `TerrainRenderer`, a
`MaterialSet`, an `IBL`, and renders `render_terrain_pbr_pom` frames. That path
is proven (stills, Rendered viewport, animation export, HDRI sky). Everything
else in the engine — scene recipes, vector/label layers, water/clouds,
buildings, point clouds, mesh rendering, bundles, certificates — is **not**
reachable from Studio yet.

The goal of this roadmap is to grow that one service into a **typed engine
bridge**, and then expose capabilities in dependency order, without ever
letting the engine inspect the canonical project or letting the UI invent
engine state.

---

## 1. Integration architecture

### 1.1 One bridge, typed commands

Keep the current transport (Rust owns the Python process, newline-delimited
JSON), but replace the single request shape with a **command dispatcher**:

```
{"cmd": "render_still",     "request": {...}}
{"cmd": "validate_scene",   "request": {...}}
{"cmd": "bake_atmosphere",  "request": {...}}
{"cmd": "import_vector",    "request": {...}}
...
```

Each command is a small Python function in a new `studio_engine.py` (or
`studio_render.py` split by domain) that maps a JSON request onto engine
objects. Rules:

- **JSON in, JSON out.** Images are base64 PNG (as today); assets are paths,
  never bytes.
- **Progress events**, not polling: the service already writes events to stdout;
  reuse that for bake/offline progress.
- **Typed errors**: `{ok: false, code, message, diagnostics}`; the shell turns
  them into the existing toast/queue error channel.
- **One process**, one job at a time for offline work; the Phase 7 queue already
  serialises, and the service should refuse concurrent jobs explicitly.

### 1.2 Capability gate

Every later UI control must be enabled by a **capability snapshot**, not by
`try/except` at render time:

- Engine: `forge3d.capabilities()` (granted/requested features, limits), plus
  `device_probe()`, `enumerate_adapters()`, `native_import_error()`,
  `native_degradations()`, `has_gpu()`, `oidn_available()`,
  `copc_laz_enabled()`, `is_weighted_oit_available()`, `proj_available()`,
  `splat_fusion_available()`, and the per-feature validators
  (`validate_terrain_vt_support`, `validate_label_support`,
  `validate_building_layer_support`, `validate_tiles3d_support`).
- Shell: one `EngineCapabilities` snapshot per session, refreshed on demand;
  a command `engine_capabilities`.
- Studio: a `caps` store (like the existing `CapabilityTable`) that decides
  `disabled` + reason for controls, and a **Diagnostics** panel that shows
  degradations verbatim. Unsupported features are visible and disabled, never
  hidden or silently approximated.

### 1.3 Canonical model stays authoritative

The engine gets a **derived recipe**. The project keeps Studio's settings; the
recipe is recomputed from them at render time. Two consequences:

- Undo/redo, autosave, and the schema never depend on engine structs.
- A new engine feature appears as a Studio setting + mapping function, not as
  imported engine state.

The only exception is **interchange** (bundle import/export, Phase 15), which is
explicitly a conversion, not a second source of truth.

### 1.4 Camera and time parity

The existing rule holds: the viewport, a camera-view still, and an exported
animation frame must agree. `workspace::camera_render_view` already pins this
for terrain. MapScene adds `OrbitCamera` and `CameraAnimation`; the recipe
mapping must keep the same golden-vector tests for any new camera path.

---

## 2. Engine inventory — what exists and how Studio would use it

Grouped by the job it does. "Readiness" is the engine's practical state today,
not the API's existence.

### 2.1 Rendering core

| API | What it does | Studio use | Readiness |
|---|---|---|---|
| `forge3d.Session`, `TerrainRenderer` | GPU session + terrain pipeline; `render_terrain_pbr_pom`, `render_with_aov` | already used by `studio_render.py` | **in use** |
| `TerrainRenderParams` / `make_terrain_params_config` | the whole frame: camera, sun, clip, quality, overlays, all feature settings | Phase 4–7 payload; grows in Phase 10 | supported |
| `MaterialSet`, `IBL`, `OverlayLayer`, `Colormap1D` | terrain material layers, environment, draped raster, palettes | HDRI + terrain albedo already; Phase 10 adds material layers | supported |
| `Frame`, `HdrFrame`, `AovFrame` | RGBA / linear HDR output, AOVs (albedo, normal, depth, transmittance, in-scatter, cloud shadow, optical depth) | stills, HDR export, look-dev passes (Phase 14) | supported |
| `render_offline` + `OfflineQualitySettings` / `OfflineProgress` | progressive accumulation to a variance target with progress | "final" stills/sequence frames; progress in the queue | supported |
| `CompiledFrame` / `compile_frame` | precompiled frame reuse | fast repeated frames (camera moves) | native |
| `TerrainSpike` | minimal validation renderer | diagnostics only | supported |

### 2.2 Scene recipes and layers (`forge3d.map_scene`)

| API | What it does | Studio use | Readiness |
|---|---|---|---|
| `MapScene`, `SceneRecipe`, `compile_passes`, `execute_passes` | declarative scene: terrain + camera + lighting + layers + output; validates before rendering | Phase 9: the render recipe that replaces the ad-hoc payload | supported |
| `TerrainSource`, `OrbitCamera`, `LightingPreset`, `OutputSpec` | inputs to the recipe | map from project (DEM, camera, sun/HDRI, render settings) | supported |
| `VectorOverlay` (+ `forge3d.vector`, `export.VectorScene`) | polylines/polygons/points, styles, CRS, width, dash | Phase 11 layers | supported |
| `LabelLayer`, `label_plan`, `text`, `text_atlas` | labels with atlas, typography, collision/occlusion plans | Phase 11 cartography | supported |
| `MapFurnitureLayer` | title, legend, scale bar, north arrow, graticule, keepouts | Phase 11 map furniture | supported |
| `BuildingLayer` (`from_geojson`, 3D Tiles) | building footprints/volumes | Phase 13 data layer; MapScene marks it underdeveloped | **underdeveloped** |
| `PointCloudLayer` | declares a COPC/EPT/LAZ source in the scene | Phase 13; engine's public offline path is not there yet | **native-required** |
| `Tiles3DLayer` | declares a 3D Tiles tileset | Phase 13; public API incomplete (`python_public_3dtiles_incomplete_diagnostic`) | **underdeveloped** |
| `ValidationReport` / `Diagnostic` | `supported_features`, `unsupported_features`, warnings, remediation | Phase 9 "Render check" panel | supported |

### 2.3 Look development (`forge3d.terrain_params`)

All of these are dataclasses accepted by `TerrainRenderParams`; Studio exposes
them progressively rather than as one wall of sliders.

| Group | Settings | Studio use |
|---|---|---|
| Water | `WaterSettings` (+ `WaterOpticsSettings`, mask path) | Phase 10 |
| Clouds | `CloudSettings` (coverage, density, shadows, quality) | Phase 10 |
| Volumetrics | `VolumetricsSettings` (mode, density, light shafts, half-res) | Phase 10 (costly) |
| Sky | `SkySettings` (hosek-wilkie, preetham, approximate, aether, hdri) | existing HDRI + Phase 10 analytic/AETHER |
| Atmosphere | `AtmosphereSettings`, `AtmosphereLutHandle`, `atmosphere_bake_luts` | Phase 10 advanced sky (bake step) |
| Post | `BloomSettings`, `DofSettings`, `LensEffectsSettings`, `TonemapSettings`, `DenoiseSettings` | Phase 10/14 |
| Screen space | `ScreenSpaceSettings` (SSAO/SSGI/SSR/TAA) | Phase 14 look-dev |
| Shadows | `ShadowSettings` (PCF/PCSS/EVSM, cascades) | Phase 10 |
| Terrain material | `MaterialLayerSettings` (snow/rock/wetness/noise), `PomSettings`, `DetailSettings`, `TerrainVTSettings` | Phase 10; VT is a Phase 13-scale subsystem of its own |

### 2.4 Sky, atmosphere and astro

| API | What it does | Studio use | Readiness |
|---|---|---|---|
| `forge3d.sky.set_observation` | persist an observing site/time across viewer restarts | time-of-day lighting control (Phase 12) | supported |
| `sun_position`, `sun_position_utc` | solar position from lat/lon/time | map sun light to a date/time; Phase 12 | supported |
| `forge3d.atmosphere` | Rayleigh/Mie/AETHER, LUT bake, spectral conversion | Phase 10 advanced sky | supported (bake cost) |
| `forge3d.astro` | moon/planet positions, phase, ΔT, refraction | optional astronomy layer; Phase 12 | supported |
| `GlobeScene` (native) | ORBIS planetary clipmap assembly | Phase 16 lab | native |

### 2.5 Geometry, meshes and path tracing

| API | What it does | Studio use | Readiness |
|---|---|---|---|
| `forge3d.geometry` | primitives, glTF/OBJ import, ribbon/tube/extrusion, LOD chain, tangents, instancing (`instance_mesh_gpu_render`) | Phase 14: meshes in Rendered | supported, plumbing needed |
| `forge3d.mesh` | cube/quad/triangle buffers, TBN, BVH build/refit, GPU upload | same; BVH for PT and picking | supported (features) |
| `forge3d.io.import_gltf` / `load_obj` | engine-side mesh import with materials | re-imports Studio assets for the engine | supported |
| `forge3d.path_tracing` (`PathTracer`, `hybrid_render_terrain_reference`, AOVs, certificates) | PT of spheres/triangles/BVH, terrain+mesh hybrid reference | Phase 14b "physical preview"; slow, deterministic | supported/reference |
| `forge3d.sdf` (`SdfScene`, `SdfSceneBuilder`) | SDF geometry for the hybrid renderer | Phase 16 lab | native |
| `dd_*`, `render_adjudication_pair` | dual-path determinism harnesses | tests, not product UI | lab |

### 2.6 Geo data and cartography

| API | What it does | Studio use | Readiness |
|---|---|---|---|
| `forge3d.cog.open_cog`, `CogDataset` | COG streaming, IFDs, windows | Phase 12 DEM/COG import without full download | supported (feature `cog_streaming`) |
| `forge3d.crs`, `forge3d.geo`, `reproject_dem_to_target`, `transform_coords`, `crs_to_epsg` | CRS handling and reprojection | Phase 12 import/display, overlay alignment | supported (PROJ optional) |
| `forge3d.vector` | polygons/lines/points + coverage | Phase 11 overlays | supported |
| `forge3d.graticule.generate_graticule` | lat/lon grid | Phase 12 furniture | supported |
| `forge3d.legend`, `scale_bar`, `north_arrow` | cartographic furniture | Phase 11 | supported |
| `forge3d.export` (`export_svg`, `export_pdf`, `generate_svg`) | vector/print export | Phase 11 export targets | supported |
| `forge3d.publication`, `MapPlate`, `PublicationRecipe` | print plates | Phase 11/16 | supported |
| `forge3d.landscape` | landscape-layout compositions + verification | Phase 16 | supported |
| `forge3d.pointcloud` (`open_copc`, `open_ept`, `open_laz`, `PointCloudRenderer`) | point cloud datasets, node visibility/budget | Phase 13 preview (three.js) + future native | **partial** |
| `forge3d.tiles3d` (`load_tileset`, `decode_b3dm`, `decode_pnts`) | 3D Tiles parsing/caching | Phase 13 data import; render incomplete | **underdeveloped** |
| `forge3d.buildings` (`add_buildings*`, materials) | building layers | Phase 13 | **underdeveloped** |

### 2.7 Projects, determinism and evidence

| API | What it does | Studio use | Readiness |
|---|---|---|---|
| `forge3d.bundle.save_bundle` / `load_bundle` | `.forge3d` bundle: DEM, overlays, labels, preset, camera bookmarks, HDR, checksums | Phase 15 interchange/export | supported |
| `forge3d.certificate` (`emit_render_certificate`, `sign_certificate`, `verify`) | signed render certificates | Phase 15 "verified render" badge | supported (signing key policy) |
| `forge3d.provenance` (`seal_provenance`, `verify_provenance`, Merkle manifests) | tile/asset provenance | Phase 15 evidence panel | supported |
| `forge3d.determinism_probe`, `ReproducibilityProfile` | determinism check and declared reproducibility inputs | Phase 9/15 render check | supported |
| `forge3d.capabilities`, `device_probe`, `native_degradations`, `memory_metrics`, `shader_report`, `render_execution_report` | machine and execution truth | Phase 8 | supported |
| `forge3d.config.load_renderer_config`, `RendererConfig` | renderer presets/overrides | Phase 8/10 settings import | supported |

### 2.8 Lab surfaces (opt-in only)

`forge3d.splat` (Gaussian splat + LiDAR/terrain fusion, `render_fused`),
`forge3d.inverse` (differentiable inverse PT), `GlobeScene`/ORBIS,
`forge3d.publication`/`MapPlate`, `dd_*` harnesses. These are capability-gated
and labelled "Lab" in the UI; no product promise, no default on.

---

## 3. Phases

Phases are ordered by dependency, not by excitement. Each phase ends with a
shippable Studio state; nothing later is required to make an earlier phase
honest.

### Phase 8 — Engine capability registry and hardened bridge
**Goal:** one snapshot of what this machine can do, and a bridge that all later
phases use.

Engine APIs: `capabilities()`, `device_probe()`, `enumerate_adapters()`,
`native_import_error()`, `native_degradations()`, `has_gpu()`,
`oidn_available()`, `copc_laz_enabled()`, `is_weighted_oit_available()`,
`proj_available()`, `splat_fusion_available()`, `memory_metrics()`,
`shader_report()`, `render_execution_report()`.

Studio changes:
- `studio-core` (or the shell's bridge type): `EngineCapabilities` snapshot and
  a `Capability` enum with `Unsupported { reason }`; no engine values in the
  project schema.
- Shell: `engine_capabilities` IPC; service dispatcher with typed commands;
  restart-on-crash and a clear "engine unavailable" path; keep the Phase 7 queue
  as the only offline scheduler.
- UI: a Diagnostics panel (adapter, backend, granted/requested features,
  degradations, memory policy) and a `useEngineCapability(name)` helper that
  disables controls with reasons.
- Tests: capability JSON parsing, degraded/absent cases, service restart,
  command dispatch errors.

**Exit:** every later phase can ask "is X available?" without a try/except at
render time, and the answer is visible to the user.

Size: M. Risk: low.

### Phase 9 — Scene recipes (MapScene), replacing the ad-hoc render payload
**Goal:** render from a declared scene, not a hand-built request.

Engine APIs: `MapScene`, `SceneRecipe`, `TerrainSource`, `OrbitCamera`,
`LightingPreset`, `OutputSpec`, `validate()`, `compile_plan()`,
`render()`/`render_passes`, `ValidationReport`, `ReproducibilityProfile`.

Studio changes:
- A pure mapping module (`render_recipe.rs` or `workspace::recipe`) from the
  canonical project to `SceneRecipe`: terrain (DEM + style), camera (the same
  eye/aim/FOV as `camera_render_view`), lighting (sun/HDRI/sky), output
  (size/HDR/AOV), target CRS. Golden-vector tests, as with the camera mapping.
- Shell: `render_scene` command for stills; keep the per-frame IPC for animation
  (the recipe is rebuilt per frame from the evaluated clone) and mark it in the
  request. `validate_scene` returns diagnostics to the UI.
- UI: a **Render check** panel listing warnings/unsupported features with
  remediation; stills use the recipe; visual regression against today's terrain
  output (same camera, same settings).
- Tests: recipe stability, camera parity, validation surfacing, output
  equivalence for the current terrain features.

**Exit:** one recipe code path renders the terrain stills, and a user can see
what the engine will/won't do before pressing Render.

Size: L. Risk: medium (must preserve current output).

### Phase 10 — Look development: water, clouds, atmosphere, volumetrics, post
**Goal:** the engine's look controls, gated and grouped.

Engine APIs: the dataclasses in §2.3, `atmosphere_bake_luts`, `IBL`,
`MaterialSet`, `SkySettings` models, `ShadowSettings`, `TonemapSettings`.

Studio changes:
- Extend `RenderSettings` into capability-tagged groups (sky, water, clouds,
  volumetrics, shadows, post, terrain material) with defaults equal to today.
  Validation ranges mirror the engine's (`turbidity 1..10`, `ozone 0..600`, …).
- Shell: payload mapping only; AETHER becomes a two-step action (bake LUTs →
  select handle), with progress; no implicit fallback to another sky model.
- UI: Inspector sections + presets (`terrain_default`, "alpine", "dusk"…),
  cost hints (AO/volumetrics/AETHER), and disabled controls for missing
  capabilities.
- Tests: round-trip, range validation, preset application, AETHER gating and
  bake error handling; one GPU smoke render per group (opt-in CI).

**Exit:** a user can author a look end to end and get the same frame in Rendered
and in the export.

Size: L. Risk: medium (AETHER bake + cost).

### Phase 11 — Cartography: vector overlays, labels, furniture, print export
**Goal:** Studio stops being only a DEM viewer.

Engine APIs: `VectorOverlay`, `forge3d.vector`, `LabelLayer` + `label_plan` +
`text`/`text_atlas`, `MapFurnitureLayer` (title, legend, scale bar, north arrow,
graticule, keepouts), `export_svg`/`export_pdf`, `legend`/`scale_bar`/
`north_arrow`/`graticule` builders.

Studio changes:
- New canonical layer types (vector, label, furniture) stored in the project:
  asset reference, style, ordering, visibility, lock. Commands: add/remove/reorder
  layer, set style, set label rules. All undoable, all validated.
- Import: GeoJSON (and, if available, Shapefile/GPKG via the engine's vector
  loaders); keep assets as file references like DEMs.
- Shell: layer → overlay payload; label plans computed by the engine and
  returned to the UI for inspection (rejections explained).
- UI: Layers panel (separate from the object tree), style editor with support
  badges, label settings (font, size, halo, collision/occlusion), furniture
  editor, export dialog (PNG/SVG/PDF, DPI, bounds).
- Tests: layer serialization/undo, style support validation, label rejection
  diagnostics, SVG/PDF parse/size smoke.

**Exit:** a labelled, styled map exports to PNG, SVG, and PDF from the same
project, with unsupported style fields reported rather than dropped silently.

Size: XL. Risk: medium-high (new model surface).

### Phase 12 — Geo data and time: COG, CRS, graticule, sun/moon
**Goal:** real geospatial input and location-aware lighting.

Engine APIs: `open_cog`/`CogDataset`, `reproject_dem_to_target`,
`transform_coords`, `crs_to_epsg`, `crs`, `geo`, `generate_graticule`,
`sun_position_utc`, `sky.set_observation`, `astro` (moon phase, refraction).

Studio changes:
- Asset kinds for COG and remote DEMs; partial/streamed reads; CRS stored with
  the terrain and shown in the Inspector; reprojection option on import.
- Lighting: "Sun from date/time/location" (lat/lon/timezone), a live solar
  vector, optional moon/planet overlay. Persist the observation like
  `sky.set_observation` does.
- Graticule as furniture (Phase 11) with CRS-aware formatting.
- Tests: sun vector goldens (the engine's own `sun_position_utc` is the
  reference), CRS round trips, COG availability gating.

**Exit:** a GeoTIFF/COG opens with the right CRS and extent, and the sun can be
driven by a timestamp with a tested vector.

Size: L. Risk: medium (PROJ optional, time zones).

### Phase 13 — Data layers: buildings, point clouds, 3D Tiles (honest split)
**Goal:** bring the datasets in, without pretending the renderer supports them.

Engine APIs: `buildings.add_buildings*`, `pointcloud` openers +
`PointCloudRenderer`, `tiles3d.load_tileset` + `decode_*` + `Tiles3dRenderer`.

Reality check (important): MapScene marks buildings and 3D Tiles
`underdeveloped`, and the Python point cloud API exposes visibility/budget, not
an offline RGBA render. So this phase is **two steps**:

- **13a — Import and preview.** Decode headers/chunks, count points/tiles,
  show bounds, stream a bounded preview into the three.js viewport (the same
  philosophy as the terrain heightfield preview), and surface engine diagnostics.
  Buildings from GeoJSON/CityJSON can be previewed as extrusions using the
  engine's `infer_roof_type`/materials.
- **13b — Engine-native rendering.** Only when the engine exposes a complete
  path (offline point cloud/tiles render): wire it into the recipe behind a
  capability; keep 13a as the fallback.

Studio changes: asset kinds, layer objects, budgets/caches in the UI, "preview
only" and "not in Rendered yet" labels sourced from capability/validation.

**Exit:** a COPC/LAZ or tileset can be inspected, bounded, and explained; no
claim of a Rendered result that the engine cannot produce.

Size: L for 13a, XL for 13b. Risk: high.

### Phase 14 — Meshes in Rendered (the big missing piece)
**Goal:** glTF/primitives actually appear in engine frames.

Two sub-phases, because they are different products:

- **14a — Instanced static meshes over terrain.** `io.import_gltf` /
  `load_obj`, `geometry` (LOD, tangents, bounds), `instance_mesh_gpu_render`
  with the project's materials, composited over the terrain frame (two passes,
  shared camera/depth); no path tracing.
- **14b — Hybrid/physical preview.** `PathTracer` /
  `hybrid_render_terrain_reference` for terrain + meshes + environment; AOVs;
  deterministic seeds. Slow, opt-in, with a clear progress/cancel path (one
  frame per IPC, like the animation export).

Studio changes: material mapping (base colour/roughness/metallic/normal),
node selection already exists; a "Physical preview" mode or quality tier;
asset re-import at render time; capability badges for mesh features.

**Exit:** 14a produces a mesh-over-terrain still that matches the viewport
camera; 14b produces a deterministic reference frame with AOVs.

Size: XL (14a), XL (14b). Risk: high (feature flags, GPU instancing, cost).

### Phase 15 — Projects and evidence: bundles, certificates, provenance
**Goal:** portable interchange and render truth.

Engine APIs: `bundle.save_bundle`/`load_bundle` (DEM, overlays, labels, preset,
camera bookmarks, HDR, checksums), `certificate.emit_render_certificate`/
`verify`, `provenance.seal_provenance`/`verify_provenance`,
`determinism_probe`, `ReproducibilityProfile`, `render_execution_report`.

Studio changes:
- **Export/import `.forge3d` bundle** (a conversion, not the canonical format):
  manifest mapping, checksum verification, explicit loss warnings.
- Render certificates: pass `certificate=` on stills/exports when the signing
  key is configured; store the certificate path with the render; a badge and a
  "verify" action.
- Diagnostics/evidence panel: provenance, determinism probe, shader report,
  memory metrics. Reproducibility profile stored with render settings.
- Tests: bundle round-trip + tamper rejection, certificate sign/verify (dev key
  in tests), provenance manifest validation.

**Exit:** a render can be reproduced and its provenance checked outside Studio,
and a Studio project can move between machines through a bundle.

Size: L. Risk: medium (signing-key policy).

### Phase 16 — Lab: planetary, splat fusion, inverse, publications
**Goal:** keep the research surfaces available without mixing them into the
product contract.

`GlobeScene`/ORBIS, `splat` (`render_fused`), `inverse`, `publication`/
`MapPlate`, `landscape`. Each is behind a Lab opt-in, capability-gated, and
labelled as experimental in the UI; results never enter the default render
path. This phase is deliberately last and can be dropped without affecting
Phases 8–15.

Size: XL. Risk: high/experimental.

---

## 4. Sequencing and effort

```
Phase 8 (capabilities, bridge)
   └─ Phase 9 (MapScene recipe)
        ├─ Phase 10 (look)
        ├─ Phase 11 (cartography)
        │    └─ Phase 12 (geo/time)
        │         └─ Phase 13 (data layers, preview first)
        └─ Phase 14a (instanced meshes) ──► Phase 14b (hybrid PT)
   └─ Phase 15 (bundles/evidence)  — can run in parallel after 9
Phase 16 (lab) — optional, after 11/14
```

Rough sizes: 8 M · 9 L · 10 L · 11 XL · 12 L · 13 L+XL · 14 XL+XL · 15 L · 16 XL.

The shortest path to a visibly bigger product is **8 → 9 → 10 → 11**. Phases 13
and 14 are the expensive ones; 13a and 14a are worth doing before their "b"
halves because they are honest, testable, and unlock real user workflows.

---

## 5. Cross-cutting rules

1. **No engine state in the project.** Recipes are derived; the schema stays
   engine-free. Bundle import is a conversion with explicit loss reporting.
2. **Capability before control.** A control that the engine cannot service is
   disabled with the reason from the capability snapshot or the validation
   report.
3. **No silent degradation.** If a feature is approximate (preview point
   clouds, underdeveloped tiles), the UI says so and the render request logs it.
4. **One camera truth.** Every new camera path (MapScene `OrbitCamera`,
   `CameraAnimation`, PT camera) gets a golden-vector test against
   `camera_render_view`.
5. **One scheduler.** Offline work goes through the Phase 7 queue; the viewport
   preview stays newest-wins. No second job system.
6. **Tests per layer.** Pure mapping/serialization in `studio-core`; payload and
   service behavior in the shell; engine smoke renders opt-in on a GPU lane.
   Golden images only where the engine already has them.
7. **Asset references, never bytes.** DEMs, vectors, labels atlases, HDRIs, mesh
   files, tilesets: paths + byte sizes, relinkable, integrity-checked.
8. **Determinism is a feature.** When a shader or recipe changes, run the
   engine's `det_instrument_rewrite` and the golden lanes; do not assume.

## 6. What not to do

- Do not reimplement engine algorithms in TypeScript (terrain sampling, label
  layout, point decoding) beyond the existing lightweight previews.
- Do not adopt `SceneRecipe` as the project schema; Studio's own model carries
  selection, undo, timeline, and asset relinking, which the engine recipe does
  not.
- Do not expose `underdeveloped` layers as if they rendered; ship the preview
  and the diagnostic.
- Do not switch the default render path to path tracing or AETHER; both are
  opt-in tiers with real cost.
- Do not add a second Python process as a "quick" integration; the queue and the
  capability snapshot assume one engine session.

## 7. Immediate next steps

1. **Phase 8 skeleton:** `engine_capabilities` command + `EngineCapabilities`
   type + Diagnostics panel; split `studio_render.py` into a dispatcher without
   changing behavior.
2. **Phase 9 mapping:** `render_recipe` from the project (terrain, camera,
   lighting, output) with golden vectors, and `validate_scene` behind a "Render
   check" panel; compare a still before/after to prove equivalence.
3. **Then Phase 10** in groups (sky/atmosphere first, then water/clouds, then
   post) with capability gating and presets.

Files touched first: `studio-tauri/src-tauri/src/render.rs`,
`studio-tauri/src-tauri/src/commands.rs`,
`studio-tauri/src-tauri/src/studio_render.py` (→ dispatcher),
`studio-tauri/crates/studio-core/src/workspace.rs` (recipe mapping),
`studio-tauri/src/state/session.tsx`, `studio-tauri/src/components/Inspector.tsx`.
