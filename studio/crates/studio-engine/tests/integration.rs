//! End-to-end integration tests driving the real forge3d renderer through
//! the embedded-Python bridge.
//!
//! These require a working GPU adapter and the Python environment documented
//! in studio/README.md (forge3d + numpy importable). When either is missing
//! the tests fail loudly — Studio cannot function without them, so silently
//! skipping would hide the integration.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use studio_engine::{Command, EngineHandle, Event};

fn fixture_dem() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets/tif/Mount_Fuji_30m.tif")
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

#[test]
fn engine_loads_dem_and_renders_frames() {
    let _ = env_logger::try_init();
    let (engine, events) = EngineHandle::spawn();

    let ready = wait_for(
        &events,
        Duration::from_secs(300),
        |e| match e {
            Event::Ready {
                adapter,
                backend,
                software_fallback,
            } => Some((adapter.clone(), backend.clone(), *software_fallback)),
            _ => None,
        },
        "GPU Ready",
    );
    println!("adapter: {ready:?}");

    let dem = fixture_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    engine.send(Command::LoadTerrain { path: dem.clone() });

    let loaded = wait_for(
        &events,
        Duration::from_secs(300),
        |e| match e {
            Event::TerrainLoaded { camera, sun, .. } => Some((*camera, *sun)),
            _ => None,
        },
        "TerrainLoaded",
    );
    assert!(loaded.0.radius > 0.0);
    assert!(loaded.1.elevation_deg > 0.0);

    engine.send(Command::RenderFrame { width: 320, height: 240 });
    let (w, h, rgba) = wait_for(
        &events,
        Duration::from_secs(300),
        |e| match e {
            Event::FrameReady {
                width, height, rgba, ..
            } => Some((*width, *height, rgba.clone())),
            _ => None,
        },
        "FrameReady",
    );
    assert_eq!((w, h), (320, 240));
    assert_eq!(rgba.len(), 320 * 240 * 4);
    // A real terrain render must produce pixel variance, not a flat frame.
    let mut min = u8::MAX;
    let mut max = 0u8;
    for &b in &rgba {
        min = min.min(b);
        max = max.max(b);
    }
    assert!(max > min, "rendered frame is flat (min={min}, max={max})");

    // Export through the real pipeline.
    let out = std::env::temp_dir().join("studio-engine-export-test.png");
    let _ = std::fs::remove_file(&out);
    engine.send(Command::RenderToFile {
        width: 480,
        height: 360,
        path: out.clone(),
    });
    wait_for(
        &events,
        Duration::from_secs(300),
        |e| match e {
            Event::ExportFinished { .. } => Some(()),
            _ => None,
        },
        "ExportFinished",
    );
    let bytes = std::fs::read(&out).expect("export file must exist");
    assert_eq!(&bytes[0..4], b"\x89PNG", "export must be a real PNG");
    std::fs::remove_file(&out).ok();

    engine.shutdown();
}
