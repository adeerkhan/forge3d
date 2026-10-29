//! M10/G0: the engine-device viewport.
//!
//! The interactive viewport paints `TerrainScene`'s viewer path on the engine's
//! negotiated device and reads the frame back as RGBA, so two properties have
//! to hold off-screen:
//!
//! * the per-scene resource cache reproduces a frame byte for byte (a cached
//!   bind group that silently rotted would show up as a shifting image);
//! * the document's colormap and sun reach the frame, and a rejected colormap
//!   leaves the previous frame untouched instead of degrading quietly.
//!
//! The sun assertions also pin the Z-up light convention: elevation must land
//! on the terrain's up axis, otherwise a low sun leaves the ground unlit.

use std::path::PathBuf;

use forge3d::terrain::renderer::TerrainScene;
use studio_engine::NativeViewport;
use studio_project::{CameraParams, SunParams};

fn fuji_dem() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets/tif/Mount_Fuji_30m.tif")
}

/// Studio's world scale for a dataset: relief-to-span 1:6, framed at 1.65x.
fn studio_camera(scene: &TerrainScene) -> (f32, CameraParams) {
    let terrain = scene.viewer_terrain().expect("terrain loaded");
    let span = (terrain.domain.1 - terrain.domain.0).abs().max(1.0) * 6.0;
    let camera = CameraParams {
        phi_deg: 135.0,
        theta_deg: 58.0,
        radius: span * 1.65,
        fov_deg: 54.0,
        target_x: 0.0,
        target_z: 0.0,
    };
    (span, camera)
}

struct Frame {
    rgba: Vec<u8>,
}

impl Frame {
    /// Mean luminance-equivalent over terrain pixels only.
    ///
    /// Frames are drawn over a cleared black target, so anything the terrain
    /// pass did not cover stays exactly 0. Measuring the whole frame would let
    /// the empty background swamp the lighting signal.
    fn terrain_mean(&self) -> f64 {
        let (sum, count) = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] != 0 || px[1] != 0 || px[2] != 0)
            .fold((0.0f64, 0usize), |(sum, count), px| {
                let value = (px[0] as f64 + px[1] as f64 + px[2] as f64) / 3.0;
                (sum + value, count + 1)
            });
        assert!(count > 0, "no terrain pixels in the frame");
        sum / count as f64
    }

    /// Share of the frame the terrain covers.
    fn terrain_fraction(&self) -> f64 {
        let covered = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] != 0 || px[1] != 0 || px[2] != 0)
            .count();
        covered as f64 / (self.rgba.len() / 4) as f64
    }

    fn brightest(&self) -> u8 {
        self.rgba.iter().copied().max().unwrap_or(0)
    }
}

fn diff_mean(a: &Frame, b: &Frame) -> f64 {
    a.rgba
        .iter()
        .zip(&b.rgba)
        .map(|(&x, &y)| (x as f64 - y as f64).abs())
        .sum::<f64>()
        / a.rgba.len() as f64
}

fn render_frame(scene: &mut TerrainScene, width: u32, height: u32) -> Frame {
    let ctx = forge3d::core::gpu::try_ctx().expect("GPU context");
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m10.viewport"),
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
            label: Some("m10.viewport"),
        });
    // Clear to black first, then draw with `background_initialized = true`, so
    // "not black" means "the terrain drew here".
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("m10.viewport.clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
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
    assert!(
        scene.render_viewer_terrain(
            &mut encoder,
            &view,
            wgpu::TextureFormat::Rgba8Unorm,
            width,
            height,
            true,
        ),
        "the viewer path must draw once a terrain is loaded"
    );
    ctx.queue.submit(Some(encoder.finish()));
    let rgba = forge3d::renderer::readback::read_texture_tight(
        &ctx.device,
        &ctx.queue,
        &texture,
        (width, height),
        wgpu::TextureFormat::Rgba8Unorm,
    )
    .expect("readback");
    Frame { rgba }
}

#[test]
fn cached_resources_reproduce_frames_and_honour_colour_and_sun() {
    // TerrainScene construction compiles the mega-shaders; the default test
    // stack overflows (same as the engine worker and the app's startup
    // thread).
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("spawn render thread")
        .join()
        .expect("render test body");
}

