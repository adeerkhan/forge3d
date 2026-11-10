# Studio × Forge3D — implementable integration plan (ponytail edition)

Status: **plan**, not code. Phases 0–7 plus the HDRI sky are implemented;
everything below is future work. This revision replaces the earlier 8–18
roadmap: same coverage, far fewer moving parts, every increment sized to land
against the **current** `studio-tauri` with the files that already exist.

The rule applied throughout: use the ladder, stop at the first rung that holds.
Most Forge3D features are *settings on the render request we already send*, not
new subsystems. The plan only spends real design on the two things that cannot
be a payload field: vector layers and meshes.

---

## 0. Delivery status

| Area | State |
|---|---|
| Project model, commands, undo, schema v4 | done |
| Solid / Material / Rendered viewport, HDRI sky | done |
| Terrain stills, Rendered frames, animation export | done |
| Animation tab (timeline, keyframes, camera-path recording), render queue, asset integrity, samples | done |
| Sun from date/time/location (increment 9.0) | done |
| AOV + OIDN denoise final stills (increment 10.0) | done |
| Denoise runtime actually works (oidn+tbb in `.venv`, 1.4 commit order, TBB preload) | done |
| Denoise runs in linear space (sRGB decode → OIDN → encode) | done |
| GPU selection prefers discrete adapters + adapter name in the status bar | done |
| Viewport asks WebGL for the high-performance adapter | done |
| One light rig: scene sun exactly, no fill light, shared sky/environment (Solid/Material/Rendered) | done |
| Still uses the physically-based terrain model that matches the preview's BRDF | done |
| Studio HDR is a valid Radiance file (biased RGBE exponents, sky above ground) | done |
| Engine backend/adapter in the probe (first half of 8.0) | done |
| HDRI/sky axis fix (Z-up terrain → Y-up environment frame for the sky pass) | done |
| Properties sidebar (Render / Scene / Lighting / Object tabs, N, remembered sections) | done |
| Animation tab (Render / Scene / Lighting / Object / **Animation**) — the timeline moved out of the bottom strip into the sidebar | done |
| About Forge3D Studio (File menu): shell version, engine + adapter, device features, license, keyboard shortcuts | done |
| Fly camera (W A S D, Q E, drag-look, Shift to sprint) and camera-path recording onto the timeline | done |
| Captured camera keeps its own orientation (it used to be re-aimed at the world origin) | done |
| 5.5 shadows — filter (PCF/PCSS) and map size wired model → payload → engine | done (softness/intensity are inert in the terrain path; not exposed) |
| 5.2 water — level + slope threshold, mask derived with the engine's own `derive_water_mask` | done (optics deferred) |
| 5.4 bloom + vignette — service-side post-process on the finished frame | done (engine-native needs a pinned-hash refresh) |
| 5.3 clouds — the engine's cloud shadows exist, but only `render_interactive` consumes them | **blocked, measured** (the interactive path's shader contract is stale) |
| One lighting rig shared by preview and payload (fixture read by Rust and node) | done |
| 8.0b — granted device features in the probe/status bar | done |
| 5.1 sky & atmosphere — model, turbidity, ground albedo, exposure; AETHER gated on the shipped bake | done (backdrop-only by design) |
| 5.6 colormap — the engine's own catalogue through `Colormap1D`/`OverlayLayer` | done (needs `albedo_mode="mix"`; one LUT slot per frame) |
| 5.7 per-DEM vertical scale — 6% relief target, derived on import and DEM replacement | done |
| 5.8 terrain sun shadows — `SunVisibilitySettings` ray march, reach + steps | done (one hard ray; soft/resolution dials measured and skipped) |
| 5.9 despeckle — the land-cover gallery's 3×3 class rule on class overlays | done |
| Material layers (snow/rock/wetness) | **blocked, measured** (`vs_main` writes a constant `world_normal`; slope ≈ 0) |
| Tonemap operators/LUT, SSAO/SSGI/SSR, planar reflections | **inert on this path, measured 0** (see 5.8) |
| Engine fidelity — Hosek–Wilkie evaluates green-dominant where the CPU radiance table is blue | open (measured; Preetham is the blue option) |
| Cartography: legend, map plate | **not built** (needs a class scheme the project does not have) |
| Vector layers (§7.1), meshes in Rendered (§7.2) | design increments, trigger-gated |

