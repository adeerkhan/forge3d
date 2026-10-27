# Forge3D Studio

A cross-platform desktop application (Rust + egui/eframe) for the
[Forge3D](https://github.com/milos-agathon/forge3d) rendering engine, built in
this fork under `studio/`. See [`../GUI.md`](../GUI.md) for the product
specification and [Status](#status) for where the implementation stands.

**This is fork-only work**: the GUI lives here and is not intended for
upstream pull requests. The only intentional engine change is
`TerrainScene::clear_viewer_terrain` (`src/terrain/renderer/viewer.rs`), a
small additive method kept documented for upstream symmetry.

## Architecture

```text
studio/
├── crates/
│   ├── forge3d-studio/    eframe/egui desktop app (bin: forge3d-studio)
│   ├── studio-engine/     engine integration worker (embedded Python)
│   └── studio-project/    versioned .forge3d project format (serde JSON)
└── assets/icons/          Lucide SVG icons (ISC license, see LICENSE.md)
```

- **UI** (`forge3d-studio`): menu bar + toolbar, scene tree (left), engine
  viewport (center, drag to orbit / scroll to zoom), properties (right:
  camera, sun, terrain parameters, camera rig, export), status bar (adapter,
  frame time, errors).
- **Engine worker** (`studio-engine`): a dedicated thread embeds CPython and
  drives forge3d's certified, golden-tested terrain pipeline:
  `Session(window=False)` → `TerrainRenderer.render_terrain_pbr_pom` →
  `Frame.save`. One persistent session; renders ship RGBA back to the UI
  through a command/event channel, so the GUI never blocks.
- **Projects** (`studio-project`): versioned JSON documents (`*.forge3d`)
  storing dataset references (paths, never pixel data), camera, sun, and
  export settings. Future versions are rejected; missing datasets surface as
  warnings on open.

### Why the engine is driven through embedded Python

The binary links libpython regardless (forge3d depends on pyo3 even as a
pure-Rust library), so embedding costs nothing extra. The maintained,
upstream-verified terrain renderer is the Python-driven
`render_terrain_pbr_pom` path — every recipe golden and determinism test
exercises exactly it. The pure-Rust `TerrainScene` viewer path has drifted
from the live pipeline (bind-group layout grew to 23 entries, depth
attachment, camera plumbing) and cannot render correctly today. Repairing
that drift for a pure-Rust in-process viewport is GUI.md Milestone 5.

The embedded interpreter uses the engine checkout's `.venv` (forge3d +
numpy must be importable — see below). It never runs arbitrary user Python;
the worker executes a fixed bridge script (`BRIDGE_SOURCE` in
`crates/studio-engine/src/lib.rs`).

## Building and running

Prerequisites (Fedora):

- Rust toolchain
- The engine checkout's `.venv` with `forge3d` + `numpy` installed
  (`maturin develop` inside the repo creates it), **including** the Python
  development headers (uv-managed Pythons ship them)
- A GPU adapter (Vulkan; software fallbacks work but render slowly)

```bash
cd studio
cargo run --release -p forge3d-studio
```

The Python environment is auto-discovered from `../.venv` (resolved through
`pyvenv.cfg`). To point elsewhere:

```bash
export FORGE3D_PYTHON_HOME=/path/to/python-base-prefix
export FORGE3D_PYTHON_SITE=/path/to/site-packages
```

Tests (the integration test performs a real DEM load + render + PNG export
and requires the GPU + Python environment):

```bash
cargo test            # from studio/
```

DEM fixtures: `assets/tif/*.tif` in the engine checkout are git-LFS
pointers; run `git lfs pull --include="assets/tif/Mount_Fuji_30m.tif"`
before loading them.

## Dependency strategy

- `forge3d` is consumed as a path dependency of the parent checkout
  (`../..`), pinned by the checkout itself. Updates are deliberate: pull
  upstream, rebuild, re-run the studio test suite.
- `eframe`/`egui` are pinned to **0.27** because their wgpu renderer feature
  uses wgpu 0.19 — the exact wgpu version forge3d builds against. One wgpu
  build in the tree; a future GPU-side viewport sharing becomes possible
  without a version bump.
- `studio-engine` no longer links forge3d's Rust crate; it talks to the
  engine through the embedded interpreter. The Rust forge3d dependency
  remains declared in the workspace for Milestone 5 work.
- libpython is linked explicitly in each crate's `build.rs` (via
  `pyo3-build-config`), because forge3d's `extension-module` configuration
  assumes the wheel's host interpreter provides it.

## Status

Milestones from GUI.md section 12:

- [x] **1. Audit** — engine API audited (see conversation/report): public
      Rust surface, viewer + IPC, offscreen paths, feature gates.
- [x] **2. Desktop shell** — egui/eframe window, panels, Lucide icon set,
      dark/light theme, project persistence, status bar.
- [x] **3. Real engine integration** — DEM load → persistent
      `TerrainRenderer` session → real PBR+POM renders; automated
      integration test covers load → render → export on real GPU.
