//! Forge3D Studio project files and shared application state types.
//!
//! The project format is a plain JSON document, versioned through
//! [`Project::format_version`]. It stores application-owned state only —
//! dataset references are kept as paths, never embedded pixel data.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Format version written by this build of the application.
pub const CURRENT_FORMAT_VERSION: u32 = 2;

/// Scene mode: direct terrain rendering or a MapScene recipe scene.
pub const SCENE_MODE_TERRAIN: &str = "terrain";
pub const SCENE_MODE_MAPSCENE: &str = "mapscene";

/// File extension used for Studio project documents (without the dot).
pub const PROJECT_EXTENSION: &str = "forge3d";

/// Errors produced when saving or loading project files.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("failed to write project file {path}: {source}")]
    SaveIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read project file {path}: {source}")]
    LoadIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("project file {path} is not valid JSON: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(
        "project file {path} uses format version {found}, but this build supports {supported}"
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
}

/// Orbit camera parameters mirroring the engine's `TerrainScene` viewer camera.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraParams {
    pub phi_deg: f32,
    pub theta_deg: f32,
    pub radius: f32,
    pub fov_deg: f32,
}

impl Default for CameraParams {
    fn default() -> Self {
        Self {
            phi_deg: 135.0,
            theta_deg: 45.0,
            radius: 1000.0,
            fov_deg: 55.0,
        }
    }
}

/// Sun direction and intensity mirroring the engine's viewer sun.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SunParams {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub intensity: f32,
}

impl Default for SunParams {
    fn default() -> Self {
        // Low-sun lighting: the terrain pipeline reads noticeably brighter
        // with a 24 degree elevation than with a high noon sun.
        Self {
            azimuth_deg: 138.0,
            elevation_deg: 24.0,
            intensity: 2.4,
        }
    }
}

/// Offline render/export output settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
        }
    }
}

/// Reference to a terrain dataset on disk. The file itself is never copied
/// into the project; only the path and the metadata captured at import time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainRef {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub height_min: f32,
    pub height_max: f32,
}

/// A Forge3D Studio project document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub format_version: u32,
    pub name: String,
    pub terrain: Option<TerrainRef>,
    pub terrain_visible: bool,
    pub camera: CameraParams,
    pub sun: SunParams,
    pub export: ExportSettings,
    /// v2: which scene mode the project was last in.
    pub scene_mode: String,
    /// v2: the serialized MapScene recipe (canonical `SceneRecipe.to_dict()`
    /// output) when `scene_mode` is `mapscene`. Stored verbatim so
    /// round-trips into the engine are byte-stable.
    pub scene_recipe_json: Option<String>,
    /// v2: named colormap for terrain rendering (forge3d registry key).
    pub colormap: Option<String>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            format_version: CURRENT_FORMAT_VERSION,
            name: "Untitled".to_owned(),
            terrain: None,
            terrain_visible: true,
            camera: CameraParams::default(),
            sun: SunParams::default(),
            export: ExportSettings::default(),
            scene_mode: SCENE_MODE_TERRAIN.to_owned(),
            scene_recipe_json: None,
            colormap: None,
        }
    }
}

impl Project {
    /// A fresh, empty project.
    pub fn new() -> Self {
        Self::default()
    }

