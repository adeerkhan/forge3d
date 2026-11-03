//! Loading and atomic saving of project files.
//!
//! Two rules drive this module:
//!
//! 1. **A failed save never damages the previous file.** Bytes go to a
//!    sibling temporary file, are flushed with `sync_all`, and only then
//!    replace the target with `rename`.
//! 2. **A newer schema is refused, not guessed at.** A project written by a
//!    future build would lose fields if this build deserialized it, so it is
//!    rejected with a message naming both versions.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::StudioError;
use crate::project::{Project, FORMAT, SCHEMA_VERSION};

/// File extension for Studio projects (without the dot).
pub const PROJECT_EXTENSION: &str = "studio3d";

/// Serialize a project to the canonical pretty JSON (trailing newline so the
/// file is diff-friendly and editors do not fight over the last line).
pub fn to_json_pretty(project: &Project) -> Result<String, StudioError> {
    project.validate().map_err(StudioError::invalid)?;
    let mut json = serde_json::to_string_pretty(project)
        .map_err(|source| StudioError::json("project", source))?;
    json.push('\n');
    Ok(json)
}

/// Parse a project from JSON, gating on the format tag and schema version.
///
/// `context` names the source in error messages (`/path/to/p.studio3d`,
/// `autosave 1712345678`); it is never parsed.
pub fn from_json(text: &str, context: &str) -> Result<Project, StudioError> {
    let value: Value =
        serde_json::from_str(text).map_err(|source| StudioError::json(context, source))?;

    let format = value.get("format").and_then(Value::as_str).unwrap_or("");
    if format != FORMAT {
        return Err(StudioError::WrongFormat {
            context: context.to_owned(),
            found: format.to_owned(),
            expected: FORMAT.to_owned(),
        });
    }

    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| StudioError::invalid(format!("{context} has no numeric schemaVersion")))?;
    if version > u64::from(SCHEMA_VERSION) {
        return Err(StudioError::UnsupportedSchema {
            context: context.to_owned(),
            found: version as u32,
            supported: SCHEMA_VERSION,
        });
    }

    let value = migrate(value, version as u32, context)?;
    let project: Project =
        serde_json::from_value(value).map_err(|source| StudioError::json(context, source))?;
    project.validate().map_err(StudioError::invalid)?;
    Ok(project)
}

/// Upgrade a document written by `from_version` to [`SCHEMA_VERSION`].
///
/// Steps are applied one version at a time, so a v1 file opened by a future v3
/// build runs v1→v2 then v2→v3. Every step rewrites the `Value` in place and
/// bumps `schemaVersion`.
fn migrate(mut value: Value, from_version: u32, context: &str) -> Result<Value, StudioError> {
    if from_version == 0 {
        return Err(StudioError::invalid(format!(
            "{context} predates schema version 1 and cannot be upgraded"
        )));
    }
    let mut version = from_version;
    while version < SCHEMA_VERSION {
        value = match version {
            1 => migrate_v1_to_v2(value),
            other => {
                return Err(StudioError::UnsupportedSchema {
                    context: context.to_owned(),
                    found: other,
                    supported: SCHEMA_VERSION,
                })
            }
        };
        version += 1;
    }
    Ok(value)
}

/// v1 → v2: meshes carried `kind.primitive`; v2 nests it under `kind.source`
/// (which may also be an imported glTF asset) and adds the asset registry.
fn migrate_v1_to_v2(mut value: Value) -> Value {
    if let Some(objects) = value
        .get_mut("scene")
        .and_then(|scene| scene.get_mut("objects"))
        .and_then(Value::as_array_mut)
    {
        for object in objects {
            let Some(kind) = object.get_mut("kind").and_then(Value::as_object_mut) else {
                continue;
            };
            if kind.get("type").and_then(Value::as_str) != Some("mesh") {
                continue;
            }
            if let Some(primitive) = kind.remove("primitive") {
                let mut source = serde_json::Map::new();
                source.insert("type".into(), Value::from("primitive"));
                source.insert("primitive".into(), primitive);
                kind.insert("source".into(), Value::Object(source));
            }
        }
    }

    if let Some(object) = value.as_object_mut() {
        object
            .entry("assets")
            .or_insert_with(|| Value::Array(Vec::new()));
        object.insert("schemaVersion".into(), Value::from(SCHEMA_VERSION));
    }
    value
}

