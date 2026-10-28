//! Part VII parity harness for M7: atmosphere, observation, smoke.
//!
//! The three M7 subsystems are engine surfaces Studio composes rather than
//! reimplements — AETHER's sky/LUT bank, SIDERA's ephemeris, and the smoke
//! volume renderer. These tests drive them through the worker exactly as the
//! app does (`SetAtmosphere` + `atmosphere.environment` / `Call`) and assert:
//!
//! * the LUT bank's reported provenance describes the payload it names (the
//!   byte size follows from the reported dimensions, the SHA changes when the
//!   inputs do);
//! * enabling Atmosphere with a sun elevation changes the frame, the
//!   environment file is a Radiance HDR the engine's reader accepts, and its
//!   reported digest is the digest of the bytes on disk;
//! * SIDERA reports compass angles and the renderer-frame angles the terrain
//!   light uses, with the same `A - 90` conversion the project type carries;
//! * a smoke volume loads, composites over a real frame with the backend the
//!   capability report names, and a failed load leaves no stale volume.
//!
//! Every check compares against a source of truth other than the value under
//! test: the hdr digest against the file's bytes, the renderer azimuth against
//! the documented conversion and `Observation::renderer_azimuth_deg`, the
//! composite's renderer against `smoke.capabilities`, and the LUT byte size
//! against the product of the dimensions the same report lists.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use studio_engine::{Command, EngineHandle, Event};
use studio_project::{
    AtmosphereParams, CameraParams, Observation, RenderQuality, SmokeParams, SunParams,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The 256² mini DEM: small enough for repeated atmosphere renders to finish
/// in seconds.
fn mini_dem() -> PathBuf {
    repo_root().join("python/forge3d/data/mini_dem.npy")
}

fn scratch_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/m7-fixtures");
    std::fs::create_dir_all(&dir).expect("create M7 fixture dir");
    dir
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

/// One `RenderFrame` round-trip: the pixels plus what the worker reported
/// alongside them.
struct Frame {
    rgba: Vec<u8>,
    smoke: Option<serde_json::Value>,
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
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("render timed out");
        match events.recv_timeout(remaining) {
            Ok(Event::FrameReady {
                width: ready_width,
                height: ready_height,
                rgba,
                smoke,
                ..
            }) => {
                assert_eq!((ready_width, ready_height), (width, height));
                assert_eq!(rgba.len(), (width * height * 4) as usize);
                return Frame { rgba, smoke };
            }
            Ok(Event::Error { context, message }) => {
                panic!("render failed: {context}: {message}")
            }
            Ok(_) => {}
            Err(err) => panic!("engine channel closed during a render: {err}"),
        }
    }
}

/// Mean of the 8-bit luma, the coarse "is this frame black / blown out"
/// measure a size-relative comparison needs.
fn mean_luma(rgba: &[u8]) -> f64 {
    let pixels = rgba.len() / 4;
    let sum: f64 = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64)
        .sum();
    sum / pixels as f64
}

fn differing_pixels(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(a.len(), b.len(), "frames must be the same size to compare");
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .filter(|(left, right)| left != right)
        .count()
}

fn render_to_file(
    engine: &EngineHandle,
    events: &std::sync::mpsc::Receiver<Event>,
    width: u32,
    height: u32,
    path: &Path,
) {
    let _ = std::fs::remove_file(path);
    engine.send(Command::RenderToFile {
        width,
        height,
        path: path.to_path_buf(),
    });
    wait_for(
        events,
        Duration::from_secs(900),
        |e| match e {
            Event::ExportFinished { .. } => Some(()),
            Event::ExportFailed { message, .. } => panic!("export failed: {message}"),
            _ => None,
        },
        "ExportFinished",
    );
    assert!(path.is_file(), "export must write {}", path.display());
}

/// A Radiance HDR as this test decodes it — the engine reader's own rules
/// (`src/formats/hdr.rs`): a run byte above 128 repeats the next value
/// `run - 128` times, anything else is a literal run of `run` bytes.
struct DecodedHdr {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
    trailing_bytes: usize,
}

