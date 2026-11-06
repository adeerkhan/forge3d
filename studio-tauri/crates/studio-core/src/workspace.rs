//! Workspace handoff: what a project can carry into the Forge3D workspace, and
//! what it cannot.
//!
//! This module is pure. It reads the canonical project and produces a
//! payload the Forge3D viewer receives, so the rules
//! are testable without Python, a GPU, or a display.
//!
//! The rules are grounded in the viewer's actual IPC surface
//! (`python/forge3d/viewer.py`): it loads one glTF/OBJ/terrain, moves a camera
//! (`cam_lookat`, `set_fov`), and drives a sun (`lit_sun`). It has no command
//! for Studio primitives, point lights, or per-object materials.

use serde::{Deserialize, Serialize};

use crate::ids::{AssetId, ProjectId, SceneId};
use crate::math::{Transform, Vec3};
use crate::project::{AssetKind, Project, Workspace};
use crate::scene::{LightKind, ObjectKind};

/// How completely one object survives the switch to the Forge3D workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLevel {
    /// Forge3D renders it as-is.
    Full,
    /// Forge3D shows part of it; the report says what is dropped.
    Partial,
    /// Forge3D cannot represent it at all.
    Unsupported,
}

impl CapabilityLevel {
    pub fn label(self) -> &'static str {
        match self {
            CapabilityLevel::Full => "supported",
            CapabilityLevel::Partial => "partial",
            CapabilityLevel::Unsupported => "unsupported",
        }
    }
}

/// Orbit camera for a terrain scene.
///
/// The viewer frames terrain with `set_terrain_camera(phi, theta, radius)` whose
/// radius is in the viewer's own terrain world units. Studio therefore transfers
/// only the *direction* (`phi`/`theta`) and the field of view; the viewer keeps
/// its radius and centre, which is what makes a 4 km DEM open framed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainCamera {
    /// Azimuth east → north in the engine's geographic frame (degrees).
    pub phi_deg: f32,
    /// Polar angle down from vertical (0 = straight above, 90 = horizon).
    pub theta_deg: f32,
    pub fov_deg: f32,
    /// Distance in the viewer's terrain world units (metres for georeferenced
    /// DEMs, pixels for local ones).
    pub radius: Option<f64>,
    /// Orbit centre in viewer world coordinates.
    pub target: Option<[f64; 3]>,
}

/// Studio's viewport camera in the Forge3D renderer's terms.
///
/// This is the transfer type between `src/viewport/viewMath.ts` (live viewport
/// renders) and [`compute_render_view`] (the Rust twin used by camera-view
/// stills and the animation export). Keeping it in the core means the shell
/// deserializes exactly what the core produces — there is no second definition
/// that can drift.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderView {
    /// Azimuth east → north in the renderer's geographic frame (degrees).
    pub phi_deg: f64,
    /// Polar angle down from vertical (0 = straight above, 90 = horizon).
    pub theta_deg: f64,
    /// Distance in the renderer's metres.
    pub radius: f64,
    /// Aim point in the renderer's mesh frame: metres, terrain centred on the
    /// origin, Z up.
    #[serde(default)]
    pub target: Option<[f64; 3]>,
    /// Vertical field of view in degrees.
    #[serde(default)]
    pub fov_deg: Option<f64>,
}

/// Inputs to the Studio-viewport → renderer camera mapping.
///
/// Mirrors `TerrainViewInput` in `src/viewport/viewMath.ts`; `camera` and
/// `aim` are in the viewport's units (Y up, north at −Z), the span and the
/// elevation range in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainView {
    pub camera: [f64; 3],
    pub aim: [f64; 3],
    pub fov_deg: f64,
    /// DEM footprint in metres, or `None` when it is not georeferenced.
    pub span_xz: Option<[f64; 2]>,
    /// DEM pixel dimensions — the span fallback for local DEMs.
    pub dimensions: Option<[u32; 2]>,
    /// Elevation range `[min, max]` in metres.
    pub elevation_range: Option<[f32; 2]>,
    pub z_scale: f32,
}

