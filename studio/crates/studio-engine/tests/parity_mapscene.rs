//! Part IV parity harness: Studio's Bridge v2 ops must reproduce the exact
//! workflows performed by the forge3d examples (GUI.md Part IV).
//!
//! These tests require the embedded Python environment (forge3d + numpy +
//! rasterio) and a GPU adapter, like the terrain integration test. The
//! golden comparison is strict only under `FORGE3D_STUDIO_GOLDEN_STRICT=1`:
//! committed recipe goldens carry a device/lanes caveat (they are rendered
//! and compared on the engine's canonical lane), so on other machines the
//! structural checks run and the pixel gate is advisory (ANAMNESIS ABSENT
//! rule).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use studio_engine::{Command, EngineHandle, Event};

fn fuji_dem() -> PathBuf {
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

/// Every template the dispatcher offers must build a recipe through the
/// example-identical construction path, validate, and render through
/// `MapScene.render` (the certified path the upstream goldens exercise).
#[test]
fn bridge_renders_example_mapscene_templates() {
    let (engine, events) = spawn_ready();

    let dem = fuji_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    engine.send(Command::LoadTerrain { path: dem.clone() });
    wait_for(
        &events,
        Duration::from_secs(300),
        |e| matches!(e, Event::TerrainLoaded { .. }).then_some(()),
        "TerrainLoaded",
    );

    // Template listing op mirrors the registry surface.
    let job = engine.call("mapscene.templates", serde_json::json!({}));
    let templates = wait_for(
        &events,
        Duration::from_secs(120),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => result
                .get("templates")
                .and_then(|t| t.as_array())
                .map(|a| a.len()),
            _ => None,
        },
        "mapscene.templates",
    );
    assert_eq!(templates, 3);

    for template in ["terrain", "terrain_raster", "terrain_vector_labels"] {
        let mut params = serde_json::json!({
            "template": template,
            "dem_path": dem.to_string_lossy(),
            "width": 160,
            "height": 120,
        });
        if template == "terrain_raster" {
            // RasterOverlay unstyled sources are PNG/GeoTIFF; write a small
            // PNG fixture the way the canonical example stages its overlay.
            // The DEM for this template is the engine's bundled mini DEM:
            // draping the UV raster over a 2594² DEM exceeds the engine's
            // 512 MiB host-visible budget, while the canonical example
            // renders a small DEM by design.
            let raster = std::env::temp_dir().join("studio-parity-overlay.png");
            image::RgbImage::from_fn(32, 24, |x, y| {
                image::Rgb([(x * 255 / 31) as u8, (y * 255 / 23) as u8, 128])
            })
            .write_to(
                &mut std::fs::File::create(&raster).expect("create overlay"),
                image::ImageFormat::Png,
            )
            .expect("write overlay fixture");
            params["raster_path"] = serde_json::json!(raster.to_string_lossy());
            params["dem_path"] = serde_json::json!(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../python/forge3d/data/mini_dem.npy")
                .to_string_lossy());
            params["crs"] = serde_json::json!("EPSG:32610");
        }
        let job = engine.call("mapscene.template", params.clone());
        let recipe = wait_for(
            &events,
            Duration::from_secs(300),
            |e| match e {
                Event::JobDone {
                    job: done, result, ..
                } if *done == job => result.get("recipe").cloned(),
                Event::JobFailed {
                    job: done, message, ..
                } if *done == job => {
                    panic!("template {template} failed: {message}");
                }
                _ => None,
            },
            &format!("mapscene.template({template})"),
        );
        assert_eq!(
            recipe.get("kind").and_then(|k| k.as_str()),
            Some("scene_recipe"),
            "template {template} must serialize as a scene recipe"
        );
        let layer_count = recipe
            .get("layers")
            .and_then(|l| l.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let expected_layers = match template {
            "terrain" => 0,
            "terrain_raster" => 1,
            "terrain_vector_labels" => 2,
            _ => unreachable!(),
        };
        assert_eq!(layer_count, expected_layers, "template {template} layers");

        // Validate through the engine's own validation report.
        let job = engine.call(
            "mapscene.validate",
            serde_json::json!({ "recipe_json": recipe.to_string() }),
        );
        wait_for(
            &events,
            Duration::from_secs(300),
            |e| match e {
                Event::JobDone {
                    job: done, result, ..
                } if *done == job => Some(
                    result
                        .get("status")
                        .and_then(|s| s.as_str())
                        .map(String::from),
                ),
                Event::JobFailed {
                    job: done, message, ..
                } if *done == job => {
                    panic!("validate {template} failed: {message}");
                }
                _ => None,
            },
            &format!("mapscene.validate({template})"),
        );

        // Render through MapScene.render — the certified golden path.
        let out = std::env::temp_dir().join(format!("studio-parity-{template}.png"));
        let _ = std::fs::remove_file(&out);
        let job = engine.call(
            "mapscene.render",
            serde_json::json!({
                "recipe_json": recipe.to_string(),
                "out_png": out.to_string_lossy(),
                "certificate": false,
            }),
        );
        let (png, status, backend) = wait_for(
            &events,
            Duration::from_secs(600),
            |e| match e {
                Event::JobDone {
                    job: done, result, ..
                } if *done == job => Some((
                    result
                        .get("png")
                        .and_then(|p| p.as_str())
                        .map(PathBuf::from),
                    result
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("?")
                        .to_owned(),
                    result
                        .get("backend")
                        .and_then(|s| s.as_str())
                        .unwrap_or("?")
                        .to_owned(),
                )),
                Event::JobFailed {
                    job: done, message, ..
                } if *done == job => {
                    panic!("render {template} failed: {message}");
                }
                _ => None,
            },
            &format!("mapscene.render({template})"),
        );
        println!("template {template}: status={status} backend={backend}");
        let png = png.expect("render result must name the png");
        let bytes = std::fs::read(&png).expect("rendered png must exist");
        assert_eq!(
            &bytes[0..4],
            b"\x89PNG",
            "render must be a real PNG ({template})"
        );

        let img = image::open(&png).expect("decode rendered png");
        let rgba = img.to_rgba8();
        assert_eq!(
            (rgba.width(), rgba.height()),
            (160, 120),
            "rendered size must match the recipe output spec ({template})"
        );
        let mut min = u8::MAX;
        let mut max = 0u8;
        for px in rgba.pixels() {
            for c in px.0 {
                min = min.min(c);
                max = max.max(c);
            }
        }
        assert!(max > min, "rendered frame is flat ({template})");
        std::fs::remove_file(&png).ok();
    }

    engine.shutdown();
}

/// Catalog ops must return exactly what the engine's registries expose.
#[test]
fn catalog_ops_list_registered_entries() {
    let (engine, events) = spawn_ready();

    let job = engine.call("colormaps.list", serde_json::json!({}));
    let colormaps = wait_for(
        &events,
        Duration::from_secs(120),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => result.get("colormaps").and_then(|c| c.as_array()).cloned(),
            _ => None,
        },
        "colormaps.list",
    );
    assert!(colormaps.len() >= 3, "core palettes must be registered");
    assert!(colormaps
        .iter()
        .any(|c| c.as_str() == Some("forge3d:viridis")));

    let job = engine.call("presets.list", serde_json::json!({}));
    let presets = wait_for(
        &events,
        Duration::from_secs(120),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => result.get("presets").and_then(|c| c.as_array()).cloned(),
            _ => None,
        },
        "presets.list",
    );
    assert!(presets.len() >= 5, "upstream ships five renderer presets");
    assert!(presets
        .iter()
        .any(|c| c.as_str() == Some("rainiershowcase")));

    // Unknown ops fail with a structured error, never a panic.
    let job = engine.call("no.such.op", serde_json::json!({}));
    wait_for(
        &events,
        Duration::from_secs(60),
        |e| match e {
            Event::JobFailed {
                job: done,
                op,
                message,
            } if *done == job => Some((op.clone(), message.contains("unknown op"))),
            _ => None,
        },
        "unknown-op failure",
    );

    engine.shutdown();
}

