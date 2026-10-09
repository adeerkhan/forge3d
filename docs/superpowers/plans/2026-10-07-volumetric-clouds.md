# Volumetric Clouds in the DEM 3D Renderer — Implementation Plan

> **Status:** PROPOSED — not started. Branch `feat/volumetric-clouds`.
> **Owner decisions required:** see [Open Decisions](#open-decisions). Two answers change
> sequencing before implementation begins.

**Goal:** A depth-aware, ray-marched volumetric cloud layer that composites into the
linear-HDR terrain beauty buffer after terrain and before tonemap/accumulation, so
clouds occlude correctly behind terrain and are lit consistently with the atmosphere
LUTs. Offline quality path first, then the one-shot forward path.

**Non-goals (phase 1):** temporal reprojection of cloud history, cloud-driven
atmospheric scattering coupling, and any CPU fallback. If native clouds are unavailable
in MapScene, raise `MapSceneNativeUnavailable` — never a CPU fake.

---

## Context: why the composite lands where it does

Tonemapping happens **inside the terrain fragment shader** (`tonemap_aces` +
`linear_to_srgb`), gated by `params5.z = output_srgb_eotf` and
`params5.w = offline_hdr_output`. That fixes the insertion point:

- **Offline path** already produces **linear-HDR per-sample beauty**
  (`offline_hdr_output=1`) and tonemaps at the end
  (`dispatch_offline_tonemap_pass`). This is the natural home for a depth-aware
  cloud composite.
- **One-shot forward path** writes display-referred output from the terrain shader
  into a target that is `Rgba8Unorm` by default (`Rgba16Float` only in deterministic
  mode — `constructor.rs:64-66`). A depth-aware composite there requires routing
  terrain output through linear HDR first.

### Target architecture

A **depth-aware volumetric cloud raymarch** composited into the linear-HDR beauty
buffer after terrain, before tonemap/accumulation:

```
render_sky_texture()
blit/copy sky -> internal (background, depth cleared)
run_main_pass()                    <- terrain forward, output LINEAR HDR
render_volumetric_clouds()         <- NEW: ray-march slab, early-out on depth_view
accumulate / tonemap (offline)     <- existing
resolve_output()
```

- Offline: insert between `run_main_pass_with_aov_pipeline` (`offline.rs:843`) and
  `dispatch_offline_accumulation_pass` (`offline.rs:869`), **per sample**, with the
  cloud ray jittered by the existing per-sample seed -> accumulation denoises the
  clouds for free.

### Hard constraint

The depth target is `Depth32Float` with `TEXTURE_BINDING` usage **only when
`effective_msaa == 1`** (`draw/setup/pipeline.rs:281-285`). Confirmed in code:
depth exposes `TEXTURE_BINDING | COPY_SRC` only for `effective_msaa == 1`, else
`TextureUsages::empty()`. Therefore:

- The depth-aware composite **requires single-sample depth**.
- Offline is single-sample (AA via accumulation) ✔.
- For MSAA > 1 one-shot, either resolve depth, or **degrade explicitly** and record
  it.

---

## Target architecture (files)

| Area | File(s) | Change |
| --- | --- | --- |
| Shaders | `src/shaders/atmosphere/clouds.wgsl` (new) | fullscreen compute raymarch |
| Shaders | `src/shaders/atmosphere/clouds_sky_baked.wgsl` (optional) | fallback path |
| Renderer | `src/terrain/renderer/clouds.rs` (new) | resources + `render_volumetric_clouds()` |
| Renderer | `src/terrain/renderer/atmosphere.rs` | lazy resource creation in `create_atmosphere_init_resources` |
| Renderer | `src/terrain/renderer/core.rs` | hold `CloudVolumeResources` on `TerrainScene` |
| Renderer | `src/terrain/renderer/offline.rs` | call site after main pass, before accumulation |
| Renderer | `src/terrain/renderer/py_api.rs`, `aov.rs` | forward-path call sites |
| Params | `src/terrain/render_params/core.rs` | `CloudsSettingsNative` on `DecodedTerrainSettings` |
| Params | `src/terrain/render_params/decode_atmosphere.rs` | `parse_clouds_settings()` |
| Params | `src/terrain/render_params/private_impl.rs`, `native_postfx.rs` | import / export |
| Python | `python/forge3d/terrain_params.py` | extend `CloudSettings`, presets |

---

## Work breakdown

### 1. Shaders

- **`src/shaders/atmosphere/clouds.wgsl`** (new): fullscreen compute.
  - Inputs: `inv_view_proj` + camera, sun dir/intensity, cloud params, terrain depth
    (`texture_depth_2d`), 3D base + detail noise, sky/atmosphere sample for ambient.
  - Output: `color`, `transmittance` — or composite directly over an input beauty
    texture via storage write / load-store.
  - Density: base FBM -> coverage remap -> Worley/detail erosion -> slab vertical
    profile.
  - Lighting: 4–6-step sun march, Beer–Lambert, HG phase, multiple-scatter/powder term,
    sky-radiance ambient.
  - March: front-to-back transmittance, blue-noise/ordered jitter (seeded by sample
    index offline), adaptive step.
  - Reuse `includes/determinism.wgsl` + `det_div`/`det_barrier` patterns
    (required by the determinism lint).
- Optional **`clouds_sky_baked.wgsl`** for the fallback path.

### 2. Rust renderer — `src/terrain/renderer/`

- **New `clouds.rs`:** `CloudVolumeResources` (tracked 3D noise textures, compute
  pipeline(s), uniform buffer, bind-group layouts, per-frame bind groups),
  `render_volumetric_clouds(encoder, render_targets, decoded, camera, sun, sample_index)`.
  - All allocations via `tracked_create_*`; wrap multi-resource lifecycles in Drop
    guards.
  - Create resources lazily in `create_atmosphere_init_resources` (`atmosphere.rs`) and
    store on `TerrainScene` (`core.rs`), reusing the existing `AtmosphereGpuLuts` for
    ambient/transmittance so clouds and sky agree.
  - Call site in `offline.rs` (after main pass, before accumulation) and in
    `py_api.rs`/`aov.rs` for the forward paths.
  - Record GPU timing (`ts_begin`/`ts_end`) and `record_shader_use("terrain.clouds.shader")`;
    register budget; emit `record_degradation` if depth isn't bindable (MSAA > 1).
  - Reuse noise generation from `src/core/clouds/renderer/` — `build_noise_data`
    is defined in `data.rs` and called from `textures.rs`; extend to
    base(128³) + detail(32³).

### 3. Rust render params — `src/terrain/render_params/`

- Add `CloudsSettingsNative` to `DecodedTerrainSettings` (`core.rs`).
- `parse_clouds_settings(&params)` in `decode_atmosphere.rs` mirroring
  `parse_sky_settings` (reads `params.clouds`, defaults when `None`); import in
  `private_impl.rs`.
- Export the native type alongside `SkySettingsNative` in `native_postfx.rs`.

### 4. Python — `python/forge3d/terrain_params.py`

- Extend `CloudSettings`: `enabled, coverage, density, altitude_m, thickness_m,
  scatter_strength, phase_g, detail, wind_dir, wind_speed, powder, mode, quality`.
  Validate in `__post_init__`, add `to_dict`/`from_mapping`.
- Add presets (`fair-weather`, `scattered`, `storm`, `cirrus`) usable from
  `LightingPreset.settings["clouds"]` and `TerrainSource.metadata["clouds"]`
  (MapScene already reads those — `_mapscene_cloud_config`, `map_scene.py:973`).
- Thread through `TerrainRenderParams` (already has the `clouds` field) so it reaches
  decode.

### 5. Unify shadows + retire the screen-space path

- Drive the existing `cloud_shadows` mask (`src/core/cloud_shadows/`) from the **same
  3D density field** so terrain shadows match visible clouds.
- Replace MapScene's NumPy darkening (`_apply_mapscene_cloud_shadow`) with the native
  shadow texture when clouds are enabled; keep the NumPy path only as an explicit,
  recorded degradation.
- Retire `Scene::enable_clouds`'s screen-space `clouds.wgsl` + `src/core/clouds/renderer`
  draw path; repoint the `Scene` py methods to the new volumetric implementation (or
  mark them deprecated and point to `CloudSettings`), and add the dead-structure gate
  entries. Delete the orphaned references to the non-existent
  `tests/test_b8_clouds.py` / `examples/clouds_demo.py`.

### 6. Tests / gates / docs

- **Unit:** camera-altitude parallax moves the field; slab bounds respected;
  `transmittance -> 1` when disabled (exact no-op — required, like fog).
- **Golden:** `mapscene_volumetric_clouds` recipe (generated only via
  `certificate-refresh.yml`).
- **Degradation test** for MSAA > 1 / missing depth binding.
- **Consistency test:** shadow mask ↔ cloud density.
- **Docs:** `docs/` gallery page + example (engine path, no PIL).

---

## Repo guardrails this must clear

- **Allocation gate** (`tests/test_allocation_gate.py`) — no raw
  `create_buffer`/`create_texture` outside `src/core/resource_tracker.rs`.
- **Budget ENFORCE** — noise volumes + intermediates budgeted/tracked.
- **Determinism** — `FORGE3D_DET_REWRITE=1 cargo test det_instrument_rewrite --lib`;
  possible `PINNED_TERRAIN_SOURCE_HASH` owner approval.
- **Certificate contract** for any new public `render_*`; GPU timings per pass.
- **`-D warnings` clippy**; recipe goldens only via `certificate-refresh.yml`.
- **Zero-placeholder** — if native clouds unavailable in MapScene ->
  `MapSceneNativeUnavailable`, never a CPU fake.

---

## Phasing

### Phase 1 — Params plumbing + noise + offline depth-aware composite  *(quality path end-to-end)*

1. `CloudsSettingsNative` + `parse_clouds_settings` + native export (work item 3).
2. Python `CloudSettings` fields + presets + threading (work item 4).
3. Noise volume generation: base(128³) + detail(32³), tracked.
4. `clouds.wgsl` compute raymarch + `CloudVolumeResources`.
5. Insert into `offline.rs` between main pass and accumulation; per-sample jitter.
6. One golden: `mapscene_volumetric_clouds`.
7. Degradation + no-op + parallax unit tests.

**Exit criteria:** offline render shows correctly occluded, accumulation-denoised
clouds; disabled is a byte-exact no-op; MSAA > 1 records a degradation.

### Phase 2 — One-shot forward path via HDR reroute

- Terrain -> `Rgba16Float` beauty -> cloud composite -> tonemap blit reusing
  `dispatch_offline_tonemap_pass`.
- *(Interim alternative if deferred: ship sky-baked clouds in one-shot only — see
  Open Decision 1.)*

### Phase 3 — Unify cloud shadows + retire the `Scene` screen-space path

- Single density field drives both visible clouds and the shadow mask.
- Retire `Scene::enable_clouds` screen-space path; dead-structure gate entries.

### Phase 4 — Docs / example / gallery

---

## Open decisions

### 1. One-shot forward path

Full HDR reroute so one-shot stills get true depth-aware clouds, **or** ship sky-baked
there and keep depth-aware offline-only for phase 1?

**Recommendation:** ship sky-baked in one-shot for Phase 1 (keeps Phase 1 shippable and
independent), then do the HDR reroute as Phase 2. Rationale: the HDR reroute touches the
format/determinism contract and should land as its own reviewed change, not bundled with
the first cloud composite.

### 2. Output fidelity / target format

The single `Rgba8Unorm` / `Rgba16Float` split (`constructor.rs:64-66`) means HDR clouds
only look right on the deterministic/HDR path. Should clouds **force** the HDR target,
or follow the existing format choice?

**Recommendation:** follow the existing format choice, but record an explicit
`record_degradation` (banding / clipped highlights) when clouds are enabled on the
`Rgba8Unorm` one-shot target. Forcing HDR globally would change unrelated output and
violate the "no silent change" policy. If a user wants full-fidelity clouds, they use the
offline/HDR path.

---

## First start (concrete first step)

**Phase 1, step 1–2: land the params plumbing only, with a disabled-by-default no-op,
and prove nothing changes.**

1. Add `CloudsSettingsNative` to `DecodedTerrainSettings` and `parse_clouds_settings()`
   in `decode_atmosphere.rs`; default to absent/disabled when `params.clouds is None`.
2. Extend Python `CloudSettings` with the field set + validation + presets.
3. Add a test asserting that with `clouds.enabled == False`, the decoded settings and
   render output are byte-identical to the current baseline (the no-op guarantee).
4. Wire `record_degradation` paths but do not yet allocate noise or dispatch.

This keeps every existing test green, establishes the params contract, and makes the
first GPU work (noise + raymarch) a purely additive change behind a flag.
