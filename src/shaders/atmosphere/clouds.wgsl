// src/shaders/atmosphere/clouds.wgsl
// Depth-aware volumetric cloud raymarch composited into the linear-HDR beauty
// buffer after terrain and before accumulation/tonemap.
//
// Density is a domain-warped cellular (Worley) field blended with fractal
// value noise, eroded by a ridged detail volume, so the clouds get billowy
// cauliflower tops and wispy edges rather than smooth blobs.
//
// Deterministic: fixed loop counts, no derivatives, `textureSampleLevel`
// instead of `textureSample`, integer-hash blue-noise jitter seeded by the
// per-sample index, and det_* arithmetic from the determinism prelude.

struct CloudUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,      // xyz camera position (world), w unused
    sun_direction: vec4<f32>,   // xyz normalized sun direction, w unused
    sun_radiance: vec4<f32>,    // rgb sun radiance * intensity, w unused
    layer: vec4<f32>,           // x altitude_m, y thickness_m, z coverage, w density
    optics: vec4<f32>,          // x scatter_strength, y phase_g, z detail, w powder
    wind: vec4<f32>,            // x dir_radians, y speed, z time_seconds, w sample_index
    screen: vec4<f32>,          // x width, y height, z far fallback for level rays, w unused
    up_axis: vec4<f32>,         // xyz world up of the camera frame, w = 1/terrain_span noise scale
    bounds: vec4<f32>,          // xyz cloud-field centre (world), w = horizontal extent radius
    feature: vec4<f32>,         // x cell-frequency multiplier (cloud size), y = shadow strength, z = raymarch step count, w unused
    weather: vec4<f32>,         // x weather-map strength (coverage variation), yzw unused
    style: vec4<f32>,           // x style flag (0 realistic, 1 cartoon), y = toon band count, z = edge darkening, w unused
};

@group(0) @binding(0) var<uniform> u: CloudUniforms;
@group(0) @binding(1) var base_noise: texture_3d<f32>;
@group(0) @binding(2) var base_sampler: sampler;
@group(0) @binding(3) var detail_noise: texture_3d<f32>;
@group(0) @binding(4) var detail_sampler: sampler;
@group(0) @binding(5) var scene_depth: texture_depth_2d;
@group(0) @binding(6) var beauty_in: texture_2d<f32>;
@group(0) @binding(7) var clouds_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(8) var worley_noise: texture_3d<f32>;
@group(0) @binding(9) var weather_map: texture_2d<f32>;
@group(0) @binding(10) var weather_sampler: sampler;

const PI: f32 = 3.141592653589793;
// Upper bound of the raymarch loop, and the step count used when the caller
// does not choose one. The loop runs this many iterations at most and breaks at
// the caller's step count (`u.feature.z`), so `dt` stays
// `(t_far - t_near) / steps` and the optical-depth integral is unchanged by the
// quality setting -- only the sampling density changes.
const MARCH_STEPS_MAX: i32 = 192;
const MARCH_STEPS: i32 = 56;
const LIGHT_STEPS: i32 = 5;

// Henyey-Greenstein phase function.
//
// The anisotropy is clamped and the denominator floored so the forward-scatter
// peak stays bounded: the single lobe accepts `phase_g` up to 0.99 and the
// multi-scatter approximation below uses g = 0.85, both of which otherwise
// drive `1 + g^2 - 2 g cos(theta)` toward zero at cos(theta) -> 1 and blow the
// phase term (and the clouds) out to white around the sun.
fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let gc = clamp(g, -0.85, 0.85);
    let g2 = det_barrier(gc * gc);
    let d1 = det_barrier(1.0 + g2) - det_barrier(det_barrier(2.0 * gc) * cos_theta);
    let denom = max(d1, 0.07);
    return det_div(1.0 - g2, det_barrier(4.0 * PI * denom) * det_sqrt(denom));
}

// Blue-noise-ish jitter in [0,1) from an integer hash of pixel + sample.
fn jitter(px: u32, py: u32, sample_index: u32) -> f32 {
    var h = px * 0x8da6b343u ^ py * 0xd8163841u ^ (sample_index + 1u) * 0xcb1ab31fu;
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return f32(h) * (1.0 / 4294967296.0);
}

