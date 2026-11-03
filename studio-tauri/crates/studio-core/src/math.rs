//! Minimal 3D math for the canonical model.
//!
//! Deliberately dependency-free: the model stores plain data, and any viewport
//! adapter converts it to its own math types. Rotation is Euler degrees in
//! X, Y, Z order, applied as `Rz * Ry * Rx` to a camera that looks down -Z.
//! Both the TypeScript preview and any future Three.js viewport must use the
//! same convention; `scene::tests` pins the camera default against it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const ONE: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Default for Vec3 {
    fn default() -> Self {
        Self::ZERO
    }
}

/// Local transform of a scene object (parent-relative).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    pub translation: Vec3,
    pub rotation_deg: Vec3,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation_deg: Vec3::ZERO,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    /// Reject NaN/infinite values before they reach a renderer, where they
    /// become invisible objects and un-diagnosable blank frames.
    pub fn validate(&self) -> Result<(), String> {
        if !self.translation.is_finite() {
            return Err("translation must be finite".to_owned());
        }
        if !self.rotation_deg.is_finite() {
            return Err("rotation must be finite".to_owned());
        }
        if !self.scale.is_finite() {
            return Err("scale must be finite".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_transform_is_identity() {
        let t = Transform::default();
        assert_eq!(t.translation, Vec3::ZERO);
        assert_eq!(t.rotation_deg, Vec3::ZERO);
        assert_eq!(t.scale, Vec3::ONE);
    }

    #[test]
    fn non_finite_transforms_are_rejected() {
        let mut t = Transform::default();
        t.translation.x = f32::NAN;
        assert!(t.validate().is_err());
        t.translation.x = 0.0;
        t.scale.y = f32::INFINITY;
        assert!(t.validate().is_err());
    }
}