The earlier roadmap's Phases 8–18 were a capability catalogue. They are mapped
to decisions in §7 and the appendix; most of them are now explicitly **not
built** until a trigger fires.

---

## 1. What already exists in studio-tauri (the map)

| Layer | File | What it already does |
|---|---|---|
| Model | `crates/studio-core/src/project.rs` | `RenderSettings`, asset kinds, validation, schema |
| Model | `crates/studio-core/src/command.rs` | `SetRenderSettings`, `AddAsset`, `Batch` — reuse these |
| Model | `crates/studio-core/src/workspace.rs` | sun/camera handoff, `sun_angles`, `camera_render_view` |
| Shell | `src-tauri/src/forge3d.rs` | engine probe (`EngineStatus`), terrain sampling |
| Shell | `src-tauri/src/render.rs` | `RenderRequest`, `with_settings`, `to_payload` |
| Shell | `src-tauri/src/studio_render.py` | one engine session, `TerrainRenderer`, IBL, sky |
| Shell | `src-tauri/src/commands.rs` | IPC, `render_frame`/`render_still`/`render_animation_frame` |
| Bridge | `src/bridge/types.ts` | `RenderSettings`, `EngineStatus`, command union |
| UI | `src/components/Inspector.tsx` | render settings + capability badges |
| UI | `src/components/StatusBar.tsx` | engine probe status |
| UI | `src/state/session.tsx` | render queue, playhead, actions |
| Tests | `studio-core/tests/*`, `render.rs` tests, `npm test` | cheap, no GPU for most |

## 2. The reusable recipe for a renderer setting

Every "look" feature follows this exact diff. No new command, no new IPC, no
new schema version (optional scalars with `serde(default)` are backward-safe,
as `environment` proved).

1. `project.rs`: add the field to `RenderSettings` with `#[serde(default…)]`;
   add its range check to `Project::validate`.
2. `types.ts`: add the field to `RenderSettings` and the default.
3. `render.rs`: copy it in `with_settings`, emit it in `to_payload`.
4. `studio_render.py`: map it to the engine dataclass in `render()`.
5. `Inspector.tsx`: one control (`NumberField`/`Toggle`/`RangeField`).
6. Tests: one schema-contract line, one payload assertion, one round-trip test.

Five places, one test each, no architecture. The
sections below list only what differs from this recipe.

---

## 3. Increment 8.0 — Engine facts in the existing probe (0.5 day)

**Trigger:** always; later gating depends on it.
**Reuse:** `forge3d.rs::probe_interpreter` + `EngineStatus`; extend its JSON.

- `PROBE_SCRIPT`: one extra line, `forge3d.capabilities()` plus
  `device_probe()` adapter/backend into the same JSON.
- `EngineStatus`: add `features: Vec<String>`, `backend: Option<String>`,
  `adapter: Option<String>`. TS type mirrors it.
- `StatusBar`: the existing title tooltip lists the adapter and granted
  features. No new panel.
- Do **not** add a capability framework, license UI or per-control registry.
  Gated controls appear only when a Pro-gated feature is actually built.

**Check:** a probe JSON fixture; status bar shows features on hover.

## 4. Increment 9.0 — Sun from date/time/location (0.5 day)

**Trigger:** always; highest value per line.
**Reuse:** the sun light object, `SetTransform`, `workspace::sun_angles`.

- Shell command `solar_position(latitude, longitude, datetime)` → Python
  `forge3d.sun_position(lat, lon, datetime)`. One command, no project fields.
- Core helper `sun_transform(azimuth_deg, elevation_deg) -> Transform` (the
  inverse of `sun_angles`), with a round-trip test.
- Inspector (light): four inputs (lat, lon, date-time) + **Apply** → compute →
  `setTransform` on the sun. The existing light transform persists the result;
  the date/lat/lon are not stored (add fields only if users ask for re-edit).
- **Check:** round-trip `sun_transform → sun_angles` within 0.5°.

## 5. Look increments — one setting group per commit

All use §2. Nothing new is introduced beyond the settings below.

