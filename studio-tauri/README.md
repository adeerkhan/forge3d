# Forge3D Studio — Tauri shell (Phase 1)

The desktop shell for the renewed **Forge3D Studio** product described in
[`../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md`](../FORGE3D_STUDIO_IMPLEMENTATION_PLAN.md):
a Tauri app around a canonical project model, with one project serving both the
Studio workspace and (from Phase 3) the Forge3D workspace.

This is the **Phase 1** deliverable: desktop shell, project schema, project
service, open/save/autosave/recovery, a scene tree, and the shared command model
with undo/redo. The Studio viewport is a wireframe preview here; Phase 2
replaces it with a Three.js solid viewport.

> The pre-existing `../studio/` tree is the earlier terrain-focused eframe/egui
> Studio. It is a separate application and a separate Cargo workspace. This
> directory is the Tauri product surface; the two do not share a build.

## Status against the Phase 1 exit criteria

| Phase 1 deliverable | Where | State |
|---|---|---|
| Tauri app shell | `src-tauri/` | done (needs the Linux webkit dev packages, see below) |
| React/TypeScript UI | `src/` | done |
| Rust project service | `crates/studio-core/`, `src-tauri/src/session.rs` | done |
| Versioned project schema | `crates/studio-core/src/project.rs`, `persist.rs` | done — `studio3d` v1, newer versions refused |
| Open/save/autosave/recovery | `persist.rs`, `autosave.rs`, `session.rs` | done — atomic writes, 30 s snapshots, startup recovery prompt |
| Scene tree with cube, camera, light | `src/components/SceneTree.tsx`, `project.rs` | done — `Project::new` seeds the starter scene |
| Shared command model + undo/redo | `command.rs`, `editor.rs` | done — 11 commands, every one returns its inverse |

Exit criterion — *a project opens, saves, closes, and reopens with stable IDs and
transforms* — is proven by `crates/studio-core/tests/phase1.rs`.

## Architecture

```text
studio-tauri/
├── crates/studio-core/        canonical model — no Tauri, no GPU, no Forge3D
│   ├── project.rs             Project document: format tag, schema version, validate
│   ├── scene.rs               Scene graph: objects, transforms, cameras, lights
│   ├── command.rs             Every mutation as a reversible Command
│   ├── editor.rs              Session: revision, dirty flag, undo/redo stacks
│   ├── persist.rs             Version gate + atomic save (tmp → fsync → rename)
│   └── autosave.rs            Recovery envelopes + pruning
├── src-tauri/                 Tauri v2 shell
│   ├── src/commands.rs        IPC surface; every call returns a full snapshot
│   ├── src/session.rs         One open editor, one recovery store
│   └── src/lib.rs             App wiring + background autosave thread
└── src/                       React + TypeScript UI
    ├── bridge/                Tauri IPC, with a browser preview harness
    ├── components/            Top bar, scene tree, inspector, viewport, status
    └── viewport/projection.ts Wireframe projection for the Phase 1 preview
```

### Decisions that matter

- **The model is its own crate.** `studio-core` compiles and tests with
  `cargo test` on a machine with no webkit, no display, and no GPU, so the exit
  criteria can be verified in CI. The Tauri shell only translates IPC to it.
- **Commands carry new values, return inverses.** `Command::apply` reads the
  previous value from the model and returns the command that reverses the edit.
  Undo is not a second implementation of editing; it applies the stored inverse.
- **Undo/redo move entries between stacks** rather than storing two commands per
  step. A test caught the earlier version re-applying the *inverse* on redo.
- **Saves are atomic.** Bytes go to a sibling temporary file, `sync_all`, then
  `rename`. A failed save cannot damage the previous file, and no `.tmp` file is
  left behind (`tests/phase1.rs`).
- **A newer schema is refused.** Loading a project written by a future build
  would silently drop fields, so it fails with both version numbers named.
- **Autosave never touches the project file.** Snapshots go to the app-data
  recovery directory; successful saves prune the snapshots for that path.
- **Selection is view state.** It persists with the project but is neither
  undoable nor a reason to mark the project dirty.
- **The wire format is locked.** `tests/schema_contract.rs` asserts the exact key
  shape that `src/bridge/types.ts` mirrors by hand.

## Building and running

### Linux prerequisites (Fedora)

The window uses the system webkitGTK. Development headers are required — the
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
npm run dev          # UI only, in a browser, with an in-memory preview harness
npm run tauri:dev    # the real desktop app (Rust + webview)
npm run build        # typecheck + production bundle of the UI
npm run tauri:build  # desktop bundles (deb/rpm/appimage)
```

### Tests

```bash
cargo test -p studio-core      # model, commands, persistence, recovery, contracts
npx tsc --noEmit               # UI types
```

The Rust tests need nothing installed beyond a Rust toolchain: 46 tests cover
the schema, validation, undo/redo, atomic save, recovery, and the wire format.

## Browser preview harness

`src/bridge/` selects the Tauri bridge when `__TAURI_INTERNALS__` exists, and
otherwise an in-memory mock with the same command semantics. That keeps `npm run
dev` useful for UI work on machines without webkit, at the cost of a second,
much smaller implementation. Anything that matters is mirrored by a Rust test;
the mock implements no file I/O, no recovery, and no Forge3D.

## Not in Phase 1 (deliberately)

- Three.js solid viewport, gizmos, add/delete/duplicate objects — **Phase 2**
- Forge3D process lifecycle, IPC adapter, workspace handoff — **Phase 3**
- Render jobs, Forge3D render settings UI — **Phase 4**
- Materials, textures, environments — **Phase 5**
- Timeline, keyframes, frame export — **Phase 6**

The top bar's Forge3D tab and Render button are present but disabled, with a
tooltip naming the phase that enables them — they are not fake controls.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl/Cmd + N` | New project |
| `Ctrl/Cmd + O` | Open project |
| `Ctrl/Cmd + S` | Save (Save As when the project has no path) |
| `Ctrl/Cmd + Shift + S` | Save as |
| `Ctrl/Cmd + Z` | Undo |
| `Ctrl/Cmd + Shift + Z`, `Ctrl/Cmd + Y` | Redo |
| `Esc` | Clear selection |

## Regenerating the app icon

```bash
python3 scripts/make_icons.py   # writes src-tauri/icons/*
```
