# Forge3D Studio — Tauri shell

The desktop shell for the renewed **Forge3D Studio** product described in
[`../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md`](../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md):
a Tauri app around a canonical project model, with one project serving both the
Studio workspace and (from Phase 3) the Forge3D workspace.

**Phases 1–6 are implemented**: the desktop shell, project schema and migration,
open/save/autosave/recovery, command-based undo/redo, a Three.js viewport with
Solid / Material / Rendered modes, orbit/pan/zoom and a camera view,
Blender-style move/rotate/scale tools, an orientation gizmo, add/duplicate/delete,
glTF/GLB import, the Forge3D workspace adapter (capability report, engine probe,
camera/sun sync), and the animation timeline with keyframes and PNG sequence
export.

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
| **Phase 3** — workspace switching | removed (see below) | dropped deliberately |
| Capability report per object | `crates/studio-core/src/workspace.rs` | done, tested |
| Engine discovery | `src-tauri/src/forge3d.rs` | done, shown in the status bar |
| Separate viewer window | — | removed: rendering lives in the viewport |
| Camera + sun transfer | `RenderRequest::from_handoff` | done |
| Diagnostics + idempotent handoff | `forge3d.rs`, `workspace.rs` | done |
| **Phase 4** — terrain-first default scene | `Project::starter`, `TerrainSeed` | done |
| DEM heightfield preview in Studio | `terrain_preview`, `buildTerrainMesh` | done |
| Embedded renderer service (viewport frames + stills) | `studio_render.py`, `render.rs` | done |
| **Rendered viewport mode (embedded)** | `studio_render.py`, `render.rs` | done |
| Render size control (viewport / DEM native / project) | `Viewport3D.tsx` render bar | done |
| Render still to disk (+ JSON sidecar), render settings UI | `render_still`, `RenderSettingsEditor` | done |
| **Phase 5** — capability-tagged material/light/camera parameters | `workspace.rs` table → `capability_table` → Inspector badges | done |
| Material inspector (terrain albedo + engine summary, mesh base colour/texture) | `Inspector.tsx` | done |
| Texture asset management (attach, replace, remove; non-empty invariant) | `session.tsx`, `commands.rs` | done |
| Environment/IBL controls (sky background, environment intensity) | `RenderSettings`, `RenderSettingsEditor` | done |
| HDRI environment for Material + Rendered (`read_hdr`, PMREM, `IBL.from_hdr`) | `RenderSettings.environment`, `Viewport3D.tsx`, `studio_render.py` | done |
| Basic lights & camera properties (direction/intensity/colour, fov/clip) | `Inspector.tsx`, `render.rs` | done |
| Asset relinking and dependency counts | `Inspector.tsx`, `command.rs` | done |
| Material-preview albedo drape (raster → PNG via engine python) | `raster_preview`, `Viewport3D.tsx` | done |
| glTF node selection (`SetGltfNode`, runtime-derived node names, Inspector picker) | `assets.ts`, `command.rs`, `Inspector.tsx` | done |
| **Phase 6** — timeline, keyframes, interpolation | `crates/studio-core/src/timeline.rs`, `src/components/Timeline.tsx` | done |
| Playhead / scrub / playback (one clock in `playback.ts`) | `setPlayhead` → `Session::set_playhead` | done |
| Per-channel keys (`SetChannelKeys`), 9 transform channels + camera FOV | `command.rs`, `timeline.rs` | done |
| Offline PNG sequence export (`renderAnimationFrame` per frame) | `src/state/session.tsx`, `render.rs` | done |
| Camera animation reaches Forge3D (`camera_render_view` → `with_view`) | `workspace.rs`, `commands.rs` | done |

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
│   ├── persist.rs             Version gate, v1→v2→…→v4 migrations, atomic save
│   ├── autosave.rs            Recovery envelopes + pruning
│   ├── session.rs             One open project: open, save, autosave, recovery
│   ├── timeline.rs            Keyframes, interpolation, the one evaluate()
│   └── workspace.rs           Capability report, camera/sun + render-view handoff
├── src-tauri/                 Tauri v2 shell
│   ├── src/commands.rs        IPC surface (mesh files, Forge3D adapter, animation)
│   ├── src/forge3d.rs         Engine probe, viewer process, diagnostics
│   ├── src/render.rs          Embedded renderer service + render request
│   ├── src/studio_render.py   Embedded Python bridge (relays viewer IPC)
│   ├── src/session.rs         Mutex around the core session
│   └── src/lib.rs             App wiring + background autosave thread
└── src/                       React + TypeScript UI
    ├── bridge/                Tauri IPC, with a browser preview harness
    ├── components/            Top bar, tree, inspector, toolbar, timeline
    ├── state/session.tsx      Session actions + active tool/space
    ├── state/playback.ts      Tested timeline playback clock (no React)
    ├── viewport/
    │   ├── Viewport3D.tsx     Three.js scene, modes, picking, gizmos, assets
    │   ├── viewMath.ts        Viewport → renderer camera mapping (+ test)
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
(cube, sphere, plane, DEM terrain from a GeoTIFF — which also works from an
empty scene — and import glTF/GLB).

