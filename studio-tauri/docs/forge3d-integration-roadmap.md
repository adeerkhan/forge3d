# Bringing Forge3D into Studio — integration roadmap (Phases 8–18)

Status: **proposal**. This document is a plan, not a changelog.

> **Delivery status (read this first).** Studio Phases 0–7 from
> `FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md` are implemented, plus the HDRI sky
> engine feature added after Phase 7. **Nothing in Phases 8–18 below is
> implemented.** They are the roadmap for the rest of the engine surface.
> The appendix maps every engine module to a phase so no area is dropped.

This is a second pass: the first draft covered the headline APIs (MapScene,
terrain params, geometry, cartography, bundles) but missed several real
subsystems — ANAMNESIS render caching, CHRONOS flythroughs, Mapbox style import,
thematic raster styles, data preparation, VT/scatter/smoke, licensing, render
passes, camera rigs, datasets and DEM codec. They are folded in below.

Readiness labels are the engine's own (`supported`, `native-required`,
`underdeveloped`) where it publishes them, so a phase cannot quietly claim more
than the engine can do.

---

## 0. What is actually in the code today

Studio already has the hard parts of a desktop product:

- `studio-tauri/crates/studio-core` — canonical project (scene, commands, undo,
  timeline, assets, validation), schema v4, no GPU/Python dependency.
- `studio-tauri/src-tauri` — Tauri shell: IPC, autosave/recovery, atomic save,
  render service (`studio_render.py`), offline render queue, asset integrity,
  packaging.
- `studio-tauri/src` — React UI: viewport (Solid/Material/Rendered), inspector,
  scene tree, timeline drawer, render queue, capability badges.

| Area | State | Where |
|---|---|---|
| Project model, schema v4, migrations | done | `studio-core/src/persist.rs` |
| Commands, undo/redo, validation | done | `command.rs`, `editor.rs` |
| Solid / Material / Rendered viewport | done | `Viewport3D.tsx` |
| Terrain preview + drape | done | `Viewport3D.tsx`, `raster_preview` |
| Rendered terrain stills + Rendered viewport | done | `studio_render.py`, `render.rs` |
| Timeline, keys, interpolation, playback, export | done | `timeline.rs`, `Timeline.tsx` |
| Camera animation → engine | done | `workspace::camera_render_view` |
| HDRI as IBL **and** as Rendered sky | done (post-7) | `sky_hdri.wgsl` |
| Offline render queue + cancellation | done | `renderQueue.ts` |
| Asset integrity + samples + perf guards | done | `integrity.rs`, `samples/` |
| Packaging (Linux deb) + CI | done | `tauri.conf.json`, workflow |
| Everything in Phases 8–18 | **not started** | this document |

Known product gaps inherited from Phases 0–7: the engine renders terrain only
(no meshes until Phase 15), there is no shared material library (per-object
materials only), the workspace switch was dropped, Windows/macOS installers are
not configured, and Rendered is a still frame rather than live playback.

---

## 1. Integration architecture

### 1.1 One bridge, typed commands

Keep the transport (Rust owns the Python process, newline-delimited JSON) and
replace the single request shape with a command dispatcher:

```json
{"cmd": "render_still",    "request": { "recipe": { ... } }}
{"cmd": "validate_scene",  "request": { "recipe": { ... } }}
{"cmd": "bake_atmosphere","request": { "lut": { ... } }}
{"cmd": "import_vector",   "request": { "path": "...", "crs": "..." }}
```

Rules: JSON in/out, assets as paths, base64 PNG for pixels, progress events on
stdout, typed errors `{ok:false, code, message, diagnostics}`, one offline job at
a time (the Phase 7 queue is the only scheduler).

### 1.2 Capability + license gate

Every control is enabled by a snapshot, never by `try/except` at render time:

- `forge3d.capabilities()` (granted/requested/limits), `device_probe()`,
  `enumerate_adapters()`, `native_import_error()`, `native_degradations()`,
  `has_gpu()`, `memory_metrics()`, `shader_report()`, feature probes
  (`copc_laz_enabled`, `is_weighted_oit_available`, `proj_available`,
  `splat_fusion_available`, `oidn_available`, `widgets_available`).
