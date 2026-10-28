//! Part VII parity harness for M6: offline accumulation and path tracing.
//!
//! The two quality paths Studio exposes are the engine's own entry points —
//! `forge3d.render_offline` (TV12 accumulation, optional A-trous/OIDN) and
//! `forge3d.render_terrain_poster` (converged hybrid-ReSTIR plate). These
//! tests drive them through the worker exactly as the app does
//! (`SetRenderQuality` + `RenderFrame` / `RenderToFile`) and assert:
//!
//! * the frame's provenance is the path that actually ran (`quality: None`
//!   for the one-shot raster, the engine's metadata otherwise);
//! * a deterministic quality render repeats byte-identically;
//! * a setting that must change the image does (samples, seed);
//! * progress streams for a long render;
//! * a denoiser or tracer this environment cannot run is refused with the
//!   engine's own reason, never silently downgraded.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use studio_engine::{Command, EngineHandle, Event};
use studio_project::{
    CameraParams, OfflineSettings, PathTraceSettings, RenderMode, RenderQuality, SunParams,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The 256² mini DEM: small enough for a quality render to finish in seconds,
/// and under the colormap-albedo cap so the per-texel albedo path is exercised.
fn mini_dem() -> PathBuf {
    repo_root().join("python/forge3d/data/mini_dem.npy")
}

fn scratch_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/quality-fixtures");
    std::fs::create_dir_all(&dir).expect("create quality fixture dir");
    dir
}

fn wait_for<T>(
    events: &std::sync::mpsc::Receiver<Event>,
    timeout: Duration,
    pred: impl Fn(&Event) -> Option<T>,
    what: &str,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("timed out waiting for {what}"));
        match events.recv_timeout(remaining) {
            Ok(event) => {
                if let Some(result) = pred(&event) {
                    return result;
                }
                if let Event::Error { context, message } = &event {
                    panic!("engine error while waiting for {what}: {context}: {message}");
                }
            }
            Err(err) => panic!("engine channel closed while waiting for {what}: {err}"),
        }
    }
}

fn spawn_ready() -> (EngineHandle, std::sync::mpsc::Receiver<Event>) {
    let _ = env_logger::try_init();
    let (engine, events) = EngineHandle::spawn();
    wait_for(
        &events,
        Duration::from_secs(300),
        |e| matches!(e, Event::Ready { .. }).then_some(()),
        "GPU Ready",
    );
    (engine, events)
}

fn call_ok(
    engine: &EngineHandle,
    events: &std::sync::mpsc::Receiver<Event>,
    op: &str,
    params: serde_json::Value,
    what: &str,
) -> serde_json::Value {
    let job = engine.call(op, params);
    wait_for(
        events,
        Duration::from_secs(600),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => Some(result.clone()),
            Event::JobFailed {
                job: done, message, ..
            } if *done == job => panic!("{op} failed: {message}"),
            _ => None,
        },
        what,
    )
}

fn call_err(
    engine: &EngineHandle,
    events: &std::sync::mpsc::Receiver<Event>,
    op: &str,
    params: serde_json::Value,
    what: &str,
) -> String {
    let job = engine.call(op, params);
    wait_for(
        events,
        Duration::from_secs(600),
        |e| match e {
            Event::JobFailed {
                job: done, message, ..
            } if *done == job => Some(message.clone()),
            Event::JobDone { job: done, .. } if *done == job => {
                panic!("{op} unexpectedly succeeded while waiting for {what}")
            }
            _ => None,
        },
        what,
    )
}

/// Camera/sun as the bridge expects them (the worker's own arguments).
fn view_args(camera: CameraParams, sun: SunParams) -> (serde_json::Value, serde_json::Value) {
    (
        serde_json::json!({
            "phi_deg": camera.phi_deg,
            "theta_deg": camera.theta_deg,
            "radius": camera.radius,
            "fov_deg": camera.fov_deg,
            "target": [camera.target_x, 0.0, camera.target_z],
        }),
        serde_json::json!({
            "azimuth_deg": sun.azimuth_deg,
            "elevation_deg": sun.elevation_deg,
            "intensity": sun.intensity,
        }),
    )
}

