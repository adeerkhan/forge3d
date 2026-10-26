# Forge3D Studio — Master Plan
## Building the complete desktop application for the Forge3D engine

**Repository:** this fork (`adeerkhan/forge3d`), directory `studio/` — fork-only work, not intended for upstream pull requests.
**Status header last updated:** 2026-10-26 (after knowledge-graph inventory of the feature surface).
**Knowledge graph:** `.ua/knowledge-graph.json` (2,372 nodes) covers the feature surface this plan migrates: `python/forge3d` (public API = the feature list), `examples/` (verification harnesses), and `studio/` (the app).

---

## Part I — Charter

Build a polished, native-feeling, cross-platform desktop application around the Forge3D engine so users work through a GUI instead of Python scripts, notebooks, and CLI tools.

### Core principles (unchanged from v1 of this document, with two additions)

1. Reuse the existing Forge3D engine wherever practical; never reimplement rendering.
2. **Full feature parity is the goal.** Everything Forge3D can do — through its Python API or its Rust APIs — must be reachable from Studio. "Basic terrain viewer" is a milestone, not a destination.
3. **Examples are the specification.** Every `examples/*.py` script is a working, end-to-end verification of one or more features. A Studio feature is "done" when the workflow that example performs can be performed from the GUI and produces equivalent output. Where an example emits deterministic artifacts, Studio's harness tests compare against them.
4. Keep the GUI responsive: no engine work on the UI thread; jobs are cancellable or honestly non-cancellable.
5. Preserve upstream compatibility; isolate all GUI-specific code in `studio/`.
6. Clearly distinguish implemented features from planned ones (this document + the Status section in `studio/README.md`).
7. Small, tested, incremental changes; no unverified big-bang rewrites.

### Application shape