### 5.1 Sky model and atmosphere (1 day) — **done**
- Shipped: `skyModel` (`hosek` | `preetham` | `aether`), `skyTurbidity`
  (1–10), `skyGroundAlbedo` (0–1), `skyExposure` (0–4) on `RenderSettings`
  (serde-default, v4-safe); payload `quality.sky*`; service maps to
  `SkySettings` and keeps the "environment ⇒ hdri" branch byte-identical
  (measured: the HDRI sample still hashes `848cea2e6fdcc450` at
  `render_scale=1`; since the viewport and the stills supersample at
  `render_scale=2` — the terrain's per-cell normals alias into sparkles at
  close camera ranges — the current anchor is `716555b1dd1a4cfd`, mean 97.8,
  and any change to `render_scale` moves it).
- The analytic branch is enabled again when no HDRI is set, so Rendered has a
  sky instead of the neutral surround.
- AETHER is offered only at the shipped bake's parameters (turbidity 2.0,
  ground albedo 0.3); any other pair fails loudly in the service.
- Measured: turbidity 2→8 moves the frame 11.6 (mean abs), sky exposure
  0.05→0.10 moves it 33.8; no-HDRI sky mean 163.
- **Open engine item:** Hosek–Wilkie reads green-dominant (75/88/85 at
  exposure 0.02) where the CPU radiance table is blue and Preetham renders
  blue; it is also ~7× brighter at the same exposure. The Studio exposes the
  model instead of hiding the difference.

### 5.2 Water (0.5 day) — **done**
- Settings: `water.enabled`, `water.level`, `water.slopeThreshold` (a
  `WaterSettings` group on `RenderSettings`).
- Service: the terrain path has **no water-level knob** — it shades water where
  a *mask* says so, and `params.water` is inert there (measured: 0 pixels
  changed). The service therefore derives the mask with the engine's own
  `forge3d.gis.derive_water_mask` (low ground below the level, flat enough to
  hold it) and passes it as the `water_mask` argument. Measured: a mask moves
  1,422–14,373 pixels on the Bryce DEM depending on the slope threshold; a
  zero mask moves 0, so the mask — not the plumbing — is what draws the water.
