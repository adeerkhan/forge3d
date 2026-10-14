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
   Also intersect the vertical cylinder of that disc
   (`intersect_cylinder`), and march the **intersection** of the two
   (`t_lo = max(slab.x, cyl.x)`, `t_hi = min(slab.y, cyl.y)`).
3. **Depth-aware early-out**: unproject the terrain depth; `t_far` is clamped
   to the first opaque surface, so clouds behind terrain never march.
   Requires a single-sample bindable depth (MSAA > 1 degrades with
   `terrain_clouds_require_single_sample_depth`).

> **Why the cylinder bound is load-bearing.** The slab is infinite
> horizontally, so a near-horizontal ray crosses it over thousands of world
> units. With a fixed step count, `dt = (t_far − t_near) / MARCH_STEPS` then
> balloons far past the deck thickness and the deck is skipped entirely — the
> clouds vanish exactly when the camera sits level with the deck, which is
> precisely a flythrough shot. Bounding by the cylinder keeps `dt` inside the
> deck: at a 0.25° grazing ray it goes from `dt ≈ 6.5` (≈4× the deck) to
> `dt ≈ 1.1`, while rays ≥ 5° to the slab are bit-identical (the slab already
> governed). `cloud_density` returns 0 beyond `extent_radius`, so the bound
> only removes steps that would have sampled empty space.

### 3.2 Density at a point `p`

```
extent clip      →  0 outside the disc, soft edge 0.6..1.0 of it
weather map      →  coverage += (weather − 0.5) · weather_strength
base_shape(p)    →  (0.1 + 0.9·FBM) × (0.3 + 0.7·Worley)      ← multiply!
profile          →  smoothstep(0,0.06,h) · (1 − (1−h)^10)     ← pow-10 top
budget           →  shape − (1 − coverage) − 0.55·(1 − profile)
coverage cut     →  smoothstep(0, 0.16, eroded)
erosion          →  detail = mix(1, d0·0.45 + d1·0.15 + w0·0.30 + w1·0.10, amount)
density          →  cut · detail · profile · edge · u.layer.w
```

Two decisions matter most for the look, both borrowed from the Frostbite /
Horizon Zero Dawn method (see §7):

- **The base shape is a *product*, not an average.** `(0.1+0.9·FBM) ×
  (0.3+0.7·Worley)` keeps high-contrast puffy interiors with hard zeros
  between puffs. Averaging the fields (our original form) washes the deck
  into a flat grey sheet.
- **The top rounds off with `pow(1−h, 10)`.** The body stays dense until near
  the top, then rounds off fast — flat bases, domed cauliflower tops. A
  `smoothstep(0.72, 1.0)` fade starts thinning the body too early and reads
  as a soft cone.

The coverage budget thins the cloud toward the top (`0.55·(1−profile)`), and
erosion is scaled by `1−cut` so thin edges break up more than thick cores
(height-gradient erosion).

**The coverage ramp is the chunkiest single knob.** It must be *narrow* enough
to give puffs a defined silhouette — a wide ramp (0.30) smears the deck into an
even haze that reads as fog, not cloud. An earlier build needed 0.30 only
because the fractal octaves were putting sub-pixel detail on the boundary, and
the aliased boundary then depended on jitter to look smooth. Capping the octaves
to resolvable feature sizes (below) removed that cause, so the ramp went back to
0.16 and the puffs regained their edges without bringing the sparkle back.

The other load-bearing rule is that the erosion frequencies stay **inside each
baked volume's Nyquist limit**. Sampling near a volume's texel frequency aliases
into a halo of isolated pixels that no amount of averaging removes — that was
the original "dispersal pixels" at the cloud edge.

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

### 3.6 Two styles, one pass

`CloudSettings.style` selects `"realistic"` (the recipe above) or `"cartoon"`.
The style flag rides in the uniform (`style.x`); `cloud_density` is a dispatcher
that routes to `realistic_density` or `cartoon_density`, and the march swaps the
per-sample `lit` term. Everything else — slab intersection, the cylinder bound,
the depth early-out, the ground shadow, the composite — is shared, so a scene
does not move or change cost when it switches style.

- **`cartoon_density`** drops the domain warp and the fine erosion and instead
  takes a coarse cellular field mixed with the low octave of the fractal,
  pushed through a wide S-curve into distinct rounded puffs, then cut by
  coverage. No detail term multiplies the cut, so the boundary is the shape's
  own contour: a single hard edge.
