# Forge3D Studio — Master Plan
## A Photoshop × QGIS × Blender-class desktop application for the Forge3D engine

**Repository:** this fork (`adeerkhan/forge3d`), directory `studio/` — fork-only work, not intended for upstream pull requests. Pushes go to `fork` (`adeerkhan/forge3d`); `origin` stays the upstream engine and is fetch-only.
**Status header last updated:** 2026-10-27 (UI refactor plan adopted; engine bridge through M4-core already landed).
**Knowledge graph:** `.ua/knowledge-graph.json` covers the feature surface this plan migrates: `python/forge3d` (public API = the feature list), `examples/` (51 verification harnesses), and `studio/` (the app).

---

## Part I — Charter

Build the desktop application Forge3D does not have: a professional workbench — the layer discipline of **Photoshop**, the geospatial rigour of **QGIS**, the 3D navigation of **Blender**, and the example-driven onboarding of **Rerun** — so that every capability the engine exposes through Python is reachable, visible, and testable in a GUI.

### Principles

1. **The GUI is the product, not a wrapper.** "Basic terrain viewer" was a milestone, not a destination.
2. **Full feature parity.** Everything Forge3D can do — Python API or Rust API — must be reachable from Studio. Where something cannot be reached yet, the UI says so with the reason and the milestone that unlocks it; nothing is faked.
3. **Examples are the specification.** Each `examples/*.py` is a working end-to-end workflow. A Studio feature is "done" when that workflow runs through the GUI and produces equivalent output, locked by a parity test. The examples are also the product's onboarding: Studio ships a gallery that opens each one as a ready-to-edit scene.
4. **Reuse the engine, never re-implement it.** Studio composes engine calls; it does not re-derive rendering, projection, styling, or validation. Engine error text is surfaced verbatim.
5. **Keep the UI responsive.** No engine work on the UI thread. Every long job is queued, cancellable or honestly non-cancellable, and visible in the status bar.
6. **Professional interaction, not widgets in a column.** Proper menu bar, docked panels with tab groups, a real layer tree, an inspector that follows selection, a viewport gizmo, keyboard-first operation, undo/redo.
7. **Preserve upstream compatibility.** All GUI code lives in `studio/`; engine changes stay minimal, additive, and documented as fork changes.
8. **Small, tested, incremental changes.** No unverified big-bang rewrites — including this UI refactor, which lands in five ordered milestones behind a working app at every step.

### Application shape

