# Forge3D Studio

A cross-platform desktop application (Rust + **GPUI Kit**) for the
[Forge3D](https://github.com/milos-agathon/forge3d) rendering engine, built in
this fork under `studio/`. See [`../GUI.md`](../GUI.md) for the product
specification (Part X is the UI-runtime workstream) and [Status](#status) for
where the implementation stands.

**This is fork-only work**: the GUI lives here and is not intended for
upstream pull requests. The only intentional engine change is
`TerrainScene::clear_viewer_terrain` (`src/terrain/renderer/viewer.rs`), a
small additive method kept documented for upstream symmetry.

## Architecture

```text
studio/
├── crates/
│   ├── forge3d-studio/    GPUI Kit desktop app (bin: forge3d-studio)
│   ├── studio-engine/     engine workers: bridge (embedded Python) + viewport
│   └── studio-project/    versioned .forge3d project format (serde JSON)
└── assets/icons/          Studio SVGs (the kit ships the full Lucide set)
```

Inside `forge3d-studio`:

| File | Job |
|---|---|
| `main.rs` | `gpui_kit::application()`, kit init, window, `argv[1]` (project or DEM) |
| `state.rs` | the `Studio` entity: project + document, selection, history, both workers, project I/O, camera/sun, coalesced frame requests |
| `actions.rs` | the single action table → menus, keybindings, shortcuts window, handlers |
| `ui/mod.rs` | `PanelHost` (the one dockable-panel type) and the default dock layout |
| `ui/shell.rs` | title bar + menu bar, status bar, dialogs |
| `ui/panels.rs` | Layers, Inspector, Console, Statistics |
| `ui/viewport.rs` | the engine frame, tools, HUD, orbit/pan/zoom |

- **UI** (`forge3d-studio`): kit title bar with the application menu bar, a
  dock area (left: Layers; right: Inspector; bottom: Console/Statistics;
  centre: the engine viewport with its own tool strip and HUD) and a status bar
  (status text, selection, frame time, adapter/backend). Panels are functions
  of the `Studio` entity — they read state and dispatch actions, so no panel
  keeps a copy of project data.
- **Engine worker** (`studio-engine`): a dedicated thread embeds CPython and
  drives forge3d's certified, golden-tested terrain pipeline:
  `Session(window=False)` → `TerrainRenderer.render_terrain_pbr_pom` →
  `Frame.save`. One persistent session; renders ship RGBA back to the UI
  through a command/event channel, so the GUI never blocks.
- **Viewport worker** (`studio-engine::ViewportWorker`): owns
  `forge3d::terrain::TerrainScene` on its own thread (the terrain shaders
  compile for seconds and recurse deeply, hence the 64 MB stack), renders on
  the engine's negotiated GPU context, reads the frame back as RGBA and sends
  it to the UI as pixels. It is a separate thread because a render blocks the
  device and must never block the UI thread. MapScene, exports and
  certificates stay on the bridge.
- **Projects** (`studio-project`): versioned JSON documents (`*.forge3d`)
  storing dataset references (paths, never pixel data), camera, sun, and
  export settings. Future versions are rejected; missing datasets surface as
  warnings on open.

### Why the engine is driven through embedded Python

The binary links libpython regardless (forge3d depends on pyo3 even as a
pure-Rust library), so embedding costs nothing extra. The maintained,
upstream-verified terrain renderer is the Python-driven
`render_terrain_pbr_pom` path — every recipe golden and determinism test
exercises exactly it, so MapScene, exports and certificates use it. The
pure-Rust `TerrainScene` viewer path had drifted from the live pipeline
(bind-group layout grew to 23 entries, depth attachment, camera plumbing)
and could not render correctly; M9 repaired that and M10 uses it for the
interactive frame (see Status).

The embedded interpreter uses the engine checkout's `.venv` (forge3d +
numpy must be importable — see below). It never runs arbitrary user Python;
the worker executes a fixed bridge script (`BRIDGE_SOURCE` in
`crates/studio-engine/src/lib.rs`).

## Building and running

Prerequisites (Fedora):

- Rust toolchain (edition 2021; GPUI needs a current stable)
- `libxkbcommon-devel` and `libxkbcommon-x11-devel` (GPUI links these
  directly). If only the runtime libraries are installed, the link step fails
  with `unable to find library -lxkbcommon`; create symlinks in a directory and
  point the linker at it:

  ```bash
  mkdir -p ~/.local/lib/gpui-compat
  ln -sf /usr/lib64/libxkbcommon.so.0 ~/.local/lib/gpui-compat/libxkbcommon.so
  ln -sf /usr/lib64/libxkbcommon-x11.so.0 ~/.local/lib/gpui-compat/libxkbcommon-x11.so
  export RUSTFLAGS="-L $HOME/.local/lib/gpui-compat"
  ```
- The engine checkout's `.venv` with `forge3d` + `numpy` installed
  (`maturin develop` inside the repo creates it), **including** the Python
  development headers (uv-managed Pythons ship them)
- A GPU adapter (Vulkan; software fallbacks work but render slowly)
- A Wayland or X11 session

```bash
cd studio
cargo run --release -p forge3d-studio            # empty workspace
cargo run --release -p forge3d-studio -- path/to/dem.tif     # open a DEM
cargo run --release -p forge3d-studio -- scene.forge3d       # open a project
```

`FORGE3D_STUDIO_DUMP_FRAME=/tmp/frame.png` writes the frame the viewport is
about to present, for comparing the interactive path against an engine export.

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
- `gpui-kit` **0.8** is the UI runtime. It renders through **wgpu 30** while
  forge3d is built against **wgpu 0.19**; the two majors cannot share a device,
  so the viewport crosses the boundary as pixels (see GUI.md Part I). This is a
  deliberate trade: the kit replaces the whole hand-rolled chrome (dock, tabs,
  menus, inputs, virtualised lists, command palette) and its wgpu version is not
  ours to choose.
- `studio-engine` no longer links forge3d's Rust crate; it talks to the
  engine through the embedded interpreter. The Rust forge3d dependency
  remains declared in the workspace for Milestone 5 work.
- libpython is linked explicitly in each crate's `build.rs` (via
  `pyo3-build-config`), because forge3d's `extension-module` configuration
  assumes the wheel's host interpreter provides it.