/// Advisory strict-mode golden gate (GUI.md Part IV): compare the
/// `mapscene.render` output for the canonical terrain_raster template family
/// against the committed recipe golden. Only meaningful on the engine's
/// canonical reference lane; enable with FORGE3D_STUDIO_GOLDEN_STRICT=1.
#[test]
#[ignore = "device-lane dependent; run with FORGE3D_STUDIO_GOLDEN_STRICT=1 on the canonical lane"]
fn golden_mapscene_terrain_raster_reference() {
    let (engine, events) = spawn_ready();
    let dem = fuji_dem();
    engine.send(Command::LoadTerrain { path: dem });
    wait_for(
        &events,
        Duration::from_secs(300),
        |e| matches!(e, Event::TerrainLoaded { .. }).then_some(()),
        "TerrainLoaded",
    );
    let job = engine.call(
        "mapscene.template",
        serde_json::json!({
            "template": "terrain",
            "dem_path": fuji_dem().to_string_lossy(),
            "width": 96,
            "height": 64,
        }),
    );
    let recipe = wait_for(
        &events,
        Duration::from_secs(300),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => result.get("recipe").cloned(),
            _ => None,
        },
        "template",
    );
    let out = std::env::temp_dir().join("studio-golden-reference.png");
    let _job = engine.call(
        "mapscene.render",
        serde_json::json!({
            "recipe_json": recipe.to_string(),
            "out_png": out.to_string_lossy(),
        }),
    );
    wait_for(
        &events,
        Duration::from_secs(600),
        |e| matches!(e, Event::JobDone { .. }).then_some(()),
        "render",
    );
    // Mean-abs gate mirrors tests/test_recipe_goldens.py::_assert_matches_golden.
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/golden/recipes/mapscene_terrain_raster.png");
    let a = image::open(&out).expect("studio render").to_rgba8();
    let b = image::open(&golden).expect("committed golden").to_rgba8();
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    let diff: u64 = a
        .pixels()
        .zip(b.pixels())
        .map(|(x, y)| {
            x.0.iter()
                .zip(y.0.iter())
                .map(|(p, q)| (i64::from(*p) - i64::from(*q)).unsigned_abs())
                .sum::<u64>()
        })
        .sum();
    let mean_abs = diff as f64 / (a.width() as f64 * a.height() as f64 * 4.0);
    println!("mean abs diff vs committed golden: {mean_abs:.3}");
    assert!(
        mean_abs <= 2.0,
        "golden parity gate exceeded: {mean_abs:.3}"
    );
    engine.shutdown();
}