- **Licensing is part of the snapshot**: `forge3d._license.set_license_key`,
  `LicenseError`, `pro_gated_path_diagnostic`. A Pro-gated path is disabled with
  its reason, not attempted.
- Per-feature validators: `validate_terrain_vt_support`,
  `validate_label_support`, `validate_building_layer_support`,
  `validate_tiles3d_support`, `validate_style_support`.

### 1.3 Canonical model stays authoritative

The engine gets a **derived recipe**. The project stores Studio settings; the
recipe is recomputed at render time. Undo, autosave and the schema never depend
on engine structs. Bundle import (Phase 17) is a conversion with explicit loss
reporting, not a second source of truth.

### 1.4 One camera truth, one scheduler

Every new camera path (MapScene `OrbitCamera`, `CameraAnimation`, camera rigs,
path-tracer camera) gets a golden-vector test against `camera_render_view`.
Offline work goes through the Phase 7 queue; viewport previews stay
newest-wins.

---

## 2. Engine inventory — what exists and how Studio would use it

### 2.1 Rendering core

| API | What it does | Studio use | Readiness | Phase |
|---|---|---|---|---|
| `Session`, `TerrainRenderer`, `render_terrain_pbr_pom`, `render_with_aov` | GPU session + terrain pipeline | in use | in use | — |
| `TerrainRenderParams` / `make_terrain_params_config` | the frame: camera, sun, quality, overlays, all features | grows in 10 | supported | 10 |
| `MaterialSet`, `IBL`, `OverlayLayer`, `Colormap1D` | material layers, environment, drape, palettes | HDRI + drape already | supported | 10 |
| `Frame`, `HdrFrame`, `AovFrame` | RGBA/linear HDR, AOVs | stills, look-dev | supported | 15 |
| `render_offline` + `OfflineQualitySettings` / `OfflineProgress` | progressive accumulation | final stills/frames | supported | 15 |
| `CompiledFrame` / `compile_frame` | precompiled frame reuse | fast repeated frames | native | 16 |
| `render_pass` (`RenderPassSpec`, `compile_passes`, `execute_passes`) | ordered composition of RGBA outputs | custom post/composite, batch | supported | 15 |
| `TerrainSpike` | minimal validation renderer | diagnostics | supported | 8 |

### 2.2 Scene recipes and layers (`map_scene`)

| API | What it does | Studio use | Readiness | Phase |
|---|---|---|---|---|
| `MapScene`, `SceneRecipe`, `validate`, `compile_plan`, `render` | declarative scene + validation | render recipe | supported | 9 |
| `TerrainSource`, `OrbitCamera`, `LightingPreset`, `OutputSpec` | recipe inputs | map from project | supported | 9 |
| `VectorOverlay` + `vector` / `vector_line` | polylines/polygons/points, styles | layers | supported | 11 |
| `LabelLayer`, `label_plan`, `text`, `text_atlas` | labels, atlas, collision plans | cartography | supported | 11 |
| `MapFurnitureLayer` | title, legend, scale bar, north arrow, graticule, keepouts | map furniture | supported | 11 |
| `BuildingLayer` (`from_geojson`, 3D Tiles) | building volumes | data layer | **underdeveloped** | 13 |
| `PointCloudLayer` | declares COPC/EPT/LAZ | data layer | **native-required** | 13 |
| `Tiles3DLayer` | declares a 3D Tiles tileset | data layer | **underdeveloped** | 13 |
| `ValidationReport`, `Diagnostic` | supported/unsupported features, remediation | Render check panel | supported | 9 |

### 2.3 Look development (`terrain_params` catalog)