## Solid, Material and Rendered modes

The Studio viewport has three modes, toggled in the viewport toolbar:

- **Solid** — a Blender-style clay view: neutral shading under the scene's sun,
  with **no albedo, textures or material colours**. Fast orbit/pan/zoom, gizmos,
  and instant selection. This is the editing view.
- **Material** — a lightweight material preview, not an engine render: the
  three.js scene lit by the project's HDRI (pre-filtered with PMREM) or, when
  none is set, a built-in studio environment, plus soft sun shadows. It shows
  the albedo (terrain drape, mesh base colour and texture, glTF materials) and
  uses the HDRI as its background, so it reads as a look-dev view. The camera
  orbits freely here exactly like Solid.
- **Rendered** — the Forge3D terrain pipeline, rendered offscreen and displayed
  in the viewport. Same engine, same shaders, same PBR/POM/shadows/sky settings
  as the golden-tested renders; no second window. It lights with the same
  project HDRI (the engine uploads it with `IBL.from_hdr`).

Implementation: `src-tauri/src/render.rs` keeps one Python process alive
(`studio_render.py`) — session, renderer, and a generated neutral sky HDR are
built once, because shader compilation and IBL prefiltering dominate start-up.
Each frame is a JSON request (camera, sun, size) answered with a base64 PNG.
Frames are requested on demand and coalesced: entering the mode, orbiting
(throttled), and every scene edit (sun, exposure, overlay — debounced) trigger at
most one queued frame. The frame's **Size** control in the render bar picks the
resolution — 1×/2×/4× the viewport, the DEM's own pixels (`DEM (1024 × 1024)`),
or the project's output size. The top-bar **Render** button opens a save dialog
defaulting to `<project>/renders/<name>-<timestamp>.png` with a JSON sidecar
describing every setting that produced it.

Mode changes only re-shade the built content (Solid swaps in a clay material and
back); they never rebuild geometry or re-fetch textures.

### Environment (HDRI)

`Inspector → Render output → Environment (HDRI)` adds a Radiance `.hdr` file to
the project as an asset and makes it the scene's image-based light. It is one
setting used by two consumers:

- **Material** reads the bytes through `read_hdr`, decodes them with three's
  `HDRLoader`, pre-filters with PMREM, and uses the result for `scene.environment`
  — with the equirect source as the background. A replaced map is freed once the
  new one is bound; the built-in studio sky is the fallback.
- **Rendered** sends the *path* with each frame; `studio_render.py` hands it to
  `IBL.from_hdr` and caches the prefiltered map by file identity (path, mtime,
  size), so orbiting costs nothing and replacing the file takes effect.