## Status

UI runtime (GUI.md Part X):

- [x] **G0 Runtime proof** — `gpui-kit 0.8` builds and runs here; the engine
      device renders a DEM and the UI presents it (frame dumped and compared
      against the engine's palette/framing).
- [x] **G1 Shell** — kit title bar + `AppMenuBar`, `DockArea` (left/right/bottom/
      centre, resizable, draggable tabs, collapsible), kit `StatusBar`,
      dark/light by `ThemeMode`, About + Shortcuts dialogs.
- [x] **G2 State + actions** — `Studio` entity with both workers and the project
      model; one action table drives menus, keys and handlers; new/open/save,
      DEM import, PNG export, undo/redo, layer duplicate/delete/visibility,
      camera presets, theme.
- [x] **G3 Panels v1** — Layers (kind icons, selection, visibility), Inspector
      (selection header, terrain facts, engine colormaps, camera/sun steppers),
      Console, Statistics, viewport tools + HUD + orbit/pan/zoom with coalesced
      frame requests.
- [ ] **G4 Params pages** — the M4 schema-driven `TerrainRenderParams` pages on
      kit inputs, inline engine validation, dock-layout persistence.
- [ ] **G5 Layers v2** — kit tree/virtual list, drag reorder, groups, rename,
      opacity, context menu, multi-select.
- [ ] **G6 Timeline, certificates, quality** — rig timeline, certificates +
      verify, determinism check, raster/path-traced stills with progress.
- [ ] **G7 Gallery + MapScene** — example cards with thumbnails, one-click open,
      MapScene mode, atmosphere/astro/smoke panels.
- [ ] **G8 Kit test harness** — headless UI tests on the kit's `TestAppContext`.
- [ ] **G9 Retire the old shell** — no egui reference left; docs and
      dependencies final.

Engine-facing milestones from GUI.md section 12:

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
- [x] **M5. Certificates & diagnostics** — every terrain render can be
      signed to a deterministic certificate JSON (`Render ▸ Request
      certificate`); MapScene certificates carry the payload SHA-256; the
      new Certificates dock tab lists session certificates with one-click
      `certificate.verify` (dev-key derivation, tamper rejection tested);
      the Statistics tab shows the degradation ledger + memory telemetry
      via `diagnostics.snapshot`; `Tools ▸ Determinism check…` renders the
      current frame twice and compares SHA-256 (parity test
      `certificate_lifecycle`).
- [x] **M6. Path tracing & offline** — `Render ▸ Render mode ▸` switches
      the still between **Raster (PBR + POM)** and **Path traced (hybrid
      ReSTIR)** and is saved with the project (format v3; older projects load
      as raster). Raster quality uses the engine's offline accumulation
      (`forge3d.render_offline`: jittered samples, AOVs when denoising, the
      reported `samples_used`/`final_p95_delta`/`converged_ratio` shown
      verbatim); path tracing drives `render_terrain_poster`
      (spp/min/max frames, variance threshold, tile size, seed, environment
      intensity) with an albedo map built from the document's colormap ramp,
      sRGB→linear, capped at 2048² with the fallback stated in the note. The
      denoiser list comes from `offline.capabilities`, and asking for a
      denoiser the engine does not have is refused with the engine's own
      message rather than silently downgraded. Both modes stream real
      progress (tiles/frames) from inside the engine through a PyO3 sink into
      the status bar. Quality modes are explicit stills: the native viewport
      stops painting, camera motion only marks the still stale, and a render
      happens on mode change, DEM load, a parameter change, resize,
      `Ctrl+Shift+Q` or `Ctrl+R`. Gate: `parity_quality.rs` checks
      capabilities honesty, accumulation/denoise provenance on a real render
      and export, and PT convergence, repeatability, seed sensitivity and
      certificate validity.

- [x] **M7. Atmosphere / astro / smoke** — atmosphere (`Scene ▸ Environment ▸
      Atmosphere…`) drives the engine's AETHER sky: LUT bake job reporting
      provenance (SHA-256, byte size, dimensions), an equirect HDR written to
      a per-engine path and bound as the IBL via `IBL.from_hdr`, rebuilt when
      the sun elevation changes; project format v4 adds `atmosphere`
      (`enabled`, `intensity`, turbidity, ozone, mie g, ground albedo,
      scattering orders). `Time & place…` asks SIDERA for the Sun and Moon
      (body position, phase, Delta T, sidereal time) and converts the compass
      azimuth to the renderer frame with `A − 90`, matching MapScene; a
      below-horizon Sun is clamped to elevation 0 with an explicit warning.
      Smoke volume layers (`.npy` density or `.npz`) load into the bridge and
      render as a plan-view plate composited over the finished terrain frame,
      reported with `certificate_covers: "terrain pass only"`; a failed load
      clears any stale volume. Project format v4 also adds `observation` and
      `smoke`. Gate: `parity_atmosphere.rs` (LUT provenance, env changes
      pixels + SHA, HDR RLE structure, SIDERA angles, smoke composite and
      failed-load clearance).

- [x] **M9. R1 native viewer repair** — the viewer used to draw the
      shader's screen mode (a 1-unit fullscreen triangle) with a Y-up
      camera, so it painted nothing that resembled terrain. `TerrainScene`'s
      viewer path now draws in mesh mode (Z-up grid, heights on +Z), builds
      its bind groups/depth/fallbacks once per terrain into a cached
      `ViewerResources` (an orbit is two uniform writes and one pass), maps
      the sun with the engine's own convention
      (`(cos el·cos az, cos el·sin az, sin el)`, azimuth 0 = +X), sets
      colormap LUTs in the same ramp encoding the export path samples, and
      fits the camera to the DEM's world span. Gate tests prove two renders
      of the same view are byte-identical, a colormap switch changes pixels,
      an unknown colormap is rejected without touching the frame, and a 5°
      sun still lights the terrain while 80° looks different (i.e. the
      light really is Z-up).
- [x] **M10. Engine-device viewport** — `studio_engine::NativeViewport`
      renders the repaired `TerrainScene` with the engine's negotiated GPU
      context, into an `Rgba8Unorm` target, and reads the frame back as RGBA
      (linear → sRGB encoded once). The scene builds — and compiles its
      shaders — on a 64 MB-stack worker thread; the UI polls its events and
      never renders on the UI thread. Colormap changes are pushed into the live
      scene, and camera/sun come from the same document values the bridge
      renders with. *History:* under eframe (wgpu 0.19) this was a zero-copy
      texture hand-off; GPUI's wgpu 30 makes that impossible, so G0 replaced it
      with the readback path. Frames are still drawn on demand, so the cost is
      one readback per gesture end.
- [x] **5. Embedded viewport** — done by M9 + M10: terrain frames are drawn
      on the engine's device and presented as an egui texture. Frames are
      still drawn on demand (gesture end, committed parameter edit, resize)
      rather than at a fixed frame rate.
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
- Viewport frames are drawn on demand (camera/sun changes, resize, committed
  edits), not at a free-running frame rate. The exception is MapScene mode,
  which renders through the bridge on request.
- A quality mode (path traced, or raster with offline accumulation) shows a
  **still**, not a live frame: the native viewport stops painting, camera
  motion only marks it stale, and the next still is a full bridge render on
  mode change, DEM load, a parameter edit, resize, `Ctrl+Shift+Q` or `Ctrl+R`.
  That is deliberate — a progressive render cannot answer a drag — but it
  means orbiting in a quality mode does not update the image until the still
  is re-requested.
- Path-traced stills take their albedo from the document's colormap ramp
  (sRGB→linear) at up to 2048² texels; a larger DEM gets the downsampled map
  and the quality window says so, rather than implying the full-resolution
  albedo was used.
- On the bundled 256² mini DEM at 128×80 the 1-, 2- and 4-sample offline
  stills are pixel-identical (the subpixel jitter rounds to the same 8-bit
  values), so `parity_quality.rs` asserts the engine's reported provenance
  (`samples_used`, `target_samples`, `denoiser_used`) instead of a pixel
  difference; larger DEMs/outputs do differ. Path-traced certificate digests
  can also differ between the first and a later still in one process, because
  the process-lifetime hybrid dummy storage buffer enters the allocation
  ledger on first use — the test asserts pixels, frame count and signature
  validity rather than digest equality.
- Terrain mode's viewport is the native `TerrainScene` path (M9/M10); its
  palette comes from the engine's colormap ramps, so a parameter the native
  path does not read (e.g. the P6.1 colour-space toggles) shows in exports
  but not in the interactive frame. MapScene mode and exports remain the
  certified comparison point.
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
