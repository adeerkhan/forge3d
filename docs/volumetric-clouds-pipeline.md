# Volumetric Clouds — the whole pipeline

How the volumetric cloud layer in forge3d works, end to end: from a Python
`CloudSettings` object to the pixels on screen, what each shader stage does,
which knobs move the look, and the traps that cost us real debugging time.

Companion to the plain-language guide in `docs/volumetric-clouds.md` — that
page explains the *idea*; this page explains the *machine*.

```
python CloudSettings
      │  (validated in __post_init__: ranges, finiteness, phase_g ≤ 0.99)
      ▼
TerrainRenderParams.clouds
      ▼
render_params::decode_atmosphere::parse_clouds_settings
      ▼
DecodedTerrainSettings.clouds (CloudsSettingsNative)
      ▼
TerrainScene.clouds  ──►  CloudVolumeResources::render(...)          offline path
TerrainScene/ViewerTerrainScene ::clouds (ViewerCloudRenderer)       viewer path
      │  CPU-baked noise volumes → GPU textures; compute shader; storage copy
      ▼
clouds.wgsl raymarch → composite into the linear-HDR beauty
      ▼
offline:  per-sample jittered accumulation → tonemap → PNG
viewer:   tonemap blit to the surface
```

---

## 1. The two paths, one shader

| | Offline (`render_offline`) | Viewer (interactive IPC) |
| --- | --- | --- |
| Entry | `f3d.render_offline(..., clouds=CloudSettings(...))` | `{"cmd": "set_terrain_clouds", ...}` |
| Code | `src/terrain/renderer/offline.rs` (`render_offline_sample`) | `src/viewer/terrain/clouds.rs` (`ViewerCloudRenderer`) |
| Pass | `CloudVolumeResources::render` per accumulation sample | the same `render`, once per frame |
| Output | linear-HDR beauty → accumulate → tonemap | linear-HDR beauty → tonemap blit to the window |
| Antialiasing | accumulation jitter (`sample_index`, seeded) | single sample |

Both call the *same* `CloudVolumeResources` and the *same* `clouds.wgsl`.
The viewer wraps it with a tonemap pass into the surface format; the offline
path composites before the accumulate/tonemap stages.

`enabled == false` is a byte-exact no-op: the pass never dispatches, no
uniforms are written, nothing changes (guarded by `tests/test_terrain_clouds.py`).

## 2. Resources: what gets baked once

`src/core/cloud_volume.rs` builds, at `TerrainScene` construction:

| Resource | Size | Contents |
| --- | --- | --- |
| `base_noise` | 128³ R8Unorm | 4-octave tiling value-noise FBM, seed 0 |
| `detail_noise` | 32³ R8Unorm | 3-octave **ridged** value noise (wisp erosion) |
| `worley_noise` | 64³ R8Unorm | tiling Worley F1 cellular (the "glob") |
| `weather_map` | 128² R8Unorm | low-frequency 4-octave field — *where it is cloudier* |
| sampler | — | Repeat/Linear (all three axes) |
| uniform buffer | 256 B | `CloudUniforms` (see §4) |
| pipeline | — | `clouds_main`, `@workgroup_size(8, 8)` |

All noise is generated on the CPU with deterministic integer hashing
(`hash3` → 32-bit mixing) and uploaded through `tracked_create_texture`, so
the volumes are bit-identical on every run and every machine. Rows are padded
to the 256-byte copy alignment.

A **custom weather map** (`weather_map="path.png"`) is loaded through the
`image` crate the first time the path changes; changing only the path reloads,
steady-state renders do no file I/O.

## 3. The shader recipe (`src/shaders/atmosphere/clouds.wgsl`)

March: 56 steps along the view ray, 5 steps toward the sun per sample.

### 3.1 Ray setup

1. Reconstruct the view ray per pixel from `inv_view_proj` (world-space
   camera; the legacy `screen` 2.5D camera degrades with
   `terrain_clouds_require_world_camera` and renders no clouds).
