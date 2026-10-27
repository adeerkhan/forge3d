//! Examples gallery contract: every `examples/*.py` is indexed, and every
//! Tier-A scene actually renders through the same bridge the app uses.
//!
//! This is the gate behind GUI.md W10 / III.7. It exists because a gallery that
//! cannot prove its entries work is worse than no gallery: it would teach users
//! that Studio's example cards are decorative. Tier A cards must produce a real
//! image here; Tier B/C must name the milestone that unlocks them.
//!
//! The renders are also the gallery's thumbnails (`studio/target/example-output`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use studio_engine::{EngineHandle, Event};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The same directory the app reads thumbnails from.
fn output_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/example-output");
    std::fs::create_dir_all(&dir).expect("create example output dir");
    dir
}

#[derive(Debug, serde::Deserialize)]
struct Index {
    entries: Vec<Entry>,
}

#[derive(Debug, serde::Deserialize)]
struct Entry {
    id: String,
    title: String,
    tier: String,
    example: String,
    description: String,
    #[serde(default)]
    unlock: Option<String>,
    #[serde(default)]
    scene: Option<serde_json::Value>,
}

fn index() -> Vec<Entry> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/index.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    let index: Index = serde_json::from_str(&text).expect("examples index is valid JSON");
    index.entries
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
                if let Some(value) = pred(&event) {
                    return value;
                }
                if let Event::Error { context, message } = &event {
                    panic!("engine error while waiting for {what}: {context}: {message}");
                }
            }
            Err(err) => panic!("engine channel closed while waiting for {what}: {err}"),
        }
    }
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
        |event| match event {
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

fn number(value: &serde_json::Value, key: &str, fallback: f64) -> f64 {
    value
        .get(key)
        .and_then(|number| number.as_f64())
        .unwrap_or(fallback)
}

/// A rendered image must be a real, non-flat picture.
fn assert_real_render(path: &std::path::Path, entry: &Entry) {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|err| panic!("{} did not write {}: {err}", entry.id, path.display()));
    assert_eq!(&bytes[0..4], b"\x89PNG", "{} must be a PNG", entry.id);
    assert!(
        bytes.len() > 20_000,
        "{} rendered only {} bytes — that is not an image",
        entry.id,
        bytes.len()
    );
    let image = image::open(path)
        .unwrap_or_else(|err| panic!("{} produced an undecodable PNG: {err}", entry.id))
        .to_rgba8();
    assert!(
        image.width() >= 320 && image.height() >= 200,
        "{} rendered at {}x{}",
        entry.id,
        image.width(),
        image.height()
    );
    let mut min = u8::MAX;
    let mut max = 0u8;
    for pixel in image.pixels() {
        for channel in pixel.0 {
            min = min.min(channel);
            max = max.max(channel);
        }
    }
    assert!(
        max as i32 - min as i32 > 24,
        "{} rendered a flat frame (min={min}, max={max})",
        entry.id
    );
}

/// The index must describe the repository honestly: real scripts, real scenes,
/// and an explicit unlock for everything that cannot run yet.
#[test]
fn gallery_index_is_honest() {
    let root = repo_root();
    let entries = index();
    assert!(
        entries.len() >= 50,
        "the index must cover the repository's examples (found {})",
        entries.len()
    );

    let mut ids = std::collections::HashSet::new();
    for entry in &entries {
        assert!(ids.insert(entry.id.clone()), "duplicate id {}", entry.id);
        assert!(
            root.join(&entry.example).is_file(),
            "{} names a missing script: {}",
            entry.id,
            entry.example
        );
        match entry.tier.as_str() {
            "A" => assert!(
                entry.scene.is_some(),
                "{} is Tier A but carries no scene",
                entry.id
            ),
            "B" | "C" => {
                let unlock = entry.unlock.as_deref().unwrap_or("");
                assert!(
                    !unlock.is_empty(),
                    "{} is Tier {} but does not say what unlocks it",
                    entry.id,
                    entry.tier
                );
            }
            other => panic!("{} has an unknown tier {other:?}", entry.id),
        }
        assert!(
            !entry.description.trim().is_empty(),
            "{} has no description",
            entry.id
        );
    }
}