- [x] **4. First end-to-end workflow** — import DEM, orbit/zoom, sun
      sliders, viewport render, export PNG at chosen size, save/reopen
      project.
- [x] **M1. Bridge v2 (W0)** — generic `studio_dispatch` JSON-op dispatcher
      in the embedded interpreter; `Command::Call`/`Event::JobDone/JobFailed`
      job protocol; terrain load/render migrated onto it. Ops: `probe`,
      `terrain.load/info/render/clear`, `terrain.params_schema/params_
      validate`, `mapscene.templates/template/render/validate`,
      `colormaps.list`, `presets.list`, `datasets.list`, `dataset.inspect`,
      `presets.get`, `rig.bake`.
- [x] **M2. MapScene in Studio (W2 ring 1)** — scene-mode switch (Terrain ⇄
      MapScene); recipe templates built with the canonical example call
      sequences (`terrain`, `terrain_raster`, `terrain_vector_labels`);
      recipe validation and rendering through `MapScene.render` (the
      certified `gpu_terrain` path); recipe camera sliders; project format
      v2 stores the serialized `SceneRecipe` verbatim.
- [x] **U2. Inspector per kind + `scene.compile`** — the document is the
      recipe source: `scene.compile` (bridge op) compiles the layer tree
      into the canonical `SceneRecipe` with the example-identical
      construction (terrain/raster/vector/labels; groups recurse; unrenderable
      kinds are excluded with an honest `layer_unrenderable` diagnostic; a
      missing terrain is a hard `no_terrain` error). Templates materialize
      document rows; renders always compile first; the inspector edits
      per-kind style (vector stroke, label text) through undoable
      `SetStyle` commands feeding the next compile. Parity-locked by
      `scene_compile_document_semantics`.
- [x] **W1 ring: colormap picker** — terrain rendering accepts any
      registered forge3d colormap (engine registry via `colormaps.list`);
      project v2 persists the selection.
- [x] **W5 ring: render certificates** — MapScene renders can request a
      signed RenderCertificate (`certificate_payload_sha256` shown in the
      scene panel).
- [x] **M3 ring: unified import (W3 ingest)** — `dataset.inspect` bridge op
      previews honest per-format metadata (GeoTIFF/COG, NPY, F3DZ, LAZ/
      COPC/EPT, 3D Tiles, OBJ/glTF, GeoJSON-family vectors; STL reported
      unsupported — export-only in the engine). Only DEM-like sources claim
      the terrain slot (`terrain_loadable: true`); everything else
      registers as a project dataset reference (path + kind + label, never
      pixel data). `terrain.load` now accepts `.npy` and `.f3dz`/`.f3z`
      directly. Parity: `parity_ingest.rs`.
- [x] **W1 depth: presets dropdown + camera rigs + timeline** —
      `presets.get` returns full preset payloads; the terrain sun maps the
      preset's first directional light (az/el, intensity ×0.5 clamped
      0–10), MapScene mode patches the certified
      `LightingPreset(name=…)` recipe path. `rig.bake` bakes orbit/rail/
      follow rigs from a loaded DEM (scatter-source units rescaled to
      renderer units), the camera panel scrubs/plays the keyframe
      timeline, and "render sequence" exports `frame_%04d.png` frames
      (center-locked replay; target-carrying playback is M4/M10).
      Parity: `parity_ingest.rs`.
- [x] **U1: document v3 + Layers panel** — the scene is now an ordered layer
      tree (`studio-project::document`): groups, visibility/opacity/lock,
      ten layer kinds, stable ids, contiguous subtrees, and snapshot
      undo/redo (one step per gesture). The Layers panel paints its rows
      instead of stacking widgets: drag to reorder (before/after/into a
      group), disclosure to collapse, eye/lock hit targets, inline rename,
      a context menu, multi-select with shift ranges, and an opacity control
      that belongs to the selection. `Edit`, `Layer` and `Scene` menus drive
      the same operations through the action registry; the Inspector follows
      the selection. v2 projects migrate on load: terrain, imported datasets
      and serialized recipe layers become rows, and saving writes v3.
      Capability honesty is built in — only the terrain and groups render
      today, and a registered-only layer says so in its row tooltip, the
      Inspector and the status bar. `scene.compile` (document → `SceneRecipe`)
      is scheduled with U2's per-kind editors.
- [x] **U4 core: examples gallery** — `studio/examples/index.json` indexes
      every one of the repository's 50 `examples/*.py` scripts with a tier
      (**A** runs today, **B/C** name the milestone that unlocks them), and
      `examples_gallery.rs` renders every Tier-A scene through the same bridge
      the app uses, writing the gallery's thumbnails to
      `studio/target/example-output/`. Open `Help ▸ Examples gallery` in the
      app: Tier-A cards are clickable and apply their scene (DEM, camera, sun,
      render params, rig or MapScene template) with one click; B/C cards state
      why they are not yet runnable instead of pretending. Two bugs this work
      uncovered are fixed on the way: the viewport's colormap overlay was
      missing the terrain's height **domain** (every frame rendered washed-out
      grey — now locked by a chroma gate), and the camera-rig default orbit
      radius put the eye outside the DEM bounds, which the engine correctly
      rejects. Parity: `examples_gallery.rs`.