| Group | Settings | Phase |
|---|---|---|
| Sky | `SkySettings` (hosek-wilkie, preetham, approximate, aether, hdri), `AtmosphereSettings`, `AtmosphereLutHandle`, `atmosphere_bake_luts` | 10 |
| Water | `WaterSettings`, `WaterOpticsSettings`, `ReflectionSettings` | 10 |
| Clouds | `CloudSettings` | 10 |
| Volumetrics | `VolumetricsSettings`, `DensityVolumeSettings`, `media`, `smoke` | 10 (volumes), 14 (simulation) |
| Post | `BloomSettings`, `DofSettings`, `MotionBlurSettings`, `LensEffectsSettings`, `TonemapSettings`, `DenoiseSettings` | 10, 15 |
| Screen space | `ScreenSpaceSettings` (SSAO/SSGI/SSR/TAA) | 10 |
| Shadows | `ShadowSettings` | 10 |
| Terrain surface | `MaterialLayerSettings`, `MaterialNoiseSettings`, `PomSettings`, `DetailSettings`, `TriplanarSettings`, `LodSettings`, `SamplingSettings`, `ClampSettings` | 10, 14 |
| Virtual texturing | `TerrainVTSettings`, `VTLayerFamily`, `VTStore`, `open_vt_store`, `vt_stats`, `validate_terrain_vt_support` | 14 |
| Scatter | `terrain_scatter`: sources, filters, LOD, wind, contact, `apply_to_renderer` | 14 |
| Presets | `presets` (`outdoor_sun`, `rainier_relief`, `studio_pbr`, …), `lighting` presets | 10 |

### 2.4 Components and materials

| API | What it does | Studio use | Phase |
|---|---|---|---|
| `materials` (`PbrMaterial`, `PbrTexSet`), `textures` | PBR material containers/descriptors | shared material library | 11 |
| `colors`, `thematic`, `raster_style` | palettes, classification, categorical/bivariate raster styles | thematic maps, overlays | 11 |
| `style`, `style_expressions` | Mapbox Style Spec import + expression evaluation | vector/label styles | 11 |
| `parse_style`, `load_style`, `apply_style`, `layer_to_*`, `paint_to_vector_style` | style plumbing | style import | 11 |
| `declutter`, `layout_label_candidates`, `label_plan` | label placement | cartography | 11 |

### 2.5 Geometry, meshes and path tracing

| API | What it does | Studio use | Readiness | Phase |
|---|---|---|---|---|
| `geometry` | primitives, glTF/OBJ import, ribbon/tube/extrusion, LOD chain, tangents, `instance_mesh_gpu_render` | meshes in Rendered | supported | 15 |
| `mesh` | TBN, BVH build/refit, GPU upload | PT/picking | supported | 15 |
| `io.import_gltf` / `load_obj` / `save_obj` / `save_stl` | engine-side mesh import/export | assets | supported | 15 |
| `path_tracing` (`PathTracer`, `hybrid_render_terrain_reference`, AOVs) | PT of triangles/BVH, terrain+mesh reference | physical preview | reference | 15 |
| `render_offscreen_rgba` | hybrid offscreen render | physical preview | native | 15 |
| `sdf`, `SdfScene` | SDF geometry | lab | native | 18 |
| `render_brdf_tile`, `render_adjudication_pair` | material/determinism QA | tests | supported | 15/17 |

### 2.6 Geo data, preparation and time

| API | What it does | Studio use | Readiness | Phase |
|---|---|---|---|---|
| `cog.open_cog`, `CogDataset` | COG streaming | DEM import | supported | 12 |
| `crs`, `geo`, `gis`, `alignment`, `transform_coords`, `reproject_dem_to_target`, `resample_raster_to_grid` | CRS/reprojection/alignment | import + layer alignment | supported (PROJ optional) | 12 |
| `preparation` (`prepare_scene`, recipes/grids) | semantic data prep for MapScene | import pipeline | supported | 12 |
| `datasets` (`list/fetch/bundled`, `mini_dem`) | sample/on-demand datasets | onboarding, samples | supported | 12 |
| `codec` (`compress_dem`, `decompress_dem`, `verify_dem`) | deterministic DEM compression | asset handling | supported | 12 |
| `io.load_dem`, `fill_nodata`, `dem_stats`, `dem_normalize` | DEM I/O/stat | import | supported | 12 |
| `sun_position`, `sun_position_utc`, `sky.set_observation`, `astro` | solar/lunar ephemeris | time-of-day lighting | supported | 12 |
| `graticule.generate_graticule` | lat/lon grid | furniture | supported | 11/12 |

### 2.7 Cartography and publication

