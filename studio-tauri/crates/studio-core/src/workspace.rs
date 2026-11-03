//! Workspace handoff: what a project can carry into the Forge3D workspace, and
//! what it cannot.
//!
//! This module is pure. It reads the canonical project and produces a
//! capability report plus the payload the Forge3D viewer receives, so the rules
//! are testable without Python, a GPU, or a display.
//!
//! The rules are grounded in the viewer's actual IPC surface
//! (`python/forge3d/viewer.py`): it loads one glTF/OBJ/terrain, moves a camera
//! (`cam_lookat`, `set_fov`), and drives a sun (`lit_sun`). It has no command
//! for Studio primitives, point lights, or per-object materials.

use serde::{Deserialize, Serialize};

use crate::ids::{AssetId, ObjectId, ProjectId, SceneId};
use crate::math::Vec3;
use crate::project::{Asset, AssetKind, Project, Workspace};
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectCapability {
    pub id: ObjectId,
    pub name: String,
    pub kind: String,
    pub level: CapabilityLevel,
    /// Why — always present, so the user is never told "unsupported" without a
    /// reason they could act on.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport {
    pub target: Workspace,
    pub objects: Vec<ObjectCapability>,
    /// Scene-level notes that are not tied to one object.
    pub degradations: Vec<String>,
    /// Whether the viewer can receive the camera (`cam_lookat` + `set_fov`).
    pub supports_camera_sync: bool,
    /// Whether at least one object can be rendered by the target.
    pub has_renderable_content: bool,
}

impl CapabilityReport {
    pub fn count(&self, level: CapabilityLevel) -> usize {
        self.objects
            .iter()
            .filter(|object| object.level == level)
            .count()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} supported, {} partial, {} unsupported",
            self.count(CapabilityLevel::Full),
            self.count(CapabilityLevel::Partial),
            self.count(CapabilityLevel::Unsupported)
        )
    }

    /// True when the switch would show nothing at all — the caller should warn
    /// before opening an empty viewer window.
    pub fn is_empty_for_target(&self) -> bool {
        !self.has_renderable_content
    }
}

/// Camera state handed to the viewer (`cam_lookat` + `set_fov`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraSync {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_deg: f32,
}

/// Sun direction in the viewer's terms (`lit_sun`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SunAngles {
    /// Degrees clockwise from +Z, [0, 360).
    pub azimuth_deg: f32,
    /// Degrees above the horizon, [-90, 90].
    pub elevation_deg: f32,
    pub intensity: f32,
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
    pub report: CapabilityReport,
    pub camera: Option<CameraSync>,
    pub assets: Vec<ForgeAsset>,
    pub sun: Option<SunAngles>,
}

/// Compute the report for a target workspace.
pub fn capability_report(project: &Project, target: Workspace) -> CapabilityReport {
    let mut objects = Vec::with_capacity(project.scene.objects.len());
    let mut degradations = Vec::new();
    let mut primitives = 0usize;
    let mut point_lights = 0usize;

    for object in &project.scene.objects {
        let (level, detail) = match (&object.kind, target) {
            (_, Workspace::Studio) => (
                CapabilityLevel::Full,
                "Studio renders this object in the solid viewport.".to_owned(),
            ),

            (ObjectKind::Mesh(mesh), Workspace::Forge3d) => match &mesh.source {
                crate::scene::MeshSource::Gltf { asset, .. } => {
                    if project.asset(*asset).is_some() {
                        (
                            CapabilityLevel::Full,
                            "The viewer loads this glTF/GLB asset directly.".to_owned(),
                        )
                    } else {
                        (
                            CapabilityLevel::Unsupported,
                            "The referenced asset is missing from the project.".to_owned(),
                        )
                    }
                }
                crate::scene::MeshSource::Primitive { .. } => {
                    primitives += 1;
                    (
                        CapabilityLevel::Partial,
                        "Primitive geometry is not exported yet, so this object will be \
                         absent from the Forge3D window (Phase 4 adds export)."
                            .to_owned(),
                    )
                }
            },

            (ObjectKind::Camera(camera), Workspace::Forge3d) => (
                CapabilityLevel::Full,
                format!(
                    "Position, orientation, and {:.0}° field of view transfer to the viewer.",
                    camera.fov_deg
                ),
            ),

            (ObjectKind::Light(light), Workspace::Forge3d) => match light.kind {
                LightKind::Directional => (
                    CapabilityLevel::Partial,
                    "Sun azimuth and elevation transfer; shadows and colour are not exposed \
                     by the viewer IPC."
                        .to_owned(),
                ),
                LightKind::Point => {
                    point_lights += 1;
                    (
                        CapabilityLevel::Unsupported,
                        "The viewer has no point-light command; this light will not exist \
                         in Forge3D."
                            .to_owned(),
                    )
                }
            },
        };

        objects.push(ObjectCapability {
            id: object.id,
            name: object.name.clone(),
            kind: object.kind_label().to_owned(),
            level,
            detail,
        });
    }

    if target == Workspace::Forge3d {
        if primitives > 0 {
            degradations.push(format!(
                "{primitives} primitive object{} will not appear in Forge3D until geometry \
                 export lands in Phase 4.",
                if primitives == 1 { "" } else { "s" }
            ));
        }
        if point_lights > 0 {
            degradations.push(format!(
                "{point_lights} point light{} cannot be transferred.",
                if point_lights == 1 { "" } else { "s" }
            ));
        }
        let gltf_assets = project
            .assets
            .iter()
            .filter(|asset| {
                project.scene.objects.iter().any(|object| {
                    object.mesh().and_then(crate::scene::MeshObject::asset_id) == Some(asset.id)
                })
            })
            .count();
        if gltf_assets > 1 {
            degradations.push(format!(
                "The viewer loads one asset per window; {gltf_assets} are referenced, so only \
                 the first will be opened. Multi-asset scenes arrive with Phase 4's bundle path."
            ));
        }
    }

    let has_renderable_content = objects
        .iter()
        .any(|object| object.level != CapabilityLevel::Unsupported);

    CapabilityReport {
        target,
        objects,
        degradations,
        supports_camera_sync: camera_sync(project).is_some(),
        has_renderable_content,
    }
}