fn decode_radiance(bytes: &[u8]) -> DecodedHdr {
    let mut offset = 0usize;
    let mut line = String::new();
    let mut read_line = |offset: &mut usize| -> String {
        line.clear();
        while *offset < bytes.len() {
            let byte = bytes[*offset];
            *offset += 1;
            if byte == b'\n' {
                break;
            }
            line.push(byte as char);
        }
        line.trim_end_matches('\r').to_owned()
    };

    let magic = read_line(&mut offset);
    assert_eq!(magic, "#?RADIANCE", "HDR magic line");
    let mut format = None;
    loop {
        let header = read_line(&mut offset);
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("FORMAT=") {
            format = Some(value.to_owned());
        }
    }
    assert_eq!(
        format.as_deref(),
        Some("32-bit_rle_rgbe"),
        "the engine reader only accepts 32-bit rle rgbe"
    );
    let resolution_line = read_line(&mut offset);
    let resolution: Vec<&str> = resolution_line.split_whitespace().collect();
    assert_eq!(resolution.len(), 4, "resolution line: {resolution:?}");
    assert_eq!(resolution[0], "-Y", "top-down rows");
    assert_eq!(resolution[2], "+X", "left-to-right columns");
    let height: u32 = resolution[1].parse().expect("HDR height");
    let width: u32 = resolution[3].parse().expect("HDR width");

    let mut pixels = Vec::with_capacity((width * height) as usize);
    for row in 0..height {
        let header = &bytes[offset..offset + 4];
        offset += 4;
        assert_eq!(header[0], 2, "row {row}: new-style RLE marker");
        assert_eq!(header[1], 2, "row {row}: new-style RLE marker");
        let declared = ((header[2] as u32) << 8) | header[3] as u32;
        assert_eq!(declared, width, "row {row}: scanline width header");

        let mut row_pixels = vec![[0u8; 4]; width as usize];
        for component in 0..4usize {
            let mut pos = 0usize;
            while pos < width as usize {
                let run = bytes[offset];
                offset += 1;
                if run > 128 {
                    let count = (run - 128) as usize;
                    assert!(
                        run != 128 && count > 0 && pos + count <= width as usize,
                        "row {row} component {component}: repeat run {count} at {pos} overflows {width}"
                    );
                    let value = bytes[offset];
                    offset += 1;
                    for slot in row_pixels.iter_mut().skip(pos).take(count) {
                        slot[component] = value;
                    }
                    pos += count;
                } else {
                    let count = run as usize;
                    assert!(
                        count > 0 && pos + count <= width as usize,
                        "row {row} component {component}: literal run {count} at {pos} overflows {width}"
                    );
                    for slot in row_pixels.iter_mut().skip(pos).take(count) {
                        slot[component] = bytes[offset];
                        offset += 1;
                    }
                    pos += count;
                }
            }
        }
        pixels.extend(row_pixels);
    }

    DecodedHdr {
        width,
        height,
        pixels,
        trailing_bytes: bytes.len() - offset,
    }
}

/// RGBe → linear, the engine reader's own unpacking (`exp = 2^(e - 136)`).
fn rgbe_luma(pixel: [u8; 4]) -> f32 {
    let [r, g, b, e] = pixel;
    if e == 0 {
        return 0.0;
    }
    let scale = 2.0f32.powi(e as i32 - 136);
    0.2126 * r as f32 * scale + 0.7152 * g as f32 * scale + 0.0722 * b as f32 * scale
}

fn atmosphere(enabled: bool, turbidity: f32, intensity: f32) -> AtmosphereParams {
    AtmosphereParams {
        enabled,
        intensity,
        turbidity,
        ..AtmosphereParams::default()
    }
}

fn environment_args(atmosphere: &AtmosphereParams, elevation: f32) -> serde_json::Value {
    serde_json::json!({
        "settings": atmosphere.engine_kwargs(),
        "sun_elevation_deg": elevation,
        "intensity": atmosphere.intensity,
    })
}