/// Every Tier-A card must render its example's headline result *now*. This is
/// the test that keeps the gallery from becoming a wall of promises.
#[test]
fn every_tier_a_example_renders() {
    let _ = env_logger::try_init();
    let entries: Vec<Entry> = index().into_iter().filter(|e| e.tier == "A").collect();
    assert!(entries.len() >= 5, "expected the Tier-A set");

    let (engine, events) = EngineHandle::spawn();
    wait_for(
        &events,
        Duration::from_secs(300),
        |event| matches!(event, Event::Ready { .. }).then_some(()),
        "GPU Ready",
    );

    let dir = output_dir();
    let mut rendered = Vec::new();
    for entry in &entries {
        let scene = entry.scene.clone().expect("checked by the index test");
        let kind = scene
            .get("kind")
            .and_then(|kind| kind.as_str())
            .expect("scene kind");
        let out = dir.join(format!("{}.png", entry.id));
        let started = Instant::now();
        match kind {
            "terrain" | "terrain_rig" => {
                let dem = scene
                    .get("dem")
                    .and_then(|dem| dem.as_str())
                    .expect("scene dem");
                let meta = call_ok(
                    &engine,
                    &events,
                    "terrain.load",
                    serde_json::json!({ "path": root_join(dem) }),
                    &format!("{}: load DEM", entry.id),
                );
                let width = meta["width"].as_u64().unwrap_or(0) as f64;
                let height = meta["height"].as_u64().unwrap_or(0) as f64;
                let span_world = (meta["domain"][1].as_f64().unwrap_or(1.0)
                    - meta["domain"][0].as_f64().unwrap_or(0.0))
                .abs()
                .max(1.0)
                    * 6.0;
                let mut camera = scene.get("camera").map(|camera| {
                    // `radius_rel` is a fraction of the engine's terrain span.
                    let mut camera = camera.clone();
                    if let Some(relative) = camera.get("radius_rel").and_then(|rel| rel.as_f64()) {
                        camera["radius"] = serde_json::json!(relative * span_world);
                    }
                    camera
                });
                if kind == "terrain_rig" {
                    let rig = scene.get("rig").expect("rig scene");
                    let span = width.max(height);
                    let baked = call_ok(
                        &engine,
                        &events,
                        "rig.bake",
                        serde_json::json!({
                            "kind": rig["kind"],
                            "params": {
                                "duration": number(rig, "duration", 8.0),
                                "radius": number(rig, "radius_rel", 0.32) * span,
                                "phi_start_deg": number(rig, "phi_start_deg", 0.0),
                                "phi_end_deg": number(rig, "phi_end_deg", 360.0),
                                "theta_start_deg": number(rig, "theta_start_deg", 42.0),
                            },
                            "samples_per_second": 12,
                            "domain": meta["domain"],
                        }),
                        &format!("{}: rig.bake", entry.id),
                    );
                    let keyframes = baked["keyframes"]
                        .as_array()
                        .unwrap_or_else(|| panic!("{}: rig.bake returned no keyframes", entry.id));
                    assert!(
                        keyframes.len() > 2,
                        "{}: a rig must bake more than a still frame",
                        entry.id
                    );
                    let first = &keyframes[0];
                    camera = Some(serde_json::json!({
                        "phi_deg": first["phi_deg"],
                        "theta_deg": first["theta_deg"],
                        "radius": first["radius"],
                        "fov_deg": first["fov_deg"],
                    }));
                }
                call_ok(
                    &engine,
                    &events,
                    "terrain.render",
                    serde_json::json!({
                        "width": 640,
                        "height": 400,
                        "camera": camera.unwrap_or_else(|| serde_json::json!({
                            "phi_deg": 135.0, "theta_deg": 45.0,
                            "radius": width.max(height) * 1.2, "fov_deg": 54.0
                        })),
                        "sun": scene.get("sun").cloned().unwrap_or_else(|| serde_json::json!({
                            "azimuth_deg": 135.0, "elevation_deg": 35.0, "intensity": 3.0
                        })),
                        "domain": meta["domain"],
                        "colormap": serde_json::Value::Null,
                        "out_png": out.to_string_lossy(),
                        "params": scene.get("params").cloned().unwrap_or_else(|| serde_json::json!({})),
                    }),
                    &format!("{}: terrain.render", entry.id),
                );
            }
            "mapscene" => {
                let mut params = serde_json::json!({
                    "template": scene["template"],
                    "dem_path": root_join(scene["dem"].as_str().expect("scene dem")),
                    "width": scene.get("width").cloned().unwrap_or(serde_json::json!(640)),
                    "height": scene.get("height").cloned().unwrap_or(serde_json::json!(400)),
                });
                if let Some(raster) = scene.get("raster").and_then(|raster| raster.as_str()) {
                    if let Some(map) = params.as_object_mut() {
                        map.insert(
                            "raster_path".to_owned(),
                            serde_json::json!(root_join(raster)),
                        );
                    }
                }
                if let Some(label) = scene.get("label_text").and_then(|label| label.as_str()) {
                    params["label_text"] = serde_json::json!(label);
                }
                if let Some(opacity) = scene.get("opacity").and_then(|opacity| opacity.as_f64()) {
                    params["opacity"] = serde_json::json!(opacity);
                }
                if let Some(camera) = scene.get("camera") {
                    for key in ["distance", "azimuth_deg", "elevation_deg", "fov_deg"] {
                        if let Some(value) = camera.get(key) {
                            params[key] = value.clone();
                        }
                    }
                }
                let built = call_ok(
                    &engine,
                    &events,
                    "mapscene.template",
                    params,
                    &format!("{}: mapscene.template", entry.id),
                );
                let recipe = built["recipe"].clone();
                assert!(
                    !recipe.is_null(),
                    "{}: template returned no recipe",
                    entry.id
                );
                let result = call_ok(
                    &engine,
                    &events,
                    "mapscene.render",
                    serde_json::json!({
                        "recipe_json": serde_json::to_string(&recipe).unwrap_or_default(),
                        "out_png": out.to_string_lossy(),
                        "certificate": false,
                    }),
                    &format!("{}: mapscene.render", entry.id),
                );
                let status = result["status"].as_str().unwrap_or("?");
                assert_eq!(
                    status, "ok",
                    "{}: MapScene reported {status} (diagnostics: {:?})",
                    entry.id, result["diagnostics"]
                );
            }
            other => panic!("{}: unknown scene kind {other:?}", entry.id),
        }

        assert_real_render(&out, entry);
        // Terrain scenes must be visibly coloured; MapScene scenes are shaded
        // rasters and labels, so they get a lower but non-zero floor.
        let chroma_floor = if kind == "mapscene" { 5.0 } else { 18.0 };
        assert_coloured(&out, entry, chroma_floor);
        rendered.push((
            entry.id.clone(),
            entry.title.clone(),
            std::fs::metadata(&out).map(|meta| meta.len()).unwrap_or(0),
            started.elapsed(),
        ));
    }

    // A readable proof for whoever runs the suite.
    println!("\nexamples gallery — Tier A renders");
    for (id, title, bytes, elapsed) in &rendered {
        println!(
            "  {id:28} {title:32} {:>6} KB  {:>5.1}s",
            bytes / 1024,
            elapsed.as_secs_f32()
        );
    }
    assert!(
        !rendered.is_empty(),
        "the gallery must have at least one runnable example"
    );

    engine.shutdown();
}

/// Coloured terrain renders must actually be coloured.
///
/// This locks the `OverlayLayer` height-domain bug: without the terrain's raw
/// height range the colormap clamps to its top colour and every frame comes
/// back washed-out grey — invisible to tests that only check pixels changed
/// (6.9 mean chroma versus 32 with the domain set).
fn assert_coloured(path: &std::path::Path, entry: &Entry, threshold: f64) {
    let image = image::open(path)
        .unwrap_or_else(|err| panic!("{} produced an undecodable PNG: {err}", entry.id))
        .to_rgba8();
    let mut chroma = 0u64;
    for pixel in image.pixels() {
        let [r, g, b, _] = pixel.0;
        chroma += u64::from(r.max(g).max(b) - r.min(g).min(b));
    }
    let mean = chroma as f64 / (f64::from(image.width()) * f64::from(image.height()));
    assert!(
        mean >= threshold,
        "{} renders washed out (mean chroma {mean:.1}, need >= {threshold:.0})",
        entry.id
    );
}

fn root_join(path: &str) -> String {
    repo_root().join(path).to_string_lossy().into_owned()
}
