//! The 3D environment grid: a ground-plane graticule drawn over the terrain.
//!
//! Why it lives here and not in the engine: the engine's viewer has no grid
//! overlay, and adding one to its shader would change the pinned terrain
//! shader hash. Studio has the engine's depth view (`viewer_depth`) and its own
//! device, so the grid is a second pass in the same encoder — depth-tested
//! against the terrain, which is the only way a grid is worth anything (lines
//! that float over hills teach you nothing about where the ground is).
//!
//! The camera matrices mirror `TerrainScene`'s viewer exactly: Z-up orbit,
//! `look_at_rh`, `perspective_rh` with the same near/far policy. If they drift,
//! the grid visibly slides off the ground, which is how it was checked.

use wgpu::util::DeviceExt;

use studio_project::CameraParams;

/// One grid vertex: world position plus what kind of line it belongs to.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck_shim::Pod, bytemuck_shim::Zeroable)]
pub struct GridVertex {
    pub position: [f32; 3],
    /// 0 = minor line, 1 = world X axis, 2 = world Y axis.
    pub kind: f32,
}

/// Integer cell index → line classification, so the axes are picked out.
fn line_kind(cell: i32) -> f32 {
    if cell == 0 {
        // Both axes meet at the origin; the X axis wins the corner.
        1.0
    } else {
        0.0
    }
}

/// The grid mesh: `±half_extent` in world units, one line every `spacing`.
///
/// Pure so the geometry can be checked without a GPU, and so the axis
/// classification is testable.
pub fn build_vertices(half_extent: f32, spacing: f32) -> Vec<GridVertex> {
    let half = half_extent.max(1e-3);
    let step = spacing.max(1e-3);
    let cells = (half / step).floor() as i32;
    let mut vertices = Vec::new();
    for cell in -cells..=cells {
        let offset = cell as f32 * step;
        if offset.abs() > half {
            continue;
        }
        // A line along Y at x = offset (the X=0 one is the Y axis).
        let kind_x = if cell == 0 {
            2.0
        } else {
            line_kind(cell) * 0.0
        };
        vertices.push(GridVertex {
            position: [offset, -half, 0.0],
            kind: kind_x,
        });
        vertices.push(GridVertex {
            position: [offset, half, 0.0],
            kind: kind_x,
        });
        // A line along X at y = offset (the Y=0 one is the X axis).
        let kind_y = line_kind(cell);
        vertices.push(GridVertex {
            position: [-half, offset, 0.0],
            kind: kind_y,
        });
        vertices.push(GridVertex {
            position: [half, offset, 0.0],
            kind: kind_y,
        });
    }
    vertices
}

/// View-projection for the engine's viewer camera, matching
/// `ViewerSnapshot::frame_uniforms` in `src/terrain/renderer/viewer.rs`.
pub fn view_projection(camera: &CameraParams, width: u32, height: u32) -> [[f32; 4]; 4] {
    let phi = camera.phi_deg.to_radians();
    let theta = camera.theta_deg.to_radians();
    let radius = camera.radius.max(1.0);
    let target = glam::Vec3::new(camera.target_x, camera.target_z, 0.0);
    let eye = target
        + glam::Vec3::new(
            radius * theta.sin() * phi.cos(),
            radius * theta.sin() * phi.sin(),
            radius * theta.cos(),
        );
    let up = if theta.sin().abs() < 1e-4 {
        glam::Vec3::Y
    } else {
        glam::Vec3::Z
    };
    let view = glam::Mat4::look_at_rh(eye, target, up);
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let near = (radius * 0.01).max(0.1);
    let far = (radius * 20.0).max(near + 1.0);
    let proj = glam::Mat4::perspective_rh(camera.fov_deg.to_radians(), aspect, near, far);
    (proj * view).to_cols_array_2d()
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck_shim::Pod, bytemuck_shim::Zeroable)]
struct GridUniforms {
    view_proj: [[f32; 4]; 4],
    minor: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
}

const SHADER: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    minor: vec4<f32>,
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) kind: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) kind: f32,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.kind = in.kind;
    return out;
}

// Colours are linear: the viewport target is Rgba8Unorm and the readback
// applies the linear -> sRGB encode exactly once.
@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if (in.kind > 1.5) {
        return uniforms.axis_y;
    }
    if (in.kind > 0.5) {
        return uniforms.axis_x;
    }
    return uniforms.minor;
}
"#;

/// How the grid tests depth. `Off`/`Always` exist so the overlay can be A/B'd
/// against the terrain pass when its geometry or depth convention is suspect.
#[derive(Clone, Copy, PartialEq)]
pub enum DepthMode {
    Less,
    Always,
    Off,
}

impl DepthMode {
    fn from_env() -> Self {
        if std::env::var_os("FORGE3D_STUDIO_GRID_NODEPTH").is_some() {
            Self::Off
        } else if std::env::var_os("FORGE3D_STUDIO_GRID_DEPTH_ALWAYS").is_some() {
            Self::Always
        } else {
            Self::Less
        }
    }
}

/// The grid pipeline plus its vertex buffer, drawn after the terrain pass.
pub struct GroundGrid {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    vertices: wgpu::Buffer,
    vertex_count: u32,
    half_extent: f32,
    spacing: f32,
    mode: DepthMode,
}