#[test]
fn atmosphere_bake_provenance_describes_the_payload_it_names() {
    let (engine, events) = spawn_ready();
    let settings = serde_json::json!({ "settings": AtmosphereParams::default().engine_kwargs() });
    let first = call_ok(
        &engine,
        &events,
        "atmosphere.bake",
        settings.clone(),
        "atmosphere.bake",
    );
    let digest = first
        .get("deterministic_sha256")
        .and_then(|v| v.as_str())
        .expect("the bake handle reports the payload's SHA-256")
        .to_owned();
    assert_eq!(digest.len(), 64, "a hex SHA-256: {digest}");
    assert!(
        digest.chars().all(|c| c.is_ascii_hexdigit()),
        "a hex SHA-256: {digest}"
    );
    assert_eq!(
        first.get("precomputed").and_then(|v| v.as_bool()),
        Some(true),
        "the default turbidity sits inside the shipped bracket: {first:?}"
    );

    // The reported byte size must follow from the reported dimensions and
    // wavelength count — a wrong payload cannot masquerade as the right sky.
    let dimensions = first
        .get("dimensions")
        .and_then(|v| v.as_object())
        .expect("dimensions of the LUT bank");
    let mut voxels: u64 = 0;
    for (name, shape) in dimensions {
        let extent: u64 = shape
            .as_array()
            .unwrap_or_else(|| panic!("{name} shape must be an array: {shape}"))
            .iter()
            .map(|value| value.as_u64().expect("integer extent"))
            .product();
        assert!(extent > 0, "{name} must have a non-empty shape: {shape}");
        voxels += extent;
    }
    let byte_size = first
        .get("byte_size")
        .and_then(|v| v.as_u64())
        .expect("byte size");
    assert_eq!(
        first.get("storage_format").and_then(|v| v.as_str()),
        Some("rgba16float"),
        "the LUTs are stored as 4 x 16-bit floats = 8 bytes per voxel"
    );
    assert_eq!(
        first.get("wavelength_count").and_then(|v| v.as_u64()),
        Some(11),
        "the sky is sampled at 11 wavelengths"
    );
    assert_eq!(
        byte_size,
        voxels * 8,
        "rgba16float is 8 bytes per voxel: {voxels} voxels"
    );
    assert_eq!(
        first.get("settings").cloned(),
        Some(AtmosphereParams::default().engine_kwargs()),
        "the report echoes the inputs the digest belongs to"
    );
    for key in ["aerial_lut_semantics", "scattering_lut_semantics"] {
        let text = first.get(key).and_then(|v| v.as_str()).unwrap_or("");
        assert!(!text.is_empty(), "{key} must state what the LUT holds");
    }

    // Same inputs, same sky; different turbidity, different tables.
    let again = call_ok(
        &engine,
        &events,
        "atmosphere.bake",
        settings,
        "atmosphere.bake repeat",
    );
    assert_eq!(
        again.get("deterministic_sha256").and_then(|v| v.as_str()),
        Some(digest.as_str()),
        "identical inputs must resolve to one payload"
    );

    let hazy = call_ok(
        &engine,
        &events,
        "atmosphere.bake",
        serde_json::json!({
            "settings": AtmosphereParams { turbidity: 5.5, ..AtmosphereParams::default() }
                .engine_kwargs(),
        }),
        "atmosphere.bake (hazy)",
    );
    let hazy_digest = hazy
        .get("deterministic_sha256")
        .and_then(|v| v.as_str())
        .expect("hazy digest");
    assert_ne!(
        hazy_digest, digest,
        "a different turbidity must not report the same scattering tables"
    );
    assert_eq!(
        hazy.get("precomputed").and_then(|v| v.as_bool()),
        Some(true),
        "the LUT bank interpolates inside the shipped bracket instead of re-baking"
    );

    engine.shutdown();
}