- The level is in the DEM's metres and is scaled with the heights into the
  renderer's units, so the control means the same thing on a 4 km amphitheatre
  and a 350 km country. Level 0 = auto (the engine's 15th percentile).
- Defer water mask paths and optics (reflections, roughness): the water is the
  engine's depth-graded blue, not a physical surface.
- Check: schema contract, payload assertion, four service tests, round-trip.

### 5.3 Clouds (0.5 day) — **blocked, measured**
- Settings: `clouds.enabled`, `coverage`, `density`, `shadowStrength`.
- Service: `CloudSettings(enabled, shadows_enabled=…, coverage, density,
  shadow_strength, quality="medium")`.
- **A cloud shadow is not a post-process**, so the service-side route that
  shipped bloom and vignette cannot deliver it.
- The engine's cloud shadows are real — `ConcordiaFrame::configure_clouds`
  reads `coverage`, `density` and `shadow_strength` and drives
  `CloudShadowRenderer` — but they live in **`render_interactive`**, and
  `render_terrain_pbr_pom` takes no `clouds` argument at all. Measured:
  `CloudSettings(enabled, shadows_enabled, coverage=0.8, density=0.8,
  shadow_strength=0.6)` on the request changes **0 pixels**.
- `render_interactive` is itself **broken today**, for an unrelated reason:
  its shader assembly does `replace_once(source, "let shadow_z = h_curved *
  h_exag;", …)`, but the terrain shader now says `let shadow_z =
  det_barrier((h_curved - 0.5) * h_range) * h_exag;` — the TERRA-DETERMINATA
  rewrite changed the spelling. The path fails with `CONCORDIA shader contract:
  expected one "let shadow_z = h_curved * h_exag;", found 0` before it renders
  anything. The fix is in Rust (not a shader source, so no pinned-hash change),
  but there may be more stale contracts behind it.
- Adopting `render_interactive` for Studio would also move the render path
  (shared-depth HDR + the CONCORDIA surface), which changes the measured
  anchors — the HDRI sample hash `848cea2e6fdcc450` and the still/preview
  fidelity work.
- Trigger: someone needs clouds in a Studio render, and is willing to take the
  engine-side fix (repair the interactive path's contract, then adopt
  `render_interactive` with the anchors re-measured).

### 5.4 Post: bloom + lens vignette (0.5 day) — **done (service-side)**
- Settings: `bloom.enabled`, `threshold`, `softness`, `intensity`; `vignette`
  0–1. A `BloomSettings` group plus one scalar on `RenderSettings`.
- **Not the engine's path.** The engine's GPU bloom (`TerrainBloomProcessor`)
  and its vignette (in the viewer's terrain shader) live in render paths the
  Studio does not use: the terrain path Studio renders through has neither, and
  both were **measured to change 0 pixels** there. The engine-native wiring
  would mean an HDR target in the terrain path and a change to
  `terrain_pbr_pom.wgsl`, which is in `shader_sources::terrain_parts()` —
  that refreshes `PINNED_TERRAIN_SOURCE_HASH` (owner approval) and moves the
  determinism anchors.
- Shipped instead as a **service-side post-process** on the finished frame,
  next to the denoise: bloom is a bright-pass plus a separable box blur, the
  vignette a smooth radial falloff. Both are *display-referred* — the terrain
  shader has already tone-mapped the frame — so the bloom threshold is a
  display brightness (0.8), not the engine's HDR 1.5.
- Scope: the Rendered preview and the still show them; the Solid and Material
  previews do not, like AOVs and denoise. AOV files are written from the raw
  render, so they never carry the glow.
- Measured on the Bryce DEM: bloom off + vignette 0 vs the dry frame is
  **0 pixels changed** (byte-identical default); bloom on changes 760,908
  channel samples and lifts the brightest pixel 171 → 225; a 0.9 vignette takes
  the corner 159.7 → 16.7 and leaves the centre at 138.7.
- Defer DoF, motion blur, TAA: costly and rarely wanted.
- Check: schema contract, payload assertion, six service tests.

### 5.5 Shadows (0.5 day) — **partly done**
- Shipped: `shadowTechnique` (`pcf` | `pcss`) and `shadowResolution`
  (256–8192) on `RenderSettings`, through `to_payload` → `ShadowSettings`;
  verified on the GPU (PCSS and a 512² map change the frame).
- Not shipped: `shadowSoftness` and `ShadowSettings.intensity` are decoded but
  **inert** in `render_terrain_pbr_pom` (measured: softness 0.05 vs 8.0 and
  intensity 0 vs 1 produce identical pixels), so exposing them would be a
  silent no-op. The shader has to consume them first.
- Deferred: `cascades` (the terrain path keeps the engine's 3).

### 5.6 Colormap (0.5 day) — **done**
- Settings: `colormap` (engine name, `""` = none) and `colormapStrength`
  (0–1) on `RenderSettings`; payload `quality.colormap` /
  `quality.colormapStrength`.
- The engine ships the catalogue: `forge3d.available_colormaps()` is
  `forge3d:magma`, `forge3d:vik`, `forge3d:viridis`. `get_colormap` returns a
  `Colormap`; the terrain takes a `Colormap1D` built with `from_stops` over the
  elevation domain, wrapped in `OverlayLayer.from_colormap1d`. No engine code
  was added.
- **The colormap does nothing under `albedo_mode="material"`.** The shader's
  material branch assigns `material_albedo` last, so a colormap overlay
  measured **0** changed samples against the material set; `mix` measures
  162,541 and viridis vs magma 163,680 (Bryce, 640×400). The service
  therefore sends `albedo_mode="mix"` (material → ramp by strength) when a
  colormap is selected, and keeps `"material"` otherwise — a `mix` frame with
  no overlay would apply the engine's *fallback* LUT to the whole terrain.
- Strength 0 is off, not "a 0% ramp": a bound colormap at strength 0 still
  normalized the height the material layers sample by its domain, which moved
  162,905 samples (mean 4.4/255) against the same frame with no overlay. At 0
  the service binds no colormap, so strength 0 is byte-identical to the
  material set (measured: 0 changed samples).
- One LUT slot per frame: `extract_overlay_binding` binds the first overlay
  that has a LUT and stops, so a raster overlay and a colormap cannot both
  apply. The service **raises** instead of silently dropping one, and the
  Inspector disables the colormap while the terrain has a raster overlay
  (the selected value stays reachable so it can be cleared).
- Measured on the Swiss DEM at the gallery camera (922×795, zScale 1.0):
  material → viridis changes 2,189,640 samples (mean 34.1). The ramp renders,
  but at true scale the Alps are 1.1% relief — that is the vertical-scale
  default below, not a colormap defect.
- Check: schema contract + fixture, payload assertion, three service tests
  (built from the catalogue, strength 0 off, overlay conflict), and the
  byte-identical no-colormap still (`9f8f5c3c2f1ae4be` before and after).

### 5.7 A per-DEM vertical scale (0.5 day) — **done**
- `zScale` used to be `1.0` for every DEM. That is right for the bundled
  Bryce sample (454 m of relief across 4 km, 11.3%) and wrong for a
  country-size DEM: the Swiss Alps are 4,031 m across 352 km — 1.1% — so at
  true scale they render as a printed sheet.
- `scene::dem_default_z_scale` targets **6% relief**: `clamp(0.06 ×
  max_span / relief, 1.0, 50.0)`. It only ever exaggerates, so a DEM that
  already reads as terrain keeps its true scale: Bryce stays `1.0`, the Swiss
  becomes `5.246` (`0.06 × 352,427 / 4,031`). No footprint or no relief
  (a local DEM, a flat one) stays at `1.0` rather than guessing.
- Applied where a DEM is attached (`TerrainObject::with_dem_info`) and where
  one is replaced (`SetTerrainDem`, which re-derives unless the command names
  a scale; its undo carries the previous value, so undo restores `5.246`).
- The engine's range is the model's range: `TerrainRenderParams` raises
  `z_scale must be 0.1-50.0` instead of clamping, but `SetTerrainZScale`
  accepted any positive number (and the Inspector offered 0.05, then 20 as a
  maximum). Both now use `MIN_Z_SCALE`/`MAX_Z_SCALE` and the command rejects
  an out-of-range value with an error instead of producing a project the
  editor cannot validate.
- Measured on the Swiss DEM at the gallery camera (922×795, `render_scale`
  2): `1.0 → 5.246` changes 1,234,577 of 2,198,970 channel samples (mean
  10.16, frame mean 113.2 → 106.1). The relief is visible at 5.246 and the
  zScale-1/2.5/5.25/10 sheet in `/tmp/opencode/zs` shows why 5.25 was chosen
  over 10 (jagged) and 2.5 (still flat).
- Still open in the cartography tier: legend, despeckle, and the map plate.

### 5.8 Terrain sun shadows (0.5 day) — **done**
- Settings: `sunVisibility`, `sunVisibilityDistance` (metres, 100–20000) and
  `sunVisibilitySteps` (8–64) on `RenderSettings`; payload
  `quality.sunVisibility { enabled, distance, steps }`.
- The engine's `SunVisibilitySettings` ray-marches the heightfield toward the
  sun per pixel, so a ridge shadows the ground behind it — terrain
  self-shadowing the shadow map cannot resolve because it samples a different
  projection. Measured on the Bryce DEM (922×795, sun 18°): off → on changes
  395,389 of 2,198,970 channel samples (mean 3.47), and the still shows the
  valley shadows the shadow map leaves flat.
- Two dials, both measured: `distance` 2000 → 8000 m changes 38,304 samples
  (a short reach only catches nearby ridges), `steps` 32 → 16 changes 14,734
  (fewer steps miss a ridge line sooner).
- Not shipped: `mode="soft"` with 8 jittered samples moved 490 samples and
  `softness` 0.1 vs 0.5 moved 491 — a sub-pixel edge — so the service sends one
  hard ray. `resolution_scale` 0.5 moved 231 samples against 1.0, so the pass
  runs full-resolution.
- Not final-gated (unlike SSAO): one ray march per pixel is cheap enough for
  the Rendered viewport, so the shadows are visible while authoring.
- Measured inert on this path, all 0 samples: `ScreenSpaceSettings` SSAO,
  SSGI and SSR; `ReflectionSettings` (planar water reflections); every
  `TonemapSettings` operator (`reinhard`, `reinhard_extended`, `aces`,
  `uncharted2`, `exposure`) and white balance. The material-layer system is
  **blocked**: `vs_main` writes a constant `world_normal` (`(0,0,1)`), so the
  layer slope is ~0 — a rock layer at 0.001° changes 160,785 samples but at 1°
  changes **0**, even from a grazing camera (98,488 → 0). Altitude-driven snow
  would draw, but on cliffs too, so it is not shipped.
- Check: schema contract + fixture, payload assertion, one service test.

### 5.9 Despeckle a class overlay (0.25 day) — **done**
- `TerrainOverlay.despeckle` (serde-default `false`) → `overlay.despeckle` in the
  render request. The service runs the gallery land-cover recipe's own 3×3
  rule on a **single-band class raster** before paletting it: a class holding
  at most 2 cells of a 3×3 block (the cell included) is a speck, and it takes
  the block's majority when that majority holds at least 4 cells. Two passes.
  Class 0 (nodata) is never moved, so the raster's mask survives. An RGB
  raster is untouched — a drape is imagery, not classes.
- Measured on a synthetic 600² class raster (trees/crops/snow plus 6%
  salt-and-pepper), Swiss DEM at 640×400: off → on changes **55,383** of
  768,000 channel samples (mean 2.17), and the still goes from a speckled
  drape to clean class fields.
- The Inspector shows the toggle with the overlay; the hint says what it does
  and that an RGB raster ignores it.
- Check: schema/default test (phase4), payload assertion, service test on the
  rule (a lone cell absorbed, a 3×3 block kept, a clean raster unchanged).

### 5.10 The release audit, run after 5.9 — **green, one range fixed**
- **Every `RenderSettings` field reaches the payload.** 26 fields; each is read
  in `with_settings`/`to_payload` or, for `width`/`height`, is the request's
  frame size.
- **Payload and service agree in both directions.** Every key the service
  reads out of `quality` is a key `to_payload` sends, and every key
  `to_payload` sends is read by the service (the nested blocks included:
  `water`, `fog`, `bloom`, `sunVisibility`, `aov`). Checked by extracting both
  sets from the source, not by eye.
- **UI ranges sit inside the renderer's range.** One mismatch found and fixed:
  Width/Height offered 16384 while the render service caps a frame at 4096 px
  per side (the viewport already caps there too), so a larger still was being
  silently shrunk. The rest check out: vertical scale 0.1–50 (model and
  command too), sun reach 100–20000 m, ray steps 16–64 (engine `< 64`), sky
  turbidity 1–10, sky exposure 0–4, shadow map 512–4096 (engine `<= 8192`),
  colormap strength 0–1, overlay opacity 0–1, fog density `>= 0`.
- **The wiring is proven end to end**, not only in unit tests: an ignored GPU
  test (`the_new_settings_reach_the_pixels`) renders one frame twice (identical
  bytes), then with a colormap and with sun visibility (different bytes), so a
  key-name drift between the Rust payload and the service fails a test.
- **No client-side re-implementation.** Supersampling is `render_scale`; the
  class palette is `thematic.apply_palette`; the colormap is
  `Colormap1D`/`OverlayLayer.from_colormap1d`; the camera is
  `camera_render_view`; water, fog, shadows, sun visibility, sky and IBL are
  the engine's settings. Shell-side code is limited to what the engine has no
  consumer for on this path, and each case says why in code: bloom/vignette
  (needs an HDR target and a pinned-shader change), the water blob filter and
  the class despeckle (raster rules, not renderer mechanisms).

## 6. Increment 10.0 — High-quality stills: AOV + denoise (1 day) — **done**

**Trigger:** a user asks for AOVs or cleaner stills.
**Shipped:** `stillAov { albedo, normal, depth }` and `stillDenoise` on
`RenderSettings` (serde-default, v4-safe); final-gated in `to_payload`
(`preview` pays neither); `render_with_aov` writes `<name>_*_albedo/normal/
depth.png` next to the still; OIDN runs on the beauty with the AOV guidance
from the same frame and **fails loudly** when the runtime is missing
(`pip install oidn` / `pip install pyoidn`); two checkboxes in the render
section (no panel); sidecar names the actual renderer and flags. Previews
and viewport frames skip both entirely. AOV files are written for sequence
frames too (they are final renders), like `ambientOcclusion` today.
**Check (done):** schema contract + wire shape + round-trip tests, payload
assertions (final vs preview), `npm test` all green; GPU-verified albedo/
normal/depth AOVs written correctly and the denoise-missing failure path.

## 7. The two real design increments

### 7.1 Vector layers (3–5 days) — trigger: a user brings GeoJSON
This is the first feature the current `TerrainRenderParams` path cannot do, so
it is the only place worth adding a second path.

**Minimal model (schema v5 + migration):**
```
MapSettings { layers: Vec<VectorLayer> }
VectorLayer { id, name, asset, visible, style: { color, widthPx, opacity } }
```
- One new asset kind (`geojson`), registered through the existing `AddAsset`.
- One command `SetMapLayers { layers }` (whole list, like `SetChannelKeys`):
  add/edit/reorder/delete is one undo step; no per-property command zoo.
- New service command `render_map` that builds a `SceneRecipe` with the terrain
  plus `VectorOverlay` layers and renders via `MapScene`. Used only when the
  project has layers; terrain-only projects keep the current path unchanged.
- UI: a Layers list under the scene tree; import GeoJSON; flat style fields.
- Defer: labels, furniture, Mapbox style import, style expressions, PDF/SVG,
  plate layouts, alignment/reprojection. Each is a later trigger.
- Check: v4→v5 migration, layer round-trip/undo, one MapScene render smoke
  (GPU-opt-in).

### 7.2 Meshes in Rendered (5–10 days) — trigger: users need glTF in Rendered
**Lazy path only:** `io.import_gltf` + `geometry` + `instance_mesh_gpu_render`,
composited over the terrain frame with the same camera. No path tracer, no
BVH product path, no hybrid preview.
- Reuse the existing glTF asset and node selection.
- Service command `render_meshes` or an extra pass inside `render_map`.
- UI: none beyond what exists (objects already exist; Rendered just starts
  including them).
- Defer: LOD chain, tangents, PT, SDF, AOV per object.
- Check: a cube import renders over terrain at the viewport camera.

## 8. Explicitly not built (with the trigger that would change that)

| Feature | Why not now | Trigger |
|---|---|---|
| MapScene as the default path | current path already renders; rewrite risk | vector layers land |
| Labels / furniture / style import | big model + style spec surface | vectors shipped, users ask for text/legend |
| Point clouds / 3D Tiles / buildings | engine marks them underdeveloped/native-required | engine exposes an offline render |
| VT / scatter / smoke volumes | subsystems, not settings | a large-scene user asks |
| Camera rigs / CHRONOS / ANAMNESIS | current per-frame export works; caching is an optimization | sequences get slow |
| Path tracing / splat / inverse / globe | different product tier | explicit request |
| Bundles / certificates / provenance | no interchange/provenance need yet | sharing/verification requested |
| COG remote streaming | local COGs are GeoTIFFs and already import via rasterio | a URL-based DEM is requested |
| Licensing UI | no Pro-gated feature is exposed yet | first gated feature |
| Detached viewer / Jupyter widgets | Studio viewport is the product path | diagnostics request |
| Datasets / presets / material library | sample projects already exist | onboarding gap measured |
| Precision/dd harnesses | internal determinism tooling | a precision bug is reported |

## 9. Schema policy (keeps the lazy path honest)

- Optional scalar settings: no version bump; `#[serde(default)]` (established
  by `environment`, `environmentBackgroundIntensity`).
- New collections (`MapSettings.layers`): **v5 + `migrate_v4_to_v5`**; older
  builds must reject, not silently drop.
- Engine recipes are always derived; no engine struct in the schema.
- Bundle import/export stays a conversion, never the project format.

## 10. Testing that fits the work

- Rust: one schema-contract line per field, one round-trip test per group.
- Shell: one `to_payload` assertion per new request field.
- Python: one model-validation test per new engine dataclass.
- Browser: one manual check per UI group; no e2e framework.
- GPU: only when the pixel is the product (e.g. HDRI sky). Look settings do not
  need golden images; their payload is the contract.

## 11. Build order and effort

| Order | Increment | Days | Depends |
|---|---|---|---|
| 1 | 8.0 engine facts in probe | 0.5 | — |
| 2 | 9.0 sun from date/time | 0.5 | done |
| 3 | 5.1 sky model | 1 | 8.0 (feature list) |
| 4 | 5.2 water | 0.5 | — |
| 5 | 5.3 clouds | 0.5 | — |
| 6 | 5.4 post (bloom/vignette) | 0.5 | — |
| 7 | 5.5 shadows | 0.5 | — |
| 8 | 10.0 AOV/denoise stills | 1 | — | done |
| 9 | 7.1 vector layers | 3–5 | user request |
| 10 | 7.2 meshes | 5–10 | user request |

That is ~4.5 days for everything a terrain studio needs now, with the two heavy
items gated on real requests.

## 12. Next three commits

1. ~~`feat(studio): sun from date/time/location`~~ — **done**: `solar_position`
   one-shot, `sun_rotation` + ephemeris/round-trip tests, light inspector group.
2. `feat(studio): engine facts in the probe` — backend/adapter shipped; the
   granted-features snapshot and gating controls are still open.
3. `feat(studio): sky model and atmosphere settings` — §2 recipe for the four
   sky fields; payload + Python validation tests.

---

## Appendix A — module coverage decisions

Every public module from the earlier audit, now with a decision instead of a
promise. `build` = in the increments above; `trigger` = §8; `tool` = test/bench
surface; `out` = deliberately outside Studio.

| Module | Purpose | Decision |
|---|---|---|
| `terrain_params` | renderer settings catalog | build (5.x) |
| `offline`, `denoise`, `denoise_oidn` | offline quality/denoise | done (10.0) |
| `sky`, `atmosphere`, `geo`, `astro` | sky/sun/moon | build (5.1, 9.0) |
| `map_scene`, `vector`, `vector_line` | vectors/layers | trigger (7.1) |
| `style`, `style_expressions`, `raster_style`, `thematic`, `colors`, `materials`, `textures`, `presets` | styling/material library | trigger |
| `label_plan`, `text`, `text_atlas`, `legend`, `scale_bar`, `north_arrow`, `graticule` | cartography | trigger |
| `export`, `map_plate`, `publication` | print/vector export | trigger |
| `geometry`, `mesh`, `io`, `path_tracing` | meshes/PT | trigger (7.2) |
| `cog`, `gis`, `alignment`, `crs`, `preparation`, `codec` | geo data | trigger (local COG already works) |
| `datasets` | sample datasets | out (samples exist) |
| `pointcloud`, `tiles3d`, `buildings` | data layers | trigger (engine incomplete) |
| `terrain` (VT), `terrain_scatter`, `smoke`, `media` | surface systems | trigger |
| `camera_rigs`, `animation`, `chronos`, `anamnesis` | rigs/caching | trigger |
| `bundle`, `certificate`, `provenance`, `recipe_manifest`, `determinism`, `verify`, `precision`, `_license` | evidence/licensing | trigger (except engine facts 8.0) |
| `render_pass` | pass composition | trigger |
| `splat`, `inverse`, `sdf`, `lighting`, `guiding`, `landscape`, `viewer*`, `interactive`, `widgets` | lab/viewer | out |
| `config`, `diagnostics`, `mem`, `_gpu`, `_memory`, `_native`, `_degradation` | runtime plumbing | build only where 8.0 needs it |
| `bench`, `smoke` (harness), `terrain_demo`, `terrain_pbr_pom`, `_map_scene_*`, `_viewer_*`, `_png`, `_canonical_json`, `_deltae`, `_ed25519`, `_pointcloud_octree`, `_render_pass_bundle`, `_detail_normals`, `_cache_policy`, `_validate` | internal/tooling | tool |
| `_anamnesis_*`, `_anamnesis_params`, `_anamnesis_scene` | cache internals | trigger with 16 |

## Appendix B — Rust surface decisions

`terrain*`/`render*`/`scene`/`passes` → build only as settings reach the
payload; `vector`/`style`/`labels`/`export` → trigger with 7.1; `geometry`/
`mesh`/`accel`/`import`/`loaders` → trigger with 7.2; `gis`/`geo`/`converters`/
`codec`/`tiles3d`/`pointcloud`/`smoke`/`media`/`splat`/`sdf`/`viewer`/`astro`/
`lighting`/`license`/`verify` → trigger or out per Appendix A. Cargo features
(`copc_laz`, `cog_streaming`, `proj`, `enable-globe`, `enable-inverse-pt`,
`atmosphere-bake`, `splat-fusion`, `weighted-oit`) are reported by 8.0 and
never assumed.

## Appendix C — earlier phase mapping

| Earlier phase | Now |
|---|---|
| 8 capabilities/licensing | 8.0 (facts only; licensing deferred) |
| 9 MapScene recipes | not now; 7.1 uses MapScene only for vectors |
| 10 look | 5.1–5.5 |
| 11 cartography | trigger (7.1 then labels/furniture) |
| 12 geo/time | 9.0 sun; COG local already works; rest trigger |
| 13 data layers | trigger |
| 14 VT/scatter/volumes | trigger |
| 15 meshes/PT | 7.2 (raster only); PT out |
| 16 rigs/CHRONOS/ANAMNESIS | trigger |
| 17 bundles/evidence | trigger |
| 18 lab | out |