// Domain-warped, cellular-eroded base shape in [0,1]. Noise is sampled in
// scene-normalized coordinates (`u.up_axis.w` = 1/terrain_span) so the field
// looks the same at any world scale.
fn base_shape(p: vec3<f32>) -> f32 {
    let q = det_barrier3(det_barrier3(p * u.up_axis.w) * u.feature.x);

    // Low-frequency domain warp: three offset reads of the base volume give a
    // smooth 3-vector that shears the field into wispy, wind-torn shapes.
    let w0 = textureSampleLevel(base_noise, base_sampler, det_barrier3(det_barrier3(q * 2.5) + vec3<f32>(13.1, 5.2, 9.7)), 0.0).r;
    let w1 = textureSampleLevel(base_noise, base_sampler, det_barrier3(det_barrier3(q * 2.5) + vec3<f32>(31.7, 17.3, 2.9)), 0.0).r;
    let w2 = textureSampleLevel(base_noise, base_sampler, det_barrier3(det_barrier3(q * 2.5) + vec3<f32>(7.3, 23.9, 41.1)), 0.0).r;
    let warp = det_barrier3(det_barrier3(vec3<f32>(w0, w1, w2) - vec3<f32>(0.5)) * 0.22);
    let w = det_barrier3(q + warp);

    // Billowy cellular mass (Worley) + fractal fluff (FBM).
    let wor = det_barrier(textureSampleLevel(worley_noise, base_sampler, det_barrier3(w * 1.5), 0.0).r);
    // Octave frequencies are capped by LATTICE CELL SIZE, not by taste: the
    // baked 128^3 volume's cells are ~3-10 m at scene scale, and at the
    // 5-12 km cloud distance of a DEM flyover that is sub-pixel. Frequencies
    // that put cells below the pixel footprint alias into the salt-and-pepper
    // halo no accumulation average can remove, so the fractal is limited to
    // feature sizes the frame can actually resolve (the worley/detail volumes
    // carry the remaining structure at their own coarser scale). Real cumulus
    // at 8 km are 100-600 m across, so this is also the physically right
    // scale for the look.
    let f0 = det_barrier(textureSampleLevel(base_noise, base_sampler, det_barrier3(w * 2.0), 0.0).r * 0.5);
    let f1 = det_barrier(textureSampleLevel(base_noise, base_sampler, det_barrier3(w * 4.0), 0.0).r * 0.3);
    let f2 = det_barrier(textureSampleLevel(base_noise, base_sampler, det_barrier3(w * 9.0), 0.0).r * 0.2);
    let fbm = det_barrier(det_barrier(f0 + f1) + f2);

    // Frostbite/HZD composite: the shape is the PRODUCT of the fractal field and
    // the cellular field, not their average. The multiply keeps high-contrast
    // puffy interiors with hard zeros between puffs; an average washes the whole
    // deck into a flat grey sheet.
    //
    // The product is then re-centred on 0.5. `coverage` is a fraction of sky
    // filled, so its cut is `1 - coverage` and the field has to straddle that at
    // its MEAN for the knob to mean anything. The raw product averages ~0.35
    // (0.55 x 0.65), so the 0.42 preset cut at 0.58 sat far above the mean and
    // left the deck as one stray cell instead of a field of puffs. Re-centring
    // puts ~(1 - coverage) of the sky above threshold, which is exactly what
    // "coverage" promises.
    let fluffy = det_barrier(fbm);
    let billowy = det_barrier(wor);
    let product = det_barrier(fluffy * billowy);
    // Stretch around the mean: product averages ~0.25, so 2*product averages
    // ~0.5, which is what `1 - coverage` is measured against. The stretch also
    // widens the histogram, so the coverage cut is a real discriminator rather
    // than a threshold sitting above everything.
    return det_barrier(2.0 * product);
}

