# Forge3D Studio — Master Plan
## A Photoshop × QGIS × Blender-class desktop application for the Forge3D engine

**Repository:** this fork (`adeerkhan/forge3d`), directory `studio/` — fork-only work, not intended for upstream pull requests. Pushes go to `fork` (`adeerkhan/forge3d`); `origin` stays the upstream engine and is fetch-only.
**Status header last updated:** 2026-10-29 (examples open again — the `TerrainLoaded` hand-off was dropped; download carousel; Blender-shaped shell with properties icon strip and navigation gizmo) — third pass (**UI runtime replaced**: the hand-rolled egui chrome is gone; the app is built on **GPUI Kit** — G0–G7 and the pure contracts of G8 landed, see Part X. The interactive viewport renders on the engine's own device and crosses into the UI as pixels, because GPUI's wgpu and forge3d's wgpu are different majors. Viewport cadence is now gated by an in-flight render flag, draws at half resolution during a drag and stops entirely when idle.) Earlier status: U0–U4, M5, M6, M7, M9, M10; `scene.compile` makes the document the recipe source; render mode switches between raster accumulation and hybrid-ReSTIR path tracing with live progress; atmosphere/astro/smoke land as engine-composed surfaces with `parity_atmosphere.rs` green.
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

- **Language:** Rust UI on **GPUI Kit 0.8** (GPUI + `gpui-base` + `gpui-component`: dock area, tree, table, inputs, menus, command palette, status bar) + an embedded-Python engine worker. The previous egui/eframe shell is deleted, not kept as a second path.
- **Integration:** the worker thread embeds CPython (the engine checkout's `.venv`) and drives forge3d's certified, golden-tested Python entry points through one JSON dispatcher (`studio_dispatch`, 37 ops). Every upstream golden (`mapscene_*`, determinism, certificates) exercises exactly those entry points, so Studio renders are engine renders by construction.
- **Why the engine is driven through embedded Python:** the certified, golden-tested `render_terrain_pbr_pom` path is what every recipe golden, certificate and determinism test exercises, so MapScene, exports and certificates go through it. The pure-Rust `TerrainScene` viewer path had drifted from the live pipeline; M9 repaired it (Z-up mesh mode, current bind groups, colour target) and it paints the interactive viewport on the engine's own device.
- **The wgpu boundary (decided, not provisional):** GPUI renders through **wgpu 30**; forge3d is built against **wgpu 0.19**. Two wgpu majors cannot share a `Device` or a `Texture`, so the M10 *zero-copy* texture hand-off is impossible under GPUI. The viewport therefore renders on the engine's negotiated device, reads the frame back once per requested frame, encodes linear → sRGB once, and hands GPUI an image. That costs one readback per gesture end (the viewport already renders on demand, not at a free-running frame rate), and it keeps the engine untouched. Revisit only if GPUI exposes a shared-device surface.
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

- **Bridge v2 (M1):** `studio_dispatch` JSON-op dispatcher with a job protocol (`Command::Call` → `Event::JobDone`/`JobFailed`, `Busy`, `Error`). Ops: `probe`, `terrain.load/info/render/clear`, `terrain.params_schema/params_validate`, `mapscene.templates/template/render/validate/furniture/export_svg`, `raster.classes`, `mapscene.pick`, `colormaps.list`, `presets.list/get`, `datasets.list`, `dataset.inspect`, `rig.bake`.
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

- **Four dock slots** (left, right, bottom, center), each a **tab group**, now provided by the kit's `DockArea`: resizable splits, draggable tabs, tab tearing into a group, collapsible docks, panel zoom, and a serializable layout (`DockAreaState`). The hand-rolled `dock.rs` from U0 is deleted; layout persistence writes the dock state, not a bespoke struct.
- **Layout presets** (`View ▸ Panel layout`): *Default*, *Cartography* (wide layers + legend), *Modeling* (wide viewport, collapsed left dock), *Data* (tall bottom console). Reset layout restores Default.
- **Every slot can collapse** to a 0-width rail with its tab icon, Photoshop-style.
- **Persistence:** the dock state plus theme in the platform config dir (`$XDG_CONFIG_HOME/forge3d-studio/`), never in the project file (a project must open identically on another machine). G4 lands the load/save (versioned: a stale layout is ignored, not restored badly).

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

- **Central viewport** shows the engine's frame (terrain mode paints on the engine's own wgpu device through a shared texture; MapScene mode uses the bridge's certified frame). Drag = orbit, middle-drag or Space-drag = pan, wheel = zoom, `F` = fit. Renders are debounced, so a drag shows the last frame and renders on gesture end. In a quality mode (path traced, or raster with offline accumulation) the viewport holds a **still** instead: camera motion only marks it stale, and the image comes from the bridge's offline/path-traced render on mode change, terrain load, a parameter change, resize, `Ctrl+Shift+Q` or `Ctrl+R`, with the badge and status chip naming the mode.
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

- Semantic tokens come from the kit's `Theme` (`ActiveTheme::theme().colors.*`: background, surface, foreground, muted_foreground, border, primary, accent, danger, radius, spacing, typography). The Studio look is expressed as a choice among those tokens plus density overrides — no parallel colour table. Dark and light are the kit's `ThemeMode`.
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
├── forge3d-studio/             the GPUI Kit application (bin: forge3d-studio)
│   └── src/
│       ├── main.rs             application(), kit init, window, argv
│       ├── state.rs            Studio entity: document, selection, both workers,
│       │                       project I/O, camera/sun, coalesced frame requests
│       ├── actions.rs          ONE action table → menus + keybindings + shortcuts
│       └── ui/
│           ├── mod.rs          PanelHost (one dockable panel type) + dock layout
│           ├── shell.rs        title bar, menu bar, status bar, dialogs
│           ├── panels.rs       Layers, Inspector, Console, Statistics
│           └── viewport.rs     frame image, tools, HUD, orbit/pan/zoom
├── studio-engine/              bridge worker (unchanged role) +
│                               native_viewport.rs (engine device → RGBA)
└── studio-project/             project format + document model (unchanged)
```

### 3.12 UI runtime (GPUI Kit)

The UI is GPUI Kit, not egui. The consequences are visible in the architecture:

- **One panel type.** `ui::PanelHost` implements the dock's `Panel`/`BasePanel`,
  `Focusable`, `EventEmitter<PanelEvent>` and `Render` once, and takes a closure
  `(&mut Window, &mut App) -> AnyElement`. Studio panels are *functions of
  state*: they read the `Studio` entity and dispatch actions for edits, so the
  dock does not need six near-identical panel types and no panel keeps private
  copies of project data.
- **One action table.** `studio_action_table!` expands the same list into
  (a) the menu tree, (b) `cx.bind_keys` bindings, (c) the shortcuts window and
  (d) the app-level handlers via the `StudioAction` trait. An action in the
  table without `run` does not compile; a duplicate shortcut fails a test.
- **Dock from the kit.** Resizable splits, draggable tabs, tear-off groups,
  collapsible rails and serializable layout come from `gpui_component::dock`
  instead of the ~290 hand-rolled lines it replaces.
- **Frame requests are coalesced.** Pointer moves only mark the view dirty; the
  state pump (20 Hz) issues at most one frame per tick, so a fast orbit cannot
  queue a render per mouse event. A drag does not talk to the bridge at all:
  the bridge is synced when the gesture ends (`end_gesture`), so exports render
  what the viewport showed.
- **The frame is an image.** See the wgpu boundary note in Part I: the engine
  device renders, the UI presents bytes. `FORGE3D_STUDIO_DUMP_FRAME=<path>`
  writes the presented frame to PNG, which is how the viewport is compared
  against engine exports in review.

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
| Path-traced renders (hybrid ReSTIR), offline accumulation *(landed 2026-10-28: `Render ▸ Render mode ▸` Raster / Path traced, persisted per project; raster offline uses `render_offline` with samples + denoiser, PT uses `render_terrain_poster`; progress streams from the engine into the status bar/Statistics)* | `Render ▸ Render mode` + quality window in the status bar/Statistics | `offline.py`, `southeast_europe_population_pt_native.py`, `swiss_landcover_pt_oblique.py` |
| Denoising (A-trous, OIDN when present) *(landed 2026-10-28: the denoiser list comes from `offline.capabilities`; an unavailable denoiser is refused with the engine's own message instead of being downgraded)* | Quality window in the status bar/Statistics | `denoise.py`, `denoise_oidn.py` |
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
| Render certificates on every render *(landed 2026-10-27: terrain + MapScene render to path, dev-key verification op, tamper rejection, Certificates dock tab)* | Bottom ▸ Certificates + status chip | `certificate.py`, `provenance_demo.py` |
| Per-pixel provenance overlay | Viewport layer toggle | `provenance.py` |
| Determinism check *(landed 2026-10-27: Tools menu — two-run SHA-256 comparison of the live frame, in-process; backend pinning via env)* | `Tools ▸ Determinism check…` + Statistics | `determinism.py` |
| Degradation ledger + memory telemetry *(landed 2026-10-27: `diagnostics.snapshot` op + Statistics dock tab)* | Statistics — never silently degrade | `_degradation.py`, `diagnostics.py` |
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
| Selection-driven Inspector per layer kind; M4 parameter pages re-homed; **`scene.compile` (document → SceneRecipe) landed** — templates materialize document rows, renders compile from the document, per-kind editors (raster/vector/labels) patch layer style, unrenderable kinds surface honest compile diagnostics | U2 *(landed 2026-10-27, parity-locked)* |
| Viewport tools, 3D navigation gizmo, HUD (zoom, scale bar, safe frame), compare mode *(landed 2026-10-27 — gizmo maths unit-tested, presets+gizmo share one mapping, inverted camera-preset convention fixed; cursor world-coords and geodesic measure arrive with engine pick/measure ops)* | U3 *(landed)* |
| Bottom dock: timeline, console, certificates, statistics | U0 (tabs) / M5 *(content landed 2026-10-27)* |
| Examples gallery with tiers and one-click scenes | U4 |

### W10 — Examples port

The gallery (3.7) plus the index contract — **landed as a core**: all 50 examples are indexed with a tier, `examples_gallery.rs` renders every Tier-A scene through the bridge (and produces the gallery's thumbnails), and B/C entries name their milestone instead of pretending. Remaining: one-click open through the *document* path once `scene.compile` lands in U2, and thumbnails for the not-yet-runnable cards.

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
| **U4** | **Examples gallery** *(core landed)* | `studio/examples/index.json` over all 50 examples, category chips, tiers, one-click open, `Help ▸ Examples gallery`; harness renders every Tier-A scene and writes the cards' thumbnails | **Landed:** index contract tests + `examples_gallery.rs` render all 5 Tier-A scenes through the bridge (866 KB–1 MB PNGs, chroma-locked). **Remaining:** thumbnails for cards whose examples are not yet Tier A, and one-click open for the MapScene document path once `scene.compile` lands (U2). | M |

### Engine-facing milestones (after the shell)

| # | Milestone | Contents | Size |
|---|---|---|---|
| M5 | **Certificates & diagnostics** | W5 first ring in the new bottom dock: certificates on every terrain/MapScene render, determinism check, degradation ledger, telemetry | M |
| M6 | **Path tracing & offline** *(landed)* | W4 ring 1: `Render ▸ Render mode` (Raster / Path traced, persisted in the project as format v3), raster offline accumulation through `render_offline` (samples, denoiser, convergence + `samples_used`/`final_p95_delta` reported), hybrid-ReSTIR stills through `render_terrain_poster` (spp/frames/variance threshold/tile/seed/env intensity), engine-streamed progress in the status bar, denoiser availability from `offline.capabilities`. Quality modes are explicit stills: `native_paints()` is false, camera motion only marks the still stale. Gate: `parity_quality.rs` (capabilities honesty, accumulation + denoise provenance, PT convergence/repeat/seed/certificate) | M |
| M7 | **Atmosphere / astro / smoke** *(landed)* | W4 ring 2: AETHER settings (`AtmosphereSettings`, LUT bake job, env HDR + IBL), `Time & place…` (SIDERA Sun/Moon, compass→renderer azimuth `A − 90`), smoke volume layers (`.npy`/`.npz`, plan-view plate composited over the frame); format v4 (`atmosphere`/`observation`/`smoke`); project layer + bridge (31 ops) + worker + app; gate `parity_atmosphere.rs` (LUT provenance, env changes pixels + sha, HDR RLE structure, SIDERA angles, smoke composite + failed-load clearance) | M |
| M8 | **Cartography depth** | W2 second ring: furniture + plate composer, thematic editors, SVG export, **plus the terrain scatter/analysis/detail-normal items deferred from M4** | L |
| M9 | **R1 native repair** *(landed)* | W8-R1: `TerrainScene::render_viewer_terrain` repaints correctly — Z-up mesh mode, cached per-terrain `ViewerResources`, Z-up sun direction matching `decode_lighting`, colormap LUTs in the engine's ramp encoding, world-span fitting; gate tests in `native_viewport.rs`/`native_viewer_path.rs` prove frames are byte-reproducible, terrain actually shades, and grazing light differs from overhead | M |
| M10 | **R2 embedded viewport** *(landed)* | W8-R2: `studio_engine::NativeViewport` renders into an offscreen `Rgba8Unorm` target on eframe's device and registers its sRGB view with egui, so an orbit costs one render pass and no readback; the scene builds on a big-stack worker thread; colormap edits push into the live scene; MapScene keeps the bridge frame. Rendering stays on demand (a gesture end or a committed edit), not a free-running animation loop | L |
| M11 | **Bundles, cache, compositor** | W5 second ring: bundle UI, Anamnesis inspector, render-pass compositor | M |
| M12 | **Packaging** | W8-R6: bundled Python runtime, Linux packages, then Windows/macOS lanes with real device verification | L |
| M13 | **Advanced extras** | SDF editor, splat-fused mode, inverse PT, precision demos — after their examples are exercised | L |

M9/M10 landed ahead of M6–M8 because the CPU readback was already the bottleneck for the U3 gizmo.

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

---

## Part X — UI runtime migration (GPUI Kit)

The U0 shell proved the engine wiring on egui; the interaction model it needed
(dockable tabs, virtualised lists, a command palette, typed actions, testable
components) is what GPUI Kit already is. This workstream replaces the UI runtime
and keeps everything else: bridge ops, project format, gallery index, parity
harnesses, certificates.

**Non-negotiable invariants**

1. The engine is untouched. The only fork-side engine code stays
   `TerrainScene::clear_viewer_terrain` and the M9 viewer repair.
2. Every bridge op, project field and parity test survives the port.
3. No dead UI. A control either works or is absent; a not-yet-ported surface
   says so in place (e.g. the Inspector states that the schema-driven parameter
   pages are G3), instead of showing disabled stubs.
4. A milestone may not leave the app unable to load a DEM and paint a frame.

| # | Milestone | Contents | Exit criteria | Status |
|---|---|---|---|---|
| **G0** | **Runtime proof** | `gpui-kit 0.8` builds and runs on the dev machine (Wayland); the engine device renders a DEM → RGBA → `RenderImage`; a window paints it | frame dumped from the running app matches the engine's own palette and framing | **landed** |
| **G1** | **Shell** | title bar + kit `AppMenuBar`, `DockArea` with left/right/bottom/center groups, kit `StatusBar`, dark/light via `ThemeMode`, dialogs | window opens, dock resizes/tabs/drag, status bar reports adapter/backend/frame ms | **landed** |
| **G2** | **State + actions** | `Studio` entity owning project, selection, history, both workers; action table → menus/keybindings/handlers; project new/open/save, DEM import, PNG export, undo/redo, layer duplicate/delete/visibility, camera presets, theme | actions reachable from menu, shortcut and panel; `cargo test` green | **landed** |
| **G3** | **Panels v1** | Layers (kind icons, selection, visibility), Inspector (selection header, terrain facts, colormap chips from the engine registry, camera/sun steppers, honest params note), Console, Statistics, viewport tools + HUD + orbit/pan/zoom | layering a DEM and orbiting it works end to end; the inspector drives the frame | **landed** |
| **G4** | **Params pages** | the M4 schema-driven `TerrainRenderParams` pages rebuilt on kit inputs (checkbox / enum dropdown / parsed text), inline engine validation, debounced `terrain.params_validate`; dock layout persistence | every field of `terrain.params_schema` renders, commits only complete values, and the engine's rejection is what is shown; layout survives restart | **landed** |
| **G5** | **Layers v2** | tree rows (disclosure, icon, name, opacity, lock, eye), range/toggle selection, inline rename, live drag reorder, context menu, groups, selection opacity | U1 behaviour with `scene`'s document commands | **landed** (rows not virtualised) |
| **G6** | **Timeline, certificates, quality** | camera-rig bake/scrub/play/sequence, certificate list + verify, determinism check, render mode (raster/path-traced) with offline and PT settings, engine-streamed progress | M5/M6 surfaces back; a quality still is the bridge's own pixels | **landed** |
| **G7** | **Gallery + MapScene** | examples gallery with tiers and one-click open, MapScene mode (template build/validate/render), AETHER atmosphere, SIDERA time & place, smoke volumes | U4/M2/M7 surfaces reachable; empty/degraded states say why | **landed** (no gallery thumbnails; recipe camera editing is next) |
| **G8** | **Tests** | pure contracts: action registry (unique shortcuts, menu coverage), render-size quantization, gallery index contract, dock panel specs, layer icons/drop thirds, params parsing | those run with `cargo test` and no display | **landed (pure)** — window-level harness blocked, see below |
| **G9** | **Retire the old shell** | delete egui-era docs/modules, update `studio/README.md`, `cargo fmt`/`clippy -D warnings`/tests green, one dependency set | no eframe/egui reference remains in `studio/` | **landed** |
| **G10** | **Examples that open** | `TerrainLoaded` hands over to the pending example; per-card state; runnable filter; download carousel (asset disclosure + `git lfs pull` + resume); entries declare the bridge ops they need and the bridge answers `ops.list` | opening a card applies its camera/sun/params/rig/recipe; a card cannot claim runnable when an op or asset is missing | **landed** |
| **P6** | **Grid + grabbable rig** | depth-tested ground grid (engine depth accessor + Studio pass), camera-rig handles (pivot/ring/arm) with drag maths, `Shift+G` / `Shift+R` | grid stops at the terrain silhouette and axes cross at the target; 40 px pivot drag moves the target 1947 units; 42 unit + 3 window tests green | **landed** |
| **P5** | **Pick, thumbnails, window tests** | `mapscene.pick` (engine projection, letterbox-correct, topmost layer), 5 real tier A thumbnails + honest placeholders, library split with `gpui`-native window tests | pick agrees with rendered pixels 12/12 and answers in 17 ms; thumbnails load in the app; 3 window tests incl. an action dispatched through the real action table; GPU pin bug found and fixed | **landed** |
| **P4** | **Perceived speed + executable** | shared terrain renderer across previews, `devopt` build profile, stripped release, `--version`/`--help`, startup measured and shown, desktop entry + installer | repeat previews 1.8× faster (1.55 s → 0.85 s) with byte-identical output; frames 4.0 ms/6.2 ms/23.6 ms for devopt/release/debug; cold build 2 m 17 s, incremental 48 s release / 3 s devopt; install writes a working desktop entry | **landed** |
| **P3** | **Cartography depth** | recipe layer list (eye/remove/reorder/opacity), post-build line style, output presets, legend derived from the raster via `raster.classes`, `mapscene.pick`, SVG export via `mapscene.export_svg` | a map composed, styled and hidden in the panel re-previews with the engine's paint; styling verified by pixel diff (3.8% of pixels move, max delta 255) | **landed** |
| **G12** | **Cartography panel** | recipe template + raster overlay picker + vector line colour/width, and the map furniture the engine draws (title, legend, scale bar, north arrow, graticule) through a new `mapscene.furniture` bridge op; Apply, Apply & render, Export map… | a rendered map shows the panel's furniture at the chosen positions, the export writes a real file, and the panel reads a recipe's furniture back when a scene is built | **landed** |
| **G11** | **Blender-shaped shell** | workspace row, one editor per dock slot, properties icon strip (Layer/Terrain/Render/World/Scene/Statistics), outliner icon tabs (Layers/Examples), bottom icon strip (Timeline/Console/Certificates), viewport mode header + tool rail, navigation gizmo | the window reads as a DCC workbench: icons instead of tab piles, gizmo snaps views | **landed** (gallery thumbnails/carousel click-through still to eyeball) |

**Ordering note:** G4 before G5 because the parameter pages are the surface the
engine vocabulary actually lives in; G6 before G7 because certificates and
render modes are what make a saved still trustworthy.

### G8 blocker (recorded, not worked around)

The kit's headless harness is behind its `test-support` feature, and enabling
it cannot resolve in this workspace: `gpui-kit 0.8.1` keeps `gpui-fast 0.1.12`
in the dependency graph, which pins `taffy =0.15.0`, while `gpui-pre 0.3.10`
requires `^0.15.0` (resolved to 0.15.1). Any feature change forces a
re-resolution that this pair cannot satisfy, so window-level UI tests (real
windows, simulated pointer/keyboard) are unavailable until the kit publishes a
consistent snapshot. The pure contracts above stay in the crate so the same
facts are at least locked today.

### Viewport performance (measured, 2026-10-28)

Three bugs made orbiting unusable; all three are fixed and the numbers are in
the G-table above:

| Condition | iGPU (before) | discrete GPU (after) |
|---|---|---|
| Idle, 12 s | 32 renders | 2 renders total, then nothing |
| 180°/s orbit | queue grew, seconds of lag | ~60 fps, 3 ms half-res / 6.6 ms full-res mean |
| First full-size frame | 88–195 ms | 16.9 ms (Rainier 2111², 954×630) |
| Live switch off | not available | no renders while dragging; one frame when the gesture ends |

The viewport camera follows the engine's Z-up orbit exactly: `theta` is polar
from +Z (`0` = straight down), while the inspector and HUD show *elevation*
(90 − θ) because that is what people read. View presets (7/1/3, Ctrl+1/Ctrl+3,
0), `5` for an approximate orthographic projection (2° field of view at a
distance that keeps the framing — the viewer is perspective-only), click-to-pick
(the orbit target follows a click and the terrain row selects), hover ground
coordinates and cursor-anchored wheel zoom all derive from the same maths in
`ui/viewport_math.rs`, which is unit-tested.

The viewport has three tiers, each labelled in its header — **live** (the
engine's raster pass, instant), **preview** (a debounced ≤640 px MapScene render,
~600 ms after an edit and coalesced while one is in flight) and **render**
(F12 / Render still, full size, certificate). P0 keeps the project honest: the
recipe and its furniture are saved with the project and restored on open, and
render quality/atmosphere/smoke are pushed to the engine on open.

The viewport is a **live preview**, like Blender's viewport shading: Terrain
mode always paints on the engine's device and stays interactive. Offline
accumulation and path tracing describe the *render*, not the viewport, so
choosing them no longer freezes the preview — a still appears when you ask for
one (Render ▸ Render still, `Ctrl+R`), the header says "Render result — back to
viewport", and moving the camera returns to the live view. MapScene mode stays
a composed still by definition.

The viewport now renders on the **discrete** adapter: the engine's default
`PowerPreference::LowPower` landed on the integrated GPU while the UI
composited on the discrete one, so every frame crossed PCIe. Studio sets
`FORGE3D_GPU_POWER=high` before any engine context exists (an explicit user
value wins, and the engine default is unchanged). A DEM import also starts the
Rust reader first and paints as soon as it is in, with the embedded-Python
load following for exports/metadata — 1.4 s for the bridge versus a first
frame in about the same time, instead of waiting for Python before drawing.
Drag resolution is adaptive: half while the last frame was slower than 9 ms,
full when the adapter keeps up.

The frame-request path is now: pointer motion only marks the view dirty, a
20 Hz pump issues at most one frame when nothing is in flight, and the drag
renders at half resolution. A drag never talks to the bridge; the bridge is
synced on gesture end so exports match what the viewport showed.

### Known consequences of the runtime swap

- **Readback instead of texture sharing.** ~1 readback per requested frame;
  measured with the frame timer in Statistics. If continuous rendering ever
  lands, this becomes the first bottleneck to revisit (and the honest reason a
  shared-device path would be needed).
- **Two adapters can be chosen.** The bridge's embedded Python builds its own
  engine context and may pick a different GPU than the pure-Rust viewport
  context (`Viewport device:` vs `Engine:` in the Console). Both are stated
  truthfully; pinning them to one adapter is follow-up work on the engine's
  selection knobs, and belongs with packaging.
- **Linux link prerequisite.** GPUI needs `libxkbcommon`/`libxkbcommon-x11`
  development symlinks; Fedora's `libxkbcommon-devel` provides them. Where only
  the runtime `.so.0` files are installed, point the linker at a directory of
  symlinks (`RUSTFLAGS="-L <dir>"`) — documented in `studio/README.md`.
