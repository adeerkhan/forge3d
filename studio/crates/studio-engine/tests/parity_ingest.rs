//! Part IV parity harness for the M3 (unified import) and W1-depth
//! (presets / camera rigs / sequences) rings.
//!
//! Like `parity_mapscene.rs`, these tests need the embedded Python
//! environment (forge3d + numpy + rasterio + the venv) and a GPU adapter.
//! They assert the bridge ops reproduce what the engine's own Python API
//! reports for the same inputs — the GUI.md "examples are the specification"
//! rule applied to ingest metadata rather than rendered pixels.
//!
//! Non-terrain formats (point clouds, tilesets, meshes, vectors) must report
//! honest metadata and `terrain_loadable: false`; only DEM-like sources
//! (GeoTIFF / COG / NPY / F3DZ) may claim the terrain slot.

use std::path::PathBuf;
use std::process::Command as ProcCommand;
use std::time::{Duration, Instant};

use studio_engine::{Command, EngineHandle, Event};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Fixtures live in a repo-local directory (created fresh each run) rather
/// than the system temp dir: this machine's tmpfiles cleaner can reclaim
/// `/tmp` entries mid-test and leave the engine reading an empty path.
fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ingest-fixtures");
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn venv_python() -> PathBuf {
    repo_root().join(".venv/bin/python3")
}

fn mini_dem() -> PathBuf {
    repo_root().join("python/forge3d/data/mini_dem.npy")
}

fn fuji_dem() -> PathBuf {
    repo_root().join("assets/tif/Mount_Fuji_30m.tif")
}

fn copc_fixture() -> PathBuf {
    repo_root().join("tests/data/copc/hierarchy-pages.copc.laz")
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

/// Call a bridge op and return its `result` object (panicking on failure).
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
        Duration::from_secs(300),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => Some(result.clone()),
            Event::JobFailed {
                job: done, message, ..
            } if *done == job => {
                panic!("{op} failed: {message}")
            }
            _ => None,
        },
        what,
    )
}

/// Compress the bundled mini DEM into a real F3DZ file using the engine's own
/// codec (the same venv the bridge embeds), so the fixture is genuinely a
/// product of `forge3d.codec.compress_dem`.
fn make_f3dz_fixture() -> PathBuf {
    let out = fixture_dir().join("studio-parity-mini.f3dz");
    let _ = std::fs::remove_file(&out);
    let src = mini_dem();
    let script = format!(
        "import numpy as np, forge3d.codec as c, sys\n\
         arr = np.load({src:?})\n\
         data = c.compress_dem(arr, eps=0.01)\n\
         open({out:?}, 'wb').write(data)\n",
        src = src.to_string_lossy(),
        out = out.to_string_lossy(),
    );
    let status = ProcCommand::new(venv_python())
        .arg("-c")
        .arg(script)
        .status()
        .expect("run venv python to build f3dz fixture");
    assert!(status.success(), "f3dz fixture generation failed");
    assert!(out.is_file(), "f3dz fixture was not written");
    out
}

/// Write a minimal but structurally valid GeoJSON LineString collection.
fn make_geojson_fixture() -> PathBuf {
    let out = fixture_dir().join("studio-parity-lines.geojson");
    let gj = serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {"name": "parity"},
            "geometry": {"type": "LineString", "coordinates": [[0.0, 0.0], [10.0, 10.0]]}
        }]
    });
    std::fs::write(&out, gj.to_string()).expect("write geojson fixture");
    out
}