#[test]
fn aether_environment_lights_the_terrain_from_a_radiance_hdr() {
    let (engine, events) = spawn_ready();
    let _view = load_dem(&engine, &events);

    let probe = render(&engine, &events, 96, 64);
    let probe_luma = mean_luma(&probe.rgba);

    let settings = atmosphere(true, 2.0, 1.0);
    engine.send(Command::SetAtmosphere(settings.clone()));
    let report = call_ok(
        &engine,
        &events,
        "atmosphere.environment",
        environment_args(&settings, 35.0),
        "atmosphere.environment",
    );
    assert_eq!(
        report.get("source").and_then(|v| v.as_str()),
        Some("aether"),
        "the environment must name its generator: {report:?}"
    );
    assert_eq!(report.get("enabled").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(report.get("width").and_then(|v| v.as_u64()), Some(64));
    assert_eq!(report.get("height").and_then(|v| v.as_u64()), Some(32));
    assert_eq!(
        report.get("mode").and_then(|v| v.as_str()),
        Some("lut"),
        "the sky comes from the LUT bank, not a different integrator: {report:?}"
    );
    assert_eq!(
        report.get("linear_hdr").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        report.get("sun_elevation_deg").and_then(|v| v.as_f64()),
        Some(35.0),
        "the sky is generated at the sun's elevation, and says which"
    );
    let path = report
        .get("path")
        .and_then(|v| v.as_str())
        .expect("the environment names the file it wrote");
    assert!(
        Path::new(path).is_file(),
        "the HDR the IBL binds must exist: {path}"
    );

    // The reported digest must be the digest of the bytes on disk: the panel
    // compares two skies by this string, so it has to describe the file.
    let bytes = std::fs::read(path).expect("read the environment HDR");
    let digest = report
        .get("hdr_sha256")
        .and_then(|v| v.as_str())
        .expect("the environment reports its digest")
        .to_owned();
    assert_eq!(
        digest,
        hex_sha256(&bytes),
        "hdr_sha256 must be the SHA-256 of the written file"
    );
    let mean_luminance = report
        .get("mean_luminance")
        .and_then(|v| v.as_f64())
        .expect("mean luminance");
    assert!(
        (0.0..4.0).contains(&mean_luminance) && mean_luminance > 0.0,
        "a daytime AETHER sky is dim but not black: mean luminance {mean_luminance}"
    );

    let aether = render(&engine, &events, 96, 64);
    let aether_luma = mean_luma(&aether.rgba);
    eprintln!("luma: probe {probe_luma:.2} aether {aether_luma:.2}");
    assert!(
        differing_pixels(&probe.rgba, &aether.rgba) > 0,
        "enabling Atmosphere must change what lights the terrain"
    );
    assert!(
        aether_luma > 1.0,
        "the AETHER sun must actually light the terrain, not black it out: {aether_luma:.2}"
    );

    // The environment is built once per (inputs, elevation): the next frame
    // reuses it, so the two frames are the same image.
    let repeat = render(&engine, &events, 96, 64);
    assert!(
        aether.rgba == repeat.rgba,
        "one environment must light repeated frames identically"
    );

    // A different sun elevation is a different sky (and a different frame).
    let lower = call_ok(
        &engine,
        &events,
        "atmosphere.environment",
        environment_args(&settings, 8.0),
        "atmosphere.environment (low sun)",
    );
    let lower_digest = lower
        .get("hdr_sha256")
        .and_then(|v| v.as_str())
        .expect("low-sun digest")
        .to_owned();
    assert_ne!(
        lower_digest, digest,
        "the sky at 8 deg must not report the 35 deg environment"
    );
    assert_eq!(
        lower.get("path").and_then(|v| v.as_str()),
        Some(path),
        "one engine keeps one environment file: the second build replaces it"
    );
    let lower_luma = lower
        .get("mean_luminance")
        .and_then(|v| v.as_f64())
        .expect("low-sun mean luminance");
    assert!(
        (lower_luma - mean_luminance).abs() > 1.0e-6,
        "a lower sun must produce a different sky: {lower_luma} vs {mean_luminance}"
    );

    // Turning Atmosphere off restores the probe environment — and says so.
    engine.send(Command::SetAtmosphere(atmosphere(false, 2.0, 1.0)));
    let restored = render(&engine, &events, 96, 64);
    assert!(
        restored.rgba == probe.rgba,
        "with Atmosphere off the probe environment must light the frame again"
    );

    engine.shutdown();
}

#[test]
fn environment_hdr_is_a_radiance_file_the_engine_reader_accepts() {
    let (engine, events) = spawn_ready();
    let settings = atmosphere(true, 2.0, 1.0);
    engine.send(Command::SetAtmosphere(settings.clone()));
    let report = call_ok(
        &engine,
        &events,
        "atmosphere.environment",
        environment_args(&settings, 45.0),
        "atmosphere.environment",
    );
    let path = report
        .get("path")
        .and_then(|v| v.as_str())
        .expect("environment path");
    let bytes = std::fs::read(path).expect("read the environment HDR");

    let decoded = decode_radiance(&bytes);
    assert_eq!((decoded.width, decoded.height), (64, 32));
    assert_eq!(decoded.pixels.len(), 64 * 32);
    assert_eq!(
        decoded.trailing_bytes, 0,
        "the writer must not leave bytes the reader would never consume"
    );

    // A sky is not a black image, and RGBE is not trivially all-ones: the
    // decoded radiance must span more than one exponent.
    let luma: Vec<f32> = decoded
        .pixels
        .iter()
        .map(|pixel| rgbe_luma(*pixel))
        .collect();
    let lit = luma.iter().filter(|value| **value > 0.0).count();
    assert!(
        lit > luma.len() / 4,
        "a daytime sky lights a substantial fraction of the texels: {lit}/{}",
        luma.len()
    );
    let brightest = luma.iter().fold(0.0f32, |a, b| a.max(*b));
    assert!(
        brightest > 0.0 && brightest < 1.0e3,
        "a 45 deg sky is a finite radiance, not a broken exponent: {brightest}"
    );
    let exponents: std::collections::BTreeSet<u8> = decoded
        .pixels
        .iter()
        .filter(|pixel| pixel[3] != 0)
        .map(|pixel| pixel[3])
        .collect();
    assert!(
        exponents.len() > 1,
        "a gradient sky spans several shared exponents, got {exponents:?}"
    );

    // The op's report and the file agree about the same sky.
    assert_eq!(
        report.get("hdr_sha256").and_then(|v| v.as_str()),
        Some(hex_sha256(&bytes).as_str()),
        "the reported digest is the file's digest"
    );

    engine.shutdown();
}

#[test]
fn sidereal_observation_reports_the_angles_the_terrain_light_uses() {
    let (engine, events) = spawn_ready();
    // Solstice noon on the equator: the Sun stands at 90 - 23.44 deg.
    let report = call_ok(
        &engine,
        &events,
        "astro.observe",
        serde_json::json!({
            "datetime_utc": "2026-06-21T12:00:00Z",
            "latitude_deg": 0.0,
            "longitude_deg": 0.0,
        }),
        "astro.observe (noon)",
    );
    let sun = report
        .get("bodies")
        .and_then(|bodies| bodies.get("sun"))
        .expect("the report carries the Sun");
    let altitude = sun
        .get("altitude_deg")
        .and_then(|v| v.as_f64())
        .expect("sun altitude");
    assert!(
        (altitude - 66.56).abs() < 0.3,
        "at the June solstice on the equator the noon Sun is 66.56 deg up, got {altitude:.3}"
    );
    let azimuth = sun
        .get("azimuth_deg")
        .and_then(|v| v.as_f64())
        .expect("sun azimuth");
    // Near local noon the Sun sits almost due north of an equatorial observer.
    assert!(
        azimuth < 5.0 || azimuth > 355.0,
        "the noon Sun is within a few degrees of due north, got {azimuth:.3}"
    );
    assert_eq!(
        sun.get("above_horizon").and_then(|v| v.as_bool()),
        Some(true)
    );
    let distance = sun
        .get("distance_au")
        .and_then(|v| v.as_f64())
        .expect("Sun distance");
    assert!(
        (0.98..1.02).contains(&distance),
        "the Sun is ~1 AU away, got {distance}"
    );

    // The renderer-frame angles are the conversion `Observation` carries, and
    // the app uses the engine's numbers rather than recomputing a position.
    let renderer_azimuth = sun
        .get("renderer_azimuth_deg")
        .and_then(|v| v.as_f64())
        .expect("renderer azimuth");
    let renderer_elevation = sun
        .get("renderer_elevation_deg")
        .and_then(|v| v.as_f64())
        .expect("renderer elevation");
    assert!(
        (renderer_azimuth - Observation::renderer_azimuth_deg(azimuth as f32) as f64).abs()
            < 1.0e-4,
        "compass {azimuth:.4} -> renderer {renderer_azimuth:.4} must be A - 90 mod 360"
    );
    assert!(
        (renderer_elevation - altitude).abs() < 1.0e-6,
        "above the horizon the renderer elevation is the altitude"
    );

    // The rest of the ephemeris rides along, from SIDERA.
    let moon = report.get("moon").expect("the report carries the Moon");
    let illuminated = moon
        .get("illuminated_fraction")
        .and_then(|v| v.as_f64())
        .expect("Moon illuminated fraction");
    assert!(
        (0.0..=1.0).contains(&illuminated),
        "an illuminated fraction is a ratio, got {illuminated}"
    );
    let delta_t = report
        .get("delta_t_seconds")
        .and_then(|v| v.as_f64())
        .expect("Delta T");
    assert!(
        delta_t > 60.0 && delta_t < 120.0,
        "Delta T in the 2020s is roughly 69 s, got {delta_t}"
    );
    let sidereal = report
        .get("sidereal_time_deg")
        .and_then(|v| v.as_object())
        .expect("sidereal time");
    assert_eq!(sidereal.len(), 2, "mean and apparent sidereal time");
    assert_eq!(
        report.get("warnings").and_then(|v| v.as_array()),
        Some(&Vec::new()),
        "a daytime observation has nothing to warn about: {report:?}"
    );

    // Below the horizon the renderer elevation is clamped, and the report says
    // why instead of returning a sun under the ground.
    let night = call_ok(
        &engine,
        &events,
        "astro.observe",
        serde_json::json!({
            "datetime_utc": "2026-06-21T00:00:00Z",
            "latitude_deg": 0.0,
            "longitude_deg": 0.0,
        }),
        "astro.observe (midnight)",
    );
    let night_sun = night
        .get("bodies")
        .and_then(|bodies| bodies.get("sun"))
        .expect("night Sun");
    let night_altitude = night_sun
        .get("altitude_deg")
        .and_then(|v| v.as_f64())
        .expect("night altitude");
    assert!(
        night_altitude < 0.0,
        "midnight on the equator is a Sun below the horizon, got {night_altitude:.3}"
    );
    assert_eq!(
        night_sun.get("above_horizon").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        night_sun
            .get("renderer_elevation_deg")
            .and_then(|v| v.as_f64()),
        Some(0.0),
        "the clamped sun is what the renderer may use"
    );
    let warnings: Vec<String> = night
        .get("warnings")
        .and_then(|v| v.as_array())
        .expect("warnings list")
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("clamped") && warning.to_lowercase().contains("sun")),
        "a clamped sun must be stated, not silent: {warnings:?}"
    );

    // A malformed instant is refused with the shape Studio accepts.
    let message = call_err(
        &engine,
        &events,
        "astro.observe",
        serde_json::json!({ "datetime_utc": "yesterday" }),
        "invalid instant",
    );
    assert!(
        message.contains("ISO-8601"),
        "the refusal must name the accepted format: {message}"
    );

    // The app applies the report in the renderer frame: feeding those angles
    // to the sun must change what the terrain renders (it is a light, not a
    // label), and the observation is reproducible from the same inputs.
    let _view = load_dem(&engine, &events);
    let before = render(&engine, &events, 96, 64);
    engine.send(Command::SetSun(SunParams {
        azimuth_deg: renderer_azimuth as f32,
        elevation_deg: renderer_elevation as f32,
        intensity: 1.0,
    }));
    let after = render(&engine, &events, 96, 64);
    assert!(
        differing_pixels(&before.rgba, &after.rgba) > 0,
        "the observed Sun angles must move the light"
    );

    engine.shutdown();
}