/// Map a Studio viewport camera onto the renderer's frame.
///
/// This is the Rust twin of `computeRenderView` in `viewMath.ts` and must
/// return the same numbers: the viewport east = +X, north = −Z, up = +Y, while
/// the renderer is +X east, +Y north, +Z up with the terrain centred on the
/// origin. Horizontal and vertical deltas both scale by
/// `maxSpan / SCENE_TERRAIN_SIZE`; the vertical datum shifts because the
/// viewport terrain sits on its base while the renderer centres its relief on
/// zero. Returns `None` when there is no terrain footprint to map onto or the
/// camera sits on the aim point.
pub fn compute_render_view(input: &TerrainView) -> Option<RenderView> {
    let span = input
        .span_xz
        .or_else(|| input.dimensions.map(|[w, h]| [w as f64, h as f64]))?;
    let max_span = span[0].max(span[1]);
    if !max_span.is_finite() || max_span <= 0.0 {
        return None;
    }
    let scale = max_span / f64::from(crate::project::SCENE_TERRAIN_SIZE);
    let [low, high] = input.elevation_range.unwrap_or([0.0, 0.0]);
    let half_relief = (f64::from(high) - f64::from(low)) * f64::from(input.z_scale) / 2.0;

    let d_east = (input.camera[0] - input.aim[0]) * scale;
    let d_north = -(input.camera[2] - input.aim[2]) * scale;
    let d_up = (input.camera[1] - input.aim[1]) * scale;
    let radius = glam::DVec3::new(d_east, d_north, d_up).length();
    if !radius.is_finite() || radius <= 1e-6 {
        return None;
    }

    let mut phi_deg = d_north.atan2(d_east).to_degrees();
    if phi_deg < 0.0 {
        phi_deg += 360.0;
    }
    let theta_deg = (d_up / radius).clamp(-1.0, 1.0).acos().to_degrees();
    Some(RenderView {
        phi_deg,
        theta_deg,
        radius,
        target: Some([
            input.aim[0] * scale,
            -input.aim[2] * scale,
            input.aim[1] * scale - half_relief,
        ]),
        fov_deg: Some(input.fov_deg),
    })
}

/// The project's active camera as the renderer's orbit view.
///
/// Aiming ten units ahead mirrors the viewport's camera
/// view, where the view axis (not the orbit target) frames the picture. That
/// makes an exported animation frame reproduce a camera-view still of the same
/// object instead of the terrain's canned framing.
pub fn camera_render_view(project: &Project) -> Option<RenderView> {
    let terrain = project.terrain_object()?.terrain()?;
    let camera = project.scene.camera()?;
    let ObjectKind::Camera(camera_object) = &camera.kind else {
        return None;
    };
    let eye = camera.transform.translation;
    let forward = camera.transform.forward();
    compute_render_view(&TerrainView {
        camera: [f64::from(eye.x), f64::from(eye.y), f64::from(eye.z)],
        aim: [
            f64::from(eye.x + forward.x * 10.0),
            f64::from(eye.y + forward.y * 10.0),
            f64::from(eye.z + forward.z * 10.0),
        ],
        fov_deg: f64::from(camera_object.fov_deg),
        span_xz: terrain.world_span_xz,
        dimensions: terrain.dimensions,
        elevation_range: terrain.elevation_range,
        z_scale: terrain.z_scale,
    })
}

/// A DEM handed to the viewer's terrain pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainHandoff {
    pub asset_id: AssetId,
    pub name: String,
    pub path: String,
    pub z_scale: f32,
    /// DEM pixel size, when known (diagnostics only).
    pub dimensions: Option<[u32; 2]>,
    /// Viewer-basis footprint in metres, when georeferenced.
    pub world_span_xz: Option<[f64; 2]>,
    /// Elevation range in metres, `[min, max]`.
    pub elevation_range: Option<[f32; 2]>,
    /// Draped raster overlay, if any.
    pub overlay: Option<TerrainOverlayHandoff>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainOverlayHandoff {
    pub path: String,
    pub name: String,
    pub opacity: f32,
    pub preserve_colors: bool,
}

