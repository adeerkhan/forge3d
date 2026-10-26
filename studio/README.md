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
  camera, sun, export), status bar (adapter, frame time, errors).
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
- [ ] **5. Embedded viewport** — current viewport is a CPU readback of the
      engine's render (~100–300 ms/frame, on-demand, not continuous).
      True embedding = repairing the native `TerrainScene` drift or sharing
      wgpu devices with eframe.
- [ ] **6. Packaging** — Linux-first; Windows/macOS untested (libpython
      embedding and window setup need per-platform work).

### Known limitations

- Viewport renders on demand (camera/sun changes), not continuously.
- One terrain dataset per scene; no mesh/point-cloud layers yet.
- No cancel support for in-flight renders (GUI.md 8: they are never
  reported as successful cancellations — the UI simply waits).
- Windows/macOS are unverified; the libpython link step in `build.rs`
  handles Linux only so far.
- The embedded interpreter requires the dev `.venv` today; a packaged
  runtime bundle is Milestone 6 work.