/// Write an (z, y, x) float32 grid as a `.npy` NumPy v1.0 array — the density
/// format `smoke.load` documents.
fn write_npy(path: &Path, shape: [usize; 3], values: &[f32]) {
    assert_eq!(shape[0] * shape[1] * shape[2], values.len());
    let mut header = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, {}, {}), }}",
        shape[0], shape[1], shape[2]
    );
    // 10 bytes of preamble; the header is padded so the data starts on a
    // 64-byte boundary, exactly as numpy writes it.
    let unpadded = 10 + header.len() + 1;
    let pad = (64 - unpadded % 64) % 64;
    header.push_str(&" ".repeat(pad));
    header.push('\n');
    let mut bytes = Vec::with_capacity(10 + header.len() + values.len() * 4);
    bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write the smoke .npy fixture");
}

#[test]
fn smoke_volume_composites_with_the_backend_the_capabilities_name() {
    let (engine, events) = spawn_ready();
    let _view = load_dem(&engine, &events);

    let caps = call_ok(
        &engine,
        &events,
        "smoke.capabilities",
        serde_json::json!({}),
        "smoke.capabilities",
    );
    let expected_renderer = match caps.get("native_backend").and_then(|v| v.as_bool()) {
        Some(true) => "native",
        Some(false) => "cpu_fallback",
        None => panic!("capabilities must say whether a native backend exists: {caps:?}"),
    };
    assert_eq!(
        caps.get("renderer").and_then(|v| v.as_str()),
        Some(expected_renderer),
        "the renderer label must follow from the backend, not be a fixed string"
    );
    assert_eq!(
        caps.get("native_available").and_then(|v| v.as_bool()),
        Some(expected_renderer == "native"),
        "the two capability fields must agree"
    );
    let importers = caps
        .get("volume_importers")
        .and_then(|v| v.as_array())
        .expect("volume importers list");
    assert!(
        importers
            .iter()
            .filter_map(|v| v.as_str())
            .any(|name| name.starts_with(".npy")),
        "the panel advertises the formats the loader accepts: {importers:?}"
    );

    // A synthetic plume: an 8x6x5 grid with a lit block in the middle.
    let shape = [8usize, 6, 5];
    let mut density = vec![0.0f32; shape.iter().product()];
    for z in 2..6 {
        for y in 2..4 {
            for x in 1..4 {
                density[(z * shape[1] + y) * shape[2] + x] = 0.8;
            }
        }
    }
    let volume = scratch_dir().join("plume.npy");
    write_npy(&volume, shape, &density);

    let loaded = call_ok(
        &engine,
        &events,
        "smoke.load",
        serde_json::json!({ "path": volume }),
        "smoke.load",
    );
    assert_eq!(
        loaded.get("kind").and_then(|v| v.as_str()),
        Some("npy density")
    );
    let dims: Vec<u64> = loaded
        .get("dims")
        .and_then(|v| v.as_array())
        .expect("volume dims")
        .iter()
        .filter_map(|v| v.as_u64())
        .collect();
    assert_eq!(dims.len(), 3, "a voxel volume has three axes: {dims:?}");
    assert_eq!(
        dims.iter().product::<u64>(),
        shape.iter().map(|v| *v as u64).product::<u64>(),
        "the reported dims must describe the grid that was loaded"
    );
    let density_max = loaded
        .get("density_max")
        .and_then(|v| v.as_f64())
        .expect("density max");
    assert!(
        (density_max - 0.8).abs() < 1.0e-5,
        "the loaded field is the file's, got max {density_max}"
    );
    let active = loaded
        .get("active_fraction")
        .and_then(|v| v.as_f64())
        .expect("active fraction");
    assert!(
        active > 0.0 && active < 1.0,
        "the plume occupies part of the grid, got {active}"
    );
    assert_eq!(
        loaded.get("renderer").and_then(|v| v.as_str()),
        Some(expected_renderer)
    );
    assert_eq!(
        loaded.get("certificate_covers").and_then(|v| v.as_str()),
        Some("terrain pass only"),
        "the terrain certificate does not sign the smoke plate"
    );

    // The plate composites over a real rendered frame, at the frame's size.
    let frame = scratch_dir().join("m7-frame.png");
    render_to_file(&engine, &events, 96, 64, &frame);
    let blended = scratch_dir().join("m7-frame-smoke.png");
    let report = call_ok(
        &engine,
        &events,
        "smoke.composite",
        serde_json::json!({
            "frame_png": frame.to_string_lossy(),
            "out_png": blended.to_string_lossy(),
            "opacity": 1.0,
            "settings": SmokeParams::default().render_settings(),
            "sun": { "azimuth_deg": 315.0, "elevation_deg": 40.0 },
        }),
        "smoke.composite",
    );
    assert_eq!(report.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        report.get("renderer").and_then(|v| v.as_str()),
        Some(expected_renderer),
        "the composite reports the backend that ran it"
    );
    assert_eq!(report.get("width").and_then(|v| v.as_u64()), Some(96));
    assert_eq!(report.get("height").and_then(|v| v.as_u64()), Some(64));
    let coverage = report
        .get("alpha_coverage")
        .and_then(|v| v.as_f64())
        .expect("alpha coverage");
    assert!(
        coverage > 0.0 && coverage < 1.0,
        "the plate covers part of the frame, got {coverage}"
    );
    let plate_alpha_max = report
        .get("alpha_max")
        .and_then(|v| v.as_f64())
        .expect("alpha max");
    assert!(
        plate_alpha_max > 0.0,
        "a loaded plume must produce a visible plate"
    );
    let sun_direction: Vec<f64> = report
        .get("sun_direction")
        .and_then(|v| v.as_array())
        .expect("sun direction")
        .iter()
        .filter_map(|v| v.as_f64())
        .collect();
    let length = (sun_direction.iter().map(|v| v * v).sum::<f64>()).sqrt();
    assert!(
        (length - 1.0).abs() < 1.0e-5,
        "the plate is lit by a unit vector, got length {length}"
    );
    assert_eq!(
        report.get("certificate_covers").and_then(|v| v.as_str()),
        Some("terrain pass only")
    );
    let frame_bytes = std::fs::read(&frame).expect("frame png");
    let blended_bytes = std::fs::read(&blended).expect("composited png");
    assert_eq!(&blended_bytes[0..4], b"\x89PNG");
    assert_ne!(
        frame_bytes, blended_bytes,
        "the composited file must differ from the terrain frame"
    );

    // The worker composites on its own render path too, reporting the plate
    // with the frame instead of silently swapping pixels.
    engine.send(Command::SetSmoke {
        params: SmokeParams {
            enabled: true,
            path: Some(volume.clone()),
            ..SmokeParams::default()
        },
        opacity: 1.0,
    });
    let with_plate = render(&engine, &events, 96, 64);
    let smoke = with_plate
        .smoke
        .as_ref()
        .expect("a loaded, visible volume must report its composite");
    assert_eq!(smoke.get("status").and_then(|v| v.as_str()), Some("ok"));

    engine.send(Command::SetSmoke {
        params: SmokeParams {
            enabled: false,
            path: Some(volume.clone()),
            ..SmokeParams::default()
        },
        opacity: 1.0,
    });
    let without_plate = render(&engine, &events, 96, 64);
    assert!(
        without_plate.smoke.is_none(),
        "a hidden smoke layer composites nothing and reports nothing"
    );
    assert!(
        differing_pixels(&with_plate.rgba, &without_plate.rgba) > 0,
        "the composited plate must be visible in the shipped frame"
    );

    // A load that fails must not leave an older volume live: the panel would
    // show the new path while the frame kept the previous plume.
    let message = call_err(
        &engine,
        &events,
        "smoke.load",
        serde_json::json!({ "path": scratch_dir().join("missing.npy") }),
        "smoke.load (missing file)",
    );
    assert!(
        message.contains("not found"),
        "the refusal must name the missing volume: {message}"
    );
    let message = call_err(
        &engine,
        &events,
        "smoke.composite",
        serde_json::json!({
            "frame_png": frame.to_string_lossy(),
            "out_png": blended.to_string_lossy(),
        }),
        "smoke.composite after a failed load",
    );
    assert!(
        message.contains("no smoke volume loaded"),
        "a failed load must clear the previous volume: {message}"
    );

    engine.shutdown();
}