/// Sun direction in the viewer's terms (`lit_sun`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SunAngles {
    /// Degrees from east toward south (the engine's terrain world, +Y south),
    /// [0, 360).
    pub azimuth_deg: f32,
    /// Degrees above the horizon, [-90, 90].
    pub elevation_deg: f32,
    pub intensity: f32,
    /// Linear RGB colour of the directional light — the renders read it, so a
    /// colour edit must not diverge between the Solid view and the renderer.
    pub color: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeAsset {
    pub id: AssetId,
    pub name: String,
    pub path: String,
    pub kind: AssetKind,
}

/// Everything the Forge3D workspace needs for one handoff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub project_id: ProjectId,
    pub scene_id: SceneId,
    pub revision: u64,
    pub target: Workspace,
    /// Stable for the same (project, scene, revision, target): reapplying a
    /// handoff is a no-op rather than a second viewer window.
    pub idempotency_key: String,
    /// Orbit camera for terrain scenes (`set_terrain_camera`).
    pub terrain_camera: Option<TerrainCamera>,
    /// The DEM the viewer should load, when the scene has terrain.
    pub terrain: Option<TerrainHandoff>,
    pub sun: Option<SunAngles>,
    /// HDRI environment for image-based lighting, when the project sets one.
    /// Both the Material preview and Rendered frames consume it.
    pub environment: Option<ForgeAsset>,
}

/// One material/light/camera parameter and what currently drives it: the
/// per-parameter half of the capability story, so the Inspector can show
/// parity limits next to the field they constrain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterCapability {
    pub parameter: String,
    pub level: CapabilityLevel,
    pub detail: String,
}

/// What every editable rendering parameter actually reaches today.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityTable {
    pub light: Vec<ParameterCapability>,
    /// Image-based lighting (the HDRI environment).
    pub environment: Vec<ParameterCapability>,
    pub mesh_material: Vec<ParameterCapability>,
    pub terrain_material: Vec<ParameterCapability>,
    pub camera: Vec<ParameterCapability>,
}

/// The parameter-level capability table (single source of truth; the
/// Inspector fetches it through the shell).
pub fn parameter_capabilities() -> CapabilityTable {
    let parameter = |parameter: &str, level: CapabilityLevel, detail: &str| ParameterCapability {
        parameter: parameter.to_owned(),
        level,
        detail: detail.to_owned(),
    };
    CapabilityTable {
        light: vec![
            parameter(
                "direction",
                CapabilityLevel::Full,
                "Solid, Material, Rendered, and stills all read the scene's directional sun.",
            ),
            parameter(
                "intensity",
                CapabilityLevel::Full,
                "Sun intensity flows into every render; re-renders follow the edit.",
            ),
            parameter(
                "colour",
                CapabilityLevel::Full,
                "The light colour reaches the renderer's direct lighting.",
            ),
            parameter(
                "point type",
                CapabilityLevel::Unsupported,
                "Neither view implements point lights — the option is disabled in the inspector.",
            ),
        ],
        environment: vec![parameter(
            "HDRI environment",
            CapabilityLevel::Full,
            "The Material preview and Rendered frames both light with this .hdr and draw it as the sky: the viewport pre-filters it (PMREM), the engine uploads it as IBL and samples it as the background. Without one both use the built-in studio sky.",
        )],
        mesh_material: vec![
            parameter(
                "base colour",
                CapabilityLevel::Partial,
                "Solid and the Material preview — meshes do not reach Rendered yet (geometry export pending).",
            ),
            parameter(
                "roughness",
                CapabilityLevel::Partial,
                "Solid and the Material preview — meshes do not reach Rendered yet (geometry export pending).",
            ),
            parameter(
                "metallic",
                CapabilityLevel::Partial,
                "Solid and the Material preview — meshes do not reach Rendered yet (geometry export pending).",
            ),
            parameter(
                "texture",
                CapabilityLevel::Partial,
                "Solid and the Material preview — meshes do not reach Rendered yet (geometry export pending).",
            ),
        ],
        terrain_material: vec![
            parameter(
                "albedo raster (overlay)",
                CapabilityLevel::Full,
                "Draped in the Material preview and in Rendered frames; composited after lighting so categorical palettes survive. Solid is a neutral clay view.",
            ),
            parameter(
                "engine material set",
                CapabilityLevel::Partial,
                "Material and Rendered shade with terrain_default's four layers; the Material preview drapes the albedo over a neutral clay surface.",
            ),
            parameter(
                "texture maps",
                CapabilityLevel::Unsupported,
                "None ship in this checkout: the engine resolves rock/grass/dirt/snow from assets/materials/ or FORGE3D_MATERIAL_DIR, so the layers render flat colours until maps appear.",
            ),
        ],
        camera: vec![
            parameter(
                "field of view",
                CapabilityLevel::Full,
                "Solid and every render frame use the active camera's FOV.",
            ),
            parameter(
                "near / far",
                CapabilityLevel::Partial,
                "Solid projection only — Rendered derives its clip planes from the frame.",
            ),
        ],
    }
}
/// Rotation for the sun light from the engine ephemeris direction.
///
/// `scene_direction` is the direction **toward** the sun in MapScene's frame
/// (+X east, +Y up, +Z north), as returned by `SunPosition.to_scene_direction`.
/// Studio's Solid frame is +X east, +Y up, +Z south, so north flips; the light
/// itself travels the opposite way.
pub fn sun_rotation(scene_direction: [f32; 3]) -> Vec3 {
    let toward_sun = Vec3::new(scene_direction[0], scene_direction[1], -scene_direction[2]);
    Transform::rotation_looking_along(Vec3::new(-toward_sun.x, -toward_sun.y, -toward_sun.z))
}

