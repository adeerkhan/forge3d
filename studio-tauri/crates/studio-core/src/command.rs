//! The command layer: every mutation is a value that can be applied and
//! inverted.
//!
//! `Command::apply` mutates the model and returns the command that reverses it.
//! The editor stores that inverse on the undo stack, so undo and redo are the
//! same code path as editing — there is no separate "restore" logic to drift.

use serde::{Deserialize, Serialize};

use std::path::Path;

use crate::ids::{new_id, AssetId, ObjectId};
use crate::math::Transform;
use crate::project::AssetKind;
use crate::project::{Asset, Project, ViewportState, Workspace};
use crate::scene::{validate_name, CameraObject, LightObject, MeshObject, Primitive, SceneObject};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("object {id} does not exist")]
    ObjectNotFound { id: ObjectId },

    #[error("an object with id {id} already exists")]
    DuplicateObject { id: ObjectId },

    #[error("object {id} has children; delete or re-parent them first")]
    ObjectHasChildren { id: ObjectId },

    #[error("object {id} is not a camera")]
    NotACamera { id: ObjectId },

    #[error("object {id} is not a light")]
    NotALight { id: ObjectId },

    #[error("object {id} is not a mesh")]
    NotAMesh { id: ObjectId },

    #[error("object {id} is an imported mesh, not a primitive")]
    NotAPrimitive { id: ObjectId },

    #[error("asset {id} does not exist")]
    AssetNotFound { id: AssetId },

    #[error("asset {id} is still used by an object")]
    AssetInUse { id: AssetId },

    #[error("an asset with id {id} already exists")]
    DuplicateAsset { id: AssetId },

    #[error("invalid {field}: {reason}")]
    InvalidValue { field: &'static str, reason: String },

    #[error("cannot insert an object at index {index}: the scene has {len} objects")]
    IndexOutOfRange { index: usize, len: usize },
}

impl CommandError {
    fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::InvalidValue {
            field,
            reason: reason.into(),
        }
    }
}

/// A reversible edit to the canonical project.
///
/// New values only: the previous values are read from the model during
/// [`Command::apply`], which is what makes an inverse exact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Command {
    /// Insert an object (already built, with a stable id) at `index`
    /// (end of the list when omitted).
    CreateObject {
        object: SceneObject,
        #[serde(default)]
        index: Option<usize>,
    },

    /// Remove an object and detach it from the selection.
    DeleteObject {
        id: ObjectId,
    },

    SetTransform {
        id: ObjectId,
        transform: Transform,
    },

    RenameObject {
        id: ObjectId,
        name: String,
    },

    SetVisibility {
        id: ObjectId,
        visible: bool,
    },

    SetLocked {
        id: ObjectId,
        locked: bool,
    },

    /// Replace the camera payload of a camera object.
    SetCamera {
        id: ObjectId,
        camera: CameraObject,
    },

    /// Replace the light payload of a light object.
    SetLight {
        id: ObjectId,
        light: LightObject,
    },

    /// Replace the mesh's primitive (shape/edge length).
    SetPrimitive {
        id: ObjectId,
        primitive: Primitive,
    },

    /// Create a primitive mesh object at the origin, in one step.
    ///
    /// The id is minted here rather than in the UI: object identity belongs to
    /// the model, and a caller-supplied id would let a stale window collide.
    CreatePrimitive {
        primitive: Primitive,
        #[serde(default)]
        name: Option<String>,
    },

    /// Set a terrain's vertical exaggeration (the DEM itself is unchanged).
    SetTerrainZScale {
        id: ObjectId,
        z_scale: f32,
    },

    /// Replace a terrain's DEM (import/re-point), carrying its measured metadata.
    SetTerrainDem {
        id: ObjectId,
        asset: crate::ids::AssetId,
        #[serde(default)]
        dimensions: Option<[u32; 2]>,
        #[serde(default)]
        elevation_range: Option<[f32; 2]>,
        #[serde(default)]
        world_origin_xz: Option<[f64; 2]>,
        #[serde(default)]
        world_span_xz: Option<[f64; 2]>,
    },

    /// Replace render output settings.
    SetRenderSettings {
        render: crate::project::RenderSettings,
    },

    /// Replace a mesh's Studio material.
    SetMeshMaterial {
        id: ObjectId,
        material: crate::scene::Material,
    },

    /// Attach, change, or remove a terrain's draped overlay.
    SetTerrainOverlay {
        id: ObjectId,
        overlay: Option<crate::scene::TerrainOverlay>,
    },

    /// Duplicate an object in place. Children are not duplicated in Phase 2.
    DuplicateObject {
        id: ObjectId,
        /// Explicit name; defaults to a free Blender-style name (`Cube.001`).
        #[serde(default)]
        name: Option<String>,
    },

    /// Register an imported asset — batched with the object that uses it.
    AddAsset {
        asset: Asset,
    },

    /// Register a glTF/GLB file and create the mesh object that renders it, as
    /// one undo step. Both ids are minted here, so the UI never invents
    /// identities and redo cannot drift.
    ImportGltf {
        /// Display name, normally the file name.
        name: String,
        /// Path as read from disk. Relative paths resolve to the project
        /// directory on load.
        path: String,
        kind: AssetKind,
        byte_size: u64,
        /// glTF node to render; `None` means the whole asset.
        #[serde(default)]
        node: Option<String>,
    },

    /// Unregister an asset. Refused while an object still references it.
    RemoveAsset {
        id: AssetId,
    },

    /// Point the viewport and Forge3D handoff at a camera (or none).
    SetActiveCamera {
        id: Option<ObjectId>,
    },

    /// Viewport view state (grid, shading mode). Not scene content, but part of
    /// the persisted project, so it goes through the same command path.
    SetViewport {
        viewport: ViewportState,
    },

    /// Switch the active workspace. Persisted view state, and undoable so a
    /// mis-click cannot leave the project in a surprising workspace.
    SetWorkspace {
        workspace: Workspace,
    },

    /// Several commands applied in order as one undo step.
    Batch {
        commands: Vec<Command>,
    },
}

