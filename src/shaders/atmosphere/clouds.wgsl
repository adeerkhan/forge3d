// src/shaders/atmosphere/clouds.wgsl
// Depth-aware volumetric cloud raymarch composited into the linear-HDR beauty
// buffer after terrain and before accumulation/tonemap.
//
// Deterministic: fixed loop counts, no derivatives, `textureSampleLevel`
// instead of `textureSample`, integer-hash blue-noise jitter seeded by the
// per-sample index, and det_* arithmetic from the determinism prelude.

struct CloudUniforms {
    // world -> clip is unused here; we invert the camera clip transform.
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,      // xyz camera position (world), w unused
    sun_direction: vec4<f32>,   // xyz normalized sun direction, w unused
    sun_radiance: vec4<f32>,    // rgb sun radiance * intensity, w unused
    layer: vec4<f32>,           // x altitude_m, y thickness_m, z coverage, w density
    optics: vec4<f32>,          // x scatter_strength, y phase_g, z detail, w powder
    wind: vec4<f32>,            // x dir_radians, y speed, z time_seconds, w sample_index
    screen: vec4<f32>,          // x width, y height, z far fallback for level rays, w unused
};

@group(0) @binding(0) var<uniform> u: CloudUniforms;
@group(0) @binding(1) var base_noise: texture_3d<f32>;
@group(0) @binding(2) var base_sampler: sampler;
@group(0) @binding(3) var detail_noise: texture_3d<f32>;
@group(0) @binding(4) var detail_sampler: sampler;
@group(0) @binding(5) var scene_depth: texture_depth_2d;
@group(0) @binding(6) var beauty_in: texture_2d<f32>;
@group(0) @binding(7) var clouds_out: texture_storage_2d<rgba16float, write>;

const PI: f32 = 3.141592653589793;
const MARCH_STEPS: i32 = 48;
const LIGHT_STEPS: i32 = 5;

// Henyey-Greenstein phase function.
fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = det_barrier(g * g);
    let denom = det_barrier(1.0 + g2) - det_barrier(det_barrier(2.0 * g) * cos_theta);
    return det_div(1.0 - g2, det_barrier(4.0 * PI * max(denom, 1.0e-4)) * det_sqrt(max(denom, 1.0e-4)));
}

// Blue-noise-ish jitter in [0,1) from an integer hash of pixel + sample.
fn jitter(px: u32, py: u32, sample_index: u32) -> f32 {
    var h = px * 0x8da6b343u ^ py * 0xd8163841u ^ (sample_index + 1u) * 0xcb1ab31fu;
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return f32(h) * (1.0 / 4294967296.0);
}

// Base spheroidal FBM shape in [0,1].
fn base_shape(p: vec3<f32>) -> f32 {
    let s0 = textureSampleLevel(base_noise, base_sampler, p * 0.5, 0.0).r;
    let s1 = textureSampleLevel(base_noise, base_sampler, p, 0.0).r;
    let s2 = textureSampleLevel(base_noise, base_sampler, p * 2.0, 0.0).r;
    let s3 = textureSampleLevel(base_noise, base_sampler, p * 4.0, 0.0).r;
    return (det_barrier(det_barrier(det_barrier(s0 * 0.5) + det_barrier(s1 * 0.25)) + det_barrier(s2 * 0.15)) + det_barrier(s3 * 0.1));
}

// Erosion detail in [0,1] (1 = no erosion).
fn detail_erosion(p: vec3<f32>, amount: f32) -> f32 {
    let d0 = textureSampleLevel(detail_noise, detail_sampler, p * 2.0, 0.0).r;
    let d1 = textureSampleLevel(detail_noise, detail_sampler, p * 5.0, 0.0).r;
    let d = det_mix(1.0, (det_barrier(d0 * 0.6) + det_barrier(d1 * 0.4)), clamp(amount, 0.0, 1.0));
    return d;
}

fn cloud_density(p: vec3<f32>, height_frac: f32) -> f32 {
    let shape = det_barrier(base_shape(p));
    let coverage = u.layer.z;
    let eroded = shape - det_barrier((1.0 - coverage) * det_mix(0.6, 1.0, height_frac));
    if eroded <= 0.0 {
        return 0.0;
    }
    let detail = detail_erosion(p, u.optics.z);
    let vprofile = det_barrier(det_smoothstep(0.0, 0.18, height_frac) * (1.0 - det_smoothstep(0.72, 1.0, height_frac)));
    return max(det_barrier(eroded * detail) * vprofile, 0.0) * u.layer.w;
}