/// The scene's directional sun in viewer terms.
pub fn sun_angles(project: &Project) -> Option<SunAngles> {
    let light = project
        .scene
        .objects
        .iter()
        .find(|object| matches!(&object.kind, ObjectKind::Light(light) if light.kind == LightKind::Directional))?;
    let ObjectKind::Light(light) = &light.kind else {
        return None;
    };
    // The object's local -Z is where the light travels, so the sun sits the
    // opposite way: rotation (-50°, 30°, 0°) puts it 50° above the horizon.
    let forward = light_direction(project, light.kind);
    let toward_sun = Vec3::new(-forward.x, -forward.y, -forward.z);
    let elevation_deg = toward_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
    // The engine decodes `light_azimuth_deg` as
    // direction = (cos el · cos az, cos el · sin az, sin el) in the terrain
    // world (+X east, +Y SOUTH for mesh geometry, +Z up) — azimuth runs east
    // → south. The Solid view is Y-up with north at -Z, so its +Z axis is
    // south: atan2(z, x) in Solid axes IS that angle. The old atan2(x, z)
    // rotated the rendered sun 90° away from the Solid one (the starter light
    // handed the engine 30° instead of 60°, i.e. northeast instead of
    // southeast).
    let mut azimuth_deg = toward_sun.z.atan2(toward_sun.x).to_degrees();
    if azimuth_deg < 0.0 {
        azimuth_deg += 360.0;
    }
    Some(SunAngles {
        azimuth_deg,
        elevation_deg,
        intensity: light.intensity,
        color: light.color,
    })
}

fn light_direction(project: &Project, kind: LightKind) -> Vec3 {
    let object = project
        .scene
        .objects
        .iter()
        .find(|object| matches!(&object.kind, ObjectKind::Light(light) if light.kind == kind))
        .expect("caller checked the light exists");
    object.transform.forward()
}

/// Build the handoff for one revision of the project.
pub fn handoff(project: &Project, revision: u64, target: Workspace) -> Handoff {
    Handoff {
        project_id: project.id,
        scene_id: project.scene.id,
        revision,
        target,
        idempotency_key: format!(
            "{}:{}:{revision}:{}",
            project.id,
            project.scene.id,
            workspace_slug(target)
        ),
        terrain_camera: terrain_camera(project),
        terrain: terrain_handoff(project),
        sun: sun_angles(project),
        environment: environment_handoff(project),
    }
}

/// The project's HDRI environment in renderer terms, when set.
///
/// The path is what both consumers need: the viewport reads the bytes for its
/// PMREM, and the engine hands it to `IBL.from_hdr`. Keeping the lookup here
/// means a missing/renamed asset can never be silently ignored by one of them.
pub fn environment_handoff(project: &Project) -> Option<ForgeAsset> {
    let asset = project.asset(project.render.environment?)?;
    Some(ForgeAsset {
        id: asset.id,
        name: asset.name.clone(),
        path: asset.path.clone(),
        kind: asset.kind,
    })
}

