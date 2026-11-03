# Forge3D Studio — Tauri shell

The desktop shell for the renewed **Forge3D Studio** product described in
[`../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md`](../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md):
a Tauri app around a canonical project model, with one project serving both the
Studio workspace and (from Phase 3) the Forge3D workspace.

**Phases 1–3 are implemented**: the desktop shell, project schema and migration,
open/save/autosave/recovery, command-based undo/redo, a Three.js solid viewport
with orbit/pan/zoom and a camera view, Blender-style move/rotate/scale tools, an
orientation gizmo, add/duplicate/delete, glTF/GLB import, and the Forge3D
workspace adapter (capability report, engine probe, viewer process, camera/sun
sync, diagnostics).

> The pre-existing `../studio/` tree is the earlier terrain-focused eframe/egui
> Studio. It is a separate application and a separate Cargo workspace. This
> directory is the Tauri product surface; the two do not share a build.

## Status against the plan

| Deliverable | Where | State |
|---|---|---|
| **Phase 1** — Tauri shell, React UI, Rust project service | `src-tauri/`, `src/`, `crates/studio-core/` | done |
| Versioned schema v2 + v1 migration | `project.rs`, `persist.rs` | done |
| Open / save / autosave / recovery | `persist.rs`, `autosave.rs`, `session.rs` | done |
| Command model + undo/redo (14 commands) | `command.rs`, `editor.rs` | done |
| **Phase 2** — Three.js solid viewport | `src/viewport/Viewport3D.tsx` | done |
| Orbit / pan / zoom | `OrbitControls` in `Viewport3D.tsx` | done |
| Object selection, tree ↔ viewport sync | `Viewport3D.tsx`, `SceneTree.tsx` | done |
| Move / rotate / scale tools (gizmos) | `TransformControls` + `ViewportToolbar.tsx` | done |
| Orientation gizmo (top right, clickable) | `src/viewport/AxisGizmo.tsx` | done |
| Add / delete / duplicate | `command.rs`, `session.tsx` | done |
| glTF / GLB import | `ImportGltf` command + `assets.ts` | done (see limitation) |
| Look through the active camera | `Viewport3D.tsx` (`0`, toolbar) | done |
| **Phase 3** — workspace switching | `setWorkspace` command, `TopBar` | done |
| Capability report per object | `crates/studio-core/src/workspace.rs` | done, tested |
| Engine / viewer probe | `src-tauri/src/forge3d.rs` | written, unbuilt here |
| Viewer process + IPC bridge | `forge3d.rs`, `studio_bridge.py` | written, unbuilt here |
| Camera + sun sync | `forge3d_sync_camera`, `SunAngles` | written, unbuilt here |
| Diagnostics + idempotent handoff | `forge3d.rs`, `workspace.rs` | done |
| **Phase 4 (in progress)** — terrain-first default scene | `Project::starter`, `TerrainSeed` | done |
| DEM heightfield preview in Studio | `terrain_preview`, `buildTerrainMesh` | done |
| Viewer launch verified end-to-end | bridge smoke + PNG snapshot | done |
| **Rendered viewport mode (embedded)** | `studio_render.py`, `render.rs` | done |
| Render still to disk, render settings UI | — | next |

Exit criteria — *edits change the canonical model, reloading preserves them* and
*no viewport-only copies* — are covered by `crates/studio-core/tests/phase1.rs`
and `phase2.rs`; the viewport only reads model state and writes through commands.

## Architecture

```text
studio-tauri/
├── crates/studio-core/        canonical model — no Tauri, no GPU, no Forge3D
│   ├── project.rs             Project document v2: format tag, assets, validate
│   ├── scene.rs               Scene graph: objects, transforms, mesh sources
│   ├── command.rs             Every mutation as a reversible Command
│   ├── editor.rs              Session: revision, dirty flag, undo/redo stacks
│   ├── persist.rs             Version gate, v1→v2 migration, atomic save
│   ├── autosave.rs            Recovery envelopes + pruning
│   ├── session.rs             One open project: open, save, autosave, recovery
│   └── workspace.rs           Capability report, camera/sun handoff payload
├── src-tauri/                 Tauri v2 shell
│   ├── src/commands.rs        IPC surface (mesh files, Forge3D adapter)
│   ├── src/forge3d.rs         Engine probe, viewer process, diagnostics
│   ├── src/studio_bridge.py   Embedded Python bridge (relays viewer IPC)
│   ├── src/session.rs         Mutex around the core session
│   └── src/lib.rs             App wiring + background autosave thread
└── src/                       React + TypeScript UI
    ├── bridge/                Tauri IPC, with a browser preview harness
    ├── components/            Top bar, tree, inspector, toolbar, Forge3D panel
    ├── state/session.tsx      Session actions + active tool/space
    ├── viewport/
    │   ├── Viewport3D.tsx     Three.js scene, picking, gizmos, asset loading
    │   ├── AxisGizmo.tsx      SVG orientation widget (no second WebGL context)
    │   └── assets.ts          glTF parsing and per-path scene cache
    └── styles.css
```