// Height-gradient detail erosion in [0,1] (1 = no erosion). Two Worley scales
// plus the ridged detail volume (Frostbite's `r + g/2 + b/4` detail mix) so
// edges break up at fine AND coarse scales instead of one uniform mush.
// Frequencies stay well inside each volume's Nyquist limit — sampling a baked
// volume near its texel frequency aliases into the salt-and-pepper scatter of
// isolated pixels around cloud edges that per-sample jitter cannot average.
fn detail_erosion(p: vec3<f32>, amount: f32) -> f32 {
    let q = det_barrier3(det_barrier3(p * u.up_axis.w) * u.feature.x);
    let d0 = textureSampleLevel(detail_noise, detail_sampler, det_barrier3(q * 5.0), 0.0).r;
    let d1 = textureSampleLevel(detail_noise, detail_sampler, det_barrier3(q * 11.0), 0.0).r;
    let w0 = textureSampleLevel(worley_noise, base_sampler, det_barrier3(q * 2.5), 0.0).r;
    let w1 = textureSampleLevel(worley_noise, base_sampler, det_barrier3(q * 6.0), 0.0).r;
    let erosion = det_barrier(
        det_barrier(det_barrier(d0 * 0.45) + det_barrier(d1 * 0.15))
            + det_barrier(det_barrier(w0 * 0.30) + det_barrier(w1 * 0.10)),
    );
    return det_mix(1.0, erosion, clamp(amount, 0.0, 1.0));
}

// Cumulus vertical profile: a flat base near the bottom of the deck, a high
// body, and a sharply rounded top — reads as volume, not a sheet. The
// `pow(1 - height_frac, 10)` top is the Frostbite/HZD shape: the body stays
// dense until close to the top, then rounds off fast, so puffs keep flat
// bases and domed cauliflower tops instead of a soft cone.
fn cumulus_profile(height_frac: f32) -> f32 {
    let h = clamp(height_frac, 0.0, 1.0);
    return det_barrier(det_smoothstep(0.0, 0.06, h) * det_barrier(1.0 - det_pow(1.0 - h, 10.0)));
}

// Extinction coefficient, in inverse world units.
//
// `u.layer.w` (`CloudSettings.density`) is the DIMENSIONLESS optical depth a ray
// accumulates traversing the whole deck vertically, so it means the same thing
// on every path. It used to be a per-world-unit coefficient, which made the very
// same number near-invisible on a 2-unit demo deck and optically infinite on a
// 900 m geospheric one -- the deck then saturated to opaque white over the whole
// sky (measured: 73% of sky pixels blown out). Dividing by the deck thickness
// bounds tau at the setting itself, so the composite stays a *veil* you can see
// structure through at 0.5-0.8 and only goes solid well past 1.0.
fn deck_extinction() -> f32 {
    return det_div(u.layer.w, max(u.layer.y, 1.0e-4));
}

// ---------------------------------------------------------------------------
// Cartoon (cel-shaded) density
//
// The stylized deck deliberately throws away the two things that make the
// realistic path read as "photographic": the low-frequency domain warp (which
// shears puffs into wispy wind-torn shapes) and the fine erosion (which breaks
// the silhouette into feathered filaments). What is left is a chunky rounded
// mass with a clean, readable outline that the flat cel lighting can fill.
// ---------------------------------------------------------------------------

// Clean rounded mass in [0,1]: a coarse cellular field with a touch of the
// low-octave fractal so the puffs are not identical, then a wide S-curve that
// pushes the midrange toward the extremes. That S-curve is what separates the
// field into distinct rounded blobs with clear sky between them -- without it
// the cellular field is a gentle gradient and the deque fills in as one wash.
fn cartoon_shape(p: vec3<f32>) -> f32 {
    let q = det_barrier3(det_barrier3(p * u.up_axis.w) * u.feature.x);
    let wor = det_barrier(textureSampleLevel(worley_noise, base_sampler, det_barrier3(q * 1.2), 0.0).r);
    let fbm = det_barrier(textureSampleLevel(base_noise, base_sampler, det_barrier3(q * 2.2), 0.0).r);
    let mass = det_barrier(det_mix(wor, fbm, 0.50));
    return det_barrier(det_smoothstep(0.34, 0.72, mass));
}

// Domed vertical profile: a flat, hard base and a fully rounded crown, so a
// puff reads as a tidy hemisphere rather than a cone. Thicker than the
// realistic profile because the cartoon deck is meant to look solid.
fn cartoon_profile(height_frac: f32) -> f32 {
    let h = clamp(height_frac, 0.0, 1.0);
    return det_barrier(det_smoothstep(0.0, 0.12, h) * det_barrier(1.0 - det_pow(1.0 - h, 4.0)));
}

