//! The command layer: every mutation is a value that can be applied and
//! inverted.
//!
//! `Command::apply` mutates the model and returns the command that reverses it.
//! The editor stores that inverse on the undo stack, so undo and redo are the
//! same code path as editing — there is no separate "restore" logic to drift.

use serde::{Deserialize, Serialize};

use crate::ids::ObjectId;
use crate::math::Transform;
use crate::project::{Project, ViewportState};
use crate::scene::{validate_name, CameraObject, LightObject, Primitive, SceneObject};

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

    /// Point the viewport and Forge3D handoff at a camera (or none).
    SetActiveCamera {
        id: Option<ObjectId>,
    },

    /// Viewport view state (grid, shading mode). Not scene content, but part of
    /// the persisted project, so it goes through the same command path.
    SetViewport {
        viewport: ViewportState,
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
        self.apply_inner(project).map(|applied| applied.inverse)
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
                let previous = match &object.kind {
                    crate::scene::ObjectKind::Mesh(mesh) => mesh.primitive.clone(),
                    _ => return Err(CommandError::NotAMesh { id: *id }),
                };
                // Validate by building the mesh payload and validating it as
                // part of a whole object, so primitive rules live in one place.
                let mut probe = object.clone();
                probe.kind = crate::scene::ObjectKind::Mesh(crate::scene::MeshObject {
                    primitive: primitive.clone(),
                });
                probe
                    .validate()
                    .map_err(|reason| CommandError::invalid("primitive", reason))?;
                if let Some(object) = project.scene.object_mut(*id) {
                    object.kind = crate::scene::ObjectKind::Mesh(crate::scene::MeshObject {
                        primitive: primitive.clone(),
                    });
                }
                Ok(Outcome {
                    inverse: Command::SetPrimitive {
                        id: *id,
                        primitive: previous,
                    },
                    affected: vec![*id],
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

    /// Object IDs this command touches — what a viewport adapter re-syncs.
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
            | Command::SetPrimitive { id, .. } => vec![*id],
            Command::SetActiveCamera { id } => id.iter().copied().collect(),
            Command::SetViewport { .. } => Vec::new(),
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
            Command::SetActiveCamera { id } => match id {
                Some(id) => format!("Set active camera {}", name_of(id)),
                None => "Clear active camera".to_owned(),
            },
            Command::SetViewport { .. } => "Viewport settings".to_owned(),
            Command::Batch { commands } => commands
                .first()
                .map(|command| command.label(project))
                .unwrap_or_else(|| "Edit".to_owned()),
        }
    }
}
