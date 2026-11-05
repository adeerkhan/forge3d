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
| Timeline drawer, render queue, asset integrity, samples | done |
| Everything below (§3–§6) | **not started** |

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
| Bridge | `src/bridge/mock.ts` | preview semantics (mirrors core) |
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
2. `types.ts` + `mock.ts`: add the field to `RenderSettings` and the default.
3. `render.rs`: copy it in `with_settings`, emit it in `to_payload`.
4. `studio_render.py`: map it to the engine dataclass in `render()`.
5. `Inspector.tsx`: one control (`NumberField`/`Toggle`/`RangeField`).
6. Tests: one schema-contract line, one payload assertion, one round-trip test.

Five places (six files counting the mock), one test each, no architecture. The
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

### 5.1 Sky model and atmosphere (1 day)
- Settings: `skyModel` (`hosek-wilkie` | `preetham` | `aether` | `hdri`),
  `turbidity` (1–10), `groundAlbedo` (0–1), `skyExposure` (0–4).
- Service: `SkySettings(enabled=sky, model=…, turbidity=…, ground_albedo=…,
  sky_exposure=…)`. Keep the existing "environment ⇒ hdri" branch.
- **Defer AETHER**: it needs `atmosphere_bake_luts` and a stored LUT handle.
  The UI option stays disabled with "requires bake" until asked.
- Check: payload fields + Python `SkySettings` validation test.

### 5.2 Water (0.5 day)
- Settings: `water.enabled`, `water.level`, `water.slopeThreshold`.
- Service: `WaterSettings(enabled, level, slope_threshold, optics=default)`.
- Defer water mask paths and optics.
- Check: round-trip.

### 5.3 Clouds (0.5 day)
- Settings: `clouds.enabled`, `coverage`, `density`, `shadowStrength`.
- Service: `CloudSettings(enabled, shadows_enabled=…, coverage, density,
  shadow_strength, quality="medium")`.
- Check: round-trip.

### 5.4 Post: bloom + lens vignette (0.5 day)
- Settings: `bloom.enabled`, `threshold`, `softness`, `intensity`;
  `vignette` 0–1 (`LensEffectsSettings.vignette_strength`).
- Defer DoF, motion blur, TAA: costly and rarely wanted.
- Check: round-trip.

### 5.5 Shadows (0.5 day)
- Settings: `shadowTechnique` (`pcf` | `pcss`), `shadowSoftness`, `cascades`.
- Service: `ShadowSettings` fields; keep the existing tuned biases.
- Check: round-trip; one GPU smoke only if the pixel is the deliverable.

## 6. Increment 10.0 — High-quality stills: AOV + denoise (1 day)

**Trigger:** a user asks for AOVs or cleaner stills.
**Reuse:** the existing still path (`render_still`), `OfflineQualitySettings`,
`oidn_denoise`.

- Settings: `stillAov: { albedo, normal, depth }` booleans, `stillDenoise` bool.
- Service: pass `AovSettings` to `render_with_aov` when any AOV is on; write
  the AOV PNGs next to the still; run OIDN on the beauty when requested.
- UI: two checkboxes in the render section; no new panel.
- Check: files exist + naming; shell test for the payload.

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
| 2 | 9.0 sun from date/time | 0.5 | — |
| 3 | 5.1 sky model | 1 | 8.0 (feature list) |
| 4 | 5.2 water | 0.5 | — |
| 5 | 5.3 clouds | 0.5 | — |
| 6 | 5.4 post (bloom/vignette) | 0.5 | — |
| 7 | 5.5 shadows | 0.5 | — |
| 8 | 10.0 AOV/denoise stills | 1 | — |
| 9 | 7.1 vector layers | 3–5 | user request |
| 10 | 7.2 meshes | 5–10 | user request |

That is ~4.5 days for everything a terrain studio needs now, with the two heavy
items gated on real requests.

## 12. Next three commits

1. `feat(studio): engine facts in the probe` — `PROBE_SCRIPT`, `EngineStatus`,
   `types.ts`, `StatusBar.tsx`; fixture test.
2. `feat(studio): sun from date/time/location` — `solar_position` command,
   `sun_transform` + round-trip test, light inspector dialog.
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
| `offline`, `denoise`, `denoise_oidn` | offline quality/denoise | build (10.0) |
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
