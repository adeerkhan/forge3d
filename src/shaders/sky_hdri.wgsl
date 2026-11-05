// HDRI sky: the project's environment map as the visible background.
//
// Kept separate from `sky.wgsl` so the terrain determinism source hash for the
// analytic models is untouched. The pixel-to-direction mapping mirrors that
// file exactly; the only extra input is the environment texture, which is the
// same HDR that already lights the scene (IBL). Sampling is deterministic:
// fixed UVs, one tap, no derivatives or hardware derivatives.

struct SkyParams {
    sun_direction_turbidity: vec4<f32>,
    ground_albedo_sun_size_sun_intensity_exposure: vec4<f32>,
    model_pad: vec4<u32>,
    hosek_coeffs_a_d: array<vec4<f32>, 3>,
    hosek_coeffs_e_h: array<vec4<f32>, 3>,
    hosek_coeff_i: vec4<f32>,
    hosek_radiance: vec4<f32>,
}

struct CameraUniforms {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_view: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    eye_position: vec3<f32>,
    _pad0: f32,
}

@group(0) @binding(0) var<uniform> sky_params: SkyParams;
@group(0) @binding(1) var output_texture: texture_storage_2d<rgba16float, write>;
@group(1) @binding(0) var<uniform> camera: CameraUniforms;
@group(2) @binding(0) var environment_texture: texture_2d<f32>;
@group(2) @binding(1) var environment_sampler: sampler;

const PI: f32 = 3.14159265359;
const RECIPROCAL_PI: f32 = 0.31830988618;
const RECIPROCAL_PI2: f32 = 0.15915494309;

fn hdri_exposure(params: SkyParams) -> f32 {
    return params.ground_albedo_sun_size_sun_intensity_exposure.w;
}

/// World-space view direction for one output pixel (same as `sky.wgsl`).
fn hdri_direction(pixel: vec2<u32>, dims: vec2<u32>) -> vec3<f32> {
    let uv = det_div2(vec2<f32>(pixel) + 0.5, vec2<f32>(dims));
    let ndc = vec2<f32>(det_barrier(uv.x * 2.0) - 1.0, 1.0 - det_barrier(uv.y * 2.0));
    let clip_pos = vec4<f32>(ndc, 1.0, 1.0);
    let view_pos = det_mat4_mul_vec4(camera.inv_proj, clip_pos);
    let view_dir_vs = det_normalize3(det_div3(view_pos.xyz, vec3<f32>(view_pos.w)));
    return det_normalize3((det_mat4_mul_vec4(camera.inv_view, vec4<f32>(view_dir_vs, 0.0))).xyz);
}

/// Equirectangular UV. `u` matches three's `equirectUv`; `v` is flipped
/// relative to it because the engine uploads Radiance rows in file order while
/// three flips them on upload, so both put the zenith at v = 0.
fn hdri_equirect_uv(dir: vec3<f32>) -> vec2<f32> {
    let u = det_barrier(det_atan2(dir.z, dir.x) * RECIPROCAL_PI2) + 0.5;
    let v = 0.5 - det_barrier(det_asin(clamp(dir.y, -1.0, 1.0)) * RECIPROCAL_PI);
    return vec2<f32>(u, v);
}

@compute @workgroup_size(8, 8, 1)
fn cs_render_hdri_sky(@builtin(global_invocation_id) global_id: vec3<u32>) {
    det_seed(f32(global_id.x));
    let pixel = global_id.xy;
    let dims = textureDimensions(output_texture);

    if (pixel.x >= dims.x || pixel.y >= dims.y) {
        return;
    }

    let view_dir_ws = hdri_direction(pixel, dims);
    let radiance = textureSampleLevel(
        environment_texture,
        environment_sampler,
        hdri_equirect_uv(view_dir_ws),
        0.0,
    ).rgb;
    let exposure = hdri_exposure(sky_params);
    textureStore(
        output_texture,
        pixel,
        vec4<f32>(
            det_barrier(radiance.r * exposure),
            det_barrier(radiance.g * exposure),
            det_barrier(radiance.b * exposure),
            1.0,
        ),
    );
}