### Decisions that matter

- **The model is its own crate.** `studio-core` compiles and tests with
  `cargo test` on a machine with no webkit, no display, and no GPU, so the exit
  criteria can be verified in CI. The Tauri shell only translates IPC to it.
- **Commands carry new values, return inverses.** `Command::apply` reads the
  previous value from the model and returns the command that reverses the edit.
  Undo is not a second implementation of editing; it applies the stored inverse.
- **Redo is canonicalised.** After applying a command, the editor derives the
  redo command by applying the inverse to a copy of the new state. Without that
  step, redoing `duplicateObject` or `createPrimitive` would mint a fresh UUID
  and "redo" would not describe the object undo removed. `tests/phase2.rs` pins
  this for both.
- **IDs are minted in Rust.** `createPrimitive`, `duplicateObject` and
  `importGltf` generate their object (and asset) IDs inside the model, so the UI
  cannot invent identities and two windows cannot collide.
- **Schema v2 is a real migration.** v1 meshes carried `kind.primitive`; v2
  nests it under `kind.source` (which may also be an imported glTF asset) and
  adds the asset registry. `persist::migrate` rewrites v1 files on load, one
  version at a time, and a test saves a migrated file back as v2.
- **Asset paths are stored relative** to the project file and resolved back to
  absolute on load, so a project directory stays portable. Files outside the
  project directory stay absolute.
- **Imported glTF geometry is cached per path and cloned per object.** Two
  objects referencing the same GLB share geometry; only geometry the app itself
  created is disposed on rebuild.
- **The viewport is a projection.** `Viewport3D` reads `SessionState` and writes
  only through commands; gizmo drags preview locally and commit one
  `setTransform` on release, so a drag is a single undo step.
- **Euler convention is pinned.** Studio applies X, then Y, then Z
  (`Rz·Ry·Rx`); three.js is told `"ZYX"`. Both `applyTransform` and the tree
  order comment say so, because getting it wrong looks like "the gizmo rotates
  the wrong way" rather than an error.
- **Autosave never touches the project file.** Snapshots go to the app-data
  recovery directory; successful saves prune the snapshots for that path.
- **The wire format is locked.** `tests/schema_contract.rs` asserts the exact
  key shape that `src/bridge/types.ts` mirrors by hand.

## Building and running

### Linux prerequisites (Fedora)

The window uses the system WebKitGTK. Development headers are required — the
runtime package alone is not enough:

```bash
sudo dnf install -y webkit2gtk4.1-devel gtk3-devel libsoup3-devel \
  javascriptcoregtk4.1-devel librsvg2-devel openssl-devel \
  libappindicator-gtk3-devel patchelf
```

On Debian/Ubuntu the equivalents are `libwebkit2gtk-4.1-dev`,
`libgtk-3-dev`, `libsoup-3.0-dev`, `libjavascriptcoregtk-4.1-dev`,
`librsvg2-dev`, `libssl-dev`, `libayatana-appindicator3-dev`, `patchelf`.

### Commands

```bash
npm install          # once
npm run tauri:dev    # the real desktop app (Rust + webview)
npm run tauri:build  # desktop bundles (deb/rpm/appimage)
npm run dev          # UI only, in a browser, with an in-memory preview harness
npm run build        # typecheck + production bundle of the UI
```

### Tests

```bash
cargo test -p studio-core      # 93 tests: model, commands, migration, workspace
npx tsc --noEmit               # UI types
```

## Viewport controls

| Input | Action |
|---|---|
| Left drag | Orbit |
| Middle drag | Pan |
| Scroll | Zoom |
| Left click | Select (shift/ctrl to add or remove) |
| `V` / `G` / `R` / `S` | Select / Move / Rotate / Scale tool |
| `Shift + D` | Duplicate selection |
| `X`, `Del` | Delete selection |
| `Esc` | Clear selection |
| `0` or the camera button | Toggle camera view (look through the active camera) |
| Click the corner gizmo | Snap the view to an axis |

The tool strip also holds **Global/Local** axis orientation and the **Add** menu
(cube, sphere, plane, import glTF/GLB).

## Solid and Rendered modes (Phase 4)

The Studio viewport has two modes, toggled in the viewport toolbar:

- **Solid** — the Three.js editor view: orbit, gizmos, instant selection.
- **Rendered** — the Forge3D terrain pipeline, rendered offscreen and displayed
  in the viewport. Same engine, same shaders, same PBR/POM/shadows/sky settings
  as the golden-tested renders; no second window.

Implementation: `src-tauri/src/render.rs` keeps one Python process alive
(`studio_render.py`) — session, renderer, and a generated neutral sky HDR are
built once, because shader compilation and IBL prefiltering dominate start-up.
Each frame is a JSON request (camera, sun, size) answered with a base64 PNG. A
960 × 600 frame takes roughly 5 s on the RTX 5060; that is why Rendered mode
renders on demand (and on entering the mode) rather than every orbit tick.