| API | What it does | Studio use | Phase |
|---|---|---|---|
| `VectorOverlay`, `VectorScene`, `vector` | vector geometry + coverage | layers | 11 |
| `label_plan`, `text`, `text_atlas`, `vector_line` | label placement/shaping/atlas, viewer ribbons | labels | 11 |
| `legend`, `scale_bar`, `north_arrow`, `graticule` | furniture builders | map furniture | 11 |
| `export` (`export_svg`, `export_pdf`, `generate_svg`) | vector/print export | export | 11 |
| `map_plate`, `publication` (`MapPlate`, `PublicationRecipe`) | print plates, linked plan-view PNGs | publication | 11 |
| `landscape` | Unreal Landscape interchange | export | 18 |
| `render_terrain_poster` | poster render | publication | 11 |

### 2.8 Animation, caching and sequences

| API | What it does | Studio use | Phase |
|---|---|---|---|
| `animation` (`CameraAnimation`, `CameraKeyframe`, `CameraState`) | keyframed camera paths | timeline handoff | 16 |
| `camera_rigs` (orbit/rail/target-follow, terrain clearance) | rig authoring baked to animations | camera tools | 16 |
| `chronos.render_flythrough`, `FlythroughManifest` | deterministic terrain flythroughs | sequence export | 16 |
| `render_flythrough` | flythrough entry | export | 16 |
| `anamnesis` (+ `_anamnesis_*`) | content-addressed execution/caching of offline sequences | render cache | 16 |

### 2.9 Projects, evidence and operations

| API | What it does | Studio use | Phase |
|---|---|---|---|
| `bundle.save_bundle` / `load_bundle` | `.forge3d` bundle: DEM, overlays, labels, preset, bookmarks, HDR, checksums | interchange | 17 |
| `certificate` (`emit_render_certificate`, `sign_certificate`, `verify`) | signed render certificates | verified renders | 17 |
| `provenance` (`seal_provenance`, `verify_provenance`) | tile/asset provenance | evidence | 17 |
| `recipe_manifest` | golden fixture intent + provenance | test/evidence | 17 |
| `determinism_probe`, `ReproducibilityProfile` | determinism check + declared inputs | render check | 17 |
| `capabilities`, `device_probe`, `native_degradations`, `memory_metrics`, `shader_report`, `render_execution_report` | machine/execution truth | diagnostics | 8 |
| `_license` (`set_license_key`, `requires_pro`, `LicenseError`) | Pro gating | licensing | 8 |
| `precision` (`dd_selftest`, `dd_harness`) | double-float precision | diagnostics | 8 |
| `verify` (`shader_report`), `_degradation` | shader proof/degradation sink | diagnostics | 8 |

### 2.10 Viewer and notebooks (deliberately out of the product path)

`viewer.open_viewer`, `ViewerHandle`, `viewer_ipc`, `viewer_contract`,
`interactive`, `widgets` (Jupyter), `_viewer_*`. Studio's embedded viewport is
the product path; a detached viewer window was tried in Phase 3 and dropped.
These stay available for diagnostics/reference (Phase 18, opt-in) but are not a
Studio feature.

### 2.11 Lab surfaces

`splat` (Gaussian splats + LiDAR/terrain fusion), `inverse` (DIFFERENTIA),
`GuidingGrid`, `GlobeScene`/ORBIS, `landscape`, `sdf`, `lighting` (ReSTIR PT),
`bench`, `smoke` simulation internals, `dd_*` harnesses. Capability-gated,
labelled "Lab", never on the default path.

---

## 3. Phases

Ordered by dependency. Each phase is shippable and honest on its own.

### Phase 8 — Engine truth: capabilities, licensing, diagnostics
**Goal:** one snapshot of what this machine can do and may use.

APIs: §2.9 machine/diagnostic rows; `_license`; `precision`; `verify`;
`has_gpu`, `native_import_error`, `native_degradations`, `capabilities`,
`device_probe`, `enumerate_adapters`, `memory_metrics`, `shader_report`,
`render_execution_report`, `determinism_probe`.

Studio: `EngineCapabilities` + `LicenseState` snapshot; `engine_capabilities`
IPC; diagnostics panel (adapter, backend, features, degradations, license,
memory policy); `useEngineCapability` gating with reasons; service dispatcher
with typed errors, restart-on-crash, version pin.

