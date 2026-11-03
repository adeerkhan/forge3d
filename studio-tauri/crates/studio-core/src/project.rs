//! The versioned project document.
//!
//! `Project` is the whole persisted truth: identity, scene graph, selection,
//! active workspace, viewport mode, and render configuration. It contains no
//! GPU resources, process handles, or file-system absolute paths, so a project
//! file is portable and diffable.

use serde::{Deserialize, Serialize};

use crate::ids::{new_id, ObjectId, ProjectId};
use crate::math::{Transform, Vec3};
use crate::now_unix_ms;
use crate::scene::{
    validate_name, CameraObject, LightKind, LightObject, MeshObject, ObjectKind, Primitive, Scene,
    SceneObject,
};

/// Format tag written into every project file.
pub const FORMAT: &str = "studio3d";

/// Schema version written by this build.
///
/// Bump this when the document shape changes, add the step to
/// [`crate::persist::migrate`], and keep the previous version loadable.
pub const SCHEMA_VERSION: u32 = 1;

/// Default project name for `File → New`.
pub const UNTITLED: &str = "Untitled";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Must equal [`FORMAT`]; a mismatched tag means the user opened the wrong
    /// kind of file, which deserves a specific error, not a JSON trace.
    pub format: String,
    pub schema_version: u32,
    pub id: ProjectId,
    pub name: String,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub scene: Scene,
    /// Selected object IDs; view state, persisted so a reopened project looks
    /// the way the user left it.
    #[serde(default)]
    pub selection: Vec<ObjectId>,
    pub workspace: Workspace,
    pub viewport: ViewportState,
    pub render: RenderSettings,
}

impl Project {
    /// A new project with the Phase 1 starter scene: one cube, one camera
    /// looking at it, and one directional light.
    pub fn new(name: impl Into<String>) -> Self {
        let now = now_unix_ms();
        let mut scene = Scene::new("Scene");

        let cube = SceneObject {
            id: new_id(),
            name: "Cube".to_owned(),
            parent: None,
            transform: Transform::default(),
            visible: true,
            locked: false,
            kind: ObjectKind::Mesh(MeshObject {
                primitive: Primitive::Cube { size: 2.0 },
            }),
        };

        let camera = SceneObject {
            id: new_id(),
            name: "Camera".to_owned(),
            parent: None,
            // Roughly (6, 4, 8) looking at the origin under the model's
            // Rz*Ry*Rx Euler convention; see `math` docs.
            transform: Transform {
                translation: Vec3::new(6.0, 4.0, 8.0),
                rotation_deg: Vec3::new(-22.0, 37.0, 0.0),
                scale: Vec3::ONE,
            },
            visible: true,
            locked: false,
            kind: ObjectKind::Camera(CameraObject {
                fov_deg: 45.0,
                near: 0.1,
                far: 1000.0,
            }),
        };

        let light = SceneObject {
            id: new_id(),
            name: "Sun".to_owned(),
            parent: None,
            transform: Transform {
                translation: Vec3::new(4.0, 6.0, 3.0),
                rotation_deg: Vec3::new(-50.0, 30.0, 0.0),
                scale: Vec3::ONE,
            },
            visible: true,
            locked: false,
            kind: ObjectKind::Light(LightObject {
                kind: LightKind::Directional,
                color: [1.0, 0.97, 0.92],
                intensity: 3.0,
            }),
        };

        scene.active_camera = Some(camera.id);
        scene.objects = vec![cube, camera, light];

        Self {
            format: FORMAT.to_owned(),
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            name: name.into(),
            created_unix_ms: now,
            updated_unix_ms: now,
            scene,
            selection: Vec::new(),
            workspace: Workspace::Studio,
            viewport: ViewportState::default(),
            render: RenderSettings::default(),
        }
    }

    /// Bump the modification timestamp. Called by the editor after every
    /// applied command, so "modified" is meaningful in a saved file.
    pub fn touch(&mut self) {
        self.updated_unix_ms = now_unix_ms();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!(
                "format tag is {:?}, expected {FORMAT:?}",
                self.format
            ));
        }
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "schema version {} is not the current version {SCHEMA_VERSION}",
                self.schema_version
            ));
        }
        validate_name(&self.name, "project name")?;
        self.scene.validate()?;

        let mut seen: Vec<ObjectId> = Vec::with_capacity(self.selection.len());
        for id in &self.selection {
            if seen.contains(id) {
                return Err(format!("selection contains {id} twice"));
            }
            seen.push(*id);
            if self.scene.object(*id).is_none() {
                return Err(format!("selection references missing object {id}"));
            }
        }

        if self.render.width == 0 || self.render.height == 0 {
            return Err("render resolution must be greater than zero".to_owned());
        }
        if self.render.width > 16_384 || self.render.height > 16_384 {
            return Err("render resolution must not exceed 16384 pixels".to_owned());
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    #[default]
    Studio,
    Forge3d,
}

impl Workspace {
    pub fn label(self) -> &'static str {
        match self {
            Workspace::Studio => "Studio",
            Workspace::Forge3d => "Forge3D",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportMode {
    /// Fast editing mode: grid, outlines, no production shading.
    #[default]
    Solid,
    /// Forge3D-backed rendering (Phase 4).
    Rendered,
}

impl ViewportMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewportMode::Solid => "Solid",
            ViewportMode::Rendered => "Rendered",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewportState {
    pub mode: ViewportMode,
    pub grid_visible: bool,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            mode: ViewportMode::Solid,
            grid_visible: true,
        }
    }
}

/// Render output configuration. Phase 1 stores the resolution only; Phase 4
/// adds the Forge3D-exposed controls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_project_has_the_starter_scene() {
        let project = Project::new(UNTITLED);
        assert_eq!(project.scene.objects.len(), 3);
        assert_eq!(
            project.scene.objects.iter().filter(|o| o.is_mesh()).count(),
            1
        );
        assert_eq!(
            project
                .scene
                .objects
                .iter()
                .filter(|o| o.is_camera())
                .count(),
            1
        );
        assert_eq!(
            project
                .scene
                .objects
                .iter()
                .filter(|o| o.is_light())
                .count(),
            1
        );
        assert!(project.scene.camera().is_some());
        assert_eq!(project.scene.camera().unwrap().name, "Camera");
        project.validate().expect("starter project is valid");
    }

    #[test]
    fn new_projects_get_distinct_ids() {
        let a = Project::new(UNTITLED);
        let b = Project::new(UNTITLED);
        assert_ne!(a.id, b.id);
        assert_ne!(a.scene.id, b.scene.id);
        for (left, right) in a.scene.objects.iter().zip(&b.scene.objects) {
            assert_ne!(left.id, right.id);
        }
    }

    #[test]
    fn validation_rejects_dangling_selection() {
        let mut project = Project::new(UNTITLED);
        project.selection = vec![new_id()];
        assert!(project.validate().unwrap_err().contains("selection"));
    }

    #[test]
    fn validation_rejects_zero_resolution() {
        let mut project = Project::new(UNTITLED);
        project.render.height = 0;
        assert!(project
            .validate()
            .unwrap_err()
            .contains("greater than zero"));
    }

    #[test]
    fn validation_rejects_wrong_format_tag() {
        let mut project = Project::new(UNTITLED);
        project.format = "forge3d".to_owned();
        assert!(project.validate().unwrap_err().contains("format tag"));
    }
}