fn load_dem(
    engine: &EngineHandle,
    events: &std::sync::mpsc::Receiver<Event>,
) -> (CameraParams, SunParams) {
    let dem = mini_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    engine.send(Command::LoadTerrain { path: dem });
    wait_for(
        events,
        Duration::from_secs(300),
        |e| match e {
            Event::TerrainLoaded { camera, sun, .. } => Some((*camera, *sun)),
            _ => None,
        },
        "TerrainLoaded",
    )
}

/// One `RenderFrame` round-trip, collecting the progress the worker reported.
struct Frame {
    rgba: Vec<u8>,
    quality: Option<serde_json::Value>,
    progress: Vec<(Option<f32>, String)>,
}

fn render(
    engine: &EngineHandle,
    events: &std::sync::mpsc::Receiver<Event>,
    width: u32,
    height: u32,
) -> Frame {
    engine.send(Command::RenderFrame {
        width,
        height,
        force: true,
    });
    let mut progress = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("quality render timed out");
        match events.recv_timeout(remaining) {
            Ok(Event::Progress { fraction, label }) => progress.push((fraction, label)),
            Ok(Event::FrameReady {
                width: ready_width,
                height: ready_height,
                rgba,
                quality,
                ..
            }) => {
                assert_eq!((ready_width, ready_height), (width, height));
                assert_eq!(rgba.len(), (width * height * 4) as usize);
                return Frame {
                    rgba,
                    quality,
                    progress,
                };
            }
            Ok(Event::Error { context, message }) => {
                panic!("render failed: {context}: {message}")
            }
            Ok(_) => {}
            Err(err) => panic!("engine channel closed during a render: {err}"),
        }
    }
}

fn offline(samples: u32, denoiser: &str) -> RenderQuality {
    RenderQuality {
        mode: RenderMode::Raster,
        offline: OfflineSettings {
            enabled: true,
            samples,
            denoiser: denoiser.to_owned(),
        },
        path_trace: PathTraceSettings::default(),
    }
}

fn path_traced(
    seed: u32,
    max_frames: u32,
    variance_threshold: f32,
    colormap_albedo: bool,
) -> RenderQuality {
    RenderQuality {
        mode: RenderMode::PathTraced,
        offline: OfflineSettings::default(),
        path_trace: PathTraceSettings {
            spp: 1,
            min_frames: 2,
            max_frames,
            variance_threshold,
            tile: 64,
            seed,
            env_intensity: 0.35,
            colormap_albedo,
        },
    }
}

#[test]
fn offline_capabilities_and_denoiser_validation_are_honest() {
    let (engine, events) = spawn_ready();
    let caps = call_ok(
        &engine,
        &events,
        "offline.capabilities",
        serde_json::json!({}),
        "offline.capabilities",
    );
    let denoisers: Vec<String> = caps
        .get("denoisers")
        .and_then(|value| value.as_array())
        .expect("capabilities must list denoisers")
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    assert!(denoisers.contains(&"none".to_owned()));
    assert!(denoisers.contains(&"atrous".to_owned()));
    assert_eq!(
        caps.get("offline_accumulation").and_then(|v| v.as_bool()),
        Some(true),
        "TV12 accumulation is part of the engine surface Studio targets"
    );

    let (camera, sun) = load_dem(&engine, &events);
    let (camera, sun) = view_args(camera, sun);
    let render_args = |denoiser: &str| {
        serde_json::json!({
            "width": 64,
            "height": 40,
            "camera": &camera,
            "sun": &sun,
            "out_png": scratch_dir().join("capability-check.png").to_string_lossy(),
            "offline": { "samples": 1, "denoiser": denoiser },
        })
    };

    // An unknown denoiser is refused with the set Studio actually offers.
    let message = call_err(
        &engine,
        &events,
        "terrain.render_offline",
        render_args("bogus"),
        "unknown denoiser",
    );
    assert!(
        message.contains("bogus") && message.contains("none"),
        "unknown denoiser must name both the input and the offered set: {message}"
    );

    // A denoiser the environment cannot run is refused, not downgraded.
    if !denoisers.contains(&"oidn".to_owned()) {
        let message = call_err(
            &engine,
            &events,
            "terrain.render_offline",
            render_args("oidn"),
            "unavailable OIDN",
        );
        assert!(
            message.to_lowercase().contains("oidn"),
            "unavailable OIDN must say so: {message}"
        );
        assert!(
            message.to_lowercase().contains("trous"),
            "the refusal must name the denoiser that does work: {message}"
        );
    }

    engine.shutdown();
}