impl GroundGrid {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        half_extent: f32,
        spacing: f32,
    ) -> Self {
        let vertices = build_vertices(half_extent, spacing);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio.grid.vertices"),
            contents: bytemuck_shim::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("studio.grid.uniforms"),
            size: std::mem::size_of::<GridUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("studio.grid.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("studio.grid.bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio.grid.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("studio.grid.layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("studio.grid.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GridVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: match Self::depth_mode() {
                DepthMode::Off => None,
                mode => Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: false,
                    // The grid sits on the ground plane, so the terrain in front
                    // of it is exactly what has to hide it.
                    depth_compare: match mode {
                        DepthMode::Always => wgpu::CompareFunction::Always,
                        _ => wgpu::CompareFunction::Less,
                    },
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
            },
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        Self {
            pipeline,
            bind_group,
            uniforms,
            vertices: vertex_buffer,
            vertex_count: vertices.len() as u32,
            half_extent,
            spacing,
            mode: DepthMode::from_env(),
        }
    }

    fn depth_mode() -> DepthMode {
        DepthMode::from_env()
    }

    /// The depth mode in use, for the viewport's log line.
    pub fn mode(&self) -> DepthMode {
        self.mode
    }

    /// Rebuild the mesh when the terrain extent changes.
    pub fn rebuild(&mut self, device: &wgpu::Device, half_extent: f32, spacing: f32) {
        if (self.half_extent - half_extent).abs() < 1e-3 && (self.spacing - spacing).abs() < 1e-6 {
            return;
        }
        let vertices = build_vertices(half_extent, spacing);
        self.vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio.grid.vertices"),
            contents: bytemuck_shim::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.vertex_count = vertices.len() as u32;
        self.half_extent = half_extent;
        self.spacing = spacing;
    }

    /// Draw into an existing colour target, depth-testing against `depth_view`.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: Option<&wgpu::TextureView>,
        camera: &CameraParams,
        size: (u32, u32),
    ) {
        let uniforms = GridUniforms {
            view_proj: view_projection(camera, size.0, size.1),
            // Linear values: roughly 0.30 sRGB for minor lines and full-strength
            // tints for the axes.
            minor: [0.055, 0.06, 0.075, 0.85],
            axis_x: [0.30, 0.055, 0.055, 0.95],
            axis_y: [0.06, 0.24, 0.06, 0.95],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck_shim::cast_slice(&[uniforms]));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("studio.grid.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: depth_view.filter(|_| self.mode != DepthMode::Off).map(
                |view| wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                },
            ),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

/// `bytemuck` is not a direct dependency here; this shim keeps the cast in one
/// place instead of pulling a crate in for two calls.
mod bytemuck_shim {
    pub use bytemuck::{cast_slice, Pod, Zeroable};
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_mesh_covers_the_extent_with_the_axes_marked() {
        let vertices = build_vertices(100.0, 10.0);
        // 21 cells each way, 2 lines per cell, 2 vertices per line.
        assert_eq!(vertices.len(), 21 * 2 * 2);
        let xs: Vec<f32> = vertices.iter().map(|v| v.position[0]).collect();
        assert!(xs.iter().cloned().fold(f32::MIN, f32::max) <= 100.0);
        assert!(xs.iter().cloned().fold(f32::MAX, f32::min) >= -100.0);
        // Every vertex sits on the ground plane.
        assert!(vertices.iter().all(|v| v.position[2] == 0.0));
        // Exactly one line is the X axis and one is the Y axis.
        let axis_x = vertices
            .iter()
            .filter(|v| v.kind > 0.5 && v.kind < 1.5)
            .count();
        let axis_y = vertices.iter().filter(|v| v.kind > 1.5).count();
        assert_eq!(axis_x, 2, "two vertices make the X axis line");
        assert_eq!(axis_y, 2, "two vertices make the Y axis line");
    }

    #[test]
    fn a_degenerate_spacing_still_builds_a_finite_mesh() {
        let vertices = build_vertices(0.0, 0.0);
        assert!(!vertices.is_empty());
        assert!(vertices
            .iter()
            .all(|v| v.position.iter().all(|c| c.is_finite())));
    }

    #[test]
    fn the_camera_matrix_matches_the_engines_orbit() {
        // Looking down at 60 degrees from +X: the target is centred, world up
        // projects upward on screen, and a point nearer the camera lands below
        // the centre (the orbit looks along -X and down, so "nearer" is lower).
        let camera = CameraParams {
            phi_deg: 0.0,
            theta_deg: 60.0,
            radius: 1000.0,
            fov_deg: 54.0,
            target_x: 0.0,
            target_z: 0.0,
        };
        let m = glam::Mat4::from_cols_array_2d(&view_projection(&camera, 400, 400));
        let project = |x: f32, y: f32, z: f32| {
            let clip = m * glam::Vec4::new(x, y, z, 1.0);
            glam::Vec3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w)
        };
        let centre = project(0.0, 0.0, 0.0);
        assert!(
            centre.x.abs() < 1e-3 && centre.y.abs() < 1e-3,
            "the orbit target sits in the middle of the frame, got {centre:?}"
        );
        // World up is screen up.
        let above = project(0.0, 0.0, 100.0);
        assert!(
            above.y > 0.02,
            "a point above the target projects above the centre, got {above:?}"
        );
        // Moving toward the camera (along +X, where the eye is) drops the point.
        let nearer = project(200.0, 0.0, 0.0);
        assert!(
            nearer.y < -0.02,
            "a point nearer the camera lands lower in the frame, got {nearer:?}"
        );
        // Depth increases with distance, so the grid can depth-test.
        let far_corner = project(-800.0, 0.0, 0.0);
        assert!(
            far_corner.z > centre.z,
            "further away is deeper in clip space: {} vs {}",
            far_corner.z,
            centre.z
        );
    }
}