fn body() {
    let _ = env_logger::try_init();
    let ctx = forge3d::core::gpu::try_ctx().expect("GPU context");
    let mut scene = TerrainScene::new(ctx.device.clone(), ctx.queue.clone(), ctx.adapter.clone())
        .expect("scene construction");
    let dem = fuji_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    scene
        .load_terrain_for_viewer(&dem.to_string_lossy())
        .expect("load DEM");

    let (span, camera) = studio_camera(&scene);
    let sun = SunParams {
        azimuth_deg: 135.0,
        elevation_deg: 35.0,
        intensity: 2.5,
    };
    scene.set_viewer_world_scale(span, 1.35);
    scene.set_viewer_target(camera.target_x, camera.target_z);
    scene.set_viewer_camera(
        camera.phi_deg,
        camera.theta_deg,
        camera.radius,
        camera.fov_deg,
    );
    scene.set_viewer_sun(sun.azimuth_deg, sun.elevation_deg, sun.intensity);

    let (width, height) = (320u32, 240u32);
    let first = render_frame(&mut scene, width, height);
    assert!(
        first.terrain_fraction() > 0.10,
        "the terrain covers only {:.1}% of the frame — the camera framing is wrong",
        first.terrain_fraction() * 100.0
    );
    assert!(
        first.brightest() > 60,
        "frame has no bright pixels — nothing shaded"
    );

    // The cached uniform buffers/bind groups/depth target must serve the same
    // bytes on a second frame: a stale cache would drift the image.
    let second = render_frame(&mut scene, width, height);
    assert_eq!(
        first.rgba, second.rgba,
        "the viewer cache must reproduce the same frame byte for byte"
    );

    // Colormap: the document's palette reaches the albedo.
    scene
        .set_viewer_colormap(Some("magma"))
        .expect("magma is a registry colormap");
    let magma = render_frame(&mut scene, width, height);
    assert!(
        diff_mean(&first, &magma) > 1.0,
        "switching the colormap changed nothing (mean abs diff {:.3})",
        diff_mean(&first, &magma)
    );
    scene
        .set_viewer_colormap(Some("forge3d:viridis"))
        .expect("the forge3d: prefix is accepted");
    let viridis = render_frame(&mut scene, width, height);
    assert!(
        diff_mean(&magma, &viridis) > 1.0,
        "magma and viridis must not paint the same frame"
    );

    // An unknown name is an error, and it must leave the frame alone.
    assert!(
        scene.set_viewer_colormap(Some("not-a-colormap")).is_err(),
        "an unknown colormap must be rejected, not silently replaced"
    );
    let after_reject = render_frame(&mut scene, width, height);
    assert_eq!(
        viridis.rgba, after_reject.rgba,
        "a rejected colormap must leave the previous frame untouched"
    );

    // Back to the relief ramp: rebuilding the palette must return the exact
    // original frame, not just a similar one.
    scene
        .set_viewer_colormap(None)
        .expect("the default ramp is always available");
    let relief = render_frame(&mut scene, width, height);
    assert_eq!(
        first.rgba, relief.rgba,
        "returning to the default ramp must restore the original frame"
    );

    // Sun elevation lives on the terrain's up (+Z) axis. A 5 degree sun is the
    // discriminating case: with the old Y-up vector its horizontal components
    // would point the light *into* the ground and the terrain would go
    // nearly black instead of grazingly lit.
    scene.set_viewer_sun(sun.azimuth_deg, 5.0, sun.intensity);
    let grazing = render_frame(&mut scene, width, height);
    scene.set_viewer_sun(sun.azimuth_deg, 80.0, sun.intensity);
    let overhead = render_frame(&mut scene, width, height);
    std::fs::write(
        "/tmp/m10-grazing-sun.png",
        rgba_as_png(&grazing.rgba, width, height),
    )
    .ok();
    std::fs::write(
        "/tmp/m10-overhead-sun.png",
        rgba_as_png(&overhead.rgba, width, height),
    )
    .ok();
    println!(
        "terrain mean: relief {:.1} grazing {:.1} overhead {:.1}; coverage {:.1}%",
        relief.terrain_mean(),
        grazing.terrain_mean(),
        overhead.terrain_mean(),
        relief.terrain_fraction() * 100.0
    );
    assert!(
        grazing.terrain_mean() > 10.0,
        "a 5 degree sun left the terrain at {:.1} — elevation is not on the up axis",
        grazing.terrain_mean()
    );
    assert!(
        diff_mean(&grazing, &overhead) > 1.0,
        "sun elevation changed nothing — the light is not reaching the frame"
    );

    // Azimuth rotates the light around the up axis: the opposite bearing must
    // light the opposite slopes, so the frame changes.
    scene.set_viewer_sun(315.0, sun.elevation_deg, sun.intensity);
    let from_west = render_frame(&mut scene, width, height);
    scene.set_viewer_sun(sun.azimuth_deg, sun.elevation_deg, sun.intensity);
    let from_east = render_frame(&mut scene, width, height);
    assert!(
        diff_mean(&from_west, &from_east) > 1.0,
        "sun azimuth changed nothing — the light has no horizontal direction"
    );
}