2. Intersect the slab `[altitude, altitude+thickness]` around `bounds_center`
   with radius `extent_radius` (a flat disc in the plane ⟂ `up_axis`).
3. **Depth-aware early-out**: unproject the terrain depth; `t_far` is clamped
   to the first opaque surface, so clouds behind terrain never march.
   Requires a single-sample bindable depth (MSAA > 1 degrades with
   `terrain_clouds_require_single_sample_depth`).

### 3.2 Density at a point `p`

```
extent clip      →  0 outside the disc, soft edge 0.6..1.0 of it
weather map      →  coverage += (weather − 0.5) · weather_strength
base_shape(p)    →  (0.1 + 0.9·FBM) × (0.3 + 0.7·Worley)      ← multiply!
profile          →  smoothstep(0,0.06,h) · (1 − (1−h)^16)     ← pow-16 top
budget           →  shape − (1 − coverage) − 0.55·(1 − profile)
coverage cut     →  smoothstep(0, 0.10, budget)
erosion          →  detail = mix(1, d0·0.45 + d1·0.25 + w0·0.20 + w1·0.10, detail·(1−cut))
density          →  cut · detail · profile · edge · u.layer.w
```

Two decisions matter most for the look, both borrowed from the Frostbite /
Horizon Zero Dawn method (see §7):

- **The base shape is a *product*, not an average.** `(0.1+0.9·FBM) ×
  (0.3+0.7·Worley)` keeps high-contrast puffy interiors with hard zeros
  between puffs. Averaging the fields (our original form) washes the deck
  into a flat grey sheet.
- **The top rounds off with `pow(1−h, 16)`.** The body stays dense until near
  the top, then rounds off fast — flat bases, domed cauliflower tops. A
  `smoothstep(0.72, 1.0)` fade starts thinning the body too early and reads
  as a soft cone.

The coverage budget thins the cloud toward the top (`0.55·(1−profile)`), and
erosion is scaled by `1−cut` so thin edges break up more than thick cores
(height-gradient erosion).

### 3.3 Lighting

For each marching step with `density > 0`:

- **Sun term**: `sun_transmittance` — 5 steps toward the sun through the same
  density field, Beer–Lambert.
- **Powder term**: `1 − exp(−2·density·powder)` darkens the sunlit edge of
  dense regions, mixed at 0.4.
- **Phase**: dual-lobe Henyey–Greenstein — `HG(cos θ, phase_g)` (forward lobe)
  plus `HG(cos θ, 0.85) × 0.35` (multi-scatter approximation). The anisotropy
  is clamped to ±0.85 with a floored denominator in `hg_phase` so the
  forward-scatter peak cannot blow the cloud to white at `cos θ → 1`.
- **Ambient**: height-graded neutral grey, darker at the deck base and
  brighter toward the sunlit top, scaled by `scatter_strength`.
- **Energy-conserving integration**: inscatter accumulates
  `transmittance · (1 − exp(−ρ·dt)) · lit`, transmittance compounds by
  `exp(−ρ·dt)`; the march bails at transmittance < 0.02.

Composite: `beauty · transmittance + inscatter` — where the cloud is opaque it
covers the terrain; where thin, the terrain shows through.

### 3.4 Ground shadow

For pixels whose view ray hits the deck *and* the depth, a single sample is
taken at the deck mid-height offset along the sun's horizontal direction
(so the shadow is cast away from the sun), and the terrain color is scaled by
`mix(1, 1 − smoothstep(0, 1.1, deck), shadow_strength · 0.55)`. One sample is
cheap but approximate — a full second raymarch is future work.

### 3.5 Aerial perspective

Far cloud inscatter is blended toward an analytic sky color by
`0.7 · (1 − exp(−0.4 · t_far / terrain_span))` — the distance is measured in
scene-widths, so the fog engages at every world scale, not just metres.