Exit: every later control can ask "available? licensed?" without try/except,
and the answer is visible.

Size M · risk low.

### Phase 9 — Scene recipes (MapScene)
**Goal:** render from a declared scene, not a hand-built payload.

APIs: `MapScene`, `SceneRecipe`, `TerrainSource`, `OrbitCamera`,
`LightingPreset`, `OutputSpec`, `validate`, `compile_plan`, `render`,
`ValidationReport`/`Diagnostic`, `ReproducibilityProfile`.

Studio: pure `render_recipe` mapping from the project (terrain, camera, lighting,
output, CRS) with golden vectors; `render_scene` for stills; `validate_scene` for
a Render check panel; per-frame IPC keeps the animation path.

Exit: terrain stills from one recipe path; warnings/unsupported features visible
before rendering; visual equivalence with today's output.

Size L · risk medium.

### Phase 10 — Look development
**Goal:** the engine's look controls, grouped and gated.

APIs: `SkySettings` (analytic/AETHER/HDRI), `atmosphere_bake_luts`,
`WaterSettings`/`WaterOpticsSettings`/`ReflectionSettings`, `CloudSettings`,
`VolumetricsSettings`/`DensityVolumeSettings`, `BloomSettings`, `DofSettings`,
`MotionBlurSettings`, `LensEffectsSettings`, `TonemapSettings`,
`ScreenSpaceSettings`, `ShadowSettings`, `MaterialLayerSettings`,
`MaterialNoiseSettings`, `PomSettings`, `DetailSettings`, `TriplanarSettings`,
`LodSettings`, `SamplingSettings`, `ClampSettings`, `presets`.

Studio: capability-tagged setting groups (defaults equal today), ranges mirroring
the engine, presets, AETHER as bake → select, cost hints, disabled controls.

Exit: an authored look renders identically in Rendered and export; AETHER bake
is explicit.

Size L · risk medium (bake/cost).

### Phase 11 — Cartography and styles
**Goal:** Studio becomes a map-making tool.

APIs: `VectorOverlay`, `vector`, `vector_line`, `style`, `style_expressions`,
`parse_style`/`load_style`/`apply_style`, `layer_to_*`, `paint_to_vector_style`,
`raster_style`, `thematic`, `colors`, `materials`, `textures`, `label_plan`,
`text`, `text_atlas`, `declutter`, furniture (`legend`, `scale_bar`,
`north_arrow`, `graticule`, `MapFurnitureLayer`), `export_svg`/`export_pdf`,
`map_plate`/`publication`, `alignment`.

Studio: canonical layer types (vector/label/furniture) with assets, styles,
ordering, visibility; style import (Mapbox Style Spec) with support validation;
thematic raster styling; shared material/palette library; Layers panel; label
rules; furniture editor; PNG/SVG/PDF export; plate layouts.

Exit: a labelled, styled map exports to PNG/SVG/PDF from one project, with
unsupported style fields and label rejections reported.

Size XL · risk medium-high (new model surface).

### Phase 12 — Geo data, preparation and time
**Goal:** real geospatial input, reproducible preparation, location lighting.

APIs: `cog`, `crs`, `geo`, `gis`, `alignment`, `transform_coords`,
`reproject_dem_to_target`, `resample_raster_to_grid`, `preparation`
(`prepare_scene`, recipes/grids), `datasets`, `codec`, `io.load_dem`/
`fill_nodata`/`dem_stats`/`dem_normalize`, `sun_position*`, `sky.set_observation`,
`astro`, `graticule`.

Studio: COG/remote DEM assets, CRS display and reprojection on import,
preparation recipes, dataset browser/sample content, DEM verification/compression,
sun from date/time/location, optional moon/planets, CRS-aware graticule.

Exit: a COG opens with correct CRS/extent; preparation is declarative; the sun
can be driven by a timestamp with a tested vector.

Size L · risk medium (PROJ optional, time zones).

### Phase 13 — Data layers: buildings, point clouds, 3D Tiles (honest split)
**Goal:** bring the datasets in without pretending the renderer supports them.