- **Language:** Rust UI (egui/eframe **0.27**, pinned to match forge3d's wgpu 0.19) + an embedded-Python engine worker.
- **Integration:** the worker thread embeds CPython (the engine checkout's `.venv`) and drives forge3d's certified, golden-tested Python entry points through one JSON dispatcher (`studio_dispatch`, 17 ops). Every upstream golden (`mapscene_*`, determinism, certificates) exercises exactly those entry points, so Studio renders are engine renders by construction.
- **Why not pure Rust yet:** the native `TerrainScene` viewer path has drifted from the live pipeline and cannot render correctly today; repairing it is milestone M9. Until then the embedded-Python bridge is the correctness anchor and the viewport is a CPU readback.
- **Targets:** Linux first (Fedora dev machine), Windows and macOS after packaging (M12).

### Design language

- **Look:** dark, dense, neutral — a working tool, not a demo. One accent colour, used only for selection, focus and primary actions. No gradients, no decorative borders, no centred marketing text.
- **Density:** 22 px list rows, 24 px tool buttons, 13 px UI text, 11–12 px for secondary metadata. Compact by default; the panel splitters are the escape hatch.
- **Hierarchy:** panels are titled by their tab, groups by a small caps label, values right-aligned in the inspector. Nothing is a "card" unless it is a discrete object (example card, certificate).
- **Type:** the system UI sans for chrome; monospace for paths, hashes, expressions and numeric fields.
- **Motion:** none beyond egui's own hover/selection feedback. A spinner only for jobs longer than 250 ms.
- **Icons:** Lucide, 16 px, stroke matching the text colour; one icon per concept (a layer kind always uses the same glyph in the tree, the menus and the gallery).
- **Words:** verbs on buttons ("Import dataset…", "Render frame"), no "OK/Cancel" where a specific verb is available, and no exclamation marks.

---

## Part II — Current state

Implemented and tested (see `studio/README.md` for the authoritative ledger):

- **Bridge v2 (M1):** `studio_dispatch` JSON-op dispatcher with a job protocol (`Command::Call` → `Event::JobDone`/`JobFailed`, `Busy`, `Error`). Ops: `probe`, `terrain.load/info/render/clear`, `terrain.params_schema/params_validate`, `mapscene.templates/template/render/validate`, `colormaps.list`, `presets.list/get`, `datasets.list`, `dataset.inspect`, `rig.bake`.
- **MapScene (M2, W2 ring 1):** scene-mode switch, recipe templates built from the canonical examples, validation, certified `gpu_terrain` rendering, signed render certificates (W5 ring).
- **Unified import (M3, W3 ring 1):** `dataset.inspect` previews honest per-format metadata (GeoTIFF/COG, NPY, F3DZ, LAZ/COPC/EPT, 3D Tiles, OBJ/glTF, GeoJSON-family); only DEM-like sources claim the terrain slot.
- **Terrain depth (W1 + M4 core):** colormap picker, renderer presets, camera-rig bake + keyframe timeline + sequence render, and the schema-driven `TerrainRenderParams` pages — 21 root parameters, 26 settings sections, 238 fields introspected from the engine, enum choices from the engine's own sets, validation errors shown inline, pinned parameters listed with reasons, project-persisted.
- **Project format v2:** dataset references, camera, sun, colormap/preset, serialized `SceneRecipe`, camera-rig JSON, terrain parameters; version-gated loader with validation warnings.
- **Harness (Part V):** `studio/tests/harness_registry.json` + parity tests (`parity_mapscene.rs`, `parity_ingest.rs`, `integration.rs`) — 26 tests green, one device-lane-dependent golden behind `FORGE3D_STUDIO_GOLDEN_STRICT`.

### UI debt — why this refactor

The current shell was built to prove the engine wiring, and it shows: 2,859 lines in one `app.rs`, panels hard-wired as three side panels, a two-item menu bar, twelve toolbar buttons in a row, no concept of a scene **object**, no selection model, no undo, and a property column that mixes camera, sun, parameters and export settings in one scroll. It cannot express "add a vector layer, select it, style it, reorder it, hide it, undo the delete".

**The refactor replaces the shell, not the engine.** Every bridge op, parity test and project field survives; the panels become dock slots, the property column becomes an inspector driven by selection, the scene tree becomes a real layer document, and the toolbar becomes menus.

### Not yet exposed

Vector/building layer *authoring* (import records metadata only today), COG-over-HTTP streaming, path tracing, atmosphere/astro, smoke volumes, splats, SDF, bundles, analysis tools, target-carrying rig playback, the interactive (continuous) viewer, and the two P6.1 colour-space toggles (`colormap_srgb`, `output_srgb_eotf`) that `make_terrain_params_config` does not accept. Each is scheduled below, with the milestone that unlocks it.

---

## Part III — UI architecture

### 3.1 Layout and docking

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Menu bar   File  Edit  View  Scene  Layer  Render  Analyze  Tools  Help       │
├──────────────────────────────────────────────────────────────────────────────┤
│ Toolbar    tools (select, orbit, pan, zoom, measure) │ transform │ snap       │
├────────────────┬──────────────────────────────────────────┬──────────────────┤
│ LEFT DOCK      │ VIEWPORT                                 │ RIGHT DOCK       │
│ [Layers][Scene]│  ┌───────────────┐  ← 3D navigation       │ [Inspector]      │
│ [Assets]       │  │        Z      │     gizmo (top-right)  │ [Render]         │
│                │  │     X     Y   │                        │ [History]        │
│ layer tree     │  └───────────────┘                        │                  │
│ with groups    │  rendered frame + HUD: zoom, coords,       │ selection-driven │
│                │  scale bar, safe frame                    │ property editor  │
├────────────────┴──────────────────────────────────────────┴──────────────────┤
│ BOTTOM DOCK   [Timeline] [Console] [Certificates] [Statistics] [Examples]     │
├──────────────────────────────────────────────────────────────────────────────┤
│ Status bar  adapter · backend · frame ms · selection · job/progress · memory  │
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Four dock slots** (left, right, bottom, center-free), each holding a **tab group** with a fixed, purpose-named set of tabs. Splitters resize; sizes persist. No free-floating windows and no tab-tearing in U0–U4: `egui 0.27` has no dock crate we can pin safely, and a hand-rolled slot+tab model is ~250 lines we fully control. (Revisit draggable tabs only if the hand-rolled version proves limiting.)
- **Layout presets** (`View ▸ Panel layout`): *Default*, *Cartography* (wide layers + legend), *Modeling* (wide viewport, collapsed left dock), *Data* (tall bottom console). Reset layout restores Default.
- **Every slot can collapse** to a 0-width rail with its tab icon, Photoshop-style.
- **Persistence:** layout, panel sizes, collapsed state and active tabs in the platform config dir (`$XDG_CONFIG_HOME/forge3d-studio/layout.json`), never in the project file (a project must open identically on another machine). Theme and Python-path preferences join them in the Preferences dialog (U1).

### 3.2 Menu bar and the action registry

Menus are the primary surface; the toolbar exposes only the six most-used actions per context.

| Menu | Contents |
|---|---|
| **File** | New, Open…, Open Recent ▸, Save, Save As…, Import ▸ (Dataset…, Example scene…), Export ▸ (PNG…, SVG…, Frame sequence…, Scene bundle…), Project settings…, Quit |
| **Edit** | Undo, Redo, Duplicate layer, Delete layer, Group layers, Ungroup, Select all layers, Preferences… |
| **View** | Zoom in/out, Fit, Camera views (Top, Front, Right, Isometric, Reset camera), Panel layout (Default, Cartography, Modeling, Data), Reset layout, Theme (Dark, Light) |
| **Scene** | Add layer ▸ (Terrain DEM…, Raster overlay…, Vector…, Buildings…, Labels…, Point cloud…, Mesh…, Furniture ▸), Environment ▸ (Sun & sky…, Atmosphere…, Time & place…), Scene template ▸ |
| **Layer** | Rename, Duplicate, Delete, Group, Ungroup, Move up/down, To top/bottom, Toggle visibility, Lock/unlock, Zoom to layer, Layer properties… |
| **Render** | Render frame, Render mode ▸ (Raster, Path-traced), Resolution ▸, Request certificate, Verify certificate…, Render queue… |
| **Analyze** | Viewshed…, Solar shadow mask…, Terrain statistics…, Volume… |
| **Tools** | Determinism check…, Detail-normal bake…, Benchmark…, Import diagnostics… |
| **Help** | Keyboard shortcuts, Examples gallery, About Forge3D Studio |

**One registry.** Every menu item and toolbar button is an `Action` value with an id, label, shortcut, icon, enabled-predicate and handler. Entries appear when their handler exists — `Add layer` lists only the layer kinds Studio can actually create, and grows as each kind's renderer lands. Consequences, all test-locked:

- No dead entries: an action that is not implemented is *absent*, never a disabled stub (except while a predicate says "no selection" / "job running", which the UI explains on hover).
- One shortcut table: duplicates are a test failure.
- Menu and toolbar cannot drift from each other or from the keyboard map.
- `Action::all()` is enumerable, so tests can assert coverage and the shortcuts window is generated, not hand-maintained.

### 3.3 Layers panel — the document made visible

The layer tree *is* the scene document. Photoshop's ordering and toggles, QGIS's symbology affordances.

- **Rows** render bottom-to-top like Photoshop (topmost = last painted). Each row: disclosure arrow (groups), kind icon, name, and on hover/selection a visibility eye, a lock, and an opacity value.
- **Selection** is single- and multi-select; it drives the Inspector and the viewport highlight. Clicking a row never reorders it.
- **Reorder** by dragging (with a drop indicator line); **grouping** into folders that carry their own visibility/opacity; **rename** on double-click or F2.
- **Context menu:** Duplicate, Delete, Group/Ungroup, Move to top/bottom, Zoom to layer, Layer properties…, Copy style / Paste style.
- **Kind icons and colour coding:** terrain, raster overlay, vector, buildings, labels, point cloud, mesh, furniture — one glyph per kind, reused everywhere.
- **What can be added today:** a terrain layer (import) and empty groups; imported datasets land as their detected kind. The rest of the kinds appear in `Add layer` when their renderer lands, so the menu never offers a layer that cannot draw — a registered-only row says so in the Inspector and the status bar.
- **Smart affordances:** a raster overlay row shows its colormap swatch; a terrain row shows its height range; a vector row shows its stroke colour; a broken source path shows a warning badge (the engine's own validation text on hover).
- **Layer kinds** (each maps to an existing engine surface — no invented concepts):

| Kind | Engine surface | Inspector |
|---|---|---|
| Terrain (DEM) | `io.load_dem`, terrain render params | render parameters (M4 pages), vertical exaggeration, colormap |
| Raster overlay | `OverlayLayer`, `thematic.py`, `raster_style.py` | colormap/stops, classification, opacity, blend |
| Vector | `vector_line.py`, `style.py`, `style_expressions.py` | stroke, width, halo, dash, expressions |
| Buildings | `buildings.py` | roof/height/material tag mapping |
| Labels | `label_plan.py`, `chronos.py`, `text.py` | text, size, halo, collision, fade |
| Point cloud | `pointcloud.py` (LAZ/COPC/EPT) | point size, colour source, budget |
| Mesh | `io.import_gltf`, `mesh.py`, `materials.py` | material, transform, cast/receive shadows |
| Furniture | `graticule.py`, `scale_bar.py`, `north_arrow.py`, `legend.py`, `map_plate.py` | placement, style, content |
| Group | — | visibility/opacity for children |

### 3.4 Inspector

Right dock, selection-driven, one tab per concern:

- **Inspector** — properties of the selected layer(s), grouped in collapsible sections with the engine's own vocabulary.
- **Render** — the `TerrainRenderParams` pages from M4-core, unchanged in behaviour, moved here: root parameters as a form, one collapsible section per settings dataclass, the "Not exposed" list at the bottom, engine validation inline.
- **History** — undo stack, click to jump.
- **Empty state** is a real instruction ("Select a layer to edit its properties"), never a blank panel.

Validation stays asynchronous and honest: an edit schedules a debounced engine validation; the value is only applied to the render when the engine accepts it, and the engine's message appears inline (the pattern M4-core already proved).

### 3.5 Viewport, tools, and the 3D gizmo

- **Central viewport** shows the engine's frame (CPU readback today; the embedded GPU viewport is M10). Drag = orbit, middle-drag or Space-drag = pan, wheel = zoom, `F` = fit. Renders are debounced, so a drag shows the last frame and renders on gesture end.
- **3D navigation gizmo, top-right:** an axis triad drawn from the live camera basis, with six click zones (Blender-style) that snap to Top/Bottom/Front/Back/Left/Right, a centre zone for isometric, and drag-to-orbit. It reflects the *document's* camera, so it is also correct while a rig is playing. Pure maths, unit-tested without a GPU.
- **HUD:** zoom percentage, cursor world coordinates, a QGIS-style scale bar, safe frame toggle, and a render-time chip. Nothing overlays the gizmo.
- **Modes:** *Viewport* (default) and *Compare* (A/B split against the last saved frame, useful for parameter work).
- **Tools:** select (pick a layer by clicking its footprint where the engine reports one), orbit, pan, zoom, measure (geodesic distance, engine-computed). Tool state is explicit in the toolbar's toggle group and the status bar.

### 3.6 Bottom dock

- **Timeline** — camera-rig keyframes, scrub, play/pause, sequence-render progress (existing W1 ring, re-homed).
- **Console** — job log with levels, the engine's traceback tail on failures, copy-as-text. Errors are never only toasts.
- **Certificates** — the signed attestation of the last render, verify button, payload SHA-256 (W5 ring, expanded in M5).
- **Statistics** — adapter/backend/degradations, frame times, GPU memory ledger, dataset cache stats.
- **Examples** — the gallery below (also reachable from `Help ▸ Examples gallery`).

### 3.7 Examples gallery (Rerun-style)

The answer to "the studio's purpose dies if the examples aren't usable". `studio/examples/index.json` is the curated index of all 51 `examples/*.py` scripts:

- **Card grid** with category chips (Terrain, Cartography, Buildings, Labels, Atmosphere, Smoke, Path tracing, Globe, Point clouds, Data prep, Provenance), a thumbnail (reusing `tests/golden/recipes/*.png` where one exists; otherwise rendered on demand and cached), a one-line description, and the engine surface it demonstrates.
- **"Open in Studio"** builds the document, imports the datasets it needs (from `python/forge3d/data` or a declared download), selects the interesting layer, and renders. One click, no Python.
- **Tiers, stated on the card:**
  - **A — runs today** through existing ops (terrain, cartography MVP, labels, rigs, presets, colormaps, import).
  - **B — needs a scheduled milestone** (buildings authoring, smoke, path tracing, globe/astro, point-cloud layers); the card names the milestone and refuses to pretend otherwise.
  - **C — needs an external asset or the embedded viewport** (download-dependent runs, interactive viewer).
- **Contract tests** (Part V): every entry's `example` path exists; every Tier-A entry's ops are registered in the bridge; every Tier-B/C entry names the milestone that unlocks it; every category has at least one entry.

### 3.8 Theme, iconography, motion

- One `theme.rs` token table: spacing (4/8/12/16), radius (6), row height (22), font sizes (11/13/15), accent, panel widths, and a dark + light palette derived from egui `Visuals` so both themes stay legible.
- Icons are loaded from `studio/assets/icons/*.svg` (Lucide, ISC) through the existing `resvg` loader; the set grows to one glyph per concept (eye/eye-off, lock, folder, table, spline, building, tag, cube, cloud, sun, moon, move, rotate, hand, zoom, ruler, palette, film, play/pause, undo/redo, upload/download, grip, more, search, chevron, x). A typed `Icon` enum prevents typo'd names, and a test asserts every enum variant resolves to a file.
- No animation beyond egui's own feedback; the only progress indicator is the status bar's job chip plus a spinner after 250 ms.

### 3.9 Keyboard map

`Ctrl+O/S/Shift+S` file, `Ctrl+Z`/`Ctrl+Shift+Z` undo/redo, `Ctrl+D` duplicate layer, `Ctrl+G`/`Ctrl+Shift+G` group/ungroup, `Del` delete layer, `F2` rename, `F` fit view, `1/3/7` front/right/top, `0` isometric, `Space` (hold) pan, `Ctrl+K` command palette, `?` shortcuts window. The map is generated from the action registry, so it cannot drift.

### 3.10 Accessibility and responsiveness

- Every icon-only control has a tooltip naming its action and shortcut.
- Colour is never the only signal: selection also changes the row weight, and layer visibility also changes the glyph.
- Focus is visible and traversable; text respects the OS scale factor.
- The status bar reports progress for every job over 250 ms; the UI never blocks on the engine.

### 3.11 Module structure

```
studio/crates/
├── forge3d-studio/
│   └── src/
│       ├── main.rs
│       ├── app.rs              // state, update loop, event routing (thin)
│       ├── actions.rs          // Action enum + registry + handlers
│       ├── doc/                // document model, layer ops, undo stack
│       │   ├── mod.rs  layer.rs  migrate.rs  undo.rs
│       └── ui/
│           ├── mod.rs  dock.rs  menu.rs  toolbar.rs  status.rs
│           ├── theme.rs  icons.rs
│           ├── panels/ (U1/U2) layers.rs  inspector.rs
│           │            render_params.rs  timeline.rs  console.rs
│           │            certificates.rs  statistics.rs  examples.rs
│           └── viewport/ (U3) mod.rs  gizmo.rs  hud.rs  tools.rs
├── studio-engine/              // bridge (unchanged role)
├── studio-project/             // project file format (v3 adds the document)
└── studio-examples/            // gallery index + fixtures contract tests
```

---

## Part IV — Feature migration workstreams

Each lists the engine modules involved, what Studio exposes, and the harness examples that verify it. W0–W8 are the feature rings; **W9 (UI shell)** and **W10 (example port)** are what this refactor adds.

### W0 — Bridge v2 *(delivered)*
Generic `studio_dispatch` JSON-op layer with a job protocol. New capabilities are usually one Python handler plus one UI surface.

### W1 — Terrain rendering to full depth

Engine surface: `terrain_params.py`, `terrain_demo.py`, `config.py`, `presets.py`, `colormaps/`, `terrain.py`, `terrain_scatter.py`, `_detail_normals.py`, `camera_rigs.py`, `animation.py`, `precision.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Full `TerrainRenderParams` editing *(delivered — M4 core)* | Inspector ▸ Render | `terrain_demo.py` |
| Renderer presets *(delivered)* | Inspector + `Render ▸` | `presets.py`, `mapscene_offline_quality.py` |
| Colormap picker *(delivered)* | Layer inspector + `Assets` tab | `colormaps/`, population spike examples |
| Camera rigs + keyframe timeline *(delivered)* | Bottom ▸ Timeline | `terrain_camera_rigs_demo.py`, `camera_animation_demo.py` |
| Terrain analysis: viewshed, solar shadow masks, curved-Earth terminus | `Analyze ▸` producing raster/overlay layers | `terrain.py`, `viewshed` goldens |
| Instanced scattering (seeded distributions, LOD chains, density masks) | Layer kind + source/distribution editors | `terrain_scatter.py` |
| Detail-normal map baking | `Tools ▸ Detail-normal bake…` | `_detail_normals.py` |
| Double-double precision demo/timing | `Tools ▸ Benchmark…` section | `precision.py` |

### W2 — MapScene & cartography (the flagship surface)

Engine surface: `map_scene.py` + `_map_scene_compiler.py`, `_map_scene_labels.py`, `_map_scene_rasters.py`, `_map_scene_projection.py`, `_map_scene_render.py`, `_map_scene_validation.py`, `preparation.py`, `alignment.py`, `style.py`, `style_expressions.py`, `vector_line.py`, `buildings.py`, the label stack, map furniture, `thematic.py`, `raster_style.py`, `colors.py`, `export.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Scene as a layer document (terrain, raster/vector/buildings/points/landmarks, labels, camera/lighting/output) compiled to the canonical `SceneRecipe` | Layers panel + Inspector (the document *is* the recipe) | `map_scene.py`, all `mapscene_*` examples |
| Canonical MVP workflows (terrain+raster, terrain+vector+labels) *(delivered as templates; becomes gallery scenes)* | Gallery cards + `Scene ▸ Scene template` | `mapscene_terrain_raster.py`, `mapscene_vector_labels.py` |
| Vector line draping → `VectorOverlayConfig` | Vector layer inspector | `vector_line_drape.py`, `luxembourg_rail_overlay.py` |
| 3D buildings from GeoJSON/CityJSON/3D Tiles with OSM-tag materials | Buildings layer inspector | `buildings.py`, `mapscene_buildings_labels.py`, `osm_city_demo.py` |
| Label planning with depth occlusion, collisions, CHRONOS fades | Labels inspector + timeline fades | `label_plan.py`, `fuji_labels_demo.py`, `ridge_labels_optimal.py`, `bryce_canyon_chronos_flyover.py` |
| Mapbox GL Style Spec import | `File ▸ Import ▸ Style JSON…` | `style.py`, `style_expressions.py` |
| Thematic styling (classified/continuous/bivariate, vector binning) | Raster/vector inspector palette + classification controls | `mapscene_thematic_rasters.py`, `thematic.py`, `raster_style.py` |
| Map furniture (graticule, scale bar, north arrow, legend, plate) | Furniture layer kind + plate composer | `graticule.py`, `scale_bar.py`, `north_arrow.py`, `legend.py`, `map_plate.py` |
| Data preparation (grids, warp/resample/alignment) | `Tools ▸ Prepare data…` wizard, cacheable | `preparation.py`, `alignment.py`, `c05_real_data.py`, `prepare_scene.py` |
| Print-grade SVG export | `File ▸ Export ▸ SVG…` | `export.py` |
| Deterministic CHRONOS flythrough | Render queue (frame sequence) | `chronos.py`, `bryce_canyon_chronos_flyover.py` |

### W3 — Data ingestion & formats

Engine surface: `gis.py`, `geo.py`, `crs.py`, `alignment.py`, `cog.py`, `pointcloud.py`, `tiles3d.py`, `io.py`, `codec.py`, `datasets.py`, `landscape.py`, bundled fixtures.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Open every supported input with a metadata preview before load *(delivered)* | `File ▸ Import ▸ Dataset…` | `io.py`, `gis.py`, `pointcloud.py`, `tiles3d.py` |
| COG streaming with LRU cache statistics | Data source inspector | `cog.py` |
| CRS reprojection and grid alignment | Import wizard step; engine error text surfaced | `crs.py`, `alignment.py`, `moon_south_pole.py` |
| Sample dataset browser (bundled + on-demand, cached) | Gallery + `Assets` tab | `datasets.py`, `mapscene_bundled_datasets_showcase.py` |
| Landscape interchange | `File ▸ Export` | `landscape.py` |
| Point-cloud streaming layer with octree inspection | Point-cloud layer kind | `pointcloud.py`, `pointcloud_viewer_interactive.py` |

### W4 — Advanced rendering

Engine surface: `path_tracing.py`, `offline.py`, `denoise.py`, `denoise_oidn.py`, `guiding.py`, `inverse.py`, `sdf.py`, `splat.py`, `smoke.py`, `atmosphere.py`, `astro.py`, `sky.py`, `lighting.py`, `mesh.py`, `geometry.py`, `materials.py`, `textures.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Path-traced renders (hybrid ReSTIR), offline accumulation | `Render ▸ Render mode` + accumulation in the status bar/Statistics | `offline.py`, `southeast_europe_population_pt_native.py`, `swiss_landcover_pt_oblique.py` |
| Denoising (A-trous, OIDN when present) | Post-render toggle in `Render ▸` | `denoise.py`, `denoise_oidn.py` |
| SDF scenes | SDF layer kind (later milestone) | `sdf.py` |
| SPLAT-FUSED scenes | Splat layer kind + fused render mode | `splat.py` |
| Atmospheric smoke (NPZ/xarray, HRRR fields) | Smoke layer kind + timeline playback | `smoke.py`, `california_*`, `bryce_canyon_storm_timelapse.py` |
| AETHER spectral atmosphere | `Scene ▸ Environment ▸ Atmosphere…` (LUT bake job) | `atmosphere.py` |
| SIDERA astro (ephemeris sun/moon/planets, night) | `Scene ▸ Environment ▸ Time & place…` | `astro.py`, `sky.py`, `humanity_globe_video.py` |
| Lights beyond the terrain sun, area lights, presets | Light layers in the Layers panel | `lighting.py` |
| Mesh pipeline: primitives, TBN, PBR materials | Mesh layer kind + material editor | `mesh.py`, `geometry.py`, `materials.py`, `helsinki_transit_daycycle.py` |
| Water optics verification | Preset scene + verify action | `terrain_water_optics.py` |
| DIFFERENTIA inverse path tracing | `Tools ▸ Inverse render…` (later) | `inverse.py` |

### W5 — Provenance, determinism & trust

Engine surface: `certificate.py`, `provenance.py`, `_ed25519.py`, `_canonical_json.py`, `verify.py`, `determinism.py`, `anamnesis.py`, `bundle.py`, `render_pass.py`, `_degradation.py`, `diagnostics.py`, `mem.py`, `_gpu.py`, `_memory.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Render certificates on every render *(MapScene: delivered)* | Bottom ▸ Certificates + status chip | `certificate.py`, `provenance_demo.py` |
| Per-pixel provenance overlay | Viewport layer toggle | `provenance.py` |
| Determinism panel (canonical frame, SHA-256, backend pinning) | `Tools ▸ Determinism check…` + Statistics | `determinism.py` |
| Degradation ledger | Statistics — never silently degrade | `_degradation.py`, `diagnostics.py` |
| Engine scene bundles (distinct from Studio projects) | `File ▸ Export ▸ Scene bundle…` | `bundle.py` |
| Anamnesis warm-store cache for sequences | Render queue + cache inspector | `anamnesis.py` |
| Render-pass composition | `Render ▸ Compositor…` (later) | `render_pass.py`, `turkiye_river_basins_3d.py` |
| GPU/memory telemetry | Status bar + Statistics | `_gpu.py`, `mem.py`, `_memory.py` |

### W6 — Interactive viewer & ecosystem

Engine surface: `viewer.py`, `viewer_ipc.py`, `viewer_contract.py`, `_viewer_binary.py`, `_viewer_transport.py`, `interactive.py`, `widgets.py`, `bench.py`.

| Studio capability | Exposed as | Harness example |
|---|---|---|
| Launch the native interactive viewer pre-populated with the current scene, synced over IPC | `View ▸ Open interactive viewer` | `viewer.py`, `terrain_viewer_interactive.py`, `c05_render.py` |
| Snapshot round-trip from viewer to Studio viewport | `View ▸ Sync from viewer` | `terrain_viewer_interactive.py` |
| Benchmark harness | `Tools ▸ Benchmark…` writing JSON evidence | `bench.py` |
| Notebook workflows as Studio templates | Gallery scenes | `widgets.py`, `examples/notebooks/*` |

### W7 — Studio-native product features

- Multi-layer documents with grouping, visibility/opacity, lock, undo/redo (U1).
- Job queue UI with states, history and retry.
- Settings persistence (theme, layout, backend pin, Python path, cache dirs).
- Recent projects, autosave, crash-safe recovery.
- Command palette (`Ctrl+K`) covering every action.

### W8 — Rust-side improvements (fork engine work)

1. **R1 — Repair the native `TerrainScene` viewer path.** Rewire `render_viewer_terrain` to the live 23-entry bind group via `create_terrain_main_bind_group`, add the required `Depth32Float` attachment, add a D2Array material fallback. Gate: pixel comparison against the Python path. *(Deliberately deferred; findings recorded in the project todo.)*
2. **R2 — Embedded GPU viewport.** Render `TerrainScene` into an offscreen texture and show it through eframe's own wgpu device (no CPU readback). Gate: ≥30 fps continuous orbit; byte-identical to the readback path on a fixed backend.
3. **R3 — Small upstreamable API additions** the GUI needs (metadata returns, layer clears, progress callbacks).
4. **R4 — Allocation/tracker hygiene** for long sessions (Drop-guarded tracked allocations, budget verified over a 1,000-render soak).
5. **R5 — Determinism guardrails for GUI-facing refactors** (pinned backend, two-run byte comparison).
6. **R6 — Build/packaging:** bundled CPython + wheel, Linux AppImage/deb first, then Windows/macOS lanes with real-device verification.

### W9 — UI shell (this refactor)

| Capability | Delivered in |
|---|---|
| Module split, design tokens, 35-glyph typed icon set | U0 *(landed)* |
| Action registry + full menu bar + toolbar + shortcuts window + command palette | U0 *(landed)* |
| Dock slots with tab groups, layout presets, persisted layout, collapsible rails | U0 *(landed)* |
| Document model v3 + Layers panel (tree, groups, drag reorder, eye/lock/opacity, rename, context menu, multi-select) | U1 *(landed)* |
| Undo/redo command stack over the document (snapshot per gesture) | U1 *(landed)* |
| Selection-driven Inspector per layer kind; M4 parameter pages re-homed | U2 |
| Viewport tools, 3D navigation gizmo, HUD (zoom, coords, scale bar, safe frame), compare mode | U3 |
| Bottom dock: timeline, console, certificates, statistics | U0 (tabs) / M5 (content) |
| Examples gallery with tiers and one-click scenes | U4 |

### W10 — Examples port

The gallery (3.7) plus the index contract. Every example enters the index with a tier; Tier-A examples get a parity test that opens the scene through the document path and renders it. Tier-B/C entries are the roadmap the user can see inside the app.

---

## Part V — Examples as the verification harness

`examples/` is the only reason we can be sure any of this works. The plan treats it as the test oracle *and* as the product's content.

1. **Harness registry** (`studio/tests/harness_registry.json`): maps each migrated capability to its verifying example(s), the studio ops used, and its deterministic artifacts. Already 8 entries; grows with every workstream item.
2. **Parity tests:** for each capability, run the example's exact engine calls through the Studio bridge and compare deterministic artifacts (pixel hash or structural JSON) on the pinned backend. Backend-dependent byte comparisons follow the ABSENT rule: claim equality only when the device probe proves a non-software adapter.
3. **Example-backed smoke suite:** the subset with no external downloads (`fuji_labels_demo`, synthetic `mapscene_*`, `vector_line_drape`, `label_api_truth_basic`, `obliqua_*`, `provenance_demo`) runs after every bridge change. Download-dependent examples stay manual acceptance runs.
4. **Gallery contract tests** (new): index integrity (paths exist, tiers valid, categories non-empty), Tier-A ops registered, Tier-B/C milestones named, thumbnails present or derivable.
5. **Golden discipline:** never regenerate upstream goldens from Studio. Studio's own goldens live under `studio/tests/goldens/` with SHA-256 sidecars.
6. **The example reads the plan:** when adding a feature, the example is read first; its call sequence becomes the bridge op, its assertions become the parity test.

---

## Part VI — Project format v3 (document & layers)

- **v1:** name, terrain path, camera, sun, export, visibility.
- **v2 (current):** the serialized `SceneRecipe`, layer list, camera-rig JSON, terrain parameters, colormap/preset, dataset references, job/display settings. Byte-stable round-trips via canonical JSON.
- **v3 (U1):** the **document** becomes the primary model — an ordered `layers` array (id, parent, kind, name, visible, opacity, locked, source ref, style, params), plus environment and output. `SceneRecipe` is *compiled from* it (`scene.compile` bridge op), never hand-edited, so the engine stays the source of truth for recipe semantics.
- **Migration:** v1 → v2 → v3 on load, in memory, with a warning listing what was wrapped (never a silent rewrite); saving writes v3. v3 files are rejected by older builds (existing version gate).
- Datasets remain **references with content hashes**, never embedded pixels; missing files produce validation warnings.

---

## Part VII — Milestones

Each milestone ends with: build green, `cargo fmt --check`, `clippy -D warnings`, tests green (including new parity/UI tests), the binary launching, docs updated, committed to the fork. **No milestone may leave the app unable to render a terrain frame.**

### UI refactor (this sequence first)

| # | Milestone | Contents | Exit criteria (testable) | Size |
|---|---|---|---|---|
| **U0** | **Shell, tokens, actions, docks** | Module split; `theme.rs` tokens + dark/light; icon set extended to ~35 glyphs; `Action` registry with the full menu tree, toolbar, shortcuts window, command palette; four dock slots with tab groups, layout presets, persisted layout; new status bar | Every `Action` has exactly one handler and one shortcut (tests); every menu path resolves to an `Action`; no unreachable action; layout round-trips through config; panels render headlessly without panic; existing engine behaviour unchanged (all parity tests still green) | M |
| **U1** | **Document v3 + Layers panel** *(landed)* | `studio-project::document` (ordered layer tree, groups, visibility/opacity/lock, 10 kinds), v2→v3 migration on load, snapshot undo/redo, panel with painted rows, drag reorder (before/after/into-group), collapse, eye/lock/opacity, inline rename, context menu, multi-select, Edit/Layer/Scene menus, selection-driven Inspector header, project v3 persistence | **Landed:** reorder/group/ungroup/duplicate/undo/migration unit tests (30 in `studio-project`, 33 in the app), a 100-layer document lays out headlessly, layer order and ids are stable across save/load. **Deferred to U2:** `scene.compile` (document → `SceneRecipe`) lands with the per-kind editors, because compiling a layer Studio cannot yet author or render would be a promise the UI cannot keep. | L |
| **U2** | **Inspector per kind** | Selection-driven inspector; M4 render pages re-homed under `Render`; raster/vector/buildings/labels style editors; inline engine validation everywhere | Each kind's editor produces a document that compiles to the recipe its example builds (parity per kind); invalid values show the engine's message and never reach a render | L |
| **U3** | **Viewport tools + 3D gizmo** | Tool group, zoom/pan/fit/presets, gizmo (6 snap zones + orbit drag + reset), HUD (zoom, coords, scale bar, safe frame), compare mode, selection highlight | Gizmo maths unit-tested (projection + snap inverse); one render per gesture end (debounce test); keyboard presets change the camera exactly once | M |
| **U4** | **Examples gallery** | `studio/examples/index.json` over all 51 examples, category chips, thumbnails, tiers, one-click open, `Help ▸ Examples gallery` | Index contract tests pass; two Tier-A scenes (`mapscene_terrain_raster`, `terrain_demo`) open from the gallery and render through the document path matching their existing parity goldens | M |

### Engine-facing milestones (after the shell)

| # | Milestone | Contents | Size |
|---|---|---|---|
| M5 | **Certificates & diagnostics** | W5 first ring in the new bottom dock: certificates on every terrain/MapScene render, determinism check, degradation ledger, telemetry | M |
| M6 | **Path tracing & offline** | W4 ring 1: render-mode switch, offline accumulation with convergence, denoise; parity vs PT example outputs | M |
| M7 | **Atmosphere / astro / smoke** | W4 ring 2: AETHER settings, `Time & place…`, smoke volume layers; example-backed acceptance runs | M |
| M8 | **Cartography depth** | W2 second ring: furniture + plate composer, thematic editors, SVG export, **plus the terrain scatter/analysis/detail-normal items deferred from M4** | L |
| M9 | **R1 native repair** | W8-R1: repair `TerrainScene`, restore the interactive viewer's terrain, pixel-parity harness | M |
| M10 | **R2 embedded viewport** | W8-R2: continuous GPU-shared viewport replacing CPU readback; the gizmo becomes live | L |
| M11 | **Bundles, cache, compositor** | W5 second ring: bundle UI, Anamnesis inspector, render-pass compositor | M |
| M12 | **Packaging** | W8-R6: bundled Python runtime, Linux packages, then Windows/macOS lanes with real device verification | L |
| M13 | **Advanced extras** | SDF editor, splat-fused mode, inverse PT, precision demos — after their examples are exercised | L |

M9/M10 can be pulled earlier if the readback viewport becomes the bottleneck for the U3 gizmo; the gizmo is designed to work either way.

---

## Part VIII — Testing & quality standards

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green in `studio/` before any commit. Baseline today: 26 passed / 1 ignored (device-lane golden).
- **Every UI feature lands with:** the action(s), the panel wiring, at least one test (unit, headless UI, or parity), and a README status line.
- **UI tests we can run headlessly** (no display in CI): panel composition and layout via `egui::Context::run`; action registry integrity (handlers, shortcuts, reachability); document mutations (reorder/group/undo/migrate); gizmo maths; gallery index contract.
- **No dead controls.** An action either works or is absent; a disabled control must have a predicate that the hover text explains.
- **No silent degradation.** Engine degradations, missing optional features and unsupported formats surface with the engine's own text.
- **No feature is marked implemented** in this document or the README until its harness example's workflow runs through Studio.
- Engine changes stay minimal, additive, fork-documented; anything reusable upstream is a candidate for a focused PR **after** it is proven in Studio.
- **Known-failure ledger:** `studio/KNOWN_ISSUES.md` records pre-existing reds (viewer-path drift, device-dependent goldens) so they are never mistaken for regressions.

---

## Part IX — Definition of success

A user opens Forge3D Studio and gets a professional workbench: a menu bar and docked panels that stay out of the way; a layers panel where every object they add — terrain, raster, vector, buildings, labels, point cloud, mesh, furniture — is a row they can select, reorder, hide, group, style and undo; an inspector that follows that selection and shows the engine's own vocabulary and validation; a viewport they can orbit with the mouse or with a Blender-style gizmo in the corner; a bottom dock with the timeline, job log, certificates and statistics; and an examples gallery that opens any of the repository's 51 example workflows as a ready-to-edit scene — with the tiers stating honestly what still needs a milestone.

And every one of those capabilities corresponds to an `examples/` workflow that Studio reproduces, with tests proving the parity.