fn cartoon_density(p: vec3<f32>, height_frac: f32) -> f32 {
    let up = det_normalize3(u.up_axis.xyz);
    // Bounded to the same finite deck as the realistic path, so switching style
    // never changes where the clouds live -- only how they look.
    let rel = det_barrier3(p - u.bounds.xyz);
    let h_off = rel - det_barrier3(up * det_dot3(rel, up));
    let extent = det_div(det_dot3(h_off, h_off), max(u.bounds.w * u.bounds.w, 1.0e-6));
    if extent >= 1.0 {
        return 0.0;
    }
    let edge = 1.0 - det_smoothstep(0.6, 1.0, extent);

    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(up.y) < 0.9);
    let right = det_normalize3(det_cross3(up, helper));
    let forward = det_cross3(up, right);
    let wuv = det_barrier2(det_barrier2(det_div2(vec2<f32>(det_dot3(h_off, right), det_dot3(h_off, forward)), vec2<f32>(max(u.bounds.w, 1.0e-3)))) * 0.5) + vec2<f32>(0.5);
    let weather = textureSampleLevel(weather_map, weather_sampler, wuv, 0.0).r;
    let coverage = clamp(u.layer.z + det_barrier((weather - 0.5) * u.weather.x), 0.0, 1.0);

    let shape = cartoon_shape(p);
    let profile = cartoon_profile(height_frac);
    let eroded = det_barrier(shape - det_barrier(1.0 - coverage));
    // A tight ramp is what gives the drawn outline. No detail term multiplies
    // this, so the boundary is the shape's own contour: a single hard edge the
    // cel lighting can key off.
    let cut = det_smoothstep(0.0, 0.05, eroded);
    return det_barrier(det_barrier(cut * profile) * edge) * deck_extinction();
}

/// Physically-based Frostbite / HZD density field: the product of a fractal
/// and a cellular field, eroded by fine detail, cut by coverage.
fn realistic_density(p: vec3<f32>, height_frac: f32) -> f32 {
    let up = det_normalize3(u.up_axis.xyz);
    // Clip the field to a bounded horizontal extent around the cloud centre,
    // with a soft edge so the cloud deck fades out instead of ending in a hard
    // disc — clouds stay a finite volume over the terrain, not an infinite slab.
    let rel = det_barrier3(p - u.bounds.xyz);
    let h_off = rel - det_barrier3(up * det_dot3(rel, up));
    let extent = det_div(det_dot3(h_off, h_off), max(u.bounds.w * u.bounds.w, 1.0e-6));
    if extent >= 1.0 {
        return 0.0;
    }
    let edge = 1.0 - det_smoothstep(0.6, 1.0, extent);

    // Weather map: a 2D field over the cloud patch that varies coverage (and
    // thus cloud type) from place to place.
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(up.y) < 0.9);
    let right = det_normalize3(det_cross3(up, helper));
    let forward = det_cross3(up, right);
    let wuv = det_barrier2(det_barrier2(det_div2(vec2<f32>(det_dot3(h_off, right), det_dot3(h_off, forward)), vec2<f32>(max(u.bounds.w, 1.0e-3)))) * 0.5) + vec2<f32>(0.5);
    let weather = textureSampleLevel(weather_map, weather_sampler, wuv, 0.0).r;
    let coverage = clamp(u.layer.z + det_barrier((weather - 0.5) * u.weather.x), 0.0, 1.0);

    let shape = base_shape(p);
    // Subtract the vertical erosion budget: thin toward the top (rounded dome)
    // so tops erode more than bases (HZD height-varying coverage).
    let profile = det_barrier(cumulus_profile(height_frac));
    let eroded = det_barrier(shape - (det_barrier(1.0 - coverage))) - det_barrier(det_barrier(1.0 - profile) * 0.55);
    // Coverage cut. The ramp has to reach ZERO between puffs, not merely get
    // small: a wide leave-a-bit-everywhere ramp (0.16) turns the whole deck into
    // a uniform thin veil, and the sky loses every clear gap (measured: 0.7% of
    // sky pixels were open sky). It is now a hard-ish threshold, which is also
    // what gives cumulus their defined silhouettes. Offline accumulation
    // antialiases the edge.
    let cut = det_smoothstep(0.0, 0.07, eroded);
    if cut <= 0.0 {
        return 0.0;
    }
    // Height-gradient erosion, but let it reach the SILHOUETTE. The earlier
    // build faded erosion to zero at the boundary to kill a sparse-pixel halo;
    // that also removed the fine nibble that turns a smooth contour into
    // cauliflower lobes, and the deck read as one soft smear (the "blur").
    // Offline accumulation averages the per-sample jitter, so the boundary
    // detail resolves instead of sparkling; the erosion still tapers toward the
    // thick core so interiors stay smooth. `0.30` is the floor at the edge.
    let edge_keep = det_smoothstep(0.0, 0.45, cut);
    let erosion_amount = det_barrier(u.optics.z * det_mix(0.30, 1.0, edge_keep));
    let detail = detail_erosion(p, erosion_amount);
    return det_barrier(det_barrier(det_barrier(cut * detail) * profile) * edge) * deck_extinction();
}