APIs: `buildings.add_buildings*`, `infer_roof_type`, `materials`,
`io.import_osm_buildings*`; `pointcloud.open_copc/open_ept/open_laz`,
`PointCloudRenderer`, `read_laz_points_info`, `_pointcloud_octree`;
`tiles3d.load_tileset`, `decode_b3dm`, `decode_pnts`, `Tiles3dRenderer`;
`preparation` for tiling.

**13a — Import and preview.** Decode headers/chunks, count points/tiles, bounds,
bounded three.js preview, engine diagnostics, "preview only" labels. Buildings
from GeoJSON/CityJSON/OSM can be previewed as extrusions.

**13b — Engine-native rendering** only when the engine exposes a complete
offline path; behind a capability, with 13a as fallback.

Exit: COPC/LAZ/tilesets can be inspected and explained; no claim of a Rendered
result the engine cannot produce.

Size L (13a) + XL (13b) · risk high.

### Phase 14 — Terrain surface systems: VT, scatter, media volumes
**Goal:** the large-scale terrain features.

APIs: `TerrainVTSettings`, `VTLayerFamily`, `VTStore`, `open_vt_store`,
`vt_stats`, `validate_terrain_vt_support`, `terrain_scatter` (sources, filters,
LOD, wind, contact, `apply_to_renderer`), `DensityVolumeSettings`, `media`,
`smoke` (`load_npz_volume`, `interpolate_density_frames`; simulation is offline).

Studio: VT layer families/atlas/residency budget UI + stats; scatter rule editor
(altitude/slope/mask/LOD/wind) with preview; volume import and playback.

Exit: a VT terrain layer and a scatter layer survive save/load and render; a
volume sequence plays; unsupported families report diagnostics.

Size XL · risk high (VT is a subsystem).

### Phase 15 — Meshes in Rendered, path tracing, AOV/denoise
**Goal:** objects actually appear in engine frames.

**15a — Instanced raster meshes over terrain.** `io.import_gltf`/`load_obj`,
`geometry` (LOD/tangents/bounds), `mesh` (BVH), `instance_mesh_gpu_render`,
material mapping, composited over the terrain frame with the shared camera.

**15b — Physical preview.** `PathTracer` / `hybrid_render_terrain_reference`,
`render_offscreen_rgba`, AOVs (`AovSettings`, `AovFrame`), OIDN
(`oidn_denoise`), `render_pass` composition, deterministic seeds; one frame per
IPC with progress/cancel.

Studio: meshes reach Rendered; material mapping; AOV export; denoise toggle;
"Physical preview" tier.

Exit: 15a matches the viewport camera; 15b is deterministic and reproducible.

Size XL + XL · risk high.

### Phase 16 — Animation systems: rigs, CHRONOS, ANAMNESIS
**Goal:** replace the per-frame loop where the engine has a better contract.

APIs: `animation` (`CameraAnimation`, keyframes), `camera_rigs` (orbit/rail/
target-follow, terrain clearance), `chronos.render_flythrough` +
`FlythroughManifest`, `render_flythrough`, `anamnesis` (content-addressed
sequence execution and store), `compile_frame`/`CompiledFrame`.

Studio: camera rigs in the timeline (bake to keys, preview), flythrough manifests
for sequence export, ANAMNESIS caching for repeated frames/sequences with cache
stats and GC, deterministic sequence manifests.

Exit: an exported sequence is reproducible from its manifest; repeated renders
hit the cache; rig math is golden-tested against `camera_render_view`.

Size L · risk medium.

### Phase 17 — Projects and evidence
**Goal:** portable interchange and render truth.

APIs: `bundle.save_bundle`/`load_bundle`, `_render_pass_bundle`, `certificate`,
`provenance`, `recipe_manifest`, `determinism_probe`, `ReproducibilityProfile`,
`render_execution_report`, `sign_render_certificate_digest`, `verify_*`,
`codec.verify_dem`.

Studio: `.forge3d` import/export with checksum verification and loss warnings;
signed renders when a key is configured; certificate badge + verify; provenance/
evidence panel; reproducibility profile stored with render settings; recipe
manifests for goldens.

Exit: a render can be reproduced and verified outside Studio; a project moves
between machines as a bundle.

Size L · risk medium (signing policy).