/// The project's active camera in viewer terms.
pub fn camera_sync(project: &Project) -> Option<CameraSync> {
    let object = project.scene.camera()?;
    let ObjectKind::Camera(camera) = &object.kind else {
        return None;
    };
    let transform = object.transform;
    let forward = transform.forward();
    Some(CameraSync {
        eye: transform.translation,
        // The viewer takes a look-at target rather than a rotation; ten units
        // ahead is far enough to be stable and short enough to stay in range.
        target: Vec3::new(
            transform.translation.x + forward.x * 10.0,
            transform.translation.y + forward.y * 10.0,
            transform.translation.z + forward.z * 10.0,
        ),
        up: transform.up(),
        fov_deg: camera.fov_deg,
    })
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
    // opposite way. The model's default (-50°, 30°) therefore means "50° up,
    // 30° around", which is exactly how the viewer reads `lit_sun`.
    let forward = light_direction(project, light.kind);
    let toward_sun = Vec3::new(-forward.x, -forward.y, -forward.z);
    let elevation_deg = toward_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
    let mut azimuth_deg = toward_sun.x.atan2(toward_sun.z).to_degrees();
    if azimuth_deg < 0.0 {
        azimuth_deg += 360.0;
    }
    Some(SunAngles {
        azimuth_deg,
        elevation_deg,
        intensity: light.intensity,
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
    let report = capability_report(project, target);
    let assets = if target == Workspace::Forge3d {
        used_assets(project)
    } else {
        Vec::new()
    };
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
        report,
        camera: camera_sync(project),
        assets,
        sun: sun_angles(project),
    }
}

fn used_assets(project: &Project) -> Vec<ForgeAsset> {
    let mut assets = Vec::new();
    for object in &project.scene.objects {
        let Some(id) = object.mesh().and_then(crate::scene::MeshObject::asset_id) else {
            continue;
        };
        if assets.iter().any(|asset: &ForgeAsset| asset.id == id) {
            continue;
        }
        if let Some(asset) = project.asset(id) {
            assets.push(ForgeAsset {
                id: asset.id,
                name: asset.name.clone(),
                path: asset.path.clone(),
                kind: asset.kind,
            });
        }
    }
    assets
}

fn workspace_slug(workspace: Workspace) -> &'static str {
    match workspace {
        Workspace::Studio => "studio",
        Workspace::Forge3d => "forge3d",
    }
}

