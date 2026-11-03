# Forge3D Studio — Tauri shell

The desktop shell for the renewed **Forge3D Studio** product described in
[`../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md`](../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md):
a Tauri app around a canonical project model, with one project serving both the
Studio workspace and (from Phase 3) the Forge3D workspace.

**Phases 1 and 2 are implemented**: the desktop shell, project schema and
migration, open/save/autosave/recovery, command-based undo/redo, a Three.js solid
viewport with orbit/pan/zoom, Blender-style move/rotate/scale tools, an
orientation gizmo, add/duplicate/delete, and glTF/GLB import.

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
│   └── session.rs             One open project: open, save, autosave, recovery
├── src-tauri/                 Tauri v2 shell
│   ├── src/commands.rs        IPC surface (incl. mesh file reading)
│   ├── src/session.rs         Mutex around the core session
│   └── src/lib.rs             App wiring + background autosave thread
└── src/                       React + TypeScript UI
    ├── bridge/                Tauri IPC, with a browser preview harness
    ├── components/            Top bar, tree, inspector, toolbar, status
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
cargo test -p studio-core      # 73 tests: model, commands, migration, persistence
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
| Click the corner gizmo | Snap the view to an axis |

The tool strip also holds **Global/Local** axis orientation and the **Add** menu
(cube, sphere, plane, import glTF/GLB).

## Known limitations

- **glTF import reads the file whole** and is capped at 32 MB (Phase 5 streams
  assets). Self-contained `.glb` works; a `.gltf` that references external
  `.bin`/texture files fails with an explicit message, because the webview
  cannot read neighbouring files.
- **Imported meshes are transform-editable only.** Materials, node selection
  within an asset, and relinking arrive in Phase 5.
- **No shadows or PBR environment** in the solid viewport; it is a modelling
  view, not the Forge3D render (Phase 4).

## Browser preview harness

`src/bridge/` selects the Tauri bridge when `__TAURI_INTERNALS__` exists, and
otherwise an in-memory mock with the same command semantics. That keeps `npm run
dev` useful for UI work on machines without webkit. The mock implements no file
I/O, no recovery, and no glTF import — anything that matters is mirrored by a
Rust test.

## Not yet implemented (deliberately)

- Forge3D process lifecycle, IPC adapter, workspace handoff — **Phase 3**
- Render jobs, Forge3D render settings UI — **Phase 4**
- Materials, textures, environments, relinking — **Phase 5**
- Timeline, keyframes, frame export — **Phase 6**

The top bar's Forge3D tab and Render button are present but disabled, with a
tooltip naming the phase that enables them — they are not fake controls.

## Regenerating the app icon

```bash
python3 scripts/make_icons.py   # writes src-tauri/icons/*
```