### Phase 18 — Lab (optional)
`GlobeScene`/ORBIS, `splat` (`render_fused`, `GaussianSplatCloud`),
`inverse` (DIFFERENTIA), `landscape`, `sdf`, `lighting` (ReSTIR),
`viewer`/`widgets` (detached viewer/notebook), `dd_*`. Labelled experimental,
capability-gated, never default. Droppable without affecting 8–17.

Size XL · risk high.

---

## 4. Sequencing

```
8 capabilities/licensing
 └ 9 MapScene recipes
    ├ 10 look
    ├ 11 cartography/styles
    │   └ 12 geo data/preparation/time
    │        └ 13 data layers (13a preview, 13b native)
    ├ 14 terrain surface systems (VT/scatter/volumes)
    ├ 15 meshes + path tracing (15a raster, 15b PT)
    └ 16 animation systems (rigs/CHRONOS/ANAMNESIS)
 └ 17 projects/evidence
18 lab (optional, last)
```

Sizes: 8 M · 9 L · 10 L · 11 XL · 12 L · 13 L+XL · 14 XL · 15 XL+XL · 16 L ·
17 L · 18 XL.

Shortest visible jump: **8 → 9 → 10 → 11**. Honest cheap wins before the heavy
phases: 13a previews, 16 ANAMNESIS caching.

---

## 5. Cross-cutting rules

1. **No engine state in the project.** Recipes are derived; bundle import is a
   conversion with explicit loss.
2. **Capability before control.** Every control is enabled by the Phase 8
   snapshot (including licensing) or a validation report.
3. **No silent degradation.** Preview-only and underdeveloped features are
   labelled and reported in the render request.
4. **One camera truth.** Golden vectors for every camera path.
5. **One scheduler.** Phase 7 queue; no second job system.
6. **Asset references, never bytes.** Paths + sizes, relinkable, integrity-checked.
7. **Evidence is a feature.** Certificates, provenance, reproducibility profiles,
   recipe manifests.
8. **Determinism is a gate.** Shader/recipe changes run the determinism rewrite
   and golden lanes; never assume.

## 6. What not to do

- Do not reimplement engine algorithms in TypeScript beyond lightweight previews.
- Do not make `SceneRecipe` the project schema.
- Do not expose `underdeveloped` layers as rendering.
- Do not default to path tracing/AETHER; both are opt-in tiers.
- Do not add a second Python process; one engine session, one queue.
- Do not reintroduce the interactive viewer as a second editor; it can only be
  an opt-in diagnostic window.

## 7. Immediate next steps

1. Phase 8 skeleton: `engine_capabilities` + `LicenseState` + Diagnostics panel;
   split `studio_render.py` into a dispatcher without changing behavior.
2. Phase 9 mapping: `render_recipe` with golden vectors and `validate_scene` in a
   Render check panel; prove a still is unchanged.
3. Then Phase 10 in groups (sky/atmosphere → water/clouds → post) with presets.

First files: `src-tauri/src/render.rs`, `src-tauri/src/commands.rs`,
`src-tauri/src/studio_render.py`, `crates/studio-core/src/workspace.rs`,
`src/state/session.tsx`, `src/components/Inspector.tsx`.

---

## Appendix A — Python module coverage

Every public module and the phase that owns it. `internal` = private helper;
`tooling` = test/bench/example surface, not product.

