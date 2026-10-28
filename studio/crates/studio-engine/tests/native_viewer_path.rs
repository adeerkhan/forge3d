//! M9 (R1): the native `TerrainScene` viewer path must render again.
//!
//! The path drifted from the live pipeline: the bind group grew to 23
//! entries, the pipeline gained a depth attachment and seven bind groups,
//! and binding 3 expects a D2Array material atlas. This test renders a DEM
//! through `render_viewer_terrain` into an offscreen target and asserts the
//! output is a real, non-degenerate terrain frame — the gate for the
//! embedded GPU viewport (M10), which shares this path.

use std::path::PathBuf;

use forge3d::terrain::renderer::TerrainScene;

fn fuji_dem() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets/tif/Mount_Fuji_30m.tif")
}

#[test]
fn readback_clear_is_not_black() {
    // Sanity: the offscreen + readback plumbing itself works.
    let ctx = forge3d::core::gpu::try_ctx().expect("GPU context");
    let (width, height) = (64u32, 64u32);
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m9.clear-test"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("m9.clear"),
        });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("m9.clear-pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.5,
                    g: 0.7,
                    b: 0.9,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    ctx.queue.submit(Some(encoder.finish()));
    ctx.device.poll(wgpu::Maintain::Wait);
    let rgba: Vec<u8> = forge3d::renderer::readback::read_texture_tight(
        &ctx.device,
        &ctx.queue,
        &texture,
        (width, height),
        wgpu::TextureFormat::Rgba8Unorm,
    )
    .expect("readback");
    let mean: f64 = rgba.iter().map(|&b| b as f64).sum::<f64>() / rgba.len() as f64;
    println!("clear-test mean: {mean:.1}");
    assert!(
        mean > 100.0,
        "clear+readback must be non-black, got {mean:.1}"
    );
}

#[test]
fn viewer_path_renders_a_real_terrain_frame() {
    // TerrainScene construction compiles the mega-shaders; the default test
    // stack overflows (same as the engine worker).
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(render_test_body)
        .expect("spawn render thread")
        .join()
        .expect("render test body");
}

fn render_test_body() {
    let _ = env_logger::try_init();
    let ctx = forge3d::core::gpu::try_ctx().expect("GPU context");
    let mut scene = TerrainScene::new(ctx.device.clone(), ctx.queue.clone(), ctx.adapter.clone())
        .expect("scene construction");

    let dem = fuji_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    scene
        .load_terrain_for_viewer(&dem.to_string_lossy())
        .expect("DEM load");
    scene.set_viewer_camera(135.0, 58.0, 4000.0, 54.0);
    scene.set_viewer_sun(138.0, 24.0, 2.4);

    let (width, height) = (480u32, 360u32);
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m9.offscreen"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("m9.encoder"),
        });
    let drawn = scene.render_viewer_terrain(
        &mut encoder,
        &view,
        wgpu::TextureFormat::Rgba8Unorm,
        width,
        height,
        false,
    );
    assert!(drawn, "render_viewer_terrain must draw the terrain");
    ctx.queue.submit(Some(encoder.finish()));
    ctx.device.poll(wgpu::Maintain::Wait);

    let rgba: Vec<u8> = forge3d::renderer::readback::read_texture_tight(
        &ctx.device,
        &ctx.queue,
        &texture,
        (width, height),
        wgpu::TextureFormat::Rgba8Unorm,
    )
    .expect("readback");
    assert_eq!(rgba.len(), (width * height * 4) as usize);

    let mut min = u8::MAX;
    let mut max = 0u8;
    let mut sum = 0u64;
    for px in rgba.as_chunks::<4>().0 {
        for &c in &px[..3] {
            min = min.min(c);
            max = max.max(c);
            sum += c as u64;
        }
    }
    let mean = sum as f64 / (width as f64 * height as f64 * 3.0);
    println!("viewer-path render: min={min} max={max} mean={mean:.1}");
    std::fs::write("/tmp/m9-viewer-path.png", rgba_as_png(&rgba, width, height)).ok();
    assert!(max > min, "render is flat — nothing drew");
    assert!(
        mean > 5.0,
        "frame is black except for nothing; the terrain did not shade"
    );
}

/// Minimal RGBA8 PNG writer for eyeballing the render in /tmp.
fn rgba_as_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let img =
            image::RgbaImage::from_vec(width, height, rgba.to_vec()).expect("buffer size matches");
        img.write_to(&mut out, image::ImageFormat::Png)
            .expect("png encode");
    }
    out.into_inner()
}
