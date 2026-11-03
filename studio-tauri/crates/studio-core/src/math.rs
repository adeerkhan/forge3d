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
    /// Rotate a local vector by this transform's Euler angles (X, then Y, then
    /// Z — the model's `Rz·Ry·Rx` convention, shared with the viewport).
    pub fn rotate_vector(&self, vector: Vec3) -> Vec3 {
        let (sin_x, cos_x) = (
            self.rotation_deg.x.to_radians().sin(),
            self.rotation_deg.x.to_radians().cos(),
        );
        let (sin_y, cos_y) = (
            self.rotation_deg.y.to_radians().sin(),
            self.rotation_deg.y.to_radians().cos(),
        );
        let (sin_z, cos_z) = (
            self.rotation_deg.z.to_radians().sin(),
            self.rotation_deg.z.to_radians().cos(),
        );

        // Rx
        let y = vector.y * cos_x - vector.z * sin_x;
        let z = vector.y * sin_x + vector.z * cos_x;
        let x = vector.x;

        // Ry — both outputs must use the *old* x and z, hence the temporaries.
        let x_ry = x * cos_y + z * sin_y;
        let z_ry = -x * sin_y + z * cos_y;

        // Rz
        let x_rz = x_ry * cos_z - y * sin_z;
        let y_rz = x_ry * sin_z + y * cos_z;

        Vec3::new(x_rz, y_rz, z_ry)
    }

    /// Local forward axis (`-Z`) in parent space.
    pub fn forward(&self) -> Vec3 {
        self.rotate_vector(Vec3::new(0.0, 0.0, -1.0))
    }

    /// Local up axis (`+Y`) in parent space.
    pub fn up(&self) -> Vec3 {
        self.rotate_vector(Vec3::new(0.0, 1.0, 0.0))
    }

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
    fn euler_rotation_matches_the_documented_order() {
        let assert_close = |actual: Vec3, expected: Vec3| {
            let close = (actual.x - expected.x).abs() < 1e-5
                && (actual.y - expected.y).abs() < 1e-5
                && (actual.z - expected.z).abs() < 1e-5;
            assert!(close, "expected {expected:?}, got {actual:?}");
        };

        // Yaw only: -Z turns to -X.
        let yaw = Transform {
            rotation_deg: Vec3::new(0.0, 90.0, 0.0),
            ..Transform::default()
        };
        assert_close(yaw.forward(), Vec3::new(-1.0, 0.0, 0.0));

        // Pitch only: -Z turns to -Y (looking down).
        let pitch = Transform {
            rotation_deg: Vec3::new(-90.0, 0.0, 0.0),
            ..Transform::default()
        };
        assert_close(pitch.forward(), Vec3::new(0.0, -1.0, 0.0));
        assert_close(pitch.up(), Vec3::new(0.0, 0.0, -1.0));

        // Pitch then yaw, the starter sun's rotation: the Y output must not be
        // contaminated by the yaw step (regression: a shadowed `x` did that).
        let sun = Transform {
            rotation_deg: Vec3::new(-50.0, 30.0, 0.0),
            ..Transform::default()
        };
        let forward = sun.forward();
        assert_close(
            forward,
            Vec3::new(
                -0.5 * 50f32.to_radians().cos(),
                -50f32.to_radians().sin(),
                -0.866_025_4 * 50f32.to_radians().cos(),
            ),
        );
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
