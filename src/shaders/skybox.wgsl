// src/shaders/skybox.wgsl
// Fullscreen HDRI skybox. Reconstructs the world-space view ray per pixel and
// samples an equirectangular environment map.

struct SkyboxUniforms {
    inv_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: SkyboxUniforms;
@group(0) @binding(1) var sky_tex: texture_2d<f32>;
@group(0) @binding(2) var sky_sampler: sampler;

const PI: f32 = 3.141592653589793;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    // Fullscreen triangle.
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let xy = p[vi];
    var out: VSOut;
    out.pos = vec4<f32>(xy, 1.0, 1.0);  // z=1 (far) so the skybox is behind everything
    out.ndc = xy;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    // Reconstruct the world-space ray through this pixel.
    let near_h = u.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far_h  = u.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let near_p = near_h.xyz / near_h.w;
    let far_p  = far_h.xyz / far_h.w;
    let dir = normalize(far_p - near_p);

    // Equirectangular UV. `v` maps world +Y (up) to v=0, i.e. the FIRST row of
    // the texture — which is what `formats::hdr::load_hdr` puts there for the
    // conventional `-Y +X` Radiance marker (row 0 = image top = sky), and the
    // same convention `ibl_equirect.wgsl` uses (`v = acos(d.y) / PI`). Adding
    // instead of subtracting here would hang the sky below the horizon.
    let uvw = atan2(dir.z, dir.x) * (1.0 / (2.0 * PI)) + 0.5;
    let v = 0.5 - asin(clamp(dir.y, -1.0, 1.0)) * (1.0 / PI);
    let col = textureSampleLevel(sky_tex, sky_sampler, vec2<f32>(uvw, v), 0.0).rgb;
    return vec4<f32>(col, 1.0);
}