/// Density dispatcher: selects the stylized or the physically-based field from
/// the uniform. Every caller (march, light march, ground shadow) goes through
/// here, so the two styles stay consistent across the whole pass.
fn cloud_density(p: vec3<f32>, height_frac: f32) -> f32 {
    if u.style.x > 0.5 {
        return cartoon_density(p, height_frac);
    }
    return realistic_density(p, height_frac);
}

fn intersect_slab(origin: vec3<f32>, dir: vec3<f32>, up: vec3<f32>) -> vec2<f32> {
    let ob = det_barrier(det_dot3(origin, up));
    let db = det_dot3(dir, up);
    let base = u.layer.x;
    let top = det_barrier(u.layer.x + u.layer.y);
    if abs(db) < 1.0e-5 {
        if ob < base || ob > top {
            return vec2<f32>(0.0, -1.0);
        }
        // The ray runs parallel to the slab and inside it, so the slab alone
        // does not bound the range. Return a placeholder and let the cloud
        // volume's cylinder (see `intersect_cylinder`) supply the far hit.
        return vec2<f32>(0.0, u.screen.z);
    }
    let t0 = det_div(base - ob, db);
    let t1 = det_div(top - ob, db);
    return vec2<f32>(min(t0, t1), max(t0, t1));
}

/// Nearest/farthest intersection of the ray with the cloud field's horizontal
/// cylinder — centre `u.bounds.xyz`, radius `u.bounds.w`, axis `up`. Returns
/// `(0, -1)` when the ray misses it.
///
/// This is what keeps the fixed step budget inside the clouds. `intersect_slab`
/// alone is unbounded horizontally: a near-horizontal ray crosses the infinite
/// slab over thousands of world units, so `dt = (t_far - t_near) / MARCH_STEPS`
/// balloons past the deck thickness and the deck is effectively skipped at
/// grazing angles. `cloud_density` already returns 0 beyond `u.bounds.w`, so
/// intersecting with the cylinder only removes steps that would have sampled
/// empty space.
fn intersect_cylinder(origin: vec3<f32>, dir: vec3<f32>, up: vec3<f32>) -> vec2<f32> {
    let radius = u.bounds.w;
    if radius <= 0.0 {
        return vec2<f32>(0.0, -1.0);
    }
    let rel = det_barrier3(origin - u.bounds.xyz);
    let o = det_barrier3(rel - det_barrier3(up * det_dot3(rel, up)));
    let d = det_barrier3(dir - det_barrier3(up * det_dot3(dir, up)));
    let a = det_dot3(d, d);
    let c = det_barrier(det_dot3(o, o)) - det_barrier(radius * radius);
    if a < 1.0e-8 {
        // Ray parallel to the cylinder axis: inside or outside for its whole
        // length, never crossing the wall. Bound an inside ray by the field's
        // diameter, which is the farthest useful draw distance for it.
        if c > 0.0 {
            return vec2<f32>(0.0, -1.0);
        }
        return vec2<f32>(0.0, det_barrier(2.0 * radius));
    }
    let b = det_dot3(o, d);
    let disc = det_barrier(det_barrier(b * b) - det_barrier(a * c));
    if disc < 0.0 {
        return vec2<f32>(0.0, -1.0);
    }
    let sq = det_sqrt(disc);
    return vec2<f32>(det_div(det_barrier(-b) - sq, a), det_div(det_barrier(-b) + sq, a));
}

/// Height of `p` within the cloud slab, measured along the world up axis.
fn slab_height_frac(p: vec3<f32>, up: vec3<f32>) -> f32 {
    return clamp(det_div(det_barrier(det_dot3(p, up)) - u.layer.x, max(u.layer.y, 1.0e-4)), 0.0, 1.0);
}