- [x] **M4 core: full `TerrainRenderParams` property pages** —
      `terrain.params_schema` introspects `make_terrain_params_config`
      and every nested settings dataclass (21 root params, 26 settings
      sections, engine enum choices, the effective defaults a render
      really uses), so a new engine parameter appears without a Rust
      change. The right panel renders them as collapsible pages with a
      widget per field type; an edit updates the project and schedules a
      debounced `terrain.params_validate`, which runs the engine's own
      constructors and whose message is shown inline — the renderer only
      ever receives params the engine accepted, so a half-typed value can
      never break the viewport. Params the viewport pipeline pins are
      listed under "Not exposed" with their reasons. Project v2 persists
      the params object. Parity: `parity_ingest.rs`
      (`terrain_params_schema_covers_the_engine_demo_surface` classifies
      every `examples/terrain_demo.py` flag, `terrain_params_validate_
      reports_the_engines_own_errors`, `terrain_params_defaults_and_
      enum_choices_validate`); integration: `render_params_reach_the_
      renderer` (the empty default reproduces the pre-M4 frame exactly).
- [x] **Part IV harness** — `tests/harness_registry.json` + parity tests
      (`parity_mapscene.rs`, `parity_ingest.rs`): every template validates
      and renders through the engine; catalog ops asserted against the
      engine registries; ingest metadata + rig bakes + preset payloads
      asserted against the engine's own Python API;
      device-lane-dependent golden gate behind `FORGE3D_STUDIO_GOLDEN_STRICT`.
- [x] **U3. Viewport tools + gizmo + HUD** — Blender-style navigation gizmo
      (six axis handles + isometric ball, click-snap + drag-orbit, maths
      unit-tested), tool group (orbit/pan/zoom with middle-drag-pan and
      wheel-zoom always), camera-target pan through the engine's
      `cam_target`, HUD (zoom %, approximate target-plane scale bar, frame
      time), safe-frame overlay, A/B compare against the last export;
      camera presets now share the gizmo mapping (fixing an inverted
      top/front convention).
- [x] **Gallery robustness** — example scenes resolve repo-relative dataset
      paths against the checkout (not the process CWD); unresolved git-LFS
      fixtures are materialized with a disclosed `git lfs pull` background
      fetch, and missing fixtures surface the exact remedy instead of a
      raw engine error.
- [ ] **5. Embedded viewport** — current viewport is a CPU readback of the
      engine's render (~100–300 ms/frame, on-demand, not continuous).
      True embedding = repairing the native `TerrainScene` drift or sharing
      wgpu devices with eframe.
- [ ] **6. Packaging** — Linux-first; Windows/macOS untested (libpython
      embedding and window setup need per-platform work).

### Embedded-interpreter notes

- `sys.executable` inside the embedded interpreter is repaired to the venv
  Python at bridge load: forge3d's terrain runtime gate proves the runtime
  by spawning `sys.executable` as a subprocess, and an embedded interpreter
  otherwise reports the interpreter pyo3 linked against (which does not own
  the forge3d environment). This is Studio-side and needs no engine change.
- MapScene layers must carry a CRS (the engine's `missing_crs` validation is
  a hard error by design); Studio templates propagate the terrain CRS from
  `io.load_dem` into every layer and never invent one.

### Known limitations

- Only the terrain layer and groups render today; imported datasets are
  registered as reference layers, and the rest of the layer kinds appear in
  `Scene ▸ Add layer` with their renderers (U2/U3).
- The gallery's Tier-A set is five scenes; the other 45 examples are listed
  with their unlock milestone (U2/U3/M5-M11) rather than rendered.
- The layer document does not yet compile to a `SceneRecipe`
  (`scene.compile`), so MapScene still builds its recipe from a template in
  the Scene tab.
- Viewport renders on demand (camera/sun changes), not continuously.
- One terrain dataset per scene; no mesh/point-cloud layers yet.
- No cancel support for in-flight renders (GUI.md 8: they are never
  reported as successful cancellations — the UI simply waits).
- The two P6.1 color-space toggles (`colormap_srgb`, `output_srgb_eotf`) are
  `TerrainRenderParams` fields that `make_terrain_params_config` does not
  accept (the demo assigns them after construction), so the parameter pages
  cannot set them yet; they are recorded as a coverage gap in the parity
  table rather than silently missing.
- Parameter edits apply on the debounce (`PARAMS_VALIDATE_DEBOUNCE`), not
  while typing: a value the engine rejects never reaches the renderer, so the
  viewport keeps showing the last valid frame until the edit is fixed.
- Windows/macOS are unverified; the libpython link step in `build.rs`
  handles Linux only so far.
- The embedded interpreter requires the dev `.venv` today; a packaged
  runtime bundle is Milestone 6 work.