/// Minimal RGBA8 PNG writer for eyeballing the renders in /tmp.
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

#[test]
fn native_viewport_reports_its_state_honestly() {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(native_viewport_body)
        .expect("spawn viewport thread")
        .join()
        .expect("viewport test body");
}

fn native_viewport_body() {
    let _ = env_logger::try_init();
    let mut viewport = NativeViewport::new().expect("native viewport construction");

    // No terrain yet: render must say so, not present a blank frame.
    assert!(!viewport.has_terrain());
    assert!(viewport.render_rgba(64, 48).is_err());

    let dem = fuji_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    let (width, height, height_min, height_max) = viewport.load_dem(&dem).expect("load DEM");
    assert!(width > 0 && height > 0, "the loader reports its grid");
    assert!(
        height_max > height_min,
        "the loader reports its elevation range"
    );
    assert!(viewport.has_terrain());

    let camera = CameraParams {
        phi_deg: 135.0,
        theta_deg: 58.0,
        radius: 34_316.0,
        fov_deg: 54.0,
        target_x: 0.0,
        target_z: 0.0,
    };
    let sun = SunParams {
        azimuth_deg: 135.0,
        elevation_deg: 35.0,
        intensity: 2.5,
    };
    viewport.set_view(&camera, &sun);

    let first = viewport.render_rgba(320, 240).expect("render");
    assert_eq!((first.width, first.height), (320, 240));
    assert_eq!(first.rgba.len(), 320 * 240 * 4, "tight RGBA rows");
    assert!(
        first.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255),
        "the presented frame is opaque"
    );
    let covered = first
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] != 0 || px[1] != 0 || px[2] != 0)
        .count();
    assert!(
        covered as f64 / (320.0 * 240.0) > 0.10,
        "the terrain covers only {:.1}% of the frame — framing is wrong",
        covered as f64 / (320.0 * 240.0) * 100.0
    );
    assert!(
        first.rgba.as_chunks::<4>().0.iter().any(|px| px[0] > 60),
        "no bright pixels: nothing shaded"
    );

    // The readback path is deterministic: same view, same bytes.
    let again = viewport.render_rgba(320, 240).expect("second render");
    assert_eq!(first.rgba, again.rgba, "two renders of one view must match");
    assert!(viewport.last_render_ms() >= 0.0);

    // A resize reallocates the target and reports the new size.
    let resized = viewport.render_rgba(96, 64).expect("resized render");
    assert_eq!((resized.width, resized.height), (96, 64));
    assert_eq!(resized.rgba.len(), 96 * 64 * 4);

    // Colormaps: a known name is accepted, an unknown one is an error.
    viewport.set_colormap(Some("viridis")).expect("viridis");
    let viridis = viewport.render_rgba(96, 64).expect("render in viridis");
    assert!(viewport.set_colormap(Some("not-a-colormap")).is_err());
    let after_reject = viewport
        .render_rgba(96, 64)
        .expect("a rejected colormap must not break the viewport");
    assert_eq!(
        viridis.rgba, after_reject.rgba,
        "a rejected colormap must leave the previous palette in place"
    );

    // The outliner's visibility and opacity reach the pixels the UI shows:
    // hidden means the backdrop the renderer clears to, and a faded layer sits
    // between the backdrop and the full frame.
    viewport.set_view(&camera, &sun);
    let full = viewport.render_rgba(96, 64).expect("baseline render");
    viewport.set_layer_style(false, 1.0);
    let hidden = viewport.render_rgba(96, 64).expect("hidden render");
    let first = &hidden.rgba[0..3];
    assert!(
        hidden
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| px[..3] == *first),
        "a hidden layer must paint the flat backdrop"
    );
    assert_ne!(full.rgba, hidden.rgba, "hiding must change the frame");

    viewport.set_layer_style(true, 0.5);
    let faded = viewport.render_rgba(96, 64).expect("faded render");
    assert_ne!(faded.rgba, full.rgba, "opacity must change the frame");
    assert_ne!(faded.rgba, hidden.rgba, "half opacity is not hidden");

    viewport.set_layer_style(true, 1.0);
    let restored = viewport.render_rgba(96, 64).expect("restored render");
    assert_eq!(
        restored.rgba, full.rgba,
        "full opacity is the original frame"
    );

    viewport.clear_dem();
    assert!(!viewport.has_terrain());
    assert!(
        viewport.render_rgba(64, 48).is_err(),
        "clearing the DEM must stop the viewport, not leave the last frame live"
    );
}
