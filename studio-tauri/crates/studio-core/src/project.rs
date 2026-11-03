//! The versioned project document.
//!
//! `Project` is the whole persisted truth: identity, scene graph, selection,
//! active workspace, viewport mode, and render configuration. It contains no
//! GPU resources, process handles, or file-system absolute paths, so a project
//! file is portable and diffable.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ids::{new_id, AssetId, ObjectId, ProjectId};
use crate::math::{Transform, Vec3};
use crate::now_unix_ms;
use crate::scene::{
    validate_name, CameraObject, LightKind, LightObject, MeshObject, ObjectKind, Primitive, Scene,
    SceneObject, TerrainObject,
};

/// Width of the terrain in the Studio viewport's scene units.
///
/// The viewport draws a DEM across this many units regardless of its real size,
/// and the renderer works in metres; every conversion between the two uses it.
pub const SCENE_TERRAIN_SIZE: f32 = 12.0;

/// Format tag written into every project file.
pub const FORMAT: &str = "studio3d";

/// Schema version written by this build.
///
/// v1 carried `kind.primitive` on meshes; v2 adds imported glTF/GLB meshes
/// (`kind.source`) and the project asset registry; v3 adds DEM-backed terrain
/// objects. Earlier files are migrated on load — see [`crate::persist::migrate`].
pub const SCHEMA_VERSION: u32 = 3;

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
    /// Imported files referenced by mesh objects. Paths are stored relative to
    /// the project file where possible (see [`crate::persist`]).
    #[serde(default)]
    pub assets: Vec<Asset>,
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
            kind: ObjectKind::Mesh(MeshObject::primitive(Primitive::Cube { size: 2.0 })),
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
            assets: Vec::new(),
            selection: Vec::new(),
            workspace: Workspace::Studio,
            viewport: ViewportState::default(),
            render: RenderSettings::default(),
        }
    }

    /// A starter scene for a real project: a terrain from `seed` when one is
    /// available, otherwise the primitive starter.
    ///
    /// The shell resolves the DEM (a bundled Bryce Canyon sample by default) and
    /// hands it in, so this crate stays free of file-system discovery.
    pub fn starter(name: impl Into<String>, seed: Option<TerrainSeed>) -> Self {
        let mut project = Self::new(name);
        let Some(seed) = seed else {
            return project;
        };
        let asset = Asset {
            id: new_id(),
            name: seed.name,
            path: seed.path,
            kind: AssetKind::Dem,
            byte_size: seed.byte_size,
        };
        project.assets.push(asset.clone());
        let mut terrain = TerrainObject::new(asset.id);
        if let (Some(dimensions), Some(range)) = (seed.dimensions, seed.elevation_range) {
            terrain =
                terrain.with_dem_info(dimensions, range, seed.world_origin_xz, seed.world_span_xz);
        }
        project.scene.objects.retain(|object| !object.is_mesh());
        project.scene.objects.insert(
            0,
            SceneObject {
                id: new_id(),
                name: "Bryce Canyon".to_owned(),
                parent: None,
                transform: Transform::default(),
                visible: true,
                locked: false,
                kind: ObjectKind::Terrain(terrain),
            },
        );
        // Frame the terrain: the default camera sits close to a 2-unit cube,
        // which would be inside a kilometre-scale DEM.
        if let Some(camera) = project
            .scene
            .objects
            .iter_mut()
            .find(|object| object.is_camera())
        {
            camera.transform.translation = Vec3::new(11.0, 7.5, 13.0);
            camera.transform.rotation_deg = Vec3::new(-24.0, 40.0, 0.0);
        }
        project.touch();
        project
    }

    /// Bump the modification timestamp. Called by the editor after every
    /// applied command, so "modified" is meaningful in a saved file.
    pub fn touch(&mut self) {
        self.updated_unix_ms = now_unix_ms();
    }

    /// The scene's terrain, if any (the viewer renders one).
    pub fn terrain_object(&self) -> Option<&SceneObject> {
        self.scene.objects.iter().find(|object| object.is_terrain())
    }

    /// Assets referenced by at least one object, in registry order.
    pub fn used_assets(&self) -> Vec<AssetId> {
        let mut used: Vec<AssetId> = Vec::new();
        for object in &self.scene.objects {
            if let Some(asset) = object.mesh().and_then(MeshObject::asset_id) {
                if !used.contains(&asset) {
                    used.push(asset);
                }
            }
            if let Some(terrain) = object.terrain() {
                if !used.contains(&terrain.asset) {
                    used.push(terrain.asset);
                }
                if let Some(albedo) = terrain.albedo {
                    if !used.contains(&albedo) {
                        used.push(albedo);
                    }
                }
            }
        }
        used
    }

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.id == id)
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

        let mut asset_ids: Vec<AssetId> = Vec::with_capacity(self.assets.len());
        for asset in &self.assets {
            asset
                .validate()
                .map_err(|reason| format!("asset {:?}: {reason}", asset.name))?;
            if asset_ids.contains(&asset.id) {
                return Err(format!("duplicate asset id {}", asset.id));
            }
            asset_ids.push(asset.id);
        }
        // A mesh or terrain may only reference an asset the project carries;
        // dangling references are what "missing asset" panics are made of.
        for object in &self.scene.objects {
            if let Some(asset) = object.mesh().and_then(MeshObject::asset_id) {
                if !asset_ids.contains(&asset) {
                    return Err(format!(
                        "object {:?} references missing asset {asset}",
                        object.name
                    ));
                }
            }
            if let Some(terrain) = object.terrain() {
                let Some(asset) = self.asset(terrain.asset) else {
                    return Err(format!(
                        "object {:?} references missing terrain asset {}",
                        object.name, terrain.asset
                    ));
                };
                if asset.kind != AssetKind::Dem {
                    return Err(format!(
                        "object {:?} uses asset {:?} as terrain, but it is {:?}",
                        object.name, asset.name, asset.kind
                    ));
                }
                if let Some(albedo) = terrain.albedo {
                    if !asset_ids.contains(&albedo) {
                        return Err(format!(
                            "object {:?} references missing albedo asset {albedo}",
                            object.name
                        ));
                    }
                }
            }
        }

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