/// Write a minimal single-triangle OBJ.
fn make_obj_fixture() -> PathBuf {
    let out = fixture_dir().join("studio-parity-tri.obj");
    std::fs::write(&out, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("write obj fixture");
    out
}

/// Write a minimal 3D Tiles tileset (one root tile, sphere bound).
fn make_tileset_fixture() -> PathBuf {
    let out = fixture_dir().join("studio-parity-tileset.json");
    let ts = serde_json::json!({
        "asset": {"version": "1.0"},
        "geometricError": 100.0,
        "root": {
            "boundingVolume": {"sphere": [0.0, 0.0, 0.0, 50.0]},
            "geometricError": 50.0,
            "refine": "ADD",
        }
    });
    std::fs::write(&out, ts.to_string()).expect("write tileset fixture");
    out
}

/// DEM-like formats (GeoTIFF, NPY, F3DZ) must preview metadata AND claim the
/// terrain slot; point clouds / tilesets / meshes / vectors must preview
/// honest metadata and never claim it.
#[test]
fn dataset_inspect_reports_honest_per_format_metadata() {
    let (engine, events) = spawn_ready();

    // GeoTIFF (real Mount Fuji raster).
    let tif = fuji_dem();
    assert!(tif.is_file(), "GeoTIFF fixture missing: {}", tif.display());
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": tif.to_string_lossy() }),
        "inspect geotiff",
    );
    assert_eq!(r.get("format").and_then(|v| v.as_str()), Some("geotiff"));
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("raster"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(true),
        "a GeoTIFF DEM must claim the terrain slot"
    );
    let meta = r.get("metadata").expect("geotiff metadata");
    assert_eq!(meta.get("width").and_then(|v| v.as_u64()), Some(2594));
    assert_eq!(meta.get("height").and_then(|v| v.as_u64()), Some(2594));
    assert_eq!(meta.get("band_count").and_then(|v| v.as_u64()), Some(1));

    // NPY (raw height grid).
    let npy = mini_dem();
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": npy.to_string_lossy() }),
        "inspect npy",
    );
    assert_eq!(r.get("format").and_then(|v| v.as_str()), Some("npy"));
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("terrain"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(true)
    );
    let shape = r
        .pointer("/metadata/shape")
        .and_then(|s| s.as_array())
        .expect("npy shape array");
    assert_eq!(shape.len(), 2, "2D height grid expected");

    // F3DZ (deterministic DEM compression).
    let f3dz = make_f3dz_fixture();
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": f3dz.to_string_lossy() }),
        "inspect f3dz",
    );
    assert_eq!(r.get("format").and_then(|v| v.as_str()), Some("f3dz"));
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("terrain"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(true)
    );
    let domain = r
        .pointer("/metadata/domain")
        .and_then(|d| d.as_array())
        .expect("f3dz domain");
    assert_eq!(domain.len(), 2);

    // COPC point cloud: honest metadata, terrain slot refused.
    let copc = copc_fixture();
    if copc.is_file() {
        let r = call_ok(
            &engine,
            &events,
            "dataset.inspect",
            serde_json::json!({ "path": copc.to_string_lossy() }),
            "inspect copc",
        );
        assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("pointcloud"));
        assert_eq!(
            r.get("terrain_loadable").and_then(|v| v.as_bool()),
            Some(false),
            "point clouds must not claim the terrain slot"
        );
        let pts = r
            .pointer("/metadata/total_points")
            .and_then(|v| v.as_u64())
            .expect("total_points");
        assert!(pts > 0, "COPC fixture must report points");
        let bounds = r.get("metadata").and_then(|m| m.get("bounds"));
        assert!(
            bounds.is_some() && !bounds.unwrap().is_null(),
            "bounds reported"
        );
    }

    // 3D Tiles tileset: honest metadata, terrain slot refused.
    let tileset = make_tileset_fixture();
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": tileset.to_string_lossy() }),
        "inspect tileset",
    );
    assert_eq!(r.get("format").and_then(|v| v.as_str()), Some("3dtiles"));
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("tileset"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        r.pointer("/metadata/tile_count").and_then(|v| v.as_u64()),
        Some(1)
    );

    // OBJ mesh: honest metadata, terrain slot refused.
    let obj = make_obj_fixture();
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": obj.to_string_lossy() }),
        "inspect obj",
    );
    assert_eq!(r.get("format").and_then(|v| v.as_str()), Some("obj"));
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("mesh"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        r.pointer("/metadata/triangle_count")
            .and_then(|v| v.as_u64()),
        Some(1)
    );

    // GeoJSON vector: honest metadata, terrain slot refused.
    let gj = make_geojson_fixture();
    let raw = std::fs::read_to_string(&gj).expect("read back geojson fixture");
    let _ = std::fs::write(fixture_dir().join("geojson-dump.json"), &raw);
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": gj.to_string_lossy() }),
        "inspect geojson",
    );
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("vector"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        r.pointer("/metadata/feature_count")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        r.pointer("/metadata/geometry_type")
            .and_then(|v| v.as_str()),
        Some("LineString")
    );

    // STL is honestly reported as unsupported (export-only in the engine).
    let stl = fixture_dir().join("studio-parity-unsupported.stl");
    std::fs::write(&stl, b"solid empty\nendsolid empty\n").expect("write stl stub");
    let r = call_ok(
        &engine,
        &events,
        "dataset.inspect",
        serde_json::json!({ "path": stl.to_string_lossy() }),
        "inspect stl",
    );
    assert_eq!(r.get("format").and_then(|v| v.as_str()), Some("stl"));
    assert_eq!(
        r.get("terrain_loadable").and_then(|v| v.as_bool()),
        Some(false)
    );
    let notes = r
        .get("notes")
        .and_then(|n| n.as_array())
        .expect("stl notes");
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .map(|s| s.contains("unsupported"))
            .unwrap_or(false)),
        "STL must be flagged unsupported, got {notes:?}"
    );

    engine.shutdown();
}

