//! Display encoding on the GPU: linear frame → sRGB bytes, with the layer
//! opacity mixed in, written into a second target the readback can copy.
//!
//! Why this exists: the CPU pass did per-pixel work on every frame — a 256-entry
//! sRGB lookup, a blend, and four `Vec::push` calls per pixel. Measured at
//! 1920×1080 on the RTX 5060 that was **3.9 ms of a 6.9 ms frame (57 %)** — more
//! than the GPU took to draw the terrain. The same work on the GPU costs a
//! fraction of a millisecond and turns the readback into a row copy.
//!
//! The arithmetic deliberately mirrors the CPU path, because a colour change
//! here would silently alter every frame:
//!
//! * the source is the rendered linear value, exactly as stored (`textureLoad`,
//!   no filtering, so `u8/255` round-trips),
//! * sRGB encoding uses the same curve and the same boundary (0.0031308) as
//!   `srgb_encode_table`,
//! * the opacity blend happens *after* encoding, in sRGB space, with the
//!   background taken from the same table — matching
//!   `(src*keep + bg*(255-keep))/255` to within the rounding of one byte.
//!
//! The test in `tests/native_viewport.rs` renders the same frame through both
//! paths and requires a per-channel difference of at most 1.

use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct Uniforms {
    background: vec3<f32>,
    opacity: f32,
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<uniform> uniforms: Uniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

// Fullscreen triangle; the fragment's own pixel coordinate addresses the source
// texture, so no filtering or uv flip can introduce a half-pixel offset.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.position = vec4<f32>(corners[index], 0.0, 1.0);
    return out;
}

// Same transfer function as `srgb_encode_table`.
fn encode_srgb(linear: vec3<f32>) -> vec3<f32> {
    let clamped = clamp(linear, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = clamped * 12.92;
    let high = 1.055 * pow(clamped, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, clamped <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(input.position.xy);
    let linear = textureLoad(source, pixel, 0).rgb;
    let encoded = encode_srgb(linear);
    // Blend in display space, exactly as the CPU path did.
    let mixed = mix(uniforms.background, encoded, uniforms.opacity);
    return vec4<f32>(mixed, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlitUniforms {
    /// Background in *sRGB* bytes/255, the space the blend happens in.
    background: [f32; 3],
    opacity: f32,
}

/// The display pass: one pipeline, one bind group per target size.
pub struct DisplayBlit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    size: (u32, u32),
}

impl DisplayBlit {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("studio.display.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio.display.uniforms"),
            contents: bytemuck::bytes_of(&BlitUniforms {
                background: [0.0, 0.0, 0.0],
                opacity: 1.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio.display.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("studio.display.layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("studio.display.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        Self {
            pipeline,
            layout,
            uniforms,
            bind_group: None,
            size: (0, 0),
        }
    }

    /// Bind the linear frame; rebuilt only when the target size changes.
    pub fn bind(&mut self, device: &wgpu::Device, source: &wgpu::TextureView, size: (u32, u32)) {
        if self.size == size && self.bind_group.is_some() {
            return;
        }
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("studio.display.bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.uniforms.as_entire_binding(),
                },
            ],
        }));
        self.size = size;
    }

    /// Encode the display pass into `target` (same size as the bound source).
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        background_srgb: [f32; 3],
        opacity: f32,
    ) {
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&BlitUniforms {
                background: background_srgb,
                opacity: opacity.clamp(0.0, 1.0),
            }),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("studio.display.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        if let Some(bind_group) = self.bind_group.as_ref() {
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