fn sun_transmittance(p: vec3<f32>, up: vec3<f32>) -> f32 {
    let dir = u.sun_direction.xyz;
    let step_len = det_div(u.layer.y, f32(LIGHT_STEPS));
    var optical = 0.0;
    var q = p;
    for (var i = 0; i < LIGHT_STEPS; i = i + 1) {
        q = det_barrier3(q + det_barrier3(dir * step_len));
        optical = det_barrier(optical + det_barrier(det_barrier(cloud_density(q, slab_height_frac(q, up))) * step_len));
    }
    return det_exp(-optical);
}

// Flat cel-shaded colour for one cartoon sample. The sun transmittance is
// quantized into `u.style.y` hard bands (a fully shaded, a mid, and a fully lit
// tone) so a surface of constant illumination fills with one flat colour and
// the light/shadow boundary is a hard step -- the defining read of a toon fill.
// `height_frac` adds a crown highlight on the sunlit top and a darkened,
// outline-like underside.
fn cartoon_lit(height_frac: f32, sun: f32) -> vec3<f32> {
    let bands = max(u.style.y, 2.0);
    // Combined illumination: the sunlit fraction plus how high the sample sits
    // in the deck, so a puff's crown is the brightest tone and its base the
    // darkest. Quantizing this into flat bands is the cel fill -- keying on the
    // sun term alone lands most of a sunlit top in the brightest band and the
    // puff reads as one flat white shape.
    let illum = clamp((det_barrier(sun * 0.65)) + (det_barrier(height_frac * 0.55)), 0.0, 1.0);
    let q = det_div(floor(illum * bands), max(bands - 1.0, 1.0));
    // Explicit three-tone ramp (shadow / mid / lit) rather than a two-colour
    // mix, so the mid band is a real colour and the fill reads as drawn rather
    // than gradient-shaded. The lit tone carries the sun colour.
    let shade = vec3<f32>(0.42, 0.48, 0.68);
    let mid = vec3<f32>(0.72, 0.77, 0.90);
    let light = vec3<f32>(0.97, 0.97, 1.0) + (det_barrier3(u.sun_radiance.rgb * 0.10));
    let col = det_mix3(
        det_mix3(shade, mid, clamp(q * 2.0, 0.0, 1.0)),
        light,
        clamp((det_barrier(q * 2.0)) - 1.0, 0.0, 1.0),
    );
    // Drawn underside: a darker outline tone under each puff, strength from
    // `style.z`.
    let under = (1.0 - det_smoothstep(0.10, 0.42, height_frac)) * clamp(u.style.z, 0.0, 1.0);
    return det_mix3(col, vec3<f32>(0.30, 0.36, 0.58), under);
}

