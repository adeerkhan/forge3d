# Volumetric Clouds — a plain guide

This page explains how the terrain clouds are made, which libraries do what, and
where to look when they look wrong. No deep math — just the recipe.

## In one line

We paint clouds by **marching a ray through a box of 3D noise** on the GPU, one
ray per pixel, and blending the result over the terrain picture.

## Quick start

The easiest way to get good clouds is the HDRI example — it lights the cloud
layer from a real sunrise environment and exposes every knob as a flag:

```
python examples/terrain_volumetric_clouds_hdri.py --preset cumulus --samples 64
```

It uses the shipped `qwantani_sunrise_puresky_2k.hdr` (override with `--hdr`),
prints the exact settings it used, and saves a PNG you can regenerate. Change the
look without editing code — `--preset {cumulus,scattered,storm,cirrus,overcast}`,
or dial individual knobs (`--coverage`, `--density`, `--size`, `--scatter`,
`--phase-g`, `--cloud-shadows`, …). Run `python examples/terrain_volumetric_clouds_hdri.py -h`
for the full list. (The plainer `examples/terrain_volumetric_clouds.py` is the
minimal reference.)

The cloud deck is placed in **world units on the offline path, not metres** — see
[Units](#units-read-this-before-tweaking-altitude_m--thickness_m) below before
setting `altitude_m` / `thickness_m` by hand.

## The libraries (what each one does)

| Library | What it does here |
| --- | --- |
| **Rust** | The engine. Loads the terrain, sets up the GPU, runs the passes. |
| **wgpu** | The GPU toolkit. Sends work to the graphics card (Vulkan/DX12/Metal) and runs the "compute" pass that draws the clouds. |
| **WGSL** | The shader language. The cloud maths lives in `clouds.wgsl`, a small program the GPU runs per pixel. |
| **naga** | The shader checker (bundled with wgpu). Rejects a broken `clouds.wgsl` at build time. |
| **PyO3** | The Rust↔Python bridge, so Python can call the renderer. |
| **numpy** | The heightmap the terrain is built from. |
| **bytemuck / glam** | Small Rust helpers: pack data for the GPU, and do matrix/vector math. |

The repo also has a **determinism prelude** (`includes/determinism.wgsl`): the
shader calls `det_*` helpers so the same scene renders the same bytes on every
run. A lint enforces this on the cloud shader.

## How a cloud is made (the recipe)

1. **Three noise "clouds in a box".** At startup we bake three small 3D textures
   on the CPU and upload them:
   - a **base** volume (128³) — the big fluffy lumps,
   - a **detail** volume (32³) — fine wisps,
   - a **worley** volume (64³) — rounded cellular blobs (the "glob" look).
   These are plain grey volumes the shader can read anywhere in space.

2. **Build a density value at a point.** The shader samples those volumes at the
   point, warps the coordinates a little (so shapes look wind-torn), and mixes
   them into one number `shape` (0 = clear, 1 = thick).

3. **Cut out clouds with `coverage`.** `density = shape − (1 − coverage)`.
   High coverage keeps most of the box; low coverage keeps only the peaks, so
   you get **separate puffs** with clear sky between them.

4. **Give it a cloud shape from top to bottom.** A **vertical profile** says
   "nothing at the very bottom, thick through the body, rounded at the top" —
   that is what makes it look like a cumulus and not a flat sheet.

5. **Keep it over the terrain (bounded).** A point too far sideways from the
   scene centre is discarded, with a soft edge, so the deck is a **finite patch**
   over the map — not an endless slab.

6. **Light it.** For each sample we march a short ray **toward the sun** through
   the box; the more cloud it passes, the darker that sample is. We add a
   forward-scattering term (bright edges facing the sun) and a "powder" term
   (dark centres in thick cloud).

7. **March the view ray.** For each screen pixel we walk from the camera into
   the box in ~48 steps, front to back, collecting light and fading the
   background as the ray gets thicker. The result is `color` and `transmittance`
   (how much of what is behind still shows through).

8. **Composite.** Final pixel = `terrain × transmittance + cloud_color`. Where a
   cloud is in front, it covers the terrain; where it is thin, the terrain shows.

9. **Shadow.** For pixels that hit the terrain, we also read the cloud right
   above that point and darken the terrain by how thick it is — the shadow on
   the ground.

## How the render flows

**Offline (quality stills):**

```
sky background → terrain (linear HDR) → CLOUD PASS → accumulate → tonemap
```

The cloud pass runs once per accumulation sample (the camera is jittered each
time), so the offline averaging smooths the clouds for free.

**Interactive viewer:**

```
terrain (linear HDR) → CLOUD PASS → tonemap to the window
```

Both paths run the **same shader** and the same GPU resources.

## Where things live

| File | Role |
| --- | --- |
| `src/shaders/atmosphere/clouds.wgsl` | The cloud maths (the whole recipe above). |
| `src/core/cloud_volume.rs` | Bakes the noise volumes, builds the GPU pipeline, runs the pass. |
| `src/shaders/skybox.wgsl` | Fullscreen HDRI background equirect lookup. |
| `src/terrain/renderer/skybox.rs` | Loads the HDRI (`formats::hdr`), renders the skybox pass. |
| `src/terrain/renderer/offline.rs` | Calls the pass in the offline flow. |
| `src/viewer/terrain/clouds.rs` | Calls the pass in the viewer. |
| `python/forge3d/terrain_params.py` | `CloudSettings` — the knobs you set from Python. |

## The knobs (`CloudSettings`) and what they do

| Field | Effect |
| --- | --- |
| `enabled` | Off = the pass never runs (picture is byte-identical to no clouds). |
| `coverage` | How much sky is filled. **Low = separate puffs, high = solid sheet.** |
| `density` | How opaque each cloud is. |
| `altitude_m`, `thickness_m` | Where the deck sits and how tall it is. **Must sit above the terrain peaks or you won't see it.** Interpreted as world-space height on the offline path, *not* metres — see [Units](#units-read-this-before-tweaking-altitude_m--thickness_m). |
| `scatter_strength`, `phase_g` | Brightness and how "forward" the light scatters (bright sun edges). |
| `detail`, `powder` | Edge wispiness and dark cloud centres. |
| `wind_dir`, `wind_speed` | Drift direction and speed (visible in animated sequences). |
| `shadows_enabled`, `shadow_strength` | Ground shadow under the clouds. |
| `size` | Cloud size multiplier. Bigger = larger puffs (2 is small, 4-5 is a big cumulus field). |
| `weather_strength` | How much the weather map varies coverage across the sky, in `[0, 1]`. |
| `weather_map` | Path to a custom greyscale image that decides **where** it is cloudier. `None` uses a built-in default field. |

### The weather map

A greyscale image laid flat over the cloud patch. Bright areas get more cloud
(and thicker cloud), dark areas stay clear. Use it to make a "front" on one side
of the map, or a hole in the middle.

- **Default:** if you pass nothing, a built-in low-frequency noise field is used,
  so the sky is never perfectly uniform.
- **Custom:** pass `weather_map="path/to/field.png"` (any image the `image` crate
  can read; it is converted to greyscale). The map is stretched over the cloud
  patch (the extent radius around the scene centre). Use a smooth, low-frequency
  image — it is a coverage field, not a colour texture.
- `weather_strength` scales the effect (0 = the map does nothing, 1 = strong
  variation).

## When it looks wrong, check these first

- **No clouds at all** → the deck is **below the terrain** (raise `altitude_m`
  above the peak height), or you are in the legacy `screen` camera (clouds need
  a 3D `mesh`/`mesh:zup` camera), or `enabled` is false.
- **A flat grey sheet** → `coverage` too high. Lower it.
- **Clouds everywhere / no edge** → the bounded extent is bigger than the map.
- **Clouds not moving** → a still has `time = 0`; motion only shows in a
  sequence. Check `wind_speed` > 0.
- **No ground shadow** → `shadows_enabled` false, or the deck is not above the
  lit terrain.
- **Wrong look at a different world scale** → the noise is normalised by the
  terrain span; a scene whose coordinates do not match its span (e.g. some
  MapScene 3D setups) will tile or look flat.
- **No sky background** → the offline background is the HDRI skybox
  (`assets/hdri/sky.hdr` by default, `#?RADIANCE` equirect). `*.hdr` is
  git-ignored, so on a fresh checkout the file may be absent: the skybox then
  records a `terrain_skybox_asset_unavailable` degradation and renders nothing.
  Point `FORGE3D_SKYBOX_HDR=/path/to/sky.hdr` at an environment to get it
  back.

## What we borrowed from the reference

The shape/lighting follows the Horizon Zero Dawn / Frostbite method (the
`evroon/bevy-volumetric-clouds` plugin is a readable Rust port of it):

- **Height-varying coverage** — the deck is wide at the bottom and narrows
  toward the top, so clouds look like cumulus, not a flat slab.
- **Height-gradient erosion** — thin edges are eaten away more than thick cores.
- **Ambient top/bottom** — the multiple-scatter term is darker at the deck base
  and brighter at the top.
- **Sky fade with distance** — the cloud fades toward the sky colour far away
  (aerial perspective), so it sits in the atmosphere.
- **Dual-lobe phase + energy-conserving integration** (already present).
- **Sun-offset soft shadow** — the ground shadow is cast away from the sun and
  softened, not a hard black blob.

## Current limits (known)

- The look is much better but still not photographic; the puffs are a little
  soft and there is no temporal accumulation in the viewer yet.
- The ground shadow is a single sample (not a full second raymarch), so it is
  approximate.
- Clouds composite on the **offline / accumulation path only**; the one-shot
  forward render records a degradation (`terrain_clouds_offline_only`) and
  draws no clouds until phase 2.
- They need a world-space camera (`mesh` / `mesh:zup`); the legacy `screen`
  2.5D camera records a degradation and renders no clouds.

## Units (read this before tweaking `altitude_m` / `thickness_m`)

Despite the `_m` suffix, the offline / procedural renderer interprets
`altitude_m` and `thickness_m` in the **same world units as the terrain**, not
in literal metres. The shader places the slab at world heights
`[altitude_m, altitude_m + thickness_m]` (`base = layer.x`), and the terrain in
these demos is a heightmap normalised to `0..1` times `z_scale` — so peaks sit
around `1.35`, not `1500`. That means:

- a deck whose base is **below** the peaks is hidden behind the ridges;
- the `CloudSettings` **defaults** (`altitude_m=1500`, `thickness_m=800`) are
  metre-scale values for the interactive geospatial viewer; on this
  world-space path they float the deck far off-screen and you see **no clouds**.
  Use small world-space values instead (the example bases the deck at ~`1.5`).

Getting the metre defaults to "just work" everywhere needs a single
metres→world conversion at the offline seam (terrain vertical extent), which is
a deliberate decision shared with the metre-based viewer path — see the
implementation plan.