## The default scene is a DEM (Phase 4)

`File → New project` opens a Bryce Canyon scene, not a cube: the shell resolves
`assets/tif/Bryce_Canyon_1024.tif` (or `FORGE3D_STUDIO_DEM`), samples it for the
viewport, and stores its pixel size, georeferenced footprint, and elevation range
on the terrain object. Studio draws a 96×96 heightfield preview; Forge3D receives
the original GeoTIFF.

Two things were needed to make the viewer agree with Studio, both of which look
like "nothing happened" if you get them wrong:

1. **The viewer needs the interpreter's `libpython`.** uv/pyenv interpreters keep
   it outside the virtualenv, so the bridge adds `sysconfig` `LIBDIR` to
   `LD_LIBRARY_PATH` before spawning the viewer. Without it the viewer exits
   immediately with "cannot open shared object file".
2. **Georeferenced DEMs live in metres, far from the origin.** The viewer's world
   is `origin = (mapX, -mapY)` with `span = (|a|·w, |e|·h)` and it frames terrain
   at `radius = 1.5·max(span)` around `(span.x/2, relief/2, span.z/2)`. Bryce
   Canyon is 4000 × 3800 m at ≈ 4 165 774 m north, so a pixel-space guess aims at
   empty space and the render is blank. `TerrainObject::viewer_framing` reproduces
   that formula exactly and is pinned by a test.

## The Forge3D workspace (Phase 3)

Switching the workspace tab moves the project into the Forge3D workspace. The
viewer is a **separate window** driven over IPC; Studio keeps the project and
every edit.

The panel answers three questions, each from real state rather than a promise:

1. **Can this machine run the viewer?** The shell probes a Python interpreter
   (repo `.venv`, then `python3`) for `import forge3d` and for a resolvable
   `interactive_viewer` binary, and names what is missing. If nothing is found,
   the launch button is disabled with the reason as its tooltip.
2. **What can the viewer take?** `workspace.rs` classifies each object against
   the viewer's actual IPC surface: imported glTF meshes and cameras are fully
   supported, directional sun is partial (no shadows/colour), primitives are
   partial until geometry export lands in Phase 4, and point lights are
   unsupported. Every row carries the reason, and scene-level degradations are
   listed separately.
3. **What did the viewer say?** The bridge process's stdout/stderr land in a
   diagnostics pane, with exit and startup failures shown verbatim.

Handoffs are idempotent per `(project, scene, revision)`: reopening the same
revision reuses the running window instead of starting a second one. The payload
is written to `<app-data>/forge3d/handoff.json` so it can be inspected.

## Known limitations

- **glTF import reads the file whole** and is capped at 32 MB (Phase 5 streams
  assets). Self-contained `.glb` works; a `.gltf` that references external
  `.bin`/texture files fails with an explicit message, because the webview
  cannot read neighbouring files.
- **Imported meshes are transform-editable only.** Materials, node selection
  within an asset, and relinking arrive in Phase 5.
- **No shadows or PBR environment** in the solid viewport; it is a modelling
  view, not the Forge3D render (Phase 4).
- **The viewer needs a built binary.** `interactive_viewer` must exist (repo
  `target/release`, an installed console script, `PATH`, or
  `FORGE3D_VIEWER_BINARY`); the probe reports the exact build command when it is
  missing. The engine package must also be importable by that interpreter
  (`FORGE3D_STUDIO_PYTHON` overrides the choice).
- **The Phase 3 process path is not yet exercised on this machine.** The shell
  needs the webkit development packages to compile at all, so the launch/probe
  code is written and reviewed but unbuilt here; the Rust core side (capability
  report, camera/sun handoff, idempotency) is fully tested.
- **One asset per viewer window** — the viewer loads a single glTF/terrain, so a
  scene referencing several assets opens the first and says so in the report.
- **Primitives still do not reach Forge3D.** The Phase 4 default scene is terrain
  precisely because that path is complete; exporting primitive meshes as glTF is
  the remaining piece of Rendered mode.
- **Rendering from Studio is not implemented yet.** The viewer can snapshot (that
  is how the render below was produced), but there is no "Render" button that
  writes a still into the project's `renders/` directory.

## Browser preview harness

`src/bridge/` selects the Tauri bridge when `__TAURI_INTERNALS__` exists, and
otherwise an in-memory mock with the same command semantics. That keeps `npm run
dev` useful for UI work on machines without webkit. The mock implements no file
I/O, no recovery, and no glTF import — anything that matters is mirrored by a
Rust test.

## Not yet implemented (deliberately)

- Geometry export for primitives, render jobs, Forge3D render settings UI — **Phase 4**
- Materials, textures, environments, relinking — **Phase 5**
- Timeline, keyframes, frame export — **Phase 6**

The top bar's Forge3D tab and Render button are present but disabled, with a
tooltip naming the phase that enables them — they are not fake controls.

## Regenerating the app icon

```bash
python3 scripts/make_icons.py   # writes src-tauri/icons/*
```