fn intersect_slab(origin: vec3<f32>, dir: vec3<f32>) -> vec2<f32> {
    let base = u.layer.x;
    let top = det_barrier(u.layer.x + u.layer.y);
    if abs(dir.y) < 1.0e-5 {
        if origin.y < base || origin.y > top {
            return vec2<f32>(0.0, -1.0);
        }
        return vec2<f32>(0.0, u.screen.z);
    }
    let t0 = det_div(base - origin.y, dir.y);
    let t1 = det_div(top - origin.y, dir.y);
    return vec2<f32>(min(t0, t1), max(t0, t1));
}

fn sun_transmittance(p: vec3<f32>) -> f32 {
    let dir = u.sun_direction.xyz;
    let step_len = det_div(u.layer.y, f32(LIGHT_STEPS));
    var optical = 0.0;
    var q = p;
    for (var i = 0; i < LIGHT_STEPS; i = i + 1) {
        q = det_barrier3(q + det_barrier3(dir * step_len));
        let hf = clamp(det_div(q.y - u.layer.x, max(u.layer.y, 1.0e-4)), 0.0, 1.0);
        optical = det_barrier(optical + det_barrier(det_barrier(cloud_density(q, hf)) * step_len));
    }
    return det_exp(-optical);
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
    let uv = (det_div2(vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5), dims_f));
    let ndc = vec2<f32>(det_barrier(uv.x * 2.0) - 1.0, 1.0 - det_barrier(uv.y * 2.0));
    let near_h = det_mat4_mul_vec4(u.inv_view_proj, vec4<f32>(ndc, 0.0, 1.0));
    let far_h = det_mat4_mul_vec4(u.inv_view_proj, vec4<f32>(ndc, 1.0, 1.0));
    let near_p = det_div3(near_h.xyz, vec3<f32>(near_h.w));
    let far_p = det_div3(far_h.xyz, vec3<f32>(far_h.w));
    let origin = u.camera_pos.xyz;
    let dir = det_normalize3(far_p - near_p);

    let slab = intersect_slab(origin, dir);
    if slab.y <= slab.x || slab.y <= 0.0 {
        return;
    }

    // Depth-aware early-out: the slab is truncated at the terrain surface.
    let depth = textureLoad(scene_depth, pixel, 0);
    var t_far = slab.y;
    if depth < 1.0 {
        let scene_t = u.screen.z * depth;
        t_far = min(t_far, scene_t);
    }
    let t_near = max(slab.x, 0.0);
    if t_far <= t_near {
        return;
    }

    let steps = f32(MARCH_STEPS);
    let dt = det_div(t_far - t_near, steps);
    let jitter_offset = det_barrier(det_barrier(jitter(gid.x, gid.y, u32(u.wind.w))) * dt);
    let wind_offset = det_barrier2(vec2<f32>(det_cos(u.wind.x), det_sin(u.wind.x)) * u.wind.y) * u.wind.z;

    var transmittance = 1.0;
    var inscatter = vec3<f32>(0.0);
    let cos_theta = det_dot3(dir, u.sun_direction.xyz);
    let phase = det_barrier(hg_phase(cos_theta, u.optics.y) * u.optics.x);

    var t = det_barrier(t_near + jitter_offset);
    for (var i = 0; i < MARCH_STEPS; i = i + 1) {
        let p = det_barrier3(origin + det_barrier3(dir * t));
        let hf = clamp(det_div(det_barrier(p.y) - u.layer.x, max(u.layer.y, 1.0e-4)), 0.0, 1.0);
        let density = det_barrier(cloud_density(p + det_barrier3(vec3<f32>(wind_offset.x, 0.0, wind_offset.y)), hf));
        if density > 0.0 {
            let extinction = density * dt;
            let sample_trans = det_exp(-extinction);
            // Beer-Powder: darken the sun-lit edge of dense regions.
            let powder = 1.0 - det_exp(det_barrier(-density * u.optics.w) * 2.0);
            let sun = det_barrier(sun_transmittance(p) * det_mix(1.0, powder, 0.4));
            let lit = det_barrier3(u.sun_radiance.rgb * (det_barrier(sun * phase))) + det_barrier3(vec3<f32>(0.35, 0.42, 0.55) * u.optics.x);
            inscatter = det_barrier3(inscatter + det_barrier3(det_barrier(transmittance * (1.0 - sample_trans)) * lit));
            transmittance = det_barrier(transmittance * sample_trans);
            if transmittance < 0.02 {
                break;
            }
        }
        t = det_barrier(t + dt);
    }

    let previous = textureLoad(beauty_in, pixel, 0);
    let composited = det_barrier3(previous.rgb * transmittance) + inscatter;
    textureStore(clouds_out, pixel, vec4<f32>(composited, previous.a));
}