/// What an applied command did: its inverse, plus the object IDs a viewport
/// adapter must re-sync. Internal — callers get the inverse from
/// [`Command::apply`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outcome {
    pub inverse: Command,
    pub affected: Vec<ObjectId>,
}

impl Command {
    /// Apply to `project` and return the command that reverses this exact edit.
    ///
    /// On error the project is left unchanged: every variant reads and
    /// validates before it writes.
    pub fn apply(&self, project: &mut Project) -> Result<Command, CommandError> {
        self.apply_inner(project).map(|outcome| outcome.inverse)
    }

    /// Apply and also return the object IDs that actually changed.
    ///
    /// This is the authoritative answer, unlike [`Command::affected`]: a
    /// duplicate's new ID does not exist until the command runs.
    pub fn apply_with_effect(
        &self,
        project: &mut Project,
    ) -> Result<(Command, Vec<ObjectId>), CommandError> {
        self.apply_inner(project)
            .map(|outcome| (outcome.inverse, outcome.affected))
    }

    fn apply_inner(&self, project: &mut Project) -> Result<Outcome, CommandError> {
        match self {
            Command::CreateObject { object, index } => {
                object
                    .validate()
                    .map_err(|reason| CommandError::invalid("object", reason))?;
                if project.scene.object(object.id).is_some() {
                    return Err(CommandError::DuplicateObject { id: object.id });
                }
                if let Some(parent) = object.parent {
                    if project.scene.object(parent).is_none() {
                        return Err(CommandError::ObjectNotFound { id: parent });
                    }
                }
                if let Some(asset) = object.mesh().and_then(MeshObject::asset_id) {
                    if project.asset(asset).is_none() {
                        return Err(CommandError::AssetNotFound { id: asset });
                    }
                }
                let len = project.scene.objects.len();
                let index = index.unwrap_or(len);
                if index > len {
                    return Err(CommandError::IndexOutOfRange { index, len });
                }
                project.scene.objects.insert(index, object.clone());

                Ok(Outcome {
                    inverse: Command::DeleteObject { id: object.id },
                    affected: vec![object.id],
                })
            }

            Command::DeleteObject { id } => {
                let index = project
                    .scene
                    .index_of(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                if !project.scene.children_of(*id).is_empty() {
                    return Err(CommandError::ObjectHasChildren { id: *id });
                }
                let object = project.scene.objects.remove(index);
                let restore_camera = project.scene.active_camera == Some(*id);
                if restore_camera {
                    project.scene.active_camera = None;
                }
                project.selection.retain(|selected| selected != id);

                // Undoing a deletion that also cleared the active camera must
                // restore both, so the inverse is a batch.
                let inverse = if restore_camera {
                    Command::Batch {
                        commands: vec![
                            Command::CreateObject {
                                object: object.clone(),
                                index: Some(index),
                            },
                            Command::SetActiveCamera { id: Some(*id) },
                        ],
                    }
                } else {
                    Command::CreateObject {
                        object: object.clone(),
                        index: Some(index),
                    }
                };

                Ok(Outcome {
                    inverse,
                    affected: vec![*id],
                })
            }

            Command::SetTransform { id, transform } => {
                transform
                    .validate()
                    .map_err(|reason| CommandError::invalid("transform", reason))?;
                let object = project
                    .scene
                    .object_mut(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = object.transform;
                object.transform = *transform;
                Ok(Outcome {
                    inverse: Command::SetTransform {
                        id: *id,
                        transform: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::RenameObject { id, name } => {
                validate_name(name, "object name")
                    .map_err(|reason| CommandError::invalid("object name", reason))?;
                let object = project
                    .scene
                    .object_mut(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = std::mem::replace(&mut object.name, name.clone());
                Ok(Outcome {
                    inverse: Command::RenameObject {
                        id: *id,
                        name: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetVisibility { id, visible } => {
                let object = project
                    .scene
                    .object_mut(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = object.visible;
                object.visible = *visible;
                Ok(Outcome {
                    inverse: Command::SetVisibility {
                        id: *id,
                        visible: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetLocked { id, locked } => {
                let object = project
                    .scene
                    .object_mut(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = object.locked;
                object.locked = *locked;
                Ok(Outcome {
                    inverse: Command::SetLocked {
                        id: *id,
                        locked: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetCamera { id, camera } => {
                camera
                    .validate()
                    .map_err(|reason| CommandError::invalid("camera", reason))?;
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = match object.kind {
                    crate::scene::ObjectKind::Camera(previous) => previous,
                    _ => return Err(CommandError::NotACamera { id: *id }),
                };
                project.scene.object_mut(*id).expect("checked above").kind =
                    crate::scene::ObjectKind::Camera(*camera);
                Ok(Outcome {
                    inverse: Command::SetCamera {
                        id: *id,
                        camera: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetLight { id, light } => {
                light
                    .validate()
                    .map_err(|reason| CommandError::invalid("light", reason))?;
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = match object.kind {
                    crate::scene::ObjectKind::Light(previous) => previous,
                    _ => return Err(CommandError::NotALight { id: *id }),
                };
                project.scene.object_mut(*id).expect("checked above").kind =
                    crate::scene::ObjectKind::Light(*light);
                Ok(Outcome {
                    inverse: Command::SetLight {
                        id: *id,
                        light: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetPrimitive { id, primitive } => {
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = match object.mesh() {
                    Some(mesh) => mesh
                        .primitive_source()
                        .cloned()
                        .ok_or(CommandError::NotAPrimitive { id: *id })?,
                    None => return Err(CommandError::NotAMesh { id: *id }),
                };
                // Validate by building the whole object, so primitive rules
                // live in one place (`scene::Primitive::validate`).
                let mut probe = object.clone();
                probe.kind =
                    crate::scene::ObjectKind::Mesh(MeshObject::primitive(primitive.clone()));
                probe
                    .validate()
                    .map_err(|reason| CommandError::invalid("primitive", reason))?;
                if let Some(object) = project.scene.object_mut(*id) {
                    object.kind =
                        crate::scene::ObjectKind::Mesh(MeshObject::primitive(primitive.clone()));
                }
                Ok(Outcome {
                    inverse: Command::SetPrimitive {
                        id: *id,
                        primitive: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::CreatePrimitive { primitive, name } => {
                let default_name = match primitive {
                    Primitive::Cube { .. } => "Cube",
                    Primitive::Sphere { .. } => "Sphere",
                    Primitive::Plane { .. } => "Plane",
                };
                let object = SceneObject {
                    id: new_id(),
                    name: match name {
                        Some(name) => {
                            validate_name(name, "object name")
                                .map_err(|reason| CommandError::invalid("object name", reason))?;
                            name.clone()
                        }
                        None => project.scene.unique_name(default_name),
                    },
                    parent: None,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    kind: crate::scene::ObjectKind::Mesh(MeshObject::primitive(primitive.clone())),
                };
                object
                    .validate()
                    .map_err(|reason| CommandError::invalid("object", reason))?;
                let id = object.id;
                project.scene.objects.push(object);
                Ok(Outcome {
                    inverse: Command::DeleteObject { id },
                    affected: vec![id],
                })
            }

            Command::SetTerrainZScale { id, z_scale } => {
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = match &object.kind {
                    crate::scene::ObjectKind::Terrain(terrain) => terrain.z_scale,
                    _ => return Err(CommandError::invalid("terrain", "object is not a terrain")),
                };
                if !z_scale.is_finite() || *z_scale <= 0.0 {
                    return Err(CommandError::invalid(
                        "terrain vertical scale",
                        "must be a positive number",
                    ));
                }
                if let Some(object) = project.scene.object_mut(*id) {
                    if let crate::scene::ObjectKind::Terrain(terrain) = &mut object.kind {
                        terrain.z_scale = *z_scale;
                    }
                }
                Ok(Outcome {
                    inverse: Command::SetTerrainZScale {
                        id: *id,
                        z_scale: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetTerrainDem {
                id,
                asset,
                dimensions,
                elevation_range,
                world_origin_xz,
                world_span_xz,
            } => {
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let Some(dem) = project.asset(*asset) else {
                    return Err(CommandError::AssetNotFound { id: *asset });
                };
                if dem.kind != crate::project::AssetKind::Dem {
                    return Err(CommandError::invalid(
                        "DEM",
                        "asset is not an elevation model",
                    ));
                }
                let previous = match &object.kind {
                    crate::scene::ObjectKind::Terrain(terrain) => (
                        terrain.asset,
                        terrain.dimensions,
                        terrain.elevation_range,
                        terrain.world_origin_xz,
                        terrain.world_span_xz,
                    ),
                    _ => return Err(CommandError::invalid("terrain", "object is not a terrain")),
                };
                if let Some(object) = project.scene.object_mut(*id) {
                    if let crate::scene::ObjectKind::Terrain(terrain) = &mut object.kind {
                        terrain.asset = *asset;
                        terrain.dimensions = *dimensions;
                        terrain.elevation_range = *elevation_range;
                        terrain.world_origin_xz = *world_origin_xz;
                        terrain.world_span_xz = *world_span_xz;
                    }
                }
                Ok(Outcome {
                    inverse: Command::SetTerrainDem {
                        id: *id,
                        asset: previous.0,
                        dimensions: previous.1,
                        elevation_range: previous.2,
                        world_origin_xz: previous.3,
                        world_span_xz: previous.4,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetRenderSettings { render } => {
                let previous = project.render;
                let mut probe = project.clone();
                probe.render = *render;
                probe
                    .validate()
                    .map_err(|reason| CommandError::invalid("render settings", reason))?;
                project.render = *render;
                Ok(Outcome {
                    inverse: Command::SetRenderSettings { render: previous },
                    affected: Vec::new(),
                })
            }

            Command::SetMeshMaterial { id, material } => {
                material
                    .validate()
                    .map_err(|reason| CommandError::invalid("material", reason))?;
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = match &object.kind {
                    crate::scene::ObjectKind::Mesh(mesh) => mesh.material,
                    _ => return Err(CommandError::NotAMesh { id: *id }),
                };
                if let Some(object) = project.scene.object_mut(*id) {
                    if let crate::scene::ObjectKind::Mesh(mesh) = &mut object.kind {
                        mesh.material = *material;
                    }
                }
                Ok(Outcome {
                    inverse: Command::SetMeshMaterial {
                        id: *id,
                        material: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::SetTerrainOverlay { id, overlay } => {
                let object = project
                    .scene
                    .object(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let previous = match &object.kind {
                    crate::scene::ObjectKind::Terrain(terrain) => terrain.overlay,
                    _ => return Err(CommandError::invalid("terrain", "object is not a terrain")),
                };
                if let Some(layer) = overlay {
                    if project.asset(layer.asset).is_none() {
                        return Err(CommandError::AssetNotFound { id: layer.asset });
                    }
                    if let Some(object) = project.scene.object_mut(*id) {
                        if let crate::scene::ObjectKind::Terrain(terrain) = &mut object.kind {
                            terrain.overlay = Some(*layer);
                        }
                    }
                } else if let Some(object) = project.scene.object_mut(*id) {
                    if let crate::scene::ObjectKind::Terrain(terrain) = &mut object.kind {
                        terrain.overlay = None;
                    }
                }
                if let Some(object) = project.scene.object(*id) {
                    object
                        .validate()
                        .map_err(|reason| CommandError::invalid("terrain", reason))?;
                }
                Ok(Outcome {
                    inverse: Command::SetTerrainOverlay {
                        id: *id,
                        overlay: previous,
                    },
                    affected: vec![*id],
                })
            }

            Command::DuplicateObject { id, name } => {
                let index = project
                    .scene
                    .index_of(*id)
                    .ok_or(CommandError::ObjectNotFound { id: *id })?;
                let mut copy = project.scene.objects[index].clone();
                copy.id = new_id();
                copy.name = match name {
                    Some(name) => {
                        validate_name(name, "object name")
                            .map_err(|reason| CommandError::invalid("object name", reason))?;
                        name.clone()
                    }
                    None => project.scene.unique_name(&copy.name),
                };
                // A duplicated camera must not silently take over rendering.
                let copy_id = copy.id;
                project.scene.objects.insert(index + 1, copy);

                Ok(Outcome {
                    inverse: Command::DeleteObject { id: copy_id },
                    affected: vec![copy_id],
                })
            }

            Command::ImportGltf {
                name,
                path,
                kind,
                byte_size,
                node,
            } => {
                let asset = Asset {
                    id: new_id(),
                    name: name.clone(),
                    path: path.clone(),
                    kind: *kind,
                    byte_size: *byte_size,
                };
                asset
                    .validate()
                    .map_err(|reason| CommandError::invalid("asset", reason))?;

                let stem = Path::new(name)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.trim().is_empty())
                    .unwrap_or_else(|| "Imported".to_owned());
                let object = SceneObject {
                    id: new_id(),
                    name: project.scene.unique_name(&stem),
                    parent: None,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    kind: crate::scene::ObjectKind::Mesh(MeshObject::gltf(asset.id, node.clone())),
                };
                object
                    .validate()
                    .map_err(|reason| CommandError::invalid("object", reason))?;

                let asset_id = asset.id;
                let object_id = object.id;
                project.assets.push(asset);
                project.scene.objects.push(object);

                // Undo removes the object first: the asset may not be dropped
                // while something still references it.
                Ok(Outcome {
                    inverse: Command::Batch {
                        commands: vec![
                            Command::DeleteObject { id: object_id },
                            Command::RemoveAsset { id: asset_id },
                        ],
                    },
                    affected: vec![object_id],
                })
            }

            Command::AddAsset { asset } => {
                asset
                    .validate()
                    .map_err(|reason| CommandError::invalid("asset", reason))?;
                if project.asset(asset.id).is_some() {
                    return Err(CommandError::DuplicateAsset { id: asset.id });
                }
                project.assets.push(asset.clone());
                Ok(Outcome {
                    inverse: Command::RemoveAsset { id: asset.id },
                    affected: Vec::new(),
                })
            }

            Command::RemoveAsset { id } => {
                let index = project
                    .assets
                    .iter()
                    .position(|asset| asset.id == *id)
                    .ok_or(CommandError::AssetNotFound { id: *id })?;
                if project.used_assets().contains(id) {
                    return Err(CommandError::AssetInUse { id: *id });
                }
                let asset = project.assets.remove(index);
                Ok(Outcome {
                    inverse: Command::AddAsset { asset },
                    affected: Vec::new(),
                })
            }

            Command::SetActiveCamera { id } => {
                if let Some(id) = id {
                    match project.scene.object(*id) {
                        Some(object) if object.is_camera() => {}
                        Some(_) => return Err(CommandError::NotACamera { id: *id }),
                        None => return Err(CommandError::ObjectNotFound { id: *id }),
                    }
                }
                let previous = project.scene.active_camera;
                project.scene.active_camera = *id;
                Ok(Outcome {
                    inverse: Command::SetActiveCamera { id: previous },
                    affected: id.iter().copied().collect(),
                })
            }

            Command::SetViewport { viewport } => {
                let previous = project.viewport;
                project.viewport = *viewport;
                Ok(Outcome {
                    inverse: Command::SetViewport { viewport: previous },
                    affected: Vec::new(),
                })
            }

            Command::SetWorkspace { workspace } => {
                let previous = project.workspace;
                project.workspace = *workspace;
                Ok(Outcome {
                    inverse: Command::SetWorkspace {
                        workspace: previous,
                    },
                    affected: Vec::new(),
                })
            }

            Command::Batch { commands } => {
                let mut inverses = Vec::with_capacity(commands.len());
                let mut affected = Vec::new();
                for command in commands {
                    let applied = command.apply_inner(project)?;
                    affected.extend(applied.affected);
                    inverses.push(applied.inverse);
                }
                inverses.reverse();
                Ok(Outcome {
                    inverse: Command::Batch { commands: inverses },
                    affected,
                })
            }
        }
    }

    /// Object IDs this command names *before* it runs.
    ///
    /// A preview only: prefer [`Command::apply_with_effect`], which reports the
    /// IDs that actually changed (a duplicate's ID is created during `apply`).
    pub fn affected(&self) -> Vec<ObjectId> {
        match self {
            Command::CreateObject { object, .. } => vec![object.id],
            Command::DeleteObject { id } => vec![*id],
            Command::SetTransform { id, .. }
            | Command::RenameObject { id, .. }
            | Command::SetVisibility { id, .. }
            | Command::SetLocked { id, .. }
            | Command::SetCamera { id, .. }
            | Command::SetLight { id, .. }
            | Command::SetPrimitive { id, .. }
            | Command::SetTerrainZScale { id, .. }
            | Command::SetTerrainOverlay { id, .. }
            | Command::SetTerrainDem { id, .. }
            | Command::SetMeshMaterial { id, .. } => vec![*id],
            Command::SetRenderSettings { .. } => Vec::new(),
            Command::SetActiveCamera { id } => id.iter().copied().collect(),
            // The duplicate's id is only known once applied; resyncing the
            // original object is enough for an adapter to refresh the scene.
            Command::DuplicateObject { id, .. } => vec![*id],
            Command::CreatePrimitive { .. } | Command::ImportGltf { .. } => Vec::new(),
            Command::SetViewport { .. } | Command::SetWorkspace { .. } => Vec::new(),
            Command::AddAsset { .. } | Command::RemoveAsset { .. } => Vec::new(),
            Command::Batch { commands } => {
                let mut ids = Vec::new();
                for command in commands {
                    ids.extend(command.affected());
                }
                ids
            }
        }
    }

    /// Human-readable undo label, e.g. `Move Cube`.
    pub fn label(&self, project: &Project) -> String {
        let name_of = |id: &ObjectId| {
            project
                .scene
                .object(*id)
                .map(|object| object.name.clone())
                .unwrap_or_else(|| "object".to_owned())
        };
        match self {
            Command::CreateObject { object, .. } => format!("Create {}", object.name),
            Command::DeleteObject { id } => format!("Delete {}", name_of(id)),
            Command::SetTransform { id, .. } => format!("Move {}", name_of(id)),
            Command::RenameObject { id, .. } => format!("Rename {}", name_of(id)),
            Command::SetVisibility { id, visible } => {
                if *visible {
                    format!("Show {}", name_of(id))
                } else {
                    format!("Hide {}", name_of(id))
                }
            }
            Command::SetLocked { id, locked } => {
                if *locked {
                    format!("Lock {}", name_of(id))
                } else {
                    format!("Unlock {}", name_of(id))
                }
            }
            Command::SetCamera { id, .. } => format!("Edit camera {}", name_of(id)),
            Command::SetLight { id, .. } => format!("Edit light {}", name_of(id)),
            Command::SetPrimitive { id, .. } => format!("Reshape {}", name_of(id)),
            Command::SetTerrainZScale { id, .. } => {
                format!("Terrain scale {}", name_of(id))
            }
            Command::SetTerrainDem { id, .. } => format!("Change DEM of {}", name_of(id)),
            Command::SetRenderSettings { .. } => "Render settings".to_owned(),
            Command::SetMeshMaterial { id, .. } => format!("Material of {}", name_of(id)),
            Command::SetTerrainOverlay { id, overlay } => {
                if overlay.is_some() {
                    format!("Overlay on {}", name_of(id))
                } else {
                    format!("Remove overlay from {}", name_of(id))
                }
            }
            Command::SetActiveCamera { id } => match id {
                Some(id) => format!("Set active camera {}", name_of(id)),
                None => "Clear active camera".to_owned(),
            },
            Command::CreatePrimitive { primitive, .. } => {
                let label = match primitive {
                    Primitive::Cube { .. } => "Cube",
                    Primitive::Sphere { .. } => "Sphere",
                    Primitive::Plane { .. } => "Plane",
                };
                format!("Add {label}")
            }
            Command::ImportGltf { name, .. } => format!("Import {name}"),
            Command::DuplicateObject { id, .. } => format!("Duplicate {}", name_of(id)),
            Command::AddAsset { asset } => format!("Import {}", asset.name),
            Command::RemoveAsset { .. } => "Remove asset".to_owned(),
            Command::SetViewport { .. } => "Viewport settings".to_owned(),
            Command::SetWorkspace { workspace } => {
                format!("Switch to {} workspace", workspace.label())
            }
            Command::Batch { commands } => commands
                .first()
                .map(|command| command.label(project))
                .unwrap_or_else(|| "Edit".to_owned()),
        }
    }
}