#[test]
fn offline_accumulation_reports_samples_and_repeats_byte_identically() {
    let (engine, events) = spawn_ready();
    let _view = load_dem(&engine, &events);

    // The pre-M6 frame: one pass, no metadata to report.
    let one_shot = render(&engine, &events, 128, 80);
    assert!(
        one_shot.quality.is_none(),
        "a one-shot raster frame must not claim a quality path: {:?}",
        one_shot.quality
    );
    assert!(
        one_shot.progress.is_empty(),
        "the one-shot frame renders in one pass and reports no progress"
    );

    engine.send(Command::SetRenderQuality(offline(2, "none")));
    let two_samples = render(&engine, &events, 128, 80);
    let meta = two_samples
        .quality
        .as_ref()
        .expect("offline accumulation must report its quality metadata");
    assert_eq!(
        meta.get("mode").and_then(|v| v.as_str()),
        Some("raster_offline")
    );
    assert_eq!(meta.get("samples_used").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(meta.get("target_samples").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(
        meta.get("denoiser_used").and_then(|v| v.as_str()),
        Some("none")
    );
    // A four-pixel-square sample count cannot converge by variance; the
    // engine says so instead of claiming convergence.
    assert_eq!(
        meta.get("adaptive").and_then(|v| v.as_bool()),
        Some(false),
        "Studio asks for a fixed sample count, not adaptive sampling"
    );
    assert!(
        !two_samples.progress.is_empty(),
        "accumulation must stream at least one progress report"
    );
    assert!(
        two_samples
            .progress
            .iter()
            .all(|(fraction, _)| fraction.is_some()),
        "TV12 accumulation reports a measurable fraction: {:?}",
        two_samples.progress
    );

    let repeat = render(&engine, &events, 128, 80);
    assert!(
        two_samples.rgba == repeat.rgba,
        "two identical accumulation renders must be byte-identical"
    );
    assert!(
        one_shot.rgba != two_samples.rgba,
        "two accumulated samples must differ from the one-shot frame"
    );

    // Fewer samples is real provenance, even when the pixels agree: the DEM
    // render is smooth enough that subpixel jitter at this size rounds to the
    // same 8-bit values (measured: 1, 2 and 4 samples are pixel-identical
    // here). The engine's own count is therefore what this asserts.
    engine.send(Command::SetRenderQuality(offline(1, "none")));
    let one_sample = render(&engine, &events, 128, 80);
    let meta = one_sample.quality.as_ref().expect("1-sample metadata");
    assert_eq!(meta.get("samples_used").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(meta.get("target_samples").and_then(|v| v.as_u64()), Some(1));

    // The denoiser runs on the guidance AOVs and is reported by the engine.
    engine.send(Command::SetRenderQuality(offline(2, "atrous")));
    let denoised = render(&engine, &events, 128, 80);
    let meta = denoised.quality.as_ref().expect("denoised metadata");
    assert_eq!(
        meta.get("denoiser_used").and_then(|v| v.as_str()),
        Some("atrous"),
        "the engine must report the denoiser it ran: {meta:?}"
    );
    assert!(
        denoised.rgba != two_samples.rgba,
        "A-trous denoising must change the accumulated frame"
    );

    // Exports carry the same provenance.
    let out = scratch_dir().join("offline-export.png");
    let _ = std::fs::remove_file(&out);
    engine.send(Command::SetRenderQuality(offline(1, "none")));
    engine.send(Command::RenderToFile {
        width: 96,
        height: 64,
        path: out.clone(),
    });
    let quality = wait_for(
        &events,
        Duration::from_secs(600),
        |e| match e {
            Event::ExportFinished { quality, .. } => Some(quality.clone()),
            Event::ExportFailed { message, .. } => panic!("export failed: {message}"),
            _ => None,
        },
        "ExportFinished",
    );
    let meta = quality.expect("an offline export must report its quality metadata");
    assert_eq!(
        meta.get("mode").and_then(|v| v.as_str()),
        Some("raster_offline")
    );
    let bytes = std::fs::read(&out).expect("export must write a PNG");
    assert_eq!(&bytes[0..4], b"\x89PNG");
    std::fs::remove_file(&out).ok();

    engine.shutdown();
}

#[test]
fn path_trace_reports_convergence_and_repeats_byte_identically() {
    let (engine, events) = spawn_ready();
    let _view = load_dem(&engine, &events);
    engine.send(Command::SetCertificate(true));

    engine.send(Command::SetRenderQuality(path_traced(7, 4, 0.5, true)));
    let first = render(&engine, &events, 96, 64);
    let meta = first
        .quality
        .as_ref()
        .expect("path tracing must report its quality metadata");
    assert_eq!(
        meta.get("mode").and_then(|v| v.as_str()),
        Some("path_traced")
    );
    assert_eq!(
        meta.get("converged").and_then(|v| v.as_bool()),
        Some(true),
        "the poster raises rather than returning an unconverged plate: {meta:?}"
    );
    assert_eq!(
        meta.get("tiles").and_then(|v| v.as_u64()),
        Some(2),
        "96x64 at a 64 px tile edge is two full-sensor tiles: {meta:?}"
    );
    assert_eq!(meta.get("tile_size").and_then(|v| v.as_u64()), Some(64));
    let frames = meta.get("frames").and_then(|v| v.as_u64()).expect("frames");
    assert!(
        (2..=4).contains(&frames),
        "frames must respect min_frames..=max_frames: {frames}"
    );
    assert_eq!(
        meta.get("samples_used").and_then(|v| v.as_u64()),
        Some(frames)
    );
    let albedo = meta.get("albedo").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        albedo.contains("colormap"),
        "the 256² DEM is under the albedo cap, so the ramp is sampled: {albedo}"
    );
    assert!(
        first
            .progress
            .iter()
            .any(|(fraction, label)| fraction.is_none() && label.starts_with("path tracing")),
        "the poster reports indeterminate progress, not a fabricated fraction: {:?}",
        first.progress
    );

    // A certificate rides along with the still.
    let certificate_meta = meta
        .get("certificate_path")
        .and_then(|v| v.as_str())
        .expect("SetCertificate must produce a certificate path for a PT still");
    assert!(
        std::path::Path::new(certificate_meta).is_file(),
        "the certificate file must exist: {certificate_meta}"
    );
    let digest = meta
        .get("certificate_payload_sha256")
        .and_then(|v| v.as_str())
        .expect("the poster reports its certificate digest")
        .to_owned();
    assert_eq!(digest.len(), 64, "the digest is a hex SHA256: {digest}");
    // The written certificate is the same payload, signed, and verifies
    // against the dev key the engine signs in-process renders with.
    let verified = call_ok(
        &engine,
        &events,
        "certificate.verify",
        serde_json::json!({ "path": certificate_meta }),
        "certificate.verify",
    );
    assert_eq!(
        verified.get("valid").and_then(|v| v.as_bool()),
        Some(true),
        "the PT still's certificate must verify: {verified:?}"
    );
    assert_eq!(
        verified.get("payload_sha256").and_then(|v| v.as_str()),
        Some(digest.as_str()),
        "the reported digest must be the digest of the written certificate"
    );

    let repeat = render(&engine, &events, 96, 64);
    assert!(
        first.rgba == repeat.rgba,
        "the same seed/settings must repeat byte-identically"
    );
    // The payload digest is NOT asserted equal across the two renders: the
    // engine's ledger records the process-lifetime `hybrid-dummy-storage`
    // buffer on first use, so the allocation section (and therefore the
    // digest) legitimately differs between the first and later stills. The
    // pixels, the sample count and the signature are what determinism means
    // here, and the pixels are identical.
    assert_eq!(
        repeat
            .quality
            .as_ref()
            .and_then(|meta| meta.get("frames"))
            .and_then(|v| v.as_u64()),
        Some(frames),
        "a repeated still must take the same number of frames"
    );

    // A different seed is a different sample sequence, not a relabelled frame.
    engine.send(Command::SetRenderQuality(path_traced(99, 4, 0.5, true)));
    let reseeded = render(&engine, &events, 96, 64);
    assert!(
        first.rgba != reseeded.rgba,
        "changing the seed must change the sampled plate"
    );

    engine.shutdown();
}