/// Rewrite absolute asset paths relative to the project file, so a project
/// directory stays portable. Paths outside the project directory are kept
/// absolute (and reported as such by the UI).
fn relativize_assets(project: &Project, dir: &Path) -> Project {
    let mut stored = project.clone();
    for asset in &mut stored.assets {
        let candidate = Path::new(&asset.path);
        if !candidate.is_absolute() {
            continue;
        }
        if let Ok(relative) = candidate.strip_prefix(dir) {
            asset.path = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
        }
    }
    stored
}

/// Resolve stored relative asset paths against the project file's directory.
fn resolve_assets(project: &mut Project, dir: &Path) {
    for asset in &mut project.assets {
        let candidate = Path::new(&asset.path);
        if candidate.is_absolute() {
            continue;
        }
        asset.path = dir.join(candidate).to_string_lossy().into_owned();
    }
}

/// Read and validate a project file.
pub fn load(path: &Path) -> Result<Project, StudioError> {
    let text = fs::read_to_string(path).map_err(|source| StudioError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut project = from_json(&text, &format!("project file {}", path.display()))?;
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_assets(&mut project, &dir);
    Ok(project)
}

/// Write `project` to `path` atomically.
///
/// The temporary file is created beside the target (same filesystem, so
/// `rename` is atomic), removed on any failure, and never left behind on
/// success.
pub fn save_atomic(path: &Path, project: &Project) -> Result<(), StudioError> {
    if path.is_dir() {
        return Err(StudioError::invalid(format!(
            "{} is a directory, not a project file",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent).map_err(|source| StudioError::Write {
        path: parent.clone(),
        source,
    })?;

    let json = to_json_pretty(&relativize_assets(project, &parent))?;

    let file_name = path
        .file_name()
        .ok_or_else(|| StudioError::invalid(format!("{} is not a file path", path.display())))?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!(
        ".{file_name}.{}-{}.tmp",
        std::process::id(),
        crate::now_unix_ms()
    ));

    let written = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(source) = written {
        let _ = fs::remove_file(&tmp);
        return Err(StudioError::Write { path: tmp, source });
    }

    if let Err(source) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(StudioError::Write {
            path: path.to_path_buf(),
            source,
        });
    }

    // Persist the rename itself where the platform supports it. Best effort:
    // the data file is already durable, and a failure here is not worth
    // failing the save the user just performed.
    #[cfg(unix)]
    {
        if let Ok(dir) = File::open(&parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::UNTITLED;

    #[test]
    fn json_round_trip_is_stable() {
        let project = Project::new(UNTITLED);
        let json = to_json_pretty(&project).unwrap();
        let parsed = from_json(&json, "test").unwrap();
        assert_eq!(parsed, project);
        assert_eq!(to_json_pretty(&parsed).unwrap(), json);
    }

    #[test]
    fn newer_schema_is_rejected_with_both_versions() {
        let project = Project::new(UNTITLED);
        let mut value: Value = serde_json::to_value(&project).unwrap();
        value["schemaVersion"] = Value::from(SCHEMA_VERSION + 1);
        let error = from_json(&value.to_string(), "future.studio3d").unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(&format!("schema version {}", SCHEMA_VERSION + 1)),
            "{message}"
        );
        assert!(
            message.contains(&format!("supports {SCHEMA_VERSION}")),
            "{message}"
        );
    }

    #[test]
    fn wrong_format_tag_names_the_expected_format() {
        let project = Project::new(UNTITLED);
        let mut value: Value = serde_json::to_value(&project).unwrap();
        value["format"] = Value::from("forge3d");
        let error = from_json(&value.to_string(), "bundle.forge3d").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("\"forge3d\""), "{message}");
        assert!(message.contains("\"studio3d\""), "{message}");
    }

    #[test]
    fn missing_version_is_not_silently_upgraded() {
        let mut value = serde_json::to_value(Project::new(UNTITLED)).unwrap();
        value.as_object_mut().unwrap().remove("schemaVersion");
        let error = from_json(&value.to_string(), "no-version").unwrap_err();
        assert!(error.to_string().contains("schemaVersion"), "{error}");
    }
}
