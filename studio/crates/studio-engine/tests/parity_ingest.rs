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

/// Call a bridge op and return the engine's own error message (panicking if
/// the op unexpectedly succeeds).
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
        Duration::from_secs(300),
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

// ------------------------------------------------ M4: TerrainRenderParams

/// How one `examples/terrain_demo.py` flag maps onto Studio's
/// `TerrainRenderParams` surface.
#[derive(Debug, Clone, Copy)]
enum Coverage {
    /// Exposed as a root-level property-page field.
    Param(&'static str),
    /// Exposed as a field of the named settings section.
    Section(&'static str, &'static str),
    /// An engine param Studio pins on purpose; the schema must report it in
    /// `excluded` with a reason rather than hiding it.
    Excluded(&'static str),
    /// Not a `TerrainRenderParams` input at all: dataset I/O, the viewer,
    /// renderer-config (`config.py`) knobs, or a post-process step.
    Demo,
}

/// Every flag of the canonical terrain demo, classified against the parameter
/// pages. A new demo flag fails the coverage test until it is classified here.
fn demo_flag_coverage() -> Vec<(&'static str, Coverage)> {
    use Coverage::{Demo, Excluded, Param, Section};
    vec![
        // --- root scalars ---------------------------------------------------
        ("albedo-mode", Param("albedo_mode")),
        ("colormap-strength", Param("colormap_strength")),
        ("debug-mode", Param("debug_mode")),
        ("exposure", Param("exposure")),
        ("height-curve-mode", Param("height_curve_mode")),
        ("height-curve-power", Param("height_curve_power")),
        ("height-curve-strength", Param("height_curve_strength")),
        ("ibl-intensity", Param("ibl_intensity")),
        ("lambert-contrast", Param("lambert_contrast")),
        ("msaa", Param("msaa_samples")),
        ("sun-color", Param("sun_color")),
        ("z-scale", Param("z_scale")),
        // --- nested settings sections ---------------------------------------
        ("cascades", Section("shadows", "cascades")),
        ("detail-fade-end", Section("detail", "fade_end")),
        ("detail-fade-start", Section("detail", "fade_start")),
        ("detail-normals", Section("detail", "detail_normal_path")),
        ("detail-sigma-px", Section("detail", "detail_sigma_px")),
        ("detail-strength", Section("detail", "detail_strength")),
        ("fog-density", Section("fog", "density")),
        ("fog-height-falloff", Section("fog", "height_falloff")),
        ("fog-inscatter", Section("fog", "inscatter")),
        ("normal-strength", Section("triplanar", "normal_strength")),
        (
            "pcss-blocker-radius",
            Section("shadows", "pcss_blocker_radius"),
        ),
        (
            "pcss-filter-radius",
            Section("shadows", "pcss_filter_radius"),
        ),
        ("pom-disabled", Section("pom", "enabled")),
        ("shadow-light-size", Section("shadows", "light_size")),
        ("shadow-map-res", Section("shadows", "resolution")),
        ("shadow-moment-bias", Section("shadows", "depth_bias")),
        ("shadow-technique", Section("shadows", "technique")),
        ("shadows", Section("shadows", "technique")),
        // --- engine params the viewport pipeline or another panel owns ------
        ("cam-fov", Excluded("fov_y_deg")),
        ("cam-phi", Excluded("cam_phi_deg")),
        ("cam-radius", Excluded("cam_radius")),
        ("cam-theta", Excluded("cam_theta_deg")),
        ("camera-mode", Excluded("camera_mode")),
        ("colormap", Excluded("overlays")),
        ("height-curve-lut", Excluded("height_curve_lut")),
        ("render-scale", Excluded("render_scale")),
        ("size", Excluded("size_px")),
        ("sun-azimuth", Excluded("light_azimuth_deg")),
        ("sun-elevation", Excluded("light_elevation_deg")),
        ("sun-intensity", Excluded("sun_intensity")),
        // --- not TerrainRenderParams inputs --------------------------------
        // Note: --colormap-srgb / --output-srgb-eotf are TerrainRenderParams
        // fields that `make_terrain_params_config` does not accept (the demo
        // sets them on the config after construction). They stay a documented
        // coverage gap rather than a silently missing page.
        ("brdf", Demo),
        ("colormap-domain", Demo),
        ("colormap-interpolate", Demo),
        ("colormap-size", Demo),
        ("colormap-srgb", Demo),
        ("debug-lights", Demo),
        ("dem", Demo),
        ("gi", Demo),
        ("hdr", Demo),
        ("ibl-cache", Demo),
        ("ibl-res", Demo),
        ("light", Demo),
        ("load-bundle", Demo),
        ("oit", Demo),
        ("output", Demo),
        ("output-srgb-eotf", Demo),
        ("overwrite", Demo),
        ("preset", Demo),
        ("render", Demo),
        ("save-bundle", Demo),
        ("sky", Demo),
        ("sun-datetime", Demo),
        ("sun-lat", Demo),
        ("sun-lon", Demo),
        ("taa", Demo),
        ("taa-history-weight", Demo),
        ("unsharp-strength", Demo),
        ("viewer", Demo),
        ("volumetric", Demo),
        ("window", Demo),
    ]
}

/// The demo's `add_argument("--…")` surface, read from its source. Matches the
/// bridge op's ground truth rather than a hand-maintained flag list.
fn demo_cli_flags() -> Vec<String> {
    let path = repo_root().join("examples/terrain_demo.py");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    let mut flags = Vec::new();
    let mut rest = source.as_str();
    let marker = "add_argument(";
    while let Some(index) = rest.find(marker) {
        rest = &rest[index + marker.len()..];
        let trimmed = rest.trim_start();
        let Some(literal) = trimmed.strip_prefix("\"--") else {
            continue;
        };
        let name: String = literal
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !name.is_empty() {
            flags.push(name);
        }
    }
    flags.sort();
    flags.dedup();
    flags
}

fn schema_section<'a>(schema: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    schema
        .get("sections")
        .and_then(|sections| sections.as_array())
        .and_then(|sections| {
            sections
                .iter()
                .find(|section| section.get("key").and_then(|k| k.as_str()) == Some(key))
        })
}

fn section_field_keys(schema: &serde_json::Value, key: &str) -> Vec<String> {
    schema_section(schema, key)
        .and_then(|section| section.get("fields"))
        .and_then(|fields| fields.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("key").and_then(|k| k.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_else(|| panic!("schema has no section '{key}'"))
}

fn excluded_keys(schema: &serde_json::Value) -> Vec<String> {
    schema
        .get("excluded")
        .and_then(|excluded| excluded.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("key").and_then(|k| k.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The property pages must cover the canonical example's parameter surface:
/// every demo flag either lands on a page field, on a section field, or is
/// reported as excluded with a reason. Nothing may be silently missing.
#[test]
fn terrain_params_schema_covers_the_engine_demo_surface() {
    let (engine, events) = spawn_ready();
    let schema = call_ok(
        &engine,
        &events,
        "terrain.params_schema",
        serde_json::json!({}),
        "params schema",
    );

    // Every exclusion is honest: a key and a reason, never a bare omission.
    let excluded = excluded_keys(&schema);
    for entry in schema
        .get("excluded")
        .and_then(|e| e.as_array())
        .expect("excluded list")
    {
        let reason = entry.get("reason").and_then(|r| r.as_str()).unwrap_or("");
        assert!(
            !reason.trim().is_empty(),
            "excluded param {:?} needs a reason the UI can show",
            entry.get("key")
        );
    }

    let coverage = demo_flag_coverage();
    let flags = demo_cli_flags();
    assert!(
        flags.len() > 50,
        "parsed only {} flags from examples/terrain_demo.py",
        flags.len()
    );

    for flag in &flags {
        let (_, target) = coverage
            .iter()
            .find(|(name, _)| *name == flag.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "--{flag} is unclassified: add it to demo_flag_coverage() as \
                     Param / Section / Excluded / Demo"
                )
            });
        match target {
            Coverage::Param(name) => assert!(
                section_field_keys(&schema, "root")
                    .iter()
                    .any(|k| k == name),
                "--{flag} maps to the root param {name}, which the schema omits"
            ),
            Coverage::Section(section, field) => assert!(
                section_field_keys(&schema, section)
                    .iter()
                    .any(|k| k == field),
                "--{flag} maps to {section}.{field}, which the schema omits"
            ),
            Coverage::Excluded(name) => assert!(
                excluded.iter().any(|k| k == name),
                "--{flag} maps to the pinned param {name}, which the schema \
                 must report as excluded"
            ),
            Coverage::Demo => {}
        }
    }

    // And the table must not outlive the flags it describes.
    for (name, _) in &coverage {
        assert!(
            flags.iter().any(|flag| flag.as_str() == *name),
            "coverage table lists --{name}, which examples/terrain_demo.py no longer defines"
        );
    }

    engine.shutdown();
}

/// An invalid value surfaces the engine's own message verbatim: the property
/// pages show `str(exc)` inline, so it must be the engine's first line and not
/// a Studio paraphrase.
#[test]
fn terrain_params_validate_reports_the_engines_own_errors() {
    let (engine, events) = spawn_ready();
    let cases: &[(&str, serde_json::Value, &str)] = &[
        (
            "msaa_samples range",
            serde_json::json!({ "msaa_samples": 3 }),
            "msaa_samples must be 1, 2, 4, or 8",
        ),
        (
            "z_scale range",
            serde_json::json!({ "z_scale": 100.0 }),
            "z_scale must be 0.1-50.0",
        ),
        (
            "albedo_mode enum",
            serde_json::json!({ "albedo_mode": "bogus" }),
            "Invalid albedo_mode: bogus",
        ),
        (
            "culling enum",
            serde_json::json!({ "culling": "sideways" }),
            "culling must be one of: none, frustum, hzb_two_phase",
        ),
        (
            "shading enum",
            serde_json::json!({ "shading": "sideways" }),
            "shading must be one of: forward, visibility",
        ),
        (
            "unknown param",
            serde_json::json!({ "not_a_param": 1 }),
            "unknown terrain param: not_a_param",
        ),
        (
            "unknown setting",
            serde_json::json!({ "fog": { "not_a_setting": 1 } }),
            "unknown fog setting: not_a_setting",
        ),
        (
            "wrong container",
            serde_json::json!({ "clip": ["a"] }),
            "clip must be an array of numbers",
        ),
    ];
    for (what, params, expected) in cases {
        let message = call_err(
            &engine,
            &events,
            "terrain.params_validate",
            serde_json::json!({ "params": params }),
            what,
        );
        let first = message.lines().next().unwrap_or_default();
        assert_eq!(
            first, *expected,
            "{what}: the inline message must be the engine's own text"
        );
    }

    // A partial edit (one root scalar, one section field) validates too.
    let ok = call_ok(
        &engine,
        &events,
        "terrain.params_validate",
        serde_json::json!({
            "params": { "exposure": 1.25, "shadows": { "technique": "PCF" } }
        }),
        "valid partial params",
    );
    assert_eq!(ok.get("valid").and_then(|v| v.as_bool()), Some(true));

    engine.shutdown();
}

/// Every default the pages display, and every enum choice they offer, must be
/// a value the engine accepts — otherwise the panel would ship values that
/// immediately fail validation.
#[test]
fn terrain_params_defaults_and_enum_choices_validate() {
    let (engine, events) = spawn_ready();
    let schema = call_ok(
        &engine,
        &events,
        "terrain.params_schema",
        serde_json::json!({}),
        "params schema",
    );
    let sections = schema
        .get("sections")
        .and_then(|sections| sections.as_array())
        .expect("schema sections");

    let mut materialized = serde_json::Map::new();
    // (section key, section defaults, field key, choices)
    let mut sweeps: Vec<(String, serde_json::Value, String, Vec<serde_json::Value>)> = Vec::new();
    for section in sections {
        let key = section.get("key").and_then(|k| k.as_str()).expect("key");
        if section.get("editable").and_then(|e| e.as_bool()) != Some(true) {
            let reason = section
                .get("reason")
                .and_then(|reason| reason.as_str())
                .unwrap_or("");
            assert!(
                !reason.trim().is_empty(),
                "section {key} is not editable and must say why"
            );
            continue;
        }
        let fields = section
            .get("fields")
            .and_then(|fields| fields.as_array())
            .expect("section fields");
        if key == "root" {
            for field in fields {
                let name = field.get("key").and_then(|k| k.as_str()).expect("key");
                if field.get("nullable").and_then(|n| n.as_bool()) != Some(true) {
                    materialized.insert(name.to_owned(), field["default"].clone());
                }
                if let Some(choices) = field.get("choices").and_then(|c| c.as_array()) {
                    sweeps.push((
                        key.to_owned(),
                        serde_json::json!({}),
                        name.to_owned(),
                        choices.clone(),
                    ));
                }
            }
            continue;
        }
        let defaults = section.get("defaults").cloned().unwrap_or_default();
        materialized.insert(key.to_owned(), defaults.clone());
        for field in fields {
            if let Some(choices) = field.get("choices").and_then(|c| c.as_array()) {
                sweeps.push((
                    key.to_owned(),
                    defaults.clone(),
                    field
                        .get("key")
                        .and_then(|k| k.as_str())
                        .expect("key")
                        .to_owned(),
                    choices.clone(),
                ));
            }
        }
    }

    let result = call_ok(
        &engine,
        &events,
        "terrain.params_validate",
        serde_json::json!({ "params": serde_json::Value::Object(materialized) }),
        "validate every page default",
    );
    assert_eq!(result.get("valid").and_then(|v| v.as_bool()), Some(true));

    assert!(
        sweeps.len() >= 8,
        "expected the engine's enum fields in the schema, found {}",
        sweeps.len()
    );
    for (section_key, defaults, field_key, choices) in sweeps {
        assert!(
            !choices.is_empty(),
            "{section_key}.{field_key} has no choices"
        );
        for choice in choices {
            let mut params = serde_json::Map::new();
            if section_key == "root" {
                params.insert(field_key.clone(), choice.clone());
            } else {
                let mut section = match defaults.as_object() {
                    Some(map) => map.clone(),
                    None => serde_json::Map::new(),
                };
                section.insert(field_key.clone(), choice.clone());
                if section_key == "shadows" {
                    // Moment techniques (VSM/EVSM/MSM) need more shadow maps
                    // than the factory default 4096²×3 cascades fit in the
                    // engine's 512 MiB budget; sweep them where they fit.
                    section.insert("resolution".to_owned(), serde_json::json!(1024));
                    section.insert("cascades".to_owned(), serde_json::json!(1));
                }
                params.insert(section_key.clone(), serde_json::Value::Object(section));
            }
            let outcome = call_ok(
                &engine,
                &events,
                "terrain.params_validate",
                serde_json::json!({ "params": serde_json::Value::Object(params) }),
                &format!("validate {section_key}.{field_key} = {choice}"),
            );
            assert_eq!(
                outcome.get("valid").and_then(|v| v.as_bool()),
                Some(true),
                "{section_key}.{field_key} = {choice} must validate"
            );
        }
    }

    engine.shutdown();
}