`Environment / IBL intensity` scales the image-based light in both views (0
turns it off; the built-in studio fallback is additionally dimmed so it reads as
fill, the project HDRI is used as authored). Removing the HDRI returns both views
to the studio sky. The asset cannot be deleted while it is the environment — the
Assets panel reports it as in use.

## The default scene is a DEM (Phase 4)

`File → New project` opens a Bryce Canyon scene, not a cube: the shell resolves
`assets/tif/Bryce_Canyon_1024.tif` (or `FORGE3D_STUDIO_DEM`), samples it for the
viewport, and stores its pixel size, georeferenced footprint, and elevation range
on the terrain object. The Bryce starter also ships its NAIP orthophoto
(`assets/tif/Bryce_Canyon_naip.tif`) draped as the terrain's albedo (overlay,
0.82 opacity, source colours preserved) whenever the file exists — that is the
"Bryce material". Studio draws a 96×96 heightfield preview; Forge3D receives
the original GeoTIFF.

Rendered frames use the camera you are looking through. The viewport maps its
camera onto the renderer's frame (east = +X, north = −Z in the viewport; +X/+Y/+Z
east/north/up in the renderer, terrain centred on the origin) and passes
`{phiDeg, thetaDeg, radius, target, fovDeg}` with every render request, so a
Rendered frame is framed like the viewport instead of a canned shot. Pressing
`End` captures the view into the active camera object, which is what camera view
(`0`) shows. When no viewport camera is available (e.g. the shell renders without
one), the fallback frames the terrain at `radius = 1.5·max(span)` aimed at the
mesh centre.

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

## Engine discovery

Studio probes a Python interpreter for `import forge3d` (repo `.venv` first, then
`python3`, `FORGE3D_STUDIO_PYTHON` to override) and reports the result in the
status bar — `forge3d 1.47.1` when the renderer can run, `engine unavailable` with
the reason when it cannot. There is no separate viewer window any more: opening
one was a Phase 3 detour, and it has been removed along with its workspace tab.
The capability report it produced still exists in `workspace.rs`, where it feeds
the render handoff.

## Known limitations

- **glTF import reads the file whole** and is capped at 32 MB (streamed asset
  loading is not implemented). Self-contained `.glb` works; a `.gltf` that
  references external `.bin`/texture files fails with an explicit message,
  because the webview cannot read neighbouring files.
- **Imported meshes are transform-editable, relinkable, and node-selectable.**
  Solid draws them as clay; the Material preview draws their glTF materials.
  Meshes do not reach Rendered at all yet (see Primitives below).
- **No shadows or PBR environment** in the Solid viewport; it is a modelling
  view, not the Forge3D render. The Material preview is where HDRI lighting and
  soft shadows live (Phase 4).
- **The engine probe needs only an importable `forge3d`.** Rendering runs
  in-process (`studio_render.py`), so no viewer binary is required; the probe
  still looks for `interactive_viewer` and reports what it finds as optional
  diagnostics in the status-bar tooltip. `FORGE3D_STUDIO_PYTHON` overrides the
  interpreter choice.
- **Primitives still do not reach Forge3D.** The Phase 4 default scene is terrain
  precisely because that path is complete; exporting primitive meshes as glTF is
  the remaining piece of Rendered mode.
- **Object animation (Phases 6) is evaluated in the model and exported per frame,
  but the engine cannot draw those objects**, so only camera animation shows up
  in Rendered frames.
- **Rendered shows one still frame at a time.** Playback animates the Solid and
  Material viewports live; Rendered re-renders on edits and camera changes
  (throttled), not per playback frame. Use **Export frames…** for the sequence.
- **HDRI environments are Radiance `.hdr` only.** Both consumers read RGBE
  (three's `HDRLoader` and the engine's `IBL.from_hdr`); `.exr` is rejected with
  a specific message rather than silently mis-lighting. The viewport reads the
  file whole over IPC (64 MB cap) and keeps one decoded map at a time, so a very
  large HDRI costs a decode (~a second) when first selected or replaced.