/// The quality settings a render uses are unrelated to M7, but the frame's
/// quality metadata must survive an atmosphere change unmixed — a still keeps
/// reporting the path that produced it.
#[test]
fn atmosphere_settings_do_not_relabel_a_quality_render() {
    let (engine, events) = spawn_ready();
    let _view = load_dem(&engine, &events);
    engine.send(Command::SetRenderQuality(RenderQuality {
        mode: studio_project::RenderMode::Raster,
        offline: studio_project::OfflineSettings {
            enabled: true,
            samples: 1,
            denoiser: "none".to_owned(),
        },
        path_trace: studio_project::PathTraceSettings::default(),
    }));
    let settings = atmosphere(true, 3.0, 1.0);
    engine.send(Command::SetAtmosphere(settings.clone()));
    engine.send(Command::RenderFrame {
        width: 96,
        height: 64,
        force: true,
    });
    let quality = wait_for(
        &events,
        Duration::from_secs(900),
        |e| match e {
            Event::FrameReady { quality, .. } => Some(quality.clone()),
            Event::Error { context, message } => {
                panic!("frame render failed: {context}: {message}")
            }
            _ => None,
        },
        "FrameReady",
    );
    let meta = quality.expect("offline accumulation reports its metadata");
    assert_eq!(
        meta.get("mode").and_then(|v| v.as_str()),
        Some("raster_offline"),
        "an atmosphere change must not change the reported quality path: {meta:?}"
    );
    assert_eq!(meta.get("samples_used").and_then(|v| v.as_u64()), Some(1));

    engine.shutdown();
}