    /// Save the project as pretty-printed JSON at `path`.
    pub fn save(&self, path: &Path) -> Result<(), ProjectError> {
        let json = serde_json::to_string_pretty(self).map_err(|source| ProjectError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        std::fs::write(path, json.as_bytes()).map_err(|source| ProjectError::SaveIo {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Load and validate a project from `path`.
    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        let bytes = std::fs::read(path).map_err(|source| ProjectError::LoadIo {
            path: path.to_path_buf(),
            source,
        })?;
        let project: Project = serde_json::from_slice(&bytes).map_err(|source| {
            ProjectError::Json {
                path: path.to_path_buf(),
                source,
            }
        })?;
        if project.format_version > CURRENT_FORMAT_VERSION {
            return Err(ProjectError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: project.format_version,
                supported: CURRENT_FORMAT_VERSION,
            });
        }
        Ok(project)
    }

    /// Non-fatal validation warnings, e.g. datasets that no longer exist.
    /// Projects still open with warnings; these are surfaced in the UI.
    pub fn validation_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(terrain) = &self.terrain {
            if !terrain.path.is_file() {
                warnings.push(format!(
                    "Terrain dataset not found: {}",
                    terrain.path.display()
                ));
            }
        }
        if self.export.width == 0 || self.export.height == 0 {
            warnings.push("Export size must be non-zero".to_owned());
        }
        if self.scene_mode == SCENE_MODE_MAPSCENE && self.scene_recipe_json.is_none() {
            warnings.push("MapScene project has no recipe yet".to_owned());
        }
        warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_project_round_trips_through_json() {
        let project = Project::new();
        let json = serde_json::to_string_pretty(&project).expect("serialize");
        let parsed: Project = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(project, parsed);
        assert_eq!(parsed.format_version, CURRENT_FORMAT_VERSION);
    }

    #[test]
    fn save_and_load_round_trips_identically() {
        let mut project = Project::new();
        project.name = "Test Scene".to_owned();
        project.terrain = Some(TerrainRef {
            path: PathBuf::from("/tmp/dem.tif"),
            width: 256,
            height: 256,
            height_min: 10.0,
            height_max: 500.5,
        });
        project.camera.phi_deg = 42.0;
        project.sun.elevation_deg = 60.0;

        let dir = std::env::temp_dir().join("studio-project-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("roundtrip.forge3d");
        project.save(&path).expect("save");
        let loaded = Project::load(&path).expect("load");
        std::fs::remove_file(&path).ok();
        assert_eq!(project, loaded);
    }

    #[test]
    fn future_format_version_is_rejected() {
        let future = CURRENT_FORMAT_VERSION + 1;
        let json = format!(
            r#"{{ "format_version": {future}, "name": "from the future" }}"#
        );
        let dir = std::env::temp_dir().join("studio-project-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("future.forge3d");
        std::fs::write(&path, json).expect("write");
        let err = Project::load(&path).expect_err("must reject future version");
        std::fs::remove_file(&path).ok();
        assert!(matches!(
            err,
            ProjectError::UnsupportedVersion { found, .. } if found == future
        ));
    }

    #[test]
    fn partial_json_fills_defaults() {
        let parsed: Project = serde_json::from_str(r#"{ "name": "partial" }"#).expect("parse");
        assert_eq!(parsed.name, "partial");
        assert_eq!(parsed.camera, CameraParams::default());
        assert_eq!(parsed.sun, SunParams::default());
        assert!(parsed.terrain.is_none());
    }

    #[test]
    fn v1_project_loads_with_v2_defaults() {
        let dir = std::env::temp_dir().join("studio-project-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("v1.forge3d");
        std::fs::write(
            &path,
            r#"{ "format_version": 1, "name": "legacy" }"#,
        )
        .expect("write");
        let project = Project::load(&path).expect("v1 must load");
        std::fs::remove_file(&path).ok();
        assert_eq!(project.name, "legacy");
        assert_eq!(project.scene_mode, SCENE_MODE_TERRAIN);
        assert!(project.scene_recipe_json.is_none());
        assert!(project.colormap.is_none());
    }

    #[test]
    fn mapscene_recipe_round_trips_through_json() {
        let mut project = Project::new();
        project.scene_mode = SCENE_MODE_MAPSCENE.to_owned();
        let recipe = serde_json::json!({
            "kind": "scene_recipe",
            "terrain": {"kind": "terrain_source", "path": "/tmp/dem.tif"},
            "camera": {"kind": "orbit_camera", "distance": 2.0}
        });
        project.scene_recipe_json = Some(recipe.to_string());
        let dir = std::env::temp_dir().join("studio-project-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("v2recipe.forge3d");
        project.save(&path).expect("save");
        let loaded = Project::load(&path).expect("load");
        std::fs::remove_file(&path).ok();
        assert_eq!(project, loaded);
    }

    #[test]
    fn missing_terrain_file_produces_warning() {
        let mut project = Project::new();
        project.terrain = Some(TerrainRef {
            path: PathBuf::from("/nonexistent/no-such-dem.tif"),
            width: 4,
            height: 4,
            height_min: 0.0,
            height_max: 1.0,
        });
        let warnings = project.validation_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not found"));
    }

    #[test]
    fn missing_file_load_error_names_the_path() {
        let err = Project::load(Path::new("/nonexistent/no-such.forge3d"))
            .expect_err("must fail on missing file");
        let ProjectError::LoadIo { path, .. } = err else {
            panic!("expected LoadIo, got {err:?}");
        };
        assert_eq!(path, PathBuf::from("/nonexistent/no-such.forge3d"));
    }
}