/// `terrain.load` must accept NPY and F3DZ directly (the two DEM-like formats
/// that are not GeoTIFF), reporting a sane size and domain.
#[test]
fn terrain_load_accepts_npy_and_f3dz() {
    let (engine, events) = spawn_ready();

    // NPY.
    let npy = mini_dem();
    let r = call_ok(
        &engine,
        &events,
        "terrain.load",
        serde_json::json!({ "path": npy.to_string_lossy() }),
        "terrain.load npy",
    );
    assert_eq!(r.get("width").and_then(|v| v.as_u64()), Some(256));
    assert_eq!(r.get("height").and_then(|v| v.as_u64()), Some(256));
    let domain = r
        .get("domain")
        .and_then(|d| d.as_array())
        .expect("domain array");
    assert_eq!(domain.len(), 2);

    // F3DZ decompresses into the same slot with matching dimensions.
    let f3dz = make_f3dz_fixture();
    let r = call_ok(
        &engine,
        &events,
        "terrain.load",
        serde_json::json!({ "path": f3dz.to_string_lossy() }),
        "terrain.load f3dz",
    );
    assert_eq!(
        r.get("width").and_then(|v| v.as_u64()),
        Some(256),
        "f3dz must decompress to the source width"
    );
    assert_eq!(r.get("height").and_then(|v| v.as_u64()), Some(256));

    engine.shutdown();
}

/// Rig baking must produce renderer-unit keyframes over a loaded DEM: a
/// full-length orbit sweeps phi monotonically, keeps a positive radius, and
/// reports the unit-scale it applied.
#[test]
fn rig_bake_orbit_yields_monotonic_renderer_keyframes() {
    let (engine, events) = spawn_ready();

    let dem = fuji_dem();
    engine.send(Command::LoadTerrain { path: dem });
    wait_for(
        &events,
        Duration::from_secs(300),
        |e| matches!(e, Event::TerrainLoaded { .. }).then_some(()),
        "TerrainLoaded",
    );

    let r = call_ok(
        &engine,
        &events,
        "rig.bake",
        serde_json::json!({ "kind": "orbit", "samples_per_second": 12 }),
        "rig.bake orbit",
    );
    assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("orbit"));
    let duration = r
        .get("duration")
        .and_then(|v| v.as_f64())
        .expect("duration");
    assert!(duration > 0.0, "orbit duration must be positive");
    let scale = r
        .get("unit_scale")
        .and_then(|v| v.as_f64())
        .expect("unit_scale");
    assert!(scale > 0.0, "unit_scale must be positive");

    let kfs = r
        .get("keyframes")
        .and_then(|k| k.as_array())
        .expect("keyframes array");
    assert!(!kfs.is_empty(), "orbit must bake at least one keyframe");
    let expected = ((duration * 12.0).round() as usize).max(1);
    assert!(
        (kfs.len() as i64 - expected as i64).abs() <= 1,
        "keyframe count {} should match duration*sps {}",
        kfs.len(),
        expected
    );

    // Phi sweeps 0..360 for a full orbit; radii stay positive.
    let mut prev_phi = f64::NEG_INFINITY;
    for (i, kf) in kfs.iter().enumerate() {
        let phi = kf
            .get("phi_deg")
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| panic!("keyframe {i} phi"));
        let radius = kf
            .get("radius")
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| panic!("keyframe {i} radius"));
        assert!(radius > 0.0, "keyframe {i} radius must be positive");
        if i > 0 {
            assert!(
                phi > prev_phi,
                "orbit phi must increase monotonically: {phi} after {prev_phi}"
            );
        }
        prev_phi = phi;
    }
    let last_phi = kfs
        .last()
        .and_then(|k| k.get("phi_deg"))
        .and_then(|v| v.as_f64())
        .expect("last phi");
    assert!(
        last_phi <= 361.0,
        "orbit should complete at most one revolution, got {last_phi}"
    );

    engine.shutdown();
}

/// `presets.get` must return the same presets `presets.list` advertises, each
/// carrying a directional light Studio can map onto the sun.
#[test]
fn presets_get_matches_list_and_carries_directional_light() {
    let (engine, events) = spawn_ready();

    let listed = call_ok(
        &engine,
        &events,
        "presets.list",
        serde_json::json!({}),
        "presets.list",
    );
    let names: Vec<String> = listed
        .get("presets")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .expect("presets array");
    assert!(names.len() >= 5, "engine ships five renderer presets");

    for name in &names {
        let r = call_ok(
            &engine,
            &events,
            "presets.get",
            serde_json::json!({ "name": name }),
            &format!("presets.get({name})"),
        );
        assert_eq!(
            r.get("name").and_then(|v| v.as_str()),
            Some(name.as_str()),
            "presets.get must echo the requested name"
        );
        let preset = r.get("preset").expect("preset payload");
        let light = preset
            .pointer("/lighting/lights/0")
            .unwrap_or_else(|| panic!("preset {name} must carry a first light"));
        let dir = light
            .get("direction")
            .and_then(|d| d.as_array())
            .unwrap_or_else(|| panic!("preset {name} light direction"));
        assert_eq!(dir.len(), 3, "preset {name} light direction must be 3D");
        assert!(
            dir.iter().all(|v| v.as_f64().is_some()),
            "preset {name} direction components must be numeric"
        );
    }

    engine.shutdown();
}