/// Assets in a project, for the UI's dependency list.
pub fn project_assets(project: &Project) -> &[Asset] {
    &project.assets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids;
    use crate::scene::MeshObject;

    fn project_with(mesh: MeshObject) -> Project {
        let mut project = Project::new("Handoff");
        project.scene.objects[0].kind = ObjectKind::Mesh(mesh);
        project
    }

    fn imported_project() -> Project {
        let mut project = project_with(MeshObject::gltf(ids::new_id(), None));
        let asset = Asset {
            id: match &project.scene.objects[0].kind {
                ObjectKind::Mesh(mesh) => mesh.asset_id().unwrap(),
                _ => unreachable!(),
            },
            name: "duck.glb".to_owned(),
            path: "/models/duck.glb".to_owned(),
            kind: AssetKind::Glb,
            byte_size: 4096,
        };
        project.assets.push(asset);
        project
    }

    #[test]
    fn primitives_are_reported_as_partial_for_forge3d() {
        let project = Project::new("Primitives");
        let report = capability_report(&project, Workspace::Forge3d);
        let mesh = report
            .objects
            .iter()
            .find(|object| object.kind == "mesh")
            .unwrap();
        assert_eq!(mesh.level, CapabilityLevel::Partial);
        assert!(mesh.detail.contains("Primitive geometry"));
        assert_eq!(report.count(CapabilityLevel::Partial), 2); // mesh + directional sun
        assert!(report.has_renderable_content);
        assert_eq!(report.degradations.len(), 1);
    }

    #[test]
    fn imported_meshes_are_fully_supported() {
        let project = imported_project();
        let report = capability_report(&project, Workspace::Forge3d);
        let mesh = report
            .objects
            .iter()
            .find(|object| object.kind == "mesh")
            .unwrap();
        assert_eq!(mesh.level, CapabilityLevel::Full);
        assert!(!report
            .degradations
            .iter()
            .any(|note| note.contains("Primitive")));
    }

    #[test]
    fn point_lights_are_unsupported_with_a_reason() {
        let mut project = Project::new("Point");
        let light = project
            .scene
            .objects
            .iter_mut()
            .find(|object| matches!(&object.kind, ObjectKind::Light(light) if light.kind == LightKind::Directional))
            .unwrap();
        if let ObjectKind::Light(light) = &mut light.kind {
            light.kind = LightKind::Point;
        }
        let report = capability_report(&project, Workspace::Forge3d);
        let light = report
            .objects
            .iter()
            .find(|object| object.kind == "light")
            .unwrap();
        assert_eq!(light.level, CapabilityLevel::Unsupported);
        assert!(light.detail.contains("point-light"));
        assert!(report
            .degradations
            .iter()
            .any(|note| note.contains("point light")));
    }

    #[test]
    fn studio_target_supports_everything() {
        let project = Project::new("Studio");
        let report = capability_report(&project, Workspace::Studio);
        assert_eq!(report.count(CapabilityLevel::Full), 3);
        assert!(report.degradations.is_empty());
    }

    #[test]
    fn camera_sync_points_the_view_at_the_scene() {
        let project = Project::new("Camera");
        let camera = camera_sync(&project).expect("starter scene has a camera");
        let eye = camera.eye;
        assert_eq!(eye, Vec3::new(6.0, 4.0, 8.0));

        // The default camera looks at the origin, so the target must lie on the
        // far side of it: dot(forward, origin - eye) > 0.
        let to_origin = Vec3::new(-eye.x, -eye.y, -eye.z);
        let forward = Vec3::new(
            camera.target.x - eye.x,
            camera.target.y - eye.y,
            camera.target.z - eye.z,
        );
        let len = (forward.x * forward.x + forward.y * forward.y + forward.z * forward.z).sqrt();
        let dot =
            (forward.x * to_origin.x + forward.y * to_origin.y + forward.z * to_origin.z) / len;
        assert!(dot > 0.0, "camera looks away from the cube: dot={dot}");
        assert!((camera.fov_deg - 45.0).abs() < 1e-4);
    }

    #[test]
    fn sun_angles_match_the_light_rotation() {
        let project = Project::new("Sun");
        let sun = sun_angles(&project).expect("starter scene has a sun");
        // The starter sun uses rotation (-50°, 30°, 0°); the model means
        // "50° above the horizon, 30° around", which is the viewer's convention.
        assert!((sun.elevation_deg - 50.0).abs() < 0.5, "{sun:?}");
        assert!((sun.azimuth_deg - 30.0).abs() < 0.5, "{sun:?}");
        assert!((sun.intensity - 3.0).abs() < 1e-4);
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

    #[test]
    fn handoff_carries_only_referenced_assets() {
        let mut project = imported_project();
        project.assets.push(Asset {
            id: ids::new_id(),
            name: "unused.glb".to_owned(),
            path: "/models/unused.glb".to_owned(),
            kind: AssetKind::Glb,
            byte_size: 12,
        });
        let handoff = handoff(&project, 1, Workspace::Forge3d);
        assert_eq!(handoff.assets.len(), 1);
        assert_eq!(handoff.assets[0].name, "duck.glb");
        assert!(handoff.camera.is_some());
        assert!(handoff.sun.is_some());
    }

    #[test]
    fn studio_handoff_carries_no_assets() {
        let project = imported_project();
        let handoff = handoff(&project, 1, Workspace::Studio);
        assert!(handoff.assets.is_empty());
    }

    #[test]
    fn primitive_scenes_still_have_renderable_content_via_the_camera() {
        let project = Project::new("Empty-ish");
        let report = capability_report(&project, Workspace::Forge3d);
        assert!(!report.is_empty_for_target());
    }

    #[test]
    fn a_scene_with_only_unsupported_objects_is_reported_empty() {
        let mut project = Project::new("Lights only");
        project.scene.objects.retain(|object| {
            matches!(&object.kind, ObjectKind::Light(light) if light.kind == LightKind::Directional)
        });
        if let Some(object) = project.scene.objects.first_mut() {
            if let ObjectKind::Light(light) = &mut object.kind {
                light.kind = LightKind::Point;
            }
        }
        let report = capability_report(&project, Workspace::Forge3d);
        assert!(report.is_empty_for_target());
    }
}
