// src/shaders/viewer_tonemap.wgsl
// Fullscreen tonemap for the viewer's linear-HDR cloud composite path.
//
// The terrain PBR shader writes linear radiance (exposure applied) into an
// Rgba16Float beauty, the volumetric cloud pass composites in linear HDR, and
// this pass applies the viewer's ACES curve + gamma into the display target —
// the same curve the terrain uses, so clouds and terrain stay consistent.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

fn aces_tonemap(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + b)) / (color * (c * color + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    var out: VsOut;
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let linear = textureSample(src_tex, src_sampler, in.uv).rgb;
    let mapped = aces_tonemap(linear);
    return vec4<f32>(pow(mapped, vec3<f32>(1.0 / 2.2)), 1.0);
}
