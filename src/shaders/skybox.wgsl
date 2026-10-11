// src/shaders/skybox.wgsl
// Fullscreen HDRI skybox. Reconstructs the world-space view ray per pixel and
// samples an equirectangular environment map.
//
// The lookup basis is built from the world UP axis (uniform), not assumed to
// be +Y: `mesh` cameras are Y-up while `mesh:zup` cameras are Z-up, and a
// hardcoded Y-up map samples a Z-up world sideways (the sky reads as a flat
// wrong-coloured band). The map convention matches `formats::hdr::load_hdr`
// (row 0 = image top for the conventional `-Y +X` marker) and
// `ibl_equirect.wgsl` (up -> v = 0).

struct SkyboxUniforms {
    inv_view_proj: mat4x4<f32>,
    up_axis: vec4<f32>,         // xyz world up of the camera frame, w unused
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

    // Up-relative equirectangular basis. For a Y-up world this reduces to
    // exactly the IBL convention: u = atan2(dir.z, dir.x) / 2PI + 0.5 and
    // v = 0.5 - asin(dir.y) / PI (up -> row 0 = image top). For a Z-up world
    // the same basis samples the environment with the correct vertical axis.
    let up = normalize(u.up_axis.xyz);
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(up.y) < 0.9);
    let right = normalize(cross(up, helper));
    let north = cross(right, up);
    let uvw = atan2(dot(dir, north), dot(dir, right)) * (1.0 / (2.0 * PI)) + 0.5;
    let v = 0.5 - asin(clamp(dot(dir, up), -1.0, 1.0)) * (1.0 / PI);
    let col = textureSampleLevel(sky_tex, sky_sampler, vec2<f32>(uvw, v), 0.0).rgb;
    return vec4<f32>(col, 1.0);
}
