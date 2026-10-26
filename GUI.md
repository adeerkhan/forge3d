# Forge3D Studio
## Cross-Platform Rust Desktop Application

> **STATUS (fork implementation, 2026-10-26):** this spec is being
> implemented in this fork under [`studio/`](studio/README.md) — an
> egui/eframe app whose worker embeds CPython and drives the certified
> `TerrainRenderer.render_terrain_pbr_pom` pipeline. Milestones 1–4 are done
> (shell, real engine integration, end-to-end DEM → render → export →
> project persistence, covered by `studio/crates/studio-engine/tests`);
> embedded viewport (5) and packaging (6) remain. See
> [studio/README.md](studio/README.md) for architecture, build steps, and
> known limitations. Not intended for upstream pull requests.

**Project type:** Standalone desktop application built around the existing Forge3D rendering engine  
**Initial platform:** Fedora Linux  
**Target platforms:** Linux, Windows, macOS  
**Primary language:** Rust  
**GUI framework:** egui / eframe  
**Rendering integration:** Existing Forge3D capabilities, with an eventual embedded viewport if technically feasible  
**Icon style:** Lucide-style outline icons  
**Development strategy:** Separate application repository, with upstream contributions to Forge3D where appropriate

---

## 1. Project Vision

Build a polished, native-feeling, cross-platform desktop application around [Forge3D](https://github.com/milos-agathon/forge3d), the existing Rust-first rendering engine developed by Milos Agathon.

The goal is to make Forge3D accessible to users who want to work through a graphical desktop application rather than primarily through Python scripts, notebooks, or command-line workflows.

The application should provide a coherent workspace for loading data, configuring scenes, interacting with the renderer, managing projects, and exporting results.

This must be a real application that invokes actual Forge3D functionality. Do not substitute mock data, placeholder rendering, or simulated operations for working features.

### Core principles

1. Reuse the existing Forge3D engine wherever practical.
2. Avoid duplicating or rewriting rendering functionality without a demonstrated technical need.
3. Keep the desktop application in a separate repository initially.
4. Use Rust throughout the desktop application and prefer native Rust libraries.
5. Design the application for Linux, Windows, and macOS from the beginning.
6. Keep the GUI responsive during loading, rendering, and export operations.
7. Use a consistent, professional interface with carefully selected Lucide-style icons.
8. Preserve compatibility with upstream Forge3D and make engine updates manageable.
9. Clearly distinguish implemented features from planned or unsupported features.
10. Favor small, tested, incremental changes over a large unverified rewrite.

---

## 2. Mandatory Initial Audit

Before implementing the GUI, inspect the existing Forge3D repository and document its actual architecture.

Do not assume that the repository has no graphical interface. It already provides an interactive viewer and viewer-control interfaces. The objective is to build a complete desktop application around those capabilities.

Inspect at least:

- `Cargo.toml` and any workspace manifests.
- The Rust library structure and public API.
- The existing interactive viewer and its executable.
- The viewer's IPC or control protocol.
- The Python extension and its relationship to the Rust implementation.
- Scene, session, terrain, camera, rendering, and export APIs.
- Existing examples, tests, and documentation.
- GPU requirements and graphics-backend initialization.
- Licensing boundaries between open-source and Pro functionality.
- Existing platform-specific dependencies and build instructions.

Run the existing project's documented build and test commands before making changes.

Produce a concise audit report containing:

1. Existing architecture and package boundaries.
2. Features accessible directly from Rust.
3. Features currently exposed only through Python or another interface.
4. How the existing viewer can be controlled.
5. Whether the viewer can be embedded into an egui window without a major refactor.
6. Recommended integration approach and its limitations.
7. Known build failures and platform-specific risks.
8. A minimal implementation plan.

Do not modify the upstream clone during the audit.

---

## 3. Repository Strategy

The desktop application must be maintained separately from Milos's original repository.

The existing Forge3D repository is upstream. The application repository owns the GUI, project-management code, application configuration, packaging, and release process.

Recommended conceptual structure:

```text
forge3d-studio/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── CONTRIBUTING.md
├── crates/
│   ├── app/
│   ├── ui/
│   ├── engine-integration/
│   ├── project/
│   └── services/
├── assets/
│   ├── icons/
│   ├── fonts/
│   └── themes/
├── packaging/
│   ├── linux/
│   ├── windows/
│   └── macos/
└── tests/
```

This is a target structure, not a requirement to create every crate immediately. Start with a minimal workspace and introduce separate crates when there are meaningful boundaries.

### Upstream integration

- Keep the Forge3D source of truth upstream.
- Prefer consuming Forge3D through its actual Rust library interface where feasible.
- Pin the dependency to a known commit or release during development.
- Update it deliberately and run regression tests after each update.
- Do not copy the entire upstream repository into the GUI repository without a clear technical reason.
- If direct library integration is not currently practical, isolate the existing viewer process behind an engine-integration interface.
- Avoid making the GUI depend on undocumented internal APIs.
- Submit focused upstream pull requests for reusable fixes or APIs that benefit Forge3D independently of the desktop interface.

Document the dependency strategy and how to update it.

---

## 4. Technology Stack

### Primary stack

- **Language:** Rust.
- **GUI:** `egui` and `eframe`.
- **Engine integration:** Existing Forge3D Rust APIs and viewer-control interfaces.
- **Concurrency:** Appropriate Rust asynchronous tasks or worker threads.
- **Serialization:** Serde with a documented, versioned project format.
- **Logging:** Structured application logs with useful error context.
- **Testing:** Unit tests, integration tests, and automated build checks.
- **Packaging:** Native Linux, Windows, and macOS distribution workflows.

Select dependency versions compatible with the current project and its minimum supported Rust version. Do not arbitrarily upgrade Forge3D's dependencies.

### GUI and rendering integration

Do not assume that Forge3D's wgpu renderer can automatically render inside an egui viewport.

Investigate the available integration options:

1. Embed the existing rendering pipeline into an egui-compatible viewport.
2. Refactor a small portion of the renderer to expose an embeddable rendering interface.
3. Use the existing viewer as a separate process during the first milestone.

Prefer the simplest maintainable solution that preserves actual rendering functionality.

If the first implementation uses a separate viewer window, make that limitation explicit. Do not present a fake embedded viewport as completed functionality.

---

## 5. Interface and Visual Design

Build a professional desktop interface suitable for a technical 3D visualization and rendering application.

The visual direction should be restrained, clean, and productivity-oriented. Use consistent spacing, clear hierarchy, compact toolbars, subtle borders, and readable typography.

Avoid an excessively colorful dashboard, oversized cards, unnecessary gradients, and decorative elements that compete with the 3D scene.

### Main application layout

The target workspace should contain:

**A. Application menu and toolbar**

- Application logo and project name.
- File and project actions.
- Import and export actions.
- Undo and redo where supported.
- Viewport navigation and camera presets.
- Rendering controls.
- Settings and help.

**B. Left sidebar**

- Project browser.
- Scene or layer tree.
- Imported datasets.
- Visibility toggles.
- Selection and organization controls.

**C. Central workspace**

- Actual Forge3D interactive viewport.
- Camera controls.
- Viewport overlays where supported.
- Clear loading, error, and empty states.
- A useful fallback view when no project is open.

**D. Right sidebar**

- Context-sensitive properties.
- Terrain and data settings.
- Camera parameters.
- Lighting and sun settings.
- Supported rendering options.
- Export configuration.

**E. Bottom panel or status bar**

- Current project and dataset status.
- Background job progress.
- Renderer and GPU information when available.
- Error notifications.
- Rendering and export status.

The interface must adapt to window resizing. Panels should be resizable or collapsible where practical.

Do not implement every panel as a disconnected visual mockup. Controls must be connected to real application state and engine operations.

---

## 6. Icons and Assets

Use [Rust/UI Icons](https://rust-ui.com/icons) as a visual reference for the icon language.

The icon design should follow the Lucide-style outline aesthetic: consistent stroke weights, simple geometry, recognizable silhouettes, and appropriate sizing.

Important technical distinction: the Rust/UI icon components are intended for Leptos and Dioxus. The application uses egui, so those components are not automatically compatible with the selected GUI framework.

Use one of these approaches:

1. Compatible SVG assets sourced from Lucide.
2. An actively maintained Rust icon crate compatible with egui.
3. A small internal icon module that loads and renders SVG assets through a suitable egui-compatible renderer.

Evaluate available libraries before selecting one. Avoid introducing a web framework solely to obtain icons.

### Initial icon mapping

| Action | Suggested icon |
|---|---|
| Open project | `folder-open` |
| Save project | `save` |
| Import data | `file-input` |
| Export result | `file-output` |
| Scene layers | `layers-3` |
| Properties | `sliders-horizontal` |
| Camera | `camera` |
| Orbit navigation | `orbit` |
| Rendering | `image` |
| Settings | `settings-2` |
| Search | `search` |
| Diagnostics | `activity` |
| GPU information | `gpu` |
| Background tasks | `list-checks` |
| Errors | `circle-alert` |
| Success | `circle-check` |

Treat these names as asset identifiers to resolve against the selected icon source, not as guaranteed API names in an egui library.

Use tooltips for icon-only controls. Provide appropriate accessibility labels where supported. Maintain consistent icon dimensions and alignment.

Do not download or embed the entire icon library unnecessarily. Include only the assets the application actually uses, and respect the relevant asset licenses.

---

## 7. Functional Scope for Version 0.1

The first release should prove the complete application workflow without attempting to reproduce every Forge3D feature.

### 7.1 Application shell

Implement:

- A native desktop window.
- Main menu and toolbar.
- Dockable or collapsible side panels where feasible.
- Light and dark themes.
- A persistent settings mechanism.
- Useful startup and empty states.
- Clear application errors and logs.

### 7.2 Data loading

Investigate and expose supported data-loading operations, prioritizing terrain and digital elevation models.

Where supported by Forge3D, allow the user to:

- Select a local input file.
- Load the dataset.
- View loading progress.
- See meaningful validation errors.
- Inspect basic dataset metadata.
- Cancel operations where cancellation is supported.

Do not invent support for file formats the engine cannot load.

### 7.3 Scene and viewport

Initially, use the existing viewer interface if it is the most reliable integration path.

Provide controls for the operations already supported by Forge3D, such as:

- Camera orientation and position.
- Orbit camera parameters.
- Terrain vertical scale.
- Sun direction and lighting.
- Other verified scene parameters.

The target experience is a viewport integrated into the main application window. Treat that as a separate engineering milestone if it requires renderer refactoring.

### 7.4 Rendering and export

Provide a rendering workflow using the existing rendering pipeline.

Where supported, expose:

- Output dimensions.
- Output location.
- Render progress and completion status.
- Supported image formats.
- Relevant rendering settings.
- Error handling for failed exports.

Do not implement a second renderer just to make the UI functional.

### 7.5 Project persistence

Design a versioned project format containing application-owned state, including:

- Project metadata.
- Dataset references.
- Scene configuration.
- Camera configuration.
- Supported layer and visibility state.
- Relevant rendering and export settings.

Use paths and references rather than copying large datasets into project files by default.

Validate project files on opening. Handle missing datasets, unsupported versions, and invalid configuration gracefully.

Only serialize engine settings that can be restored reliably.

---

## 8. Application Architecture

Keep the GUI separate from engine operations.

The GUI should communicate with an application service layer. That layer should own project state, engine calls, background jobs, error conversion, and progress reporting.

Define a small, typed interface around actual Forge3D capabilities. Potential operations include:

- Open and save a project.
- Load a dataset.
- Update camera parameters.
- Update supported terrain or lighting parameters.
- Start rendering.
- Export a result.
- Query engine and renderer status.

These are proposed application-level operations. Implement only those that map to verified engine capabilities.

### Responsiveness

- Do not perform heavy rendering or file operations directly in the UI update loop.
- Avoid blocking the GUI while waiting for the viewer or renderer.
- Send job status and progress back to the UI through an appropriate channel.
- Use clear task states such as queued, running, completed, cancelled, and failed.
- Do not report cancellation as successful unless the operation actually stops.
- Avoid unnecessary GPU resource duplication.
- Ensure background tasks and child processes are cleaned up when the application exits.

### Error handling

Convert engine failures into actionable user-facing errors while retaining diagnostic details in logs.

Avoid panics for normal user errors such as invalid files, missing assets, unavailable GPU features, or unsupported settings.

---

## 9. Cross-Platform Requirements

The application must be designed for Linux, Windows, and macOS even though initial development happens on Fedora Linux.

Avoid unnecessary dependencies on Linux-specific paths, shell commands, window-system behavior, or packaging mechanisms.

Use platform-appropriate application-data and configuration directories.

Keep platform-specific code isolated behind small abstractions.

Verify:

- Native window creation.
- GPU initialization and fallback behavior.
- File dialogs.
- Filesystem path handling.
- Child-process management if the viewer runs separately.
- Application shutdown.
- Rendering and export behavior.
- Distribution requirements.

Do not claim Windows or macOS support based solely on successful compilation on Linux. Each platform must be built and tested on a suitable runner or machine.

---

## 10. Upstream Collaboration

The GUI is initially maintained in its own repository.

Keep changes in the application repository unless they improve Forge3D itself.

When an engine change is needed:

1. Determine whether the issue can be solved through the existing public API.
2. Identify the smallest reusable engine change.
3. Add or update tests.
4. Open an upstream discussion when the change affects architecture or public APIs.
5. Submit a focused pull request to Forge3D.
6. Keep the GUI compatible with the current upstream version until the change is accepted.

Do not assume that the upstream maintainer will merge the complete desktop application.

The application should remain useful and independently maintainable if it is never incorporated into Forge3D's main repository.

---

## 11. Testing and Quality Standards

At every stage:

- Run formatting checks.
- Run relevant Rust static checks.
- Run unit and integration tests.
- Verify the actual GUI launches.
- Exercise real engine operations.
- Check failure paths and missing files.
- Test project save and reopen behavior.
- Confirm that the GUI remains responsive during long operations.
- Avoid adding large dependencies without justification.
- Do not suppress compiler warnings merely to obtain a clean build.

Keep a record of known failures that existed before the changes.

Use automated checks for builds and tests on all supported platforms as the project matures.

---

## 12. Implementation Milestones

### Milestone 1: Audit

Deliver the architecture report, baseline build results, integration recommendation, and a concrete first implementation plan.

### Milestone 2: Desktop shell

Create a minimal egui/eframe application with the intended layout, icon system, theme, and application state.

### Milestone 3: Real engine integration

Connect the application to an actual Forge3D operation. Prefer loading a supported dataset and controlling the real viewer or renderer.

### Milestone 4: First end-to-end workflow

Load a dataset, adjust supported parameters, render an output, and save project state.

### Milestone 5: Embedded viewport

Investigate and implement viewport integration if it can be done without destabilizing the engine. Otherwise document the remaining work and the constraints of the current viewer architecture.

### Milestone 6: Packaging

Create a repeatable Linux build and distribution process, then add Windows and macOS builds.

Do not skip ahead to packaging or cosmetic polish while the engine integration remains unproven.

---

## 13. First Agent Task

Start with Milestone 1 only.

Inspect the existing repository, report the architecture and build results, and recommend the smallest working implementation path.

Do not immediately generate the entire application. Do not replace the existing renderer. Do not refactor upstream code without a demonstrated need.

After the audit, implement the smallest useful vertical slice: a working egui/eframe window that can invoke one real Forge3D capability through a clean integration boundary.

The result must compile, launch, and demonstrate a real operation. Provide a summary of files changed, commands executed, tests performed, limitations discovered, and the next recommended task.

**Definition of success:** a maintainable foundation for a standalone cross-platform Rust desktop application that adds a complete graphical workflow to Forge3D without unnecessarily duplicating or destabilizing its engine.