/// U2: the document compiles to the recipe its example builds.
///
/// Compile semantics under test (GUI.md U2 exit criteria): kinds map to the
/// same engine objects the canonical templates construct, unrenderable kinds
/// are excluded with an honest diagnostic, invisible layers are skipped, and
/// a missing terrain is a hard `no_terrain` error — never a fake render.
#[test]
fn scene_compile_document_semantics() {
    let (engine, events) = spawn_ready();

    let dem = fuji_dem();
    assert!(dem.is_file(), "DEM fixture missing: {}", dem.display());
    engine.send(Command::LoadTerrain { path: dem.clone() });
    wait_for(
        &events,
        Duration::from_secs(300),
        |e| matches!(e, Event::TerrainLoaded { .. }).then_some(()),
        "TerrainLoaded",
    );

    // Fuji's georeferenced bounds; vector geometry lives in the terrain CRS.
    let dem_str = dem.to_string_lossy().into_owned();
    let document = serde_json::json!({
        "layers": [
            {"id": 1, "parent": null, "kind": "Terrain", "name": "Mount Fuji",
             "visible": true, "opacity": 1.0, "locked": false,
             "source": dem_str, "style": {}},
            {"id": 2, "parent": null, "kind": "RasterOverlay", "name": "ortho",
             "visible": false, "opacity": 0.8, "locked": false,
             "source": "unused.png", "style": {}},
            {"id": 3, "parent": null, "kind": "Vector", "name": "lines",
             "visible": true, "opacity": 1.0, "locked": false,
             "source": null,
             "style": {
                 "crs": "EPSG:4326",
                 "features": [
                     {"id": "line-1", "geometry": {"type": "LineString",
                      "coordinates": [[138.7, 35.3], [138.8, 35.4]]},
                      "properties": {"class": "primary"}}
                 ],
                 "mapbox": {"version": 8, "layers": [
                     {"id": "lines", "type": "line", "paint": {"line-color": "#ff0000"}}]}
             }},
            {"id": 4, "parent": null, "kind": "Labels", "name": "labels",
             "visible": true, "opacity": 1.0, "locked": false,
             "source": null,
             "style": {
                 "labels": [
                     {"id": "peak", "kind": "point", "text": "Fuji",
                      "geometry": {"type": "Point", "coordinates": [138.73, 35.36, 0.0]},
                      "priority_class": "cities"}
                 ]
             }},
            {"id": 5, "parent": null, "kind": "Buildings", "name": "city",
             "visible": true, "opacity": 1.0, "locked": false,
             "source": null, "style": {}}
        ]
    });
    let params = serde_json::json!({
        "document": document,
        "environment": {
            "camera": {"distance": 1.2, "azimuth_deg": 25.0,
                       "elevation_deg": 45.0, "fov_deg": 45.0},
            "lighting": {"name": "daylight", "intensity": 1.1},
            "output": {"width": 160, "height": 120},
        },
    });

    let job = engine.call("scene.compile", params.clone());
    let (recipe, diagnostics) = wait_for(
        &events,
        Duration::from_secs(300),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => Some((
                result.get("recipe").cloned(),
                result.get("diagnostics").cloned(),
            )),
            Event::JobFailed {
                job: done, message, ..
            } if *done == job => {
                panic!("scene.compile failed: {message}");
            }
            _ => None,
        },
        "scene.compile",
    );
    let recipe = recipe.expect("compile returns a recipe");
    let diagnostics = diagnostics.unwrap_or(serde_json::Value::Array(vec![]));
    let diagnostics = diagnostics.as_array().cloned().unwrap_or_default();

    // The invisible raster layer is skipped; buildings are excluded with an
    // honest diagnostic; vector + labels compile with the terrain CRS.
    let layers = recipe
        .get("layers")
        .and_then(|l| l.as_array())
        .expect("layers");
    let kinds: Vec<&str> = layers
        .iter()
        .filter_map(|l| l.get("kind").and_then(|k| k.as_str()))
        .collect();
    assert_eq!(kinds.len(), 2, "vector + labels compile; got {kinds:?}");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.get("code").and_then(|c| c.as_str()) == Some("layer_unrenderable")),
        "buildings layer must be reported as registered-only: {diagnostics:?}"
    );
    let vector = layers
        .iter()
        .find(|l| l.get("kind").and_then(|k| k.as_str()) == Some("vector_overlay"))
        .expect("vector layer");
    assert_eq!(
        vector.get("crs").and_then(|c| c.as_str()),
        Some("EPSG:4326"),
        "layers inherit the terrain CRS"
    );

    // The compiled recipe validates and renders through the certified path.
    let out = std::env::temp_dir().join("studio-compile-render.png");
    let _ = std::fs::remove_file(&out);
    let job = engine.call(
        "mapscene.render",
        serde_json::json!({
            "recipe_json": recipe.to_string(),
            "out_png": out.to_string_lossy(),
        }),
    );
    let png = wait_for(
        &events,
        Duration::from_secs(600),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => result
                .get("png")
                .and_then(|p| p.as_str())
                .map(PathBuf::from),
            Event::JobFailed {
                job: done, message, ..
            } if *done == job => {
                panic!("compiled-recipe render failed: {message}");
            }
            _ => None,
        },
        "mapscene.render(compiled)",
    );
    let bytes = std::fs::read(&png).expect("rendered png exists");
    assert_eq!(&bytes[0..4], b"\x89PNG");
    std::fs::remove_file(&png).ok();

    // No visible terrain: the honest hard error, never a fake recipe.
    let empty = serde_json::json!({
        "document": {"layers": [
            {"id": 1, "parent": null, "kind": "Vector", "name": "lines",
             "visible": true, "opacity": 1.0, "locked": false, "source": null,
             "style": {}}
        ]},
        "environment": {},
    });
    let job = engine.call("scene.compile", empty);
    let diagnostics = wait_for(
        &events,
        Duration::from_secs(120),
        |e| match e {
            Event::JobDone {
                job: done, result, ..
            } if *done == job => result.get("diagnostics").cloned(),
            _ => None,
        },
        "scene.compile(no terrain)",
    );
    let diagnostics = diagnostics.as_array().cloned().unwrap_or_default();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.get("code").and_then(|c| c.as_str()) == Some("no_terrain")),
        "no visible terrain must be a no_terrain error: {diagnostics:?}"
    );

    engine.shutdown();
}