/// The scene's DEM in viewer terms.
pub fn terrain_handoff(project: &Project) -> Option<TerrainHandoff> {
    let object = project.terrain_object()?;
    let terrain = object.terrain()?;
    let asset = project.asset(terrain.asset)?;
    Some(TerrainHandoff {
        asset_id: asset.id,
        name: asset.name.clone(),
        path: asset.path.clone(),
        z_scale: terrain.z_scale,
        dimensions: terrain.dimensions,
        world_span_xz: terrain.world_span_xz,
        elevation_range: terrain.elevation_range,
        overlay: terrain.overlay.and_then(|overlay| {
            project
                .asset(overlay.asset)
                .map(|asset| TerrainOverlayHandoff {
                    path: asset.path.clone(),
                    name: asset.name.clone(),
                    opacity: overlay.opacity,
                    preserve_colors: overlay.preserve_colors,
                })
        }),
    })
}

/// Studio's camera expressed as the viewer's terrain orbit.
pub fn terrain_camera(project: &Project) -> Option<TerrainCamera> {
    let terrain = project.terrain_object()?;
    let camera = project.scene.camera()?;
    let ObjectKind::Camera(camera_object) = &camera.kind else {
        return None;
    };

    // The viewer places the eye at target + r * (sinθcosφ, cosθ, sinθsinφ).
    let offset = Vec3::new(
        camera.transform.translation.x - terrain.transform.translation.x,
        camera.transform.translation.y - terrain.transform.translation.y,
        camera.transform.translation.z - terrain.transform.translation.z,
    );
    let length = offset.geometry().length();
    if length < 1e-3 {
        return None;
    }
    let up = offset.y / length;
    let theta_deg = up.clamp(-1.0, 1.0).acos().to_degrees();
    // The engine's north-mode orbit reads phi in the geographic frame
    // (+X east, +Y north, +Z up): phi runs east → north, while the Solid
    // view's +Z axis is south — so the north component is -z. The old
    // atan2(z, x) mirrored the fallback azimuth by ~100° for the starter
    // scene and disagreed with the viewport's live view, which already
    // maps -z → north.
    let mut phi_deg = (-offset.z).atan2(offset.x).to_degrees();
    if phi_deg < 0.0 {
        phi_deg += 360.0;
    }
    let framing = terrain
        .terrain()
        .and_then(|terrain| terrain.viewer_framing());
    Some(TerrainCamera {
        phi_deg,
        theta_deg,
        fov_deg: camera_object.fov_deg,
        radius: framing.map(|(radius, _)| radius),
        target: framing.map(|(_, target)| target),
    })
}