- **`cartoon_lit`** quantizes a combined sun + height illumination into
  `style.y` (3) flat bands on an explicit shadow/mid/lit ramp, brightens the
  crown and darkens the underside (`style.z`) — the cel fill.

Aerial perspective is skipped for the cartoon style (`1 - style.x`) so the flat
fills stay pure.

### 3.7 Realism notes (what fixed the "soft smear")

Two changes to the realistic path came out of the same review:

- **Erosion now reaches the silhouette.** The boundary had been exempted from
  detail erosion to cure a sparse-pixel halo, which also removed the fine
  nibble that breaks a smooth contour into cauliflower lobes — the deck read as
  one soft smear. The erosion floor at the edge is now `0.40` (offline
  accumulation averages the resulting per-sample jitter).
- **Weaker aerial fog.** The far-fade cap dropped from `0.7` to `0.45` at a
  slower `0.22` rate (was `0.4`), and the analytic sky base was warmed. At
  flyover distances the old values replaced most of a distant cloud's colour
  with a flat blue wash, which is what greyed the deck out.
- **Brighter, warmer ambient** top tone so the deck stops reading as painted
  cardboard.

The remaining sharpness gap is the baked-noise resolution (see §7): the deck's
finest structure is fixed by the 128³ / 32³ / 64³ volumes, not by the shading.

### 3.8 Viewer-specific fixes (Bryce Canyon flyover)

Three viewer-only defects surfaced when the deck was first driven from the
interactive IPC path; the offline path was unaffected.