- **Country-scale DEMs render through a scale budget.** The studio renderer
  scales any DEM into a 30 000-unit world, so wide maps frame correctly
  (verified end-to-end with Switzerland, 353 km — its relief is only ~1 % of
  the span at Vertical scale 1, so raise the slider to read the Alps). The
  older ±65 536 view-coordinate clamp no longer bites in this path
  (`experiments/swiss-land-cover.py` documents the engine-level case).
- **Raster overlays are the terrain's albedo.** The engine has no other terrain
  material override; a draped raster (the Bryce NAIP orthophoto) is how the
  surface gets imagery — the Material preview and Rendered drape it (Material
  decodes the raster through the shell's `raster_preview`), while Solid stays
  neutral clay. The inspector shows it under the terrain's **Material**
  section — `Add albedo raster…` / `Replace…` / `Remove` — alongside what the
  engine actually uses (`MaterialSet.terrain_default()`, four triplanar layers,
  no texture maps shipped in this checkout, so the default is flat colours until
  an albedo is attached or maps appear in `assets/materials/`).

## Timeline (Phase 6)

The strip under the viewport is the animation timeline. The model lives in
`crates/studio-core/src/timeline.rs` and is persisted with the document (schema
v4; older builds reject a v4 file rather than silently dropping its tracks).

- One `evaluate(project, frame)` samples every keyed channel into the project's
  object transforms. **Playback and frame export both call it** — the session's
  playhead through `Session::set_playhead`, the export through a project clone in
  `render_animation_frame` — so what you scrub is what renders.
- Moving the playhead is **view state**: it does not bump the revision, dirty the
  project, or push undo, so scrubbing never floods the undo stack or autosave.
- Channels are the nine transform scalars plus a camera's FOV, with linear,
  constant, and smooth (smoothstep) interpolation. Terrain cannot be animated
  (its transform is georeferenced); its track is refused with a reason.
- **Key selected** keys the selected object's current values at the playhead;
  **Clear keys** removes the track; **Export frames…** writes one PNG per frame
  into a chosen folder, with live progress and a working Stop.
- Playback is a fixed-rate clock (`src/state/playback.ts`, unit-tested). It
  advances at most one frame per animation tick, so a slow shell runs playback
  slow instead of queueing IPC calls.
- A camera animation reaches Forge3D through `workspace::camera_render_view`:
  the exported frame uses the evaluated camera's eye, rotation, distance, and
  FOV — matching a camera-view still — rather than the terrain's canned framing.
- **Only camera animation is Forge3D-native.** The engine draws terrain, so a
  mesh's animation is visible in the Studio viewport and evaluated for export,
  but it does not appear in Rendered frames (the same geometry-export gap as
  primitives). The UI does not claim otherwise.

## Browser preview harness

`src/bridge/` selects the Tauri bridge when `__TAURI_INTERNALS__` exists, and
otherwise an in-memory mock with the same command semantics. That keeps `npm run
dev` useful for UI work on machines without webkit. The mock implements no file
I/O, no recovery, no glTF import, and no frame export — anything that matters is
mirrored by a Rust test (`crates/studio-core/tests/phase6.rs` covers the timeline;
the viewport clock and camera math have dependency-free `node` tests run by
`npm test`).

## Not yet implemented (deliberately)

- Geometry export for primitives — Rendered still draws terrain only
- Shared material library — **Phase 5** (environment/IBL controls, capability badges, the Material-preview albedo drape, and glTF node selection shipped)
- Render queue and cancellation — **Phase 7**

Relinking, the material inspector (terrain albedo, mesh base colour and
texture), the render settings UI, and the Phase 6 timeline already shipped; there
is no Forge3D workspace tab (Phase 3 was dropped) — the top-bar **Render** button
renders to a chosen path with a JSON sidecar.

## Regenerating the app icon

```bash
python3 scripts/make_icons.py   # writes src-tauri/icons/*
```