/// A DEM the shell resolved on disk, used to seed a new project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainSeed {
    pub name: String,
    pub path: String,
    pub byte_size: u64,
    /// Measured DEM size, when the shell sampled it.
    #[serde(default)]
    pub dimensions: Option<[u32; 2]>,
    #[serde(default)]
    pub elevation_range: Option<[f32; 2]>,
    /// Viewer-basis `[map X, -map Y]` origin in metres.
    #[serde(default)]
    pub world_origin_xz: Option<[f64; 2]>,
    /// Viewer-basis span in metres.
    #[serde(default)]
    pub world_span_xz: Option<[f64; 2]>,
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

/// An imported source file. The project stores a reference, never the bytes:
/// the viewport reads the file when it needs it, and Phase 5 adds relinking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: AssetId,
    /// Display name, normally the imported file name.
    pub name: String,
    /// Path to the source file. Relative paths resolve against the project
    /// file's directory; absolute paths survive only when the file cannot be
    /// expressed relatively.
    pub path: String,
    pub kind: AssetKind,
    pub byte_size: u64,
}

impl Asset {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_nil() {
            return Err("asset id must not be nil".to_owned());
        }
        validate_name(&self.name, "asset name")?;
        if self.path.trim().is_empty() {
            return Err("asset path must not be empty".to_owned());
        }
        if self.path.chars().any(|ch| ch.is_control()) {
            return Err("asset path must not contain control characters".to_owned());
        }
        if self.byte_size == 0 {
            return Err("asset file is empty".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Gltf,
    Glb,
    /// A GeoTIFF elevation model (single band, float or integer).
    Dem,
    /// An orthophoto / albedo raster draped over terrain.
    Orthophoto,
}

impl AssetKind {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("gltf") => Some(AssetKind::Gltf),
            Some("glb") => Some(AssetKind::Glb),
            Some("tif" | "tiff") => Some(AssetKind::Dem),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            AssetKind::Gltf => "gltf",
            AssetKind::Glb => "glb",
            AssetKind::Dem => "tif",
            AssetKind::Orthophoto => "tif",
        }
    }

    pub fn is_raster(self) -> bool {
        matches!(self, AssetKind::Dem | AssetKind::Orthophoto)
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
    fn assets_are_validated() {
        let good = Asset {
            id: new_id(),
            name: "duck.glb".to_owned(),
            path: "assets/duck.glb".to_owned(),
            kind: AssetKind::Glb,
            byte_size: 1024,
        };
        good.validate().unwrap();

        let mut empty = good.clone();
        empty.byte_size = 0;
        assert!(empty.validate().is_err());

        let mut nameless = good.clone();
        nameless.name = "  ".to_owned();
        assert!(nameless.validate().is_err());

        let mut pathless = good.clone();
        pathless.path = String::new();
        assert!(pathless.validate().is_err());

        let mut nil = good;
        nil.id = uuid::Uuid::nil();
        assert!(nil.validate().is_err());
    }

    #[test]
    fn asset_kind_is_read_from_the_extension() {
        assert_eq!(
            AssetKind::from_path(Path::new("a.glb")),
            Some(AssetKind::Glb)
        );
        assert_eq!(
            AssetKind::from_path(Path::new("DIR/A.GLTF")),
            Some(AssetKind::Gltf)
        );
        assert_eq!(AssetKind::from_path(Path::new("a.obj")), None);
    }

    #[test]
    fn meshes_may_not_reference_assets_the_project_lacks() {
        let mut project = Project::new(UNTITLED);
        project.scene.objects[0].kind = ObjectKind::Mesh(MeshObject::gltf(new_id(), None));
        let error = project.validate().unwrap_err();
        assert!(error.contains("missing asset"), "{error}");
    }

    #[test]
    fn validation_rejects_wrong_format_tag() {
        let mut project = Project::new(UNTITLED);
        project.format = "forge3d".to_owned();
        assert!(project.validate().unwrap_err().contains("format tag"));
    }
}