fn workspace_slug(workspace: Workspace) -> &'static str {
    match workspace {
        Workspace::Studio => "studio",
        Workspace::Forge3d => "forge3d",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObjectId;

    #[test]
    fn sun_angles_match_the_light_rotation() {
        let project = Project::new("Sun");
        let sun = sun_angles(&project).expect("starter scene has a sun");
        // Rotation (-50°, 30°, 0°): 50° above the horizon; the horizontal
        // toward-sun component is east 0.321, south 0.557, and the engine's
        // azimuth (east → south) is atan2(0.557, 0.321) = 60°. The old
        // atan2(east, south) = 30° put the rendered sun 90° off the Solid one.
        assert!((sun.elevation_deg - 50.0).abs() < 0.5, "{sun:?}");
        assert!((sun.azimuth_deg - 60.0).abs() < 0.5, "{sun:?}");
        assert!((sun.intensity - 3.0).abs() < 1e-4);
        // The renderer receives the light's colour too — Solid and Rendered
        // must agree on it.
        assert_eq!(sun.color, [1.0, 0.97, 0.92], "{sun:?}");
    }

    fn sun_object_id(project: &Project) -> ObjectId {
        project
            .scene
            .objects
            .iter()
            .find(|object| {
                matches!(&object.kind, ObjectKind::Light(light) if light.kind == LightKind::Directional)
            })
            .expect("starter scene has a sun")
            .id
    }

    #[test]
    fn sun_rotation_round_trips_the_light_angles() {
        let mut project = Project::new("Solar");
        let sun_id = sun_object_id(&project);
        for (azimuth_deg, elevation_deg) in [
            (0.0f32, 10.0f32),
            (60.0, 50.0),
            (107.2, 66.08),
            (200.0, 5.0),
            (330.0, -10.0),
        ] {
            let azimuth = azimuth_deg.to_radians();
            let elevation = elevation_deg.to_radians();
            // Solid axes: x east, y up, z south.
            let toward_sun = Vec3::new(
                elevation.cos() * azimuth.cos(),
                elevation.sin(),
                elevation.cos() * azimuth.sin(),
            );
            // The engine's scene direction is (east, up, north).
            let scene = [toward_sun.x, toward_sun.y, -toward_sun.z];
            let rotation = sun_rotation(scene);
            project
                .scene
                .object_mut(sun_id)
                .unwrap()
                .transform
                .rotation_deg = rotation;

            let angles = sun_angles(&project).expect("sun still exists");
            let azimuth_delta = ((angles.azimuth_deg - azimuth_deg + 540.0) % 360.0) - 180.0;
            assert!(
                azimuth_delta.abs() < 0.5,
                "azimuth {azimuth_deg} -> {} via {rotation:?}",
                angles.azimuth_deg
            );
            assert!(
                (angles.elevation_deg - elevation_deg).abs() < 0.5,
                "elevation {elevation_deg} -> {} via {rotation:?}",
                angles.elevation_deg
            );
        }
    }

    #[test]
    fn sun_rotation_matches_a_known_ephemeris_vector() {
        // forge3d.sun_position(46.6, 7.9, "2026-06-21T12:00:00Z")
        //   .to_scene_direction()  (x east, y up, z north)
        // Sun in the south at local noon, elevation 66.08°.
        let scene = [-0.118_866_32, 0.914_083_86, -0.387_713_16];
        let mut project = Project::new("Solar");
        let sun_id = sun_object_id(&project);
        project
            .scene
            .object_mut(sun_id)
            .unwrap()
            .transform
            .rotation_deg = sun_rotation(scene);

        let angles = sun_angles(&project).expect("sun still exists");
        // Engine azimuth is east -> south: north-based 197.04 becomes 107.04.
        assert!(
            (angles.azimuth_deg - 107.16).abs() < 0.5,
            "azimuth {}",
            angles.azimuth_deg
        );
        assert!(
            (angles.elevation_deg - 66.08).abs() < 0.5,
            "elevation {}",
            angles.elevation_deg
        );
    }

    #[test]
    fn handoff_sun_angles_rebuild_the_solid_view_sun_vector() {
        // The decode contract: the engine turns (azimuth, elevation) into
        // direction = (cos el · cos az, cos el · sin az, sin el) in the terrain
        // world (+X east, +Y south, +Z up). The Solid view's toward-sun vector
        // is (east, up, south); after the axis transposition the handoff's
        // numbers must reproduce it exactly — this is the regression that let
        // the rendered sun sit 90° away from the Solid one.
        let project = Project::new("Sun");
        let sun = sun_angles(&project).expect("starter scene has a sun");
        let (az, el) = (sun.azimuth_deg.to_radians(), sun.elevation_deg.to_radians());
        let engine = [
            el.cos() * az.cos(), // east
            el.cos() * az.sin(), // south (world +Y)
            el.sin(),            // up
        ];

        let light = project
            .scene
            .objects
            .iter()
            .find(|object| {
                matches!(&object.kind, ObjectKind::Light(light) if light.kind == LightKind::Directional)
            })
            .expect("starter scene has a directional light");
        let forward = light.transform.forward();
        // Solid toward-sun = -forward, axes (east, up, south) → engine axes.
        let expected = [-forward.x, -forward.z, -forward.y];
        for (index, (got, want)) in engine.iter().zip(expected).enumerate() {
            assert!(
                (got - want).abs() < 1e-3,
                "axis {index}: engine {got} vs solid-mapped {want}"
            );
        }
    }

    #[test]
    fn parameter_capabilities_pin_parity_limits() {
        let table = parameter_capabilities();
        // Direction, intensity, and colour transfer; point lights don't exist
        // in either view.
        assert_eq!(table.light.len(), 4);
        let point = table
            .light
            .iter()
            .find(|row| row.parameter == "point type")
            .expect("point-type row");
        assert_eq!(point.level, CapabilityLevel::Unsupported);
        for row in table
            .light
            .iter()
            .filter(|row| row.parameter != "point type")
        {
            assert_eq!(row.level, CapabilityLevel::Full, "{}", row.parameter);
        }
        // Mesh materials show in Solid/Clay and in the Material preview, but
        // geometry still does not reach Rendered.
        assert!(!table.mesh_material.is_empty());
        assert!(table
            .mesh_material
            .iter()
            .all(|row| row.level == CapabilityLevel::Partial));
        // The terrain drape reaches the Material preview and Rendered.
        let drape = table
            .terrain_material
            .iter()
            .find(|row| row.parameter.starts_with("albedo"))
            .expect("albedo row");
        assert_eq!(drape.level, CapabilityLevel::Full);
        // Camera near/far stay Solid-only: Rendered derives its clip planes
        // from the frame.
        let clip = table
            .camera
            .iter()
            .find(|row| row.parameter.contains("near"))
            .expect("near/far row");
        assert_eq!(clip.level, CapabilityLevel::Partial);
        // Every row explains itself — the Inspector badges show it on hover.
        for row in table
            .light
            .iter()
            .chain(&table.environment)
            .chain(&table.mesh_material)
            .chain(&table.terrain_material)
            .chain(&table.camera)
        {
            assert!(!row.detail.trim().is_empty(), "{}", row.parameter);
        }
    }

    #[test]
    fn compute_render_view_matches_the_viewport_golden_vector() {
        // The starter scene's camera, exactly as `viewMath.test.ts` pins it.
        // Both implementations must agree, or Rendered frames and exported
        // frames stop matching the viewport.
        let view = compute_render_view(&TerrainView {
            camera: [11.0, 7.5, 13.0],
            aim: [0.0, 0.8, 0.0],
            fov_deg: 45.0,
            span_xz: Some([4000.0, 3800.0]),
            dimensions: None,
            elevation_range: Some([2099.23, 2553.58]),
            z_scale: 1.0,
        })
        .expect("the starter view maps");
        assert!((view.phi_deg - 310.21).abs() < 0.05, "{view:?}");
        assert!((view.theta_deg - 68.52).abs() < 0.05, "{view:?}");
        assert!((view.radius - 6100.0).abs() < 0.5, "{view:?}");
        let target = view.target.expect("target is always present");
        assert!(target[0].abs() < 1e-9, "{target:?}");
        assert!(target[1].abs() < 1e-9, "{target:?}");
        assert!((target[2] - 39.49).abs() < 0.01, "{target:?}");
        assert_eq!(view.fov_deg, Some(45.0));
    }

    #[test]
    fn compute_render_view_uses_the_viewport_axis_conventions() {
        let square = TerrainView {
            camera: [10.0, 5.0, 0.0],
            aim: [0.0, 5.0, 0.0],
            fov_deg: 40.0,
            span_xz: Some([1200.0, 1200.0]),
            dimensions: None,
            elevation_range: Some([0.0, 100.0]),
            z_scale: 1.0,
        };

        // Camera east of the aim: phi 0; level camera: theta 90; the scale is
        // maxSpan / SCENE_TERRAIN_SIZE, so 10 viewport units are 1000 m.
        let east = compute_render_view(&square).unwrap();
        assert!(east.phi_deg.abs() < 1e-9, "{east:?}");
        assert!((east.theta_deg - 90.0).abs() < 1e-9, "{east:?}");
        assert!((east.radius - 1000.0).abs() < 1e-9, "{east:?}");
        // The renderer centres relief on zero: 500 m aim − half relief.
        assert!((east.target.unwrap()[2] - 450.0).abs() < 1e-9);

        // +Z is south in the viewport, so a camera at −Z stands north: phi 90.
        let north = compute_render_view(&TerrainView {
            camera: [0.0, 5.0, -10.0],
            ..square
        })
        .unwrap();
        assert!((north.phi_deg - 90.0).abs() < 1e-9, "{north:?}");

        let top = compute_render_view(&TerrainView {
            camera: [0.0, 20.0, 0.0],
            ..square
        })
        .unwrap();
        assert!(top.theta_deg.abs() < 1e-9, "{top:?}");

        let stretched = compute_render_view(&TerrainView {
            z_scale: 2.0,
            ..square
        })
        .unwrap();
        assert!((stretched.target.unwrap()[2] - 400.0).abs() < 1e-9);

        let no_range = compute_render_view(&TerrainView {
            elevation_range: None,
            ..square
        })
        .unwrap();
        assert!((no_range.target.unwrap()[2] - 500.0).abs() < 1e-9);

        // A local DEM falls back to its pixel dimensions.
        let local = compute_render_view(&TerrainView {
            span_xz: None,
            dimensions: Some([1024, 1024]),
            camera: [1.0, 0.0, 0.0],
            aim: [0.0, 0.0, 0.0],
            ..square
        })
        .unwrap();
        assert!((local.radius - 1024.0 / 12.0).abs() < 1e-9, "{local:?}");

        // Guards mirror the TypeScript.
        assert!(compute_render_view(&TerrainView {
            camera: [0.0, 5.0, 0.0],
            ..square
        })
        .is_none());
        assert!(compute_render_view(&TerrainView {
            span_xz: None,
            ..square
        })
        .is_none());
        assert!(compute_render_view(&TerrainView {
            span_xz: Some([0.0, 0.0]),
            ..square
        })
        .is_none());
    }

    fn terrain_project() -> Project {
        Project::starter(
            "Camera view",
            Some(crate::project::TerrainSeed {
                title: Some("Bryce Canyon".to_owned()),
                name: "Bryce_Canyon_1024.tif".to_owned(),
                path: "/data/Bryce_Canyon_1024.tif".to_owned(),
                byte_size: 3_321_356,
                dimensions: Some([1024, 1024]),
                elevation_range: Some([2099.23, 2553.58]),
                world_origin_xz: Some([395_998.5, -4_165_574.5]),
                world_span_xz: Some([4000.0, 3800.0]),
                overlay: None,
            }),
        )
    }

    #[test]
    fn camera_render_view_follows_the_camera_object() {
        let mut project = terrain_project();
        let camera_id = project.scene.camera().unwrap().id;
        let view = camera_render_view(&project).expect("terrain + camera");
        // Aim is ten viewport units ahead, so the radius is exactly ten units
        // of the mapping (10 * 4000/12 m) — the camera's distance is honoured,
        // which `terrain_camera`'s viewer framing ignored.
        assert!(
            (view.radius - 10.0 * 4000.0 / 12.0).abs() < 1e-3,
            "{view:?}"
        );
        let fov = project.scene.camera().unwrap();
        let ObjectKind::Camera(camera) = &fov.kind else {
            unreachable!()
        };
        assert_eq!(view.fov_deg, Some(f64::from(camera.fov_deg)));

        // Rotating the camera turns the aim, which moves the orbit angle and
        // the target — a rotation-only animation must reach the renderer.
        let before = view.target.unwrap();
        project
            .scene
            .object_mut(camera_id)
            .unwrap()
            .transform
            .rotation_deg
            .y += 90.0;
        let rotated = camera_render_view(&project).expect("still a valid view");
        let after = rotated.target.unwrap();
        assert!(
            (after[0] - before[0]).abs() > 1.0 || (after[1] - before[1]).abs() > 1.0,
            "camera rotation must move the aim: {before:?} vs {after:?}"
        );
        assert!(
            (rotated.phi_deg - view.phi_deg).abs() > 1.0,
            "camera rotation must move the orbit azimuth"
        );
    }

    #[test]
    fn handoff_is_idempotent_for_a_revision() {
        let project = Project::new("Handoff");
        let first = handoff(&project, 7, Workspace::Forge3d);
        let second = handoff(&project, 7, Workspace::Forge3d);
        assert_eq!(first.idempotency_key, second.idempotency_key);
        assert_eq!(first, second);

        let next = handoff(&project, 8, Workspace::Forge3d);
        assert_ne!(first.idempotency_key, next.idempotency_key);
    }
}