- **Noise scale.** The viewer scaled cloud noise by `1 / terrain_span`. On the
  world-unit offline demos (span ≈ 3) that is fine, but on a geospatial scene
  (Bryce's context DEM is 12,288 m) it stretches one noise period across the
  whole map, so the whole deck samples one point of the field and flattens into
  a uniform (often fully-off) sheet. `cloud_noise_scale` now caps the reference
  span at `CLOUD_NOISE_REFERENCE_SPAN_M` (1200 m) so puffs stay ~1 km — the
  physical cumulus size — regardless of how large the scene is.
- **Snapshot sky erased the deck.** The viewer's snapshot path calls
  `present_snapshot_sky` *after* `render_to_texture`, repainting the sky
  wherever depth == far. That pass owns the background, so it erased every
  cloud in front of it — the deck reached the beauty and the tonemap, then lost
  the sky pixels to the sky pass. It is now skipped when
  `pbr_config.clouds.enabled`; the caller sets `background` for the clear
  colour, exactly as the offline path does.
- **`scatter_strength` blowout.** Both Bryce scripts hard-coded
  `scatter_strength: 16.0` (the docs put the white-blowout threshold at ~8). The
  default is now 2.2 and exposed as `--cloud-scatter` / `--scatter`.

A residual: the example's `with_sky` recolour matches the background by colour,
so once clouds fill the sky its corner-sample heuristic can mis-key. Use the
cloud shader's own sky (skip `with_sky`) or a distinctive `background` colour.

### 3.9 Clouds in the live window

The interactive window never drew the deck at all — `render_viewer_clouds` had
exactly one caller, the snapshot path — even though §1 documents the window
taking the cloud pass. Fixed: when `clouds.enabled`, the window path renders the
same offscreen linear-HDR composite (`render_to_texture`) and blits it onto the
swapchain, so the window and the saved frames agree by construction.

Two supporting details:

- The swapchain's `usage` gains `COPY_DST` (`init/device_init.rs`); with
  `RENDER_ATTACHMENT` only, that blit fails wgpu validation with a bare
  `Copy error` — the deck is silently absent from the window.
- The blit is size-guarded and **logs** when the surface and composite
  disagree (window chrome makes the real surface 1920x1045, not 1920x1080), so
  a skipped present can never be silent again.

Known cost while clouds are up: the window composite re-creates its colour/HDR/
depth targets each frame, and a snapshot request on the same frame renders the
composite a second time. Hoisting those into a resize-keyed cache (and reusing
the window composite when the snapshot size matches) is the obvious follow-up;
it is a performance, not correctness, regression, and only affects the
clouds-enabled path.




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
| `feature` | `x = 1/size` (puff scale), `y shadow_strength`, `z = raymarch step count` |
| `weather` | `x weather_strength` |
| `style` | `x = style flag` (0 realistic, 1 cartoon), `y = cel band count`, `z = edge darkening` |

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

### 5.1 Two caller-side traps that fail silently

Both of these render *something*, so neither shows up as an error — the clouds
are just wrong or absent. Two shipped cloud examples hit both.

**Camera mode.** The offline terrain mesh lives in the world **XY plane with
heights along +Z**, so the matching orbit is `camera_mode="mesh:zup"`
(`cam_theta_deg` = polar angle from +Z: 0 is top-down, 90 is the horizon).
Plain `"mesh"` is the legacy **Y-up** orbit, whose up vector is a grid axis:
the terrain then renders rolled and, at azimuths where the orbit crosses the
grid plane, collapses to an edge-on sliver. Measured on the Bryce DEM, span 12,
radius 16, theta 32 — terrain columns covered out of 400:

| `cam_phi_deg` | `mesh` | `mesh:zup` |
|---|---|---|
| 108 | 215 | 205 |
| 150 | 109 | 221 |
| 180 | **13** | 197 |
| 210 | 76 | 201 |
| 252 | 187 | 201 |

Use `mesh:zup` unless you specifically want the legacy frame.

**Sample count.** `render_offline` picks the sample budget by mode:

- `adaptive=False` → exactly `params.aa_samples`; `OfflineQualitySettings`
  `min_samples`/`max_samples` are **ignored**.
- `adaptive=True` → up to `settings.max_samples`, stopping at
  `settings.min_samples` once the frame converges.

A caller that passes only `--samples 64` into `min_samples`/`max_samples` with
`adaptive=False` therefore renders **one** sample and the flag reads as dead.
Set `aa_samples=N` in the render params. `result.metadata["samples_used"]` (not
`total_samples`) reports what actually ran — check it.

**DEM source.** `assets/tif/*.tif` are Git-LFS pointers; an unpulled checkout
has 134-byte text files, not rasters. A loader that only tests `path.exists()`
silently falls back to a procedural terrain. Test the TIFF magic (`II`/`MM`)
and try every candidate path before falling back.

### 5.2 Framing the deck

The deck is a bounded slab, so where the camera sits relative to
`[altitude, altitude+thickness]` decides what the clouds even look like. The
camera's height along `up_axis` is `cam_target.z + cam_radius · cos(cam_theta)`:

- **Above the deck top** → you see cloud *tops* through to the terrain. This is
  the "clouds over the landscape" shot, and it is the only framing where a
  thick deck reads as thick clouds rather than as a ceiling.
- **Between the two planes** → the camera is *inside* the deck. Every ray
  accumulates almost immediately and the frame turns into uniform fog. Easy to
  hit by accident: raise `--thickness` or lower `--radius` and the camera
  silently ends up inside.
- **Below the deck base** → you see the underside: a dramatic ceiling, and the
  view that makes the deck read as *above* you.

Getting the third shot automatically needs the camera and the aim point to be
different heights, because the orbit always looks *at* `cam_target`. Setting
`cam_theta > 90°` puts the eye *below* the target, so the view tilts upward.
Both cloud examples implement this as `--view canyon`: it takes the lowest cell
of the heightmap as the floor, stands the eye just above it, and aims at a
point up in (or below) the deck and across the canyon, then converts that
eye/aim pair back into `cam_target` + `radius` + `theta` + `phi`. The
`--canyon-look-up` angle (degrees above horizontal) is the knob for how much
terrain stays in the bottom of the frame: ~10° keeps a wall visible, ~25° is
sky almost to the corners.

## 6. Determinism, provenance, degradation

- **Determinism**: the shader runs the `det_*` prelude (`includes/determinism.wgsl`;
  `det_div`, `det_barrier`, `det_pow`, `det_mix` …). `clouds.wgsl` is its own
  assembled module (`atmosphere_clouds()`), *not* part of `terrain_parts()`, so
  editing it does not move `PINNED_TERRAIN_SOURCE_HASH` — verified: the pinned
  hash test stays green across a clouds-only edit. It is still covered by the
  determinism IR lint, so `FORGE3D_DET_REWRITE=1 cargo test det_instrument_rewrite --lib`
  must stay green after any shader edit (it auto-instruments new arithmetic; a
  dry run with 0 edits is the pass condition).
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
| top = `1 − pow(1−h, 16)` | `1 − smoothstep(0.72, 1.0, h)` | `1 − pow(1−h, 10)` |
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
| `style` | look: `realistic` (haze) or `cartoon` (cel) | realistic |
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
| `src/terrain/renderer/skybox.rs`, `src/shaders/skybox.wgsl` | the HDRI background behind the deck (equirect basis follows the camera's up axis: +Y for `mesh`, +Z for `mesh:zup`) |
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