@compute @workgroup_size(8, 8)
fn clouds_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    det_seed(f32(gid.x));
    let dims = vec2<u32>(u32(u.screen.x), u32(u.screen.y));
    if gid.x >= dims.x || gid.y >= dims.y {
        return;
    }
    let pixel = vec2<i32>(i32(gid.x), i32(gid.y));

    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let uv = det_div2(vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5), dims_f);
    let ndc = vec2<f32>(det_barrier(uv.x * 2.0) - 1.0, 1.0 - det_barrier(uv.y * 2.0));
    let near_h = det_mat4_mul_vec4(u.inv_view_proj, vec4<f32>(ndc, 0.0, 1.0));
    let far_h = det_mat4_mul_vec4(u.inv_view_proj, vec4<f32>(ndc, 1.0, 1.0));
    let near_p = det_div3(near_h.xyz, vec3<f32>(near_h.w));
    let far_p = det_div3(far_h.xyz, vec3<f32>(far_h.w));
    let origin = u.camera_pos.xyz;
    let dir = det_normalize3(far_p - near_p);
    let up = det_normalize3(u.up_axis.xyz);

    // Pixels with no cloud march must pass the beauty through unchanged, so the
    // store below always runs (early `return`s would leave the output unwritten
    // and the copy-back would clobber the beauty).
    var transmittance = 1.0;
    var inscatter = vec3<f32>(0.0);
    // Terrain cloud shadow: how much sun the cloud deck blocks above each
    // terrain point (1 = full sun, 0 = fully shadowed).
    var terrain_shadow = 1.0;

    let slab = intersect_slab(origin, dir, up);
    // Bound the march to the intersection of the flat slab and the cloud
    // field's cylinder, so the fixed step budget is spent inside the clouds.
    let cylinder = intersect_cylinder(origin, dir, up);
    let t_lo = max(slab.x, cylinder.x);
    let t_hi = min(slab.y, cylinder.y);
    if t_hi > t_lo && t_hi > 0.0 {
        // Depth-aware early-out: unproject the terrain depth to world space and
        // truncate the slab at the surface distance along the ray.
        let depth = textureLoad(scene_depth, pixel, 0);
        var t_far = t_hi;
        if depth < 1.0 {
            let scene_h = det_mat4_mul_vec4(u.inv_view_proj, vec4<f32>(ndc, depth, 1.0));
            let scene_p = det_div3(scene_h.xyz, vec3<f32>(scene_h.w));
            t_far = min(t_far, det_dot3(scene_p - origin, dir));
            // Cloud shadow: sample the deck offset along the sun's horizontal
            // direction (so the shadow is cast away from the sun) and soften it.
            let sun = u.sun_direction.xyz;
            let sun_up = max(det_dot3(sun, up), 0.25);
            let sun_h_raw = sun - det_barrier3(up * det_dot3(sun, up));
            let sun_h = det_div3(sun_h_raw, vec3<f32>(max(det_length3(sun_h_raw), 1.0e-3)));
            let deck_h = det_barrier(u.layer.x + det_barrier(u.layer.y * 0.5)) - det_barrier(det_dot3(scene_p, up));
            let above = det_barrier3(det_barrier3(scene_p + det_barrier3(up * deck_h)) - det_barrier3(sun_h * (det_div(deck_h, sun_up))));
            let deck = det_barrier(cloud_density(above, 0.5)) * u.layer.y;
            // `deck` is now the optical depth along the sun path (bounded by the
            // density setting), so the shadow threshold moves down with it: 1.1
            // was tuned for the old raw-density scale and cast nothing.
            let s = 1.0 - det_smoothstep(0.0, 0.30, deck);
            terrain_shadow = det_mix(1.0, s, clamp(u.feature.y, 0.0, 1.0) * 0.55);
        }
        let t_near = max(t_lo, 0.0);
        if t_far > t_near {
            // March step count from the caller (0 = use the historical default),
            // clamped into the loop's static bound so the trip count is always
            // decidable without breaking the fixed-iteration contract.
            let wanted = i32(det_barrier(u.feature.z));
            let steps = f32(clamp(wanted, 1, MARCH_STEPS_MAX));
            let dt = det_div(t_far - t_near, steps);
            let jitter_offset = det_barrier(det_barrier(jitter(gid.x, gid.y, u32(u.wind.w))) * dt);
            // Advect the field with wind in the plane perpendicular to the up
            // axis (Y-up: XZ plane; Z-up: XY plane), scaled by speed * time.
            let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(up.y) < 0.9);
            let right = det_normalize3(det_cross3(up, helper));
            let forward = det_cross3(up, right);
            let wind_vec = det_barrier3(det_barrier3((det_barrier3(right * det_cos(u.wind.x)) + det_barrier3(forward * det_sin(u.wind.x))) * u.wind.y) * u.wind.z);
            let cos_theta = det_dot3(dir, u.sun_direction.xyz);
            // Single scattering forward lobe + a cheap dual-lobe multiple
            // scattering approximation so dense interiors do not go black.
            // Frostbite dual lobe: a FORWARD lobe for the sunlit silver lining
            // plus a BACKWARD lobe (g < 0) that rounds the shaded side, so the
            // cloud reads as a lit volume instead of a bright rim on black.
            let phase_single = det_barrier(hg_phase(cos_theta, u.optics.y) * u.optics.x);
            let phase_multi = det_barrier(
                det_barrier(hg_phase(cos_theta, 0.85) * det_barrier(u.optics.x * 0.25))
                    + det_barrier(hg_phase(cos_theta, -0.2) * det_barrier(u.optics.x * 0.20)),
            );

            var t = det_barrier(t_near + jitter_offset);
            for (var i = 0; i < MARCH_STEPS_MAX; i = i + 1) {
                if i >= i32(steps) {
                    break;
                }
                let p = det_barrier3(origin + det_barrier3(dir * t));
                let hf = slab_height_frac(p, up);
                let density = det_barrier(cloud_density(det_barrier3(p - wind_vec), hf));
                if density > 0.0 {
                    let extinction = density * dt;
                    let sample_trans = det_exp(-extinction);
                    // Beer-Powder: darken the sun-lit edge of dense regions.
                    let powder = 1.0 - det_exp(det_barrier(-density * u.optics.w) * 2.0);
                    let sun = det_barrier(sun_transmittance(p, up) * det_mix(1.0, powder, 0.4));
                    // Ambient multiple-scatter follows the SKY's own colour
                    // gradient (Frostbite/HZD): darker blue near the deck base
                    // where the ground blocks the sky, brighter sky blue toward
                    // the top. Neutral grey here is what makes clouds look like
                    // painted cardboard instead of sky-lit volumes.
                    let ambient = det_barrier3(det_mix3(
                        vec3<f32>(0.26, 0.31, 0.42),
                        vec3<f32>(0.74, 0.82, 1.02),
                        hf) * u.optics.x);
                    // Realistic: the sun scatters through the dual lobe. Cartoon:
                    // flat cel bands keyed off the sun term, with the phase lobe
                    // dropped (a stylized deck has no single-scatter rim).
                    var lit = det_barrier3(u.sun_radiance.rgb * det_barrier(sun * det_barrier(phase_single + phase_multi))) + ambient;
                    if u.style.x > 0.5 {
                        lit = cartoon_lit(hf, sun);
                    }
                    inscatter = det_barrier3(inscatter + det_barrier3(det_barrier(transmittance * (1.0 - sample_trans)) * lit));
                    transmittance = det_barrier(transmittance * sample_trans);
                    if transmittance < 0.02 {
                        break;
                    }
                }
                t = det_barrier(t + dt);
            }

            // Aerial perspective: fade the cloud contribution toward the sky
            // colour with distance so the deck sits in the atmosphere rather
            // than on a flat backdrop. `u.up_axis.w` = 1/terrain_span, so
            // `t_far * u.up_axis.w` is the distance measured in scene-widths —
            // the fog then engages at the offline renderer's world units
            // instead of implicitly assuming metre-scale distances.
            //
            // The mix is deliberately weak (0.45 cap, slow 0.22 rate): the
            // earlier 0.7/0.4 pulled the majority of every distant cloud's
            // colour toward a flat analytic sky, which is what greyed the deck
            // out into haze. The cartoon style skips the fog entirely so the
            // flat fills stay pure.
            // Aerial perspective: fade the cloud contribution toward the sky
            // colour with distance so the deck sits in the atmosphere rather
            // than on a flat backdrop. `u.up_axis.w` = 1/terrain_span, so
            // `t_far * u.up_axis.w` is the distance measured in scene-widths —
            // the fog then engages at the offline renderer's world units
            // instead of implicitly assuming metre-scale distances.
            //
            // The sky colour is the real environment, not a bright generic one:
            // a pale analytic sky used to pull EVERY distant puff toward mid
            // grey, collapsing the deck's dynamic range (measured sky-luminance
            // std 13 of 255 — flat, low-contrast mush). Darkening it to the
            // scene's own blue means distance desaturates the cloud toward the
            // sky instead of averaging it out, which is what gives the deck
            // depth. The warm horizon term and the forward-scatter sun glow stay.
            let up_amt = clamp(det_dot3(dir, up), 0.0, 1.0);
            let sky = det_barrier3(det_barrier3(vec3<f32>(0.10, 0.17, 0.30) + det_barrier3(vec3<f32>(0.22, 0.16, 0.08) * det_pow(1.0 - up_amt, 4.0))) + det_barrier3(u.sun_radiance.rgb * det_barrier(0.12 * det_pow(clamp(det_dot3(dir, u.sun_direction.xyz), 0.0, 1.0), 8.0))));
            let fog = det_barrier(det_barrier(0.55 * (1.0 - det_exp(-0.26 * det_barrier(t_far * u.up_axis.w)))) * det_barrier(1.0 - u.style.x));
            inscatter = det_barrier3(det_mix3(inscatter, det_barrier3(sky * (1.0 - transmittance)), fog));
        }
    }

    let previous = textureLoad(beauty_in, pixel, 0);
    let composited = det_barrier3(det_barrier3(previous.rgb * transmittance) * terrain_shadow) + inscatter;
    textureStore(clouds_out, pixel, vec4<f32>(composited, previous.a));
}