## 4. Uniforms

`CloudUniforms` (WGSL struct, 256 B, `bytemuck::Pod`):

| Field | Packed from |
| --- | --- |
| `inv_view_proj` | per-frame inverse view-projection |
| `camera_pos`, `sun_direction`, `sun_radiance` | per-frame camera + decoded light |
| `layer` | `x altitude`, `y thickness`, `z coverage`, `w density` |
| `optics` | `x scatter_strength`, `y phase_g`, `z detail`, `w powder` |
| `wind` | `x dir_rad`, `y speed`, `z time`, `w sample_index` |
| `screen` | `x width`, `y height` |
| `up_axis` | `xyz` world up of the camera frame, `w = 1/terrain_span` |
| `bounds` | `xyz` deck centre, `w extent_radius` |
| `feature` | `x = 1/size` (puff scale), `y shadow_strength` |
| `weather` | `x weather_strength` |

Bindings: 0 uniforms, 1 base_noise, 2 sampler, 3 detail_noise, 4 sampler,
5 depth, 6 beauty_in, 7 clouds_out (storage), 8 worley, 9 weather map,
10 sampler. The bind group is cached and rebuilt only when the output size,
the weather path, or the beauty/depth *view identity* changes (wgpu ids are
globally unique and never reused, so a stale view can never serve a frame).

## 5. Units — the trap everyone falls into

`altitude_m` / `thickness_m` **do not mean metres on the offline path**. The
shader places the slab at world heights `[altitude, altitude+thickness]`, and
the offline terrain's world height is `heightmap · z_scale` (peaks ≈ `z_scale`).
The `_m` defaults (1500/800) are for the metre-based geospatial viewer; on the
offline path they float the deck off-screen. The interpreter/example scripts
place the deck as `z_scale · factor` above the peaks instead.

Rendering a **real geospatial DEM** offline (Bryce-style) needs the world
scaled: 1 world unit = 0.05 m worked well — `terrain_span = width_m / 20`,
`z_scale = relief_m / 20`, deck/camera/radius all divided by 20, and normalize
the heightmap against the **context DEM's** minimum so the camera world heights
line up with the terrain datum. `z_scale` is validated to ≤ 50, which is what
forces the scaling.

`up_axis` is `[0,1,0]` for `mesh`/Y-up cameras and `[0,0,1]` for `mesh:zup`.
Noise is scaled by `1/terrain_span` so puff *sizes* are a property of the
scene, not the units.

## 6. Determinism, provenance, degradation

- **Determinism**: the shader runs the `det_*` prelude (`includes/determinism.wgsl`;
  `det_div`, `det_barrier`, `det_pow`, `det_mix` …) and `clouds.wgsl` is part
  of `terrain_parts()`, so `FORGE3D_DET_REWRITE=1 cargo test det_instrument_rewrite --lib`
  must stay green after any shader edit (it gates `PINNED_TERRAIN_SOURCE_HASH`).
  Offline, the per-sample jitter (`sample_index` seeded by `aa_seed`) is
  averaged by accumulation, which denoises the march for free. The viewer has
  no accumulation yet.
- **Provenance**: the shader is registered under `terrain.clouds.shader`;
  a dispatched pass calls `record_shader_use`, which lands in the render
  certificate's provenance set.
- **Degradations are explicit, never silent**: `terrain_clouds_require_world_camera`,
  `terrain_clouds_require_single_sample_depth`, `terrain_clouds_require_hdr_beauty`
  (the composite needs the `Rgba16Float` beauty), `terrain_clouds_offline_only`
  (one-shot forward path). Each renders the honest fallback and records why.

## 7. What we learned from the reference implementation

The `bevy-volumetric-clouds` reference (evroon's Rust port of the Frostbite /
Horizon Zero Dawn clouds) was studied side-by-side; the differences that
actually matter for the look:

| bevy reference | forge3d before | forge3d now |
| --- | --- | --- |
| `(0.1+0.9·perlin_fbm×7) · (0.3+0.7·voronoi×8)` | `wor·0.55 + fbm·0.45` (average) | same multiply composite |
| top = `1 − pow(1−h, 16)` | `1 − smoothstep(0.72, 1.0, h)` | `1 − pow(1−h, 16)` |
| coverage floor baked in a noise channel | separate weather map | weather map (equally expressive) |
| 3-scale worley detail (`r + g/2 + b/4`) | single ridged detail volume | detail + 2 worley scales |
| `S·(1−ΔT)/ρ` energy-conserving integration | same | same |
| planet-sphere slab intersection (infinite deck) | bounded local deck | bounded deck (correct for scenes) |
| temporal reprojection storage texture | accumulation (offline) | accumulation (offline) |

The two structural gaps left are the baked noise *quality* (their atlas is a
7-octave perlin × 8-octave voronoi baked on the GPU; ours is 4-octave value
noise baked on the CPU) and viewer-side temporal reprojection. Both are
future work, not correctness.

## 8. Knobs, and what they actually do

| Knob | Effect | Tuned starting point |
| --- | --- | --- |
| `coverage` | sky fill; low = separate puffs | 0.42–0.5 |
| `density` | opacity multiplier | 0.6 |
| `altitude_m`/`thickness_m` | deck placement (**world units offline**) | just above the peaks |
| `size` | puff scale (divides noise frequency) | 2.5–3.5 |
| `scatter_strength` | in-scatter **and** ambient multiplier | ~2.4 (≥ 8 blows out to white) |
| `phase_g` | forward-scatter anisotropy | 0.8 (clamped to ±0.85 in-shader) |
| `detail` | edge erosion amount | 0.5 |
| `powder` | dark-edge strength | 0.55 |
| `weather_strength` | how much the weather field varies coverage | 0.5–0.6 |
| `wind_dir`, `wind_speed` | advection (needs a sequence; a still has t=0) | 45–60°, 0 for stills |
| `shadows_enabled`, `shadow_strength` | terrain shadow from the deck | on, 0.4 |

`examples/terrain_volumetric_clouds_hdri.py --preset {cumulus,scattered,storm,
cirrus,overcast}` is the reference tuning UI; `examples/bryce_canyon_clouds_flyover.py`
is the same look on a real DEM.

## 9. Where things live

| File | Role |
| --- | --- |
| `src/shaders/atmosphere/clouds.wgsl` | the raymarch (recipe §3) |
| `src/core/cloud_volume.rs` | noise bake, pipeline, uniforms, bind-group cache, weather reload |
| `src/terrain/renderer/offline.rs` | offline call site (per sample, after terrain, before accumulate) |
| `src/viewer/terrain/clouds.rs` | viewer wrapper + tonemap pass |
| `src/terrain/render_params/decode_atmosphere.rs` | Python → native decode |
| `src/terrain/render_params/native_postfx/atmosphere.rs` | `CloudsSettingsNative` + defaults |
| `python/forge3d/terrain_params.py` | `CloudSettings` + presets |
| `src/terrain/renderer/skybox.rs`, `src/shaders/skybox.wgsl` | the HDRI background behind the deck |
| `docs/volumetric-clouds.md` | the plain-language guide |

## 10. Known limits

- No temporal reprojection in the viewer (offline accumulation only) — the
  viewer's march shows per-frame jitter.
- The ground shadow is one sun-offset sample, not a second raymarch.
- One-shot forward (non-accumulating) renders record
  `terrain_clouds_offline_only` and draw no clouds.
- No cloud × atmosphere-scattering coupling (clouds sit *in front of* the
  sky LUTs, they do not occlude/light each other through them).
- Clouds composite on the offline/HDR path only; an `Rgba8Unorm` one-shot
  target degrades honestly via `terrain_clouds_require_hdr_beauty`.