- **Language:** Rust UI (egui/eframe 0.27, pinned to match forge3d's wgpu 0.19) + embedded Python engine worker.
- **Integration:** the worker thread embeds CPython (the engine checkout's `.venv`) and drives forge3d's **certified, golden-tested Python entry points**. Every upstream golden (`mapscene_*`, `determinism`, certificates) exercises exactly those entry points, so Studio renders are real engine renders by construction.
- **Why not pure Rust yet:** the native `TerrainScene` viewer path has drifted from the live pipeline (bind-group layout grew to 23 entries, depth attachment, camera plumbing) and only the Python-driven path is maintained. Milestone R1 (below) repairs that drift; until then the embedded-Python bridge is the correctness anchor.
- **Targets:** Linux first (Fedora dev machine), Windows and macOS after packaging infrastructure exists.

---

## Part II — Current state (what exists in `studio/` today)

Implemented and tested (see `studio/README.md` for details):

- egui/eframe shell: menu + Lucide-icon toolbar, scene tree, viewport (drag-orbit / scroll-zoom), properties (camera/sun/export), status bar, dark/light themes.
- Engine integration: worker embeds Python, loads a DEM (GeoTIFF decoded in Rust), maintains a persistent `Session(window=False)` + `TerrainRenderer`, renders through `render_terrain_pbr_pom` with camera/sun/colormap overlays, exports PNG at arbitrary size.
- Project persistence v1: dataset reference, camera, sun, export settings; version-gated loader with validation warnings.
- Automated verification: `cargo test` in `studio/` — project-format round-trip/version tests + an integration test doing real DEM → render → export with non-flat-pixel assertions (RTX 5060, Vulkan).

What is NOT yet exposed (the rest of this document plans exactly that):

Everything else in the inventory below — MapScene cartography, labels, vector/building layers, GIS ingestion (COG/COPC/3D Tiles), path tracing, atmosphere/astro, smoke, splats, SDF, provenance/certificates, bundles, camera rigs/animation, analysis tools, and the interactive viewer.

---

## Part III — Feature migration plan (Forge3D → Studio)

The plan is organized as eight workstreams (W1–W8). Each lists the engine modules involved (from the knowledge-graph inventory), what Studio exposes, and the **harness examples** that verify it. Within each workstream, features are ordered by dependency and user value.

### W0 — Bridge v2: a generic engine-command layer (prerequisite for everything)

Today the worker calls one hand-written bridge function (`studio_render`). Before mass migration, generalize:

- A **typed command protocol** between UI and worker: `Command::Call { op: EngineOp, params: serde_json::Value }` with an enum of every engine operation Studio supports. Each `EngineOp` declares: Python entry point, parameter schema (mirroring the engine's validated dataclasses), progress semantics, cancellability, and expected artifact kind (RGBA frame / file on disk / structured metadata).
- The bridge script stops being per-feature glue; it becomes a **dispatcher**: `studio_dispatch(op, params)` → resolves the forge3d callable, builds validated kwargs, runs, returns (artifact path, metadata dict). Parameter coercion uses the engine's own validators (`_validate.py`, typed dataclasses) — Studio never bypasses engine validation.
- Worker gains: a job queue (UI can queue while one render runs), progress events (`Event::Progress { job, stage, fraction }` where the engine reports any), honest cancellation (only for ops that support it), and per-job error capture with the full engine traceback retained in logs.
- **DoD:** at least three heterogeneous ops (terrain frame, MapScene frame, GIS metadata read) run through the same dispatcher; the existing integration test still passes.

### W1 — Terrain rendering to full depth

Engine surface: `terrain_params.py` (the canonical 2,694-line typed parameter catalog), `terrain_demo.py`, `config.py`, `presets.py`, `colormaps/`, `terrain.py` (analysis), `terrain_scatter.py`, `_detail_normals.py`, `camera_rigs.py`, `animation.py`, `precision.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Full `TerrainRenderParams` editing (lighting, IBL, shadows, fog, water, clouds, bloom, screen-space GI/SSR/SSAO, tonemap, DOF, motion blur, lens effects, volumetrics, sky, height curve, POM, LOD, sampling) | Right-panel property pages with the engine's own validation errors surfaced inline | `terrain_demo.py` arg surface; `make_terrain_params_config` |
| Renderer presets (P7-01) | Preset dropdown + "apply & tweak" | `presets.py`, `mapscene_offline_quality.py` |
| Colormap picker (all registered colormaps + custom stops) | Asset browser + stops editor; overlay construction like the examples do | `colormaps/`, population spike examples |
| Camera rigs (orbit / rail / target-follow) + keyframe animation playback | Camera panel: rig mode, keyframe list, timeline scrubber, "render sequence" | `terrain_camera_rigs_demo.py`, `camera_animation_demo.py`, `bryce_canyon_flyover/` |
| Terrain analysis: viewshed, solar shadow masks, curved-Earth terminus | Analysis dialog producing raster/overlay results | `terrain.py` (viewshed/solar paths) |
| Terrain instanced scattering (seeded distributions, LOD chains, density masks) | Scatter layer type with source/distribution editors | `terrain_scatter.py` |
| Detail-normal map baking | Tools menu (offline batch op) | `_detail_normals.py` |
| Double-double precision demo/timing (absolute GPU coordinates) | Diagnostics panel section | `precision.py` |

### W2 — MapScene & cartography (the flagship surface)

Engine surface: `map_scene.py` (facade) + `_map_scene_compiler.py`, `_map_scene_labels.py`, `_map_scene_rasters.py`, `_map_scene_projection.py`, `_map_scene_render.py`, `_map_scene_native_inputs.py`, `_map_scene_validation.py`, `preparation.py`, `alignment.py`, `style.py`, `style_expressions.py`, `vector_line.py`, `buildings.py`, `labels` stack (`label_plan.py`, `text.py`, `text_atlas.py`, `chronos.py`), map furniture (`graticule.py`, `map_plate.py`, `legend.py`, `scale_bar.py`, `north_arrow.py`), `thematic.py`, `raster_style.py`, `colors.py`, `export.py` (SVG).

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Scene-as-recipe editing: terrain source, overlay layers (raster/vector/buildings/points/landmarks), label layers, camera/lighting/output specs — the `SceneRecipe` model, edited as a tree, serialized into the Studio project format v2 (recipes replace the v1 camera/sun fields) | Center-left scene tree becomes a real recipe editor | `map_scene.py`, all `mapscene_*` examples |
| Canonical MVP workflows: terrain+raster, terrain+vector+labels | One-click "new scene from template" flows | `mapscene_terrain_raster.py`, `mapscene_vector_labels.py` (the golden baselines) |
| Vector line draping (Mapbox-style line layers → `VectorOverlayConfig`) | Vector layer importer with style mapping | `vector_line_drape.py`, `luxembourg_rail_overlay.py` |
| 3D buildings from GeoJSON/CityJSON/3D Tiles with OSM-tag materials | Buildings layer importer + roof/material options | `buildings.py`, `mapscene_buildings_labels.py`, `osm_city_demo.py` |
| Label planning with depth occlusion, collision resolution, CHRONOS fade timelines | Label layer editor + timeline controls for flythroughs | `label_plan.py`, `fuji_labels_demo.py`, `ridge_labels_optimal.py`, `bryce_canyon_chronos_flyover.py` |
| Mapbox GL Style Spec import (layers, expressions, filters) | "Import style JSON" → auto-builds recipe layers | `style.py`, `style_expressions.py`, `amsterdam_gas/` |
| Thematic styling: classified/continuous/bivariate raster drapes, vector-value binning (equal-interval, quantile, Jenks) | Thematic layer editor with palette pickers | `mapscene_thematic_rasters.py`, `thematic.py`, `raster_style.py` |
| Map furniture: graticule, scale bar (geodetic), north arrow, legend, full map-plate composition (title/inset/frame) | Furniture layer gallery + plate composer dialog | `graticule.py`, `scale_bar.py`, `north_arrow.py`, `legend.py`, `map_plate.py`, notebooks |
| Data preparation pipeline: preparation grids, warp/resample/alignment of sources into scene space | "Prepare data" wizard (offline, cacheable) | `preparation.py`, `alignment.py`, `c05_real_data.py`, `prepare_scene.py` |
| Print-grade SVG vector export | Export dialog second tab | `export.py` |
| Deterministic CHRONOS flythrough rendering (recipe frozen per frame, label fades) | "Render flythrough" job (frame sequence + encode) | `chronos.py`, `bryce_canyon_chronos_flyover.py` |

### W3 — Data ingestion & formats

Engine surface: `gis.py`, `geo.py`, `crs.py`, `alignment.py`, `cog.py`, `pointcloud.py` (LAZ/COPC/EPT), `tiles3d.py`, `io.py`, `codec.py` (F3Z), `datasets.py`, `landscape.py`, `python/forge3d/data/*` fixtures.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Dataset open dialog for every supported input: local GeoTIFF (all dtypes), COG over HTTP, F3Z compressed DEM, OBJ/STL/glTF meshes, LAZ/COPC/EPT point clouds, 3D Tiles tilesets, GeoJSON/CityJSON/GeoPackage vectors | Unified import dialog with per-format metadata preview (raster info, CRS, bounds) before load | `io.py`, `gis.py`, `pointcloud.py`, `tiles3d.py` |
| COG streaming with cache statistics visible | Data source inspector (live LRU stats) | `cog.py` |
| CRS reprojection (MENSURA engine) and grid alignment before load | Import wizard step; unsupported CRS raises the engine's own error text | `crs.py`, `alignment.py`, `moon_south_pole.py` (typed lunar CRS) |
| Sample dataset browser (bundled + on-demand download with caching) | Welcome screen "sample scenes" gallery | `datasets.py`, `mapscene_bundled_datasets_showcase.py` |
| Unreal Engine Landscape interchange export/import | Export dialog | `landscape.py` |
| Point-cloud streaming into an interactive session | Point-cloud layer type with octree node inspection | `pointcloud.py`, `pointcloud_viewer_interactive.py` |

### W4 — Advanced rendering

Engine surface: `path_tracing.py`, `offline.py`, `denoise.py`, `denoise_oidn.py`, `guiding.py`, `inverse.py`, `sdf.py`, `splat.py`, `smoke.py`, `atmosphere.py`, `astro.py`, `sky.py`, `lighting.py`, `mesh.py`, `geometry.py`, `materials.py`, `textures.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Path-traced renders (hybrid ReSTIR integrator), offline progressive accumulation with convergence trends | Render mode switch (raster ⇄ path-traced) + accumulation panel (samples, trend chart, stop/pause) | `offline.py`, `southeast_europe_population_pt_native.py`, `swiss_landcover_pt_oblique.py` |
| Denoising (A-trous CPU, OIDN if installed) | Post-render pipeline toggle | `denoise.py`, `denoise_oidn.py` |
| SDF scenes (primitives + CSG, hybrid ray-marcher with CPU fallback) | SDF scene editor (primitive tree) — later milestone | `sdf.py` |
| SPLAT-FUSED scenes (Gaussian splats + LiDAR + terrain in one lit scene) | Splat layer type + fused render mode | `splat.py` |
| Atmospheric smoke: volume domains (NPZ/xarray), HRRR fields, hybrid procedural + data fields | Smoke layer importer + playback controls | `smoke.py`, `california_cigar_smoke_demo.py`, `california_fire_smoke_effect.py`, `bryce_canyon_storm_timelapse.py` |
| AETHER spectral atmosphere (LUT-baked), sky models | Atmosphere settings page (bake LUT job) | `atmosphere.py`, `aether` golden flows |
| SIDERA astro: ephemeris-correct sun/moon/planets, observation state, night rendering | Observation dialog (time/place → sun/moon) that feeds scene lighting | `astro.py`, `sky.py`, `humanity_globe_video.py` |
| ReSTIR direct lighting, area lights, light presets | Lighting editor beyond the terrain sun | `lighting.py` |
| Mesh pipeline: primitives, TBN, BVH upload, PBR material/textures | Mesh layer type with material editor | `mesh.py`, `geometry.py`, `materials.py`, `textures.py`, `helsinki_transit_daycycle.py` |
| Water optics verification fixtures | Preset scene + "verify optics" action | `terrain_water_optics.py` |
| DIFFERENTIA inverse path tracing (recover albedo/sun/turbidity from an observed frame) | Tools dialog — later milestone | `inverse.py` |

### W5 — Provenance, determinism & trust (a Studio differentiator)

Engine surface: `certificate.py`, `provenance.py`, `_ed25519.py`, `_canonical_json.py`, `verify.py`, `determinism.py`, `anamnesis.py` (warm store), `bundle.py`, `render_pass.py`, `_render_pass_bundle.py`, `_degradation.py`, `diagnostics.py`, `mem.py`, `_gpu.py`, `_memory.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Render certificates: every GPU render from Studio requests a certificate; the status bar shows the signed attestation; "verify" re-checks signatures | Certificate viewer dialog | `certificate.py`, `provenance_demo.py` |
| Per-pixel provenance (which source painted which pixel) | Provenance overlay toggle | `provenance.py` |
| Determinism panel: canonical frame render + SHA-256 display, backend pinning (`WGPU_BACKENDS`) | Diagnostics page | `determinism.py` |
| Degradation ledger viewer (explicit feature degradations of the last render) | Diagnostics page — never silently degrade | `_degradation.py`, `diagnostics.py` |
| Scene bundles (.forge3d engine format ≠ Studio project) save/load with checksums | File menu (distinct from Studio projects) | `bundle.py` |
| Warm-store (Anamnesis) cache management for render sequences | Job queue shows cache hits; cache inspector | `anamnesis.py` |
| Render-pass composition (ordered compositing of existing RGBA outputs) | Compositor panel — later milestone | `render_pass.py`, `turkiye_river_basins_3d.py` |
| GPU/memory telemetry (adapter probes, budget enforcement) | Status bar + diagnostics page | `_gpu.py`, `mem.py`, `_memory.py` |

### W6 — Interactive viewer & project ecosystem

Engine surface: `viewer.py`, `viewer_ipc.py`, `viewer_contract.py`, `_viewer_binary.py`, `_viewer_transport.py`, `interactive.py`, `widgets.py`, `bench.py`, `diagnostics.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Launch the native interactive viewer as a companion window from Studio (pre-populated with the current scene), controlled over the existing ~80-command IPC | Toolbar button "Open interactive viewer"; Studio forwards camera/sun/layer edits live | `viewer.py`, `viewer_ipc.py`, `terrain_viewer_interactive.py`, `c05_render.py` |
| Snapshot round-trip: viewer snapshot → Studio viewport (bridges the gap until Milestone R1) | Automatic: "sync from viewer" action | `terrain_viewer_interactive.py` |
| Benchmark harness for renderer/MapScene/codec ops | Tools menu (writes JSON evidence like the examples do) | `bench.py` |
| Jupyter widgets stay out of scope for Studio itself, but the notebook workflows they enable (quickstart, terrain explorer, map plate) become Studio templates | Welcome-screen templates | `widgets.py`, `examples/notebooks/*` |

### W7 — Studio-native product features (not in the engine)

- Multi-dataset scenes with per-layer visibility/opacity (project v2 already has the field).
- Job queue UI (queued/running/completed/cancelled/failed states per GUI.md v1 section 8) with history and retry.
- Settings persistence (theme, backend pin, Python environment path, cache dirs) via platform-appropriate config dirs.
- Recent projects, autosave, crash-safe project recovery.
- Welcome screen: sample scene gallery (via `datasets.py`), recent projects, templates derived from examples.

### W8 — Rust-side improvements (engine work done in this fork)

Ordered; each is small, tested, and documented as a fork change:

1. **R1 — Repair the native `TerrainScene` viewer path.** Rewire `render_viewer_terrain` to the live 23-entry bind group via `create_terrain_main_bind_group`, add the required `Depth32Float` attachment, and add a D2Array material fallback. Gate: renders the same scene as the Python path with equivalent output (pixel comparison harness), then the interactive viewer binary (`interactive_viewer`) regains working terrain. **This is the prerequisite for R2/R3 and removes the "drifted native path" caveat entirely.**
2. **R2 — Pure-Rust embedded viewport.** Render `TerrainScene` into an offscreen texture and display it through eframe's wgpu renderer without CPU readback. eframe 0.27 already shares wgpu 0.19; the worker produces a `wgpu::Texture` handle per frame (or renders on the UI thread's device behind a mutex). Gate: ≥30 fps continuous orbit on the dev machine; byte-identical output vs the readback path on a fixed backend.
3. **R3 — Engine API additions the GUI needs (small, upstreamable):** expose `RasterInfo`-style metadata returns for the viewer load path, a `clear_viewer_terrain`-style API for layers, and progress callbacks for long loads. Each lands with tests and stays mergeable upstream.
4. **R4 — Allocation/tracker hygiene for long-running GUI sessions:** Drop-guarded tracked allocations on every new render path (per the PROMETHEUS rules in AGENTS.md), budget enforcement verified over a 1,000-render soak.
5. **R5 — Determinism guardrails for the GUI:** the studio integration suite pins `WGPU_BACKENDS` and compares rendered bytes across two runs, mirroring `test_determinism_hash.py`'s subprocess pattern, so GUI-facing refactors cannot silently change output.
6. **R6 — Build/packaging:** `cargo xtask`-style tasks for wheel + binary builds; bundle a Python runtime for machines without the dev `.venv` (embedded distribution + forge3d wheel + numpy), Linux AppImage/deb first, then Windows (MSVC + python313.dll side-by-side) and macOS.

---

## Part IV — Examples as the verification harness

`examples/` is the only reason we can be sure any of this works: each script is a self-contained, end-to-end use of real engine features against real or synthetic data. The plan treats them as the test oracle.

### Harness strategy

1. **Harness registry.** `studio/tests/harness_registry.json` maps each migrated Studio capability to its verifying example(s), the command to run the example, and the deterministic artifacts it produces (PNG frames, JSON evidence, certificates). Maintained alongside code; the W-tables above are its source.
2. **Parity tests.** For each capability: a `studio/crates/studio-engine/tests/parity/` test that (a) runs the example's exact engine calls through the Studio bridge, and (b) where the example produces a deterministic artifact (population spikes, mapscene goldens, water optics, provenance demo), compares Studio's output against it (pixel hash or structural JSON equality) on the pinned backend. Backend-dependent byte comparisons follow the ANAMNESIS ABSENT rule: only claim equality when the device probe proves a non-software adapter; otherwise run the pipeline and assert structural success.
3. **Example-backed smoke suite.** A scripted subset of examples (those with no external downloads: `fuji_labels_demo`, `mapscene_*` synthetic ones, `vector_line_drape`, `label_api_truth_basic`, `obliqua_*` fixtures, `provenance_demo`) runs in CI-style local verification after every Studio change that touches the bridge. Download-dependent examples (C05, Bryce, GHSL/WorldPop) remain manual acceptance runs.
4. **Golden discipline.** Never regenerate upstream goldens from Studio. Where a new Studio workflow needs its own golden, it lives under `studio/tests/goldens/` with a SHA-256 sidecar, exactly like the engine's determinism sidecars.
5. **The example reads the plan.** When adding a feature to Studio, the corresponding example is read first; its call sequence becomes the bridge op's implementation, its assertions become the parity test's assertions.

### Initial registry (subset — grows with every workstream item)

| Example | Verifies | Studio workstream |
|---|---|---|
| `mapscene_terrain_raster.py` | MapScene terrain+raster golden path | W2 |
| `mapscene_vector_labels.py` | Vector drape + label plans | W2 |
| `fuji_labels_demo.py` | Label styling/occlusion on synthetic terrain | W2 |
| `vector_line_drape.py` | Local line-ribbon drape | W2 |
| `terrain_camera_rigs_demo.py` | Orbit/rail/follow rigs + animation | W1 |
| `camera_animation_demo.py` | Keyframe flyover + sequence render | W1 |
| `terrain_water_optics.py` | Water glint/reflection/shadow correctness | W4 |
| `provenance_demo.py` | Per-pixel provenance verification | W5 |
| `determinism.py` | Canonical frame + hash | W5, R5 |
| `pointcloud_viewer_interactive.py` | Point-cloud streaming + viewer IPC | W3, W6 |
| `southeast_europe_population_pt_native.py` | Native path-traced PT with overlays | W4 |
| `california_cigar_smoke_demo.py` | Hybrid smoke fields, largest integration | W4 (manual) |
| `c05_render.py` | Real-data QGIS-comparison workflow | W2/W3 (manual) |
| `moon_south_pole.py` | Typed lunar CRS through MapScene | W3 |
| `humanity_globe_video.py` | Globe aggregation + astro lighting | W4 (manual) |

---

## Part V — Project format evolution

- **v1 (current):** name, terrain path, camera, sun, export settings, visibility.
- **v2 (W0/W2):** embeds the full serialized `SceneRecipe` (MapScene JSON), layer list with visibility/opacity, job settings, and the certificate/diagnostics display options. v2 stores the recipe exactly as MapScene serializes it (canonical JSON via `_canonical_json.py`) so round-trips into the engine are byte-stable. v1 files load by wrapping their fields into a minimal v2 recipe; v2+ files are rejected by older builds (version gate already implemented).
- Datasets stay as **references with content hashes** (recipe identity), never embedded pixel data; missing files produce validation warnings on open, per the v1 behavior.

---

## Part VI — Milestones and order of execution

Each milestone ends with: build green, tests green (including new parity tests), binary launches, docs updated, committed to the fork.

| # | Milestone | Contents | Size |
|---|---|---|---|
| M1 | **Bridge v2** | W0: typed EngineOp protocol, dispatcher, job queue, progress/error events; project format v2 skeleton | M |
| M2 | **MapScene in Studio** | W2 first ring: recipe editor for terrain+raster+vector+labels; template flows from the two canonical MVP examples; parity tests vs their goldens | L |
| M3 | **Data ingestion** | W3: unified import dialog (GeoTIFF/COG/F3Z/mesh/point cloud/3D Tiles/GeoJSON), CRS step, dataset browser | M |
| M4 | **Terrain depth** | W1: full `TerrainRenderParams` property pages, presets, colormap picker, rigs+animation timeline, scatter, analysis | L |
| M5 | **Certificates & diagnostics** | W5 first ring: certificates on every render, determinism panel, degradation ledger, telemetry | M |
| M6 | **Path tracing & offline** | W4 ring 1: PT render mode, offline accumulation, denoise; parity vs PT example outputs | M |
| M7 | **Atmosphere/astro/smoke** | W4 ring 2: AETHER settings, observation dialog, smoke volume import; example-backed acceptance runs | M |
| M8 | **Furniture & plates** | W2 second ring: graticule/scale bar/north arrow/legend/map-plate composer, thematic editors, SVG export | M |
| M9 | **R1 native repair** | W8-R1: repair `TerrainScene`, restore the interactive viewer's terrain, pixel-parity harness | M |
| M10 | **R2 embedded viewport** | W8-R2: continuous GPU-shared viewport replacing CPU readback | L |
| M11 | **Bundles, cache, compositor** | W5 second ring: bundle save/load UI, Anamnesis cache inspector, render-pass compositor | M |
| M12 | **Packaging** | W8-R6: bundled Python runtime, Linux packages, then Windows/macOS lanes with real device verification | L |
| M13 | **Advanced extras** | SDF editor, splat-fused mode, inverse PT, precision DD demos — after their examples are exercised | L |

R1 (M9) is deliberately scheduled after the GUI-facing milestones that users feel first; it can be pulled earlier if the CPU-readback viewport becomes the bottleneck. Every milestone's parity tests keep the examples green as the engine surface gets touched.

---

## Part VII — Testing & quality standards (carried over from v1, operationalized)

- `cargo fmt`/`clippy` clean in `studio/` (`-D warnings`), `cargo test` green — including the engine integration test and all parity tests — before any commit.
- Every GUI-facing feature lands with: the bridge op, the UI wiring, at least one parity or unit test, and a README status update.
- No feature is marked implemented in this document or the README until its harness example's workflow runs through Studio.
- Engine (`forge3d` root) changes stay minimal, additive, and fork-documented; anything reusable upstream is a candidate for a focused PR **after** it is proven in Studio.
- Known-failure ledger: keep a `studio/KNOWN_ISSUES.md` recording pre-existing reds (e.g., viewer-path drift details) so they are never mistaken for regressions.

---

## Part VIII — Definition of success

A user can open Forge3D Studio, import any dataset the engine supports, author a MapScene recipe with cartographic layers and labels, render it raster or path-traced with full parameter control, verify the output with certificates/provenance/determinism tools, animate and export it — and every one of those capabilities corresponds to an `examples/` workflow that Studio reproduces, with tests proving the parity.