| Module | Purpose | Phase |
|---|---|---|
| `alignment` | CRS/geotransform alignment | 11/12 |
| `anamnesis` | content-addressed offline sequence cache | 16 |
| `animation` | camera keyframes/Animation | 16 |
| `astro` | moon/planet ephemeris (SIDERA) | 12 |
| `atmosphere` | AETHER helpers | 10 |
| `bench` | benchmark harness | tooling |
| `buildings` | buildings pipeline | 13 |
| `bundle` | `.forge3d` bundles | 17 |
| `camera_rigs` | orbit/rail/follow rigs | 16 |
| `certificate` | signed render certificates | 17 |
| `chronos` | deterministic flythroughs | 16 |
| `codec` | F3DZ DEM compression | 12/17 |
| `cog` | COG streaming | 12 |
| `colors` | colour conversions | 11 |
| `config` | renderer config | 8/10 |
| `crs` | CRS utilities | 12 |
| `datasets` | bundled/on-demand datasets | 12 |
| `denoise`, `denoise_oidn` | denoising | 15 |
| `determinism` | deterministic reference | 17 |
| `diagnostics` | structured validation diagnostics | 8/9 |
| `export` | SVG/PDF vector export | 11 |
| `geo` | solar geometry | 12 |
| `geometry` | mesh authoring/instancing | 15 |
| `gis` | GIS raster/vector ops | 12 |
| `graticule` | lat/lon grid | 11 |
| `guiding` | PT guiding grid | 18 |
| `interactive` | example command loop | 18 |
| `io` | DEM/mesh I/O, OSM buildings | 12/13/15 |
| `label_plan` | label planning | 11 |
| `landscape` | Unreal Landscape interchange | 18 |
| `legend` | legend builder | 11 |
| `lighting` | ReSTIR PT lighting | 18 |
| `map_plate` | print plate layout | 11 |
| `map_scene` | scene recipes + layers | 9 |
| `materials` | PBR material containers | 11 |
| `media` | participating media | 14 |
| `mem` | memory helpers | 8 |
| `mesh` | TBN/BVH/GPU mesh | 15 |
| `north_arrow` | compass furniture | 11 |
| `offline` | progressive offline rendering | 15 |
| `path_tracing` | path tracer | 15/18 |
| `pointcloud` | COPC/EPT/LAZ | 13 |
| `precision` | double-float precision | 8 |
| `preparation` | semantic scene preparation | 12 |
| `presets` | look presets | 10 |
| `provenance` | per-pixel provenance | 17 |
| `publication` | linked plan-view publications | 11 |
| `raster_style` | thematic raster styles | 11 |
| `recipe_manifest` | golden/recipe provenance | 17 |
| `render_pass` | pass composition | 15 |
| `scale_bar` | scale bar furniture | 11 |
| `sdf` | SDF scenes | 18 |
| `sky` | observation state | 12 |
| `smoke` | smoke volumes/simulation | 14 |
| `splat` | splat fusion | 18 |
| `style`, `style_expressions` | Mapbox style import/eval | 11 |
| `terrain` | terrain analysis/out-of-core | 12/14 |
| `terrain_demo`, `terrain_pbr_pom` | reference harnesses | tooling |
| `terrain_params` | renderer parameter catalog | 10 |
| `terrain_scatter` | scatter systems | 14 |
| `text`, `text_atlas` | text shaping/atlas | 11 |
| `textures` | PBR texture descriptors | 11 |
| `thematic` | vector classification | 11 |
| `tiles3d` | 3D Tiles | 13 |
| `vector`, `vector_line` | vectors/ribbons | 11 |
| `verify` | shader proof reports | 8 |
| `viewer`, `viewer_contract`, `viewer_ipc`, `widgets` | interactive viewer/notebooks | 18 (opt-in) |
| `_license` | Pro gating | 8 |
| `_degradation`, `_gpu`, `_memory`, `_native`, `_cache_policy` | internal runtime | 8 |
| `_anamnesis_*`, `_map_scene_*`, `_render_pass_bundle`, `_pointcloud_octree`, `_detail_normals`, `_deltae`, `_ed25519`, `_png`, `_canonical_json`, `_validate`, `_viewer_*` | internal helpers | owning phase |

## Appendix B — Rust surface

The same phases map onto the crate modules: `license`, `core` (memory/budget/
determinism), `verify`, `passes`, `scene`/`render`/`renderer`/`pipeline`,
`labels`/`vector`/`style`, `gis`/`geo`/`converters`/`import`/`loaders`/`codec`/
`tiles3d`, `pointcloud`/`accel`/`mesh`/`geometry`, `smoke`/`media`,
`animation`/`camera`, `bundle`/`export`/`offscreen`, `splat`/`sdf`/`viewer`/
`astro`/`lighting`. Features in `Cargo.toml` (`copc_laz`, `cog_streaming`,
`proj`, `enable-globe`, `enable-inverse-pt`, `atmosphere-bake`, `splat-fusion`,
`weighted-oit`, …) are probed by Phase 8, never assumed.
