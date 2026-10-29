//! Viewport camera maths: the engine's orbit convention, picking, zoom and the
//! orthographic approximation.
//!
//! The engine's viewer orbit is **Z-up** with `theta` measured from +Z:
//! `eye = target + r * (sinθ cosφ, sinθ sinφ, cosθ)`, target on the ground
//! plane, `theta = 0` is straight down and `theta = 90°` is the horizon. The UI
//! shows "elevation" (90 − θ) because that is what a person expects, and these
//! functions keep the conversion in one place.
//!
//! Everything here is pure and unit-tested; the panels only convert screen
//! coordinates to NDC and hand them over.

use glam::{Vec2, Vec3};

/// Engine theta for a given user-facing elevation (both in degrees).
pub fn theta_from_elevation(elevation_deg: f32) -> f32 {
    (90.0 - elevation_deg).clamp(1.0, 89.0)
}

/// User-facing elevation for an engine theta (both in degrees).
pub fn elevation_from_theta(theta_deg: f32) -> f32 {
    90.0 - theta_deg
}

/// Camera eye position for the engine's orbit, in world units (target on z=0).
pub fn eye(target: Vec2, phi_deg: f32, theta_deg: f32, radius: f32) -> Vec3 {
    let phi = phi_deg.to_radians();
    let theta = theta_deg.to_radians();
    Vec3::new(
        target.x + radius * theta.sin() * phi.cos(),
        target.y + radius * theta.sin() * phi.sin(),
        radius * theta.cos(),
    )
}

/// Basis of the view: `(forward, right, up)` for the current orbit.
pub fn view_basis(phi_deg: f32, theta_deg: f32) -> (Vec3, Vec3, Vec3) {
    let phi = phi_deg.to_radians();
    let theta = theta_deg.to_radians();
    // From the eye toward the target.
    let forward = Vec3::new(
        -theta.sin() * phi.cos(),
        -theta.sin() * phi.sin(),
        -theta.cos(),
    );
    // Straight down degenerates the cross product; fall back to +Y as "up".
    let reference_up = if forward.z.abs() > 0.999 {
        Vec3::new(0.0, 1.0, 0.0)
    } else {
        Vec3::Z
    };
    let right = forward.cross(reference_up).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();
    (forward, right, up)
}

/// Direction of the ray through a normalised device coordinate.
///
/// `ndc` is in [-1, 1] with y up, so the caller converts from pixels.
pub fn ray_direction(phi_deg: f32, theta_deg: f32, fov_deg: f32, aspect: f32, ndc: Vec2) -> Vec3 {
    let (forward, right, up) = view_basis(phi_deg, theta_deg);
    let tan_half = (fov_deg.to_radians() * 0.5).tan();
    (forward + right * (ndc.x * tan_half * aspect) + up * (ndc.y * tan_half)).normalize_or_zero()
}

/// Where the cursor ray meets the ground plane (z = 0), if it points downward.
///
/// Returns world `(x, y)`. Rays at or above the horizon miss: the caller keeps
/// the previous point instead of inventing one.
pub fn pick_ground(
    target: Vec2,
    phi_deg: f32,
    theta_deg: f32,
    radius: f32,
    fov_deg: f32,
    aspect: f32,
    ndc: Vec2,
) -> Option<Vec2> {
    let origin = eye(target, phi_deg, theta_deg, radius);
    let direction = ray_direction(phi_deg, theta_deg, fov_deg, aspect, ndc);
    if direction.z >= -1e-4 {
        return None;
    }
    let distance = -origin.z / direction.z;
    if !distance.is_finite() || distance < 0.0 {
        return None;
    }
    let hit = origin + direction * distance;
    Some(Vec2::new(hit.x, hit.y))
}

/// One wheel step toward the cursor.
///
/// Blender zooms about the pointer: the ground point under the cursor stays put
/// while everything else scales. Returns the new target and radius.
pub fn zoom_to_cursor(
    target: Vec2,
    ground_under_cursor: Vec2,
    radius: f32,
    factor: f32,
) -> (Vec2, f32) {
    let radius = (radius * factor).max(1.0);
    // `factor < 1` zooms in; the target moves toward the cursor by the same
    // proportion the radius shrank.
    let new_target = ground_under_cursor + (target - ground_under_cursor) * factor.max(0.0);
    (new_target, radius)
}

/// Radius that keeps the framing when switching to the narrow "orthographic"
/// field of view (`ortho_fov`), and back.
pub fn radius_for_fov(radius: f32, fov_deg: f32, new_fov_deg: f32) -> f32 {
    let old = (fov_deg.to_radians() * 0.5).tan();
    let new = (new_fov_deg.to_radians() * 0.5).tan();
    if new <= 1e-6 || old <= 1e-6 {
        return radius;
    }
    radius * (old / new)
}

/// The field of view used to approximate orthographic projection.
pub const ORTHO_FOV_DEG: f32 = 2.0;

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn close(a: f32, b: f32, tolerance: f32) {
        assert!(
            (a - b).abs() <= tolerance,
            "{a} != {b} (tolerance {tolerance})"
        );
    }

    #[test]
    fn the_engine_convention_is_polar_from_up() {
        // theta = 0 -> straight above the target.
        let top = eye(Vec2::ZERO, 0.0, 0.0, 100.0);
        close(top.x, 0.0, 1e-3);
        close(top.y, 0.0, 1e-3);
        close(top.z, 100.0, 1e-3);
        // theta = 90 -> on the horizon along phi.
        let side = eye(Vec2::ZERO, 0.0, 90.0, 100.0);
        close(side.z, 0.0, 1e-3);
        close(side.x, 100.0, 1e-3);
        // The UI's elevation is the complement.
        close(elevation_from_theta(14.0), 76.0, 1e-4);
        close(theta_from_elevation(76.0), 14.0, 1e-4);
    }

    #[test]
    fn a_centre_click_hits_the_target() {
        let target = Vec2::new(30.0, -12.0);
        let hit = pick_ground(target, 135.0, 40.0, 500.0, 54.0, 1.4, Vec2::ZERO)
            .expect("centre ray hits the ground");
        close(hit.x, target.x, 0.5);
        close(hit.y, target.y, 0.5);
    }

    #[test]
    fn a_click_off_centre_lands_away_from_the_target() {
        let target = Vec2::ZERO;
        // Looking from +X (phi 0, theta 45): screen-right is world +Y, and a
        // click above the horizon moves the ground point away from the eye.
        let right = pick_ground(target, 0.0, 45.0, 500.0, 54.0, 1.4, Vec2::new(0.5, 0.0))
            .expect("ray hits");
        assert!(
            right.y > 1.0,
            "a click to the right lands along +Y, got {right:?}"
        );
        assert!(
            right.x.abs() < 1.0,
            "screen-right must not move along X here"
        );
        let above = pick_ground(target, 0.0, 45.0, 500.0, 54.0, 1.4, Vec2::new(0.0, 0.5))
            .expect("ray hits");
        assert!(
            above.length() > right.length(),
            "a click above the centre lands further away, got {above:?}"
        );
    }

    #[test]
    fn a_ray_above_the_horizon_misses() {
        // theta = 89 looks almost along the ground; the top of a tall frame
        // points above the horizon and must return nothing.
        let miss = pick_ground(Vec2::ZERO, 0.0, 89.0, 500.0, 60.0, 1.0, Vec2::new(0.0, 1.0));
        assert!(miss.is_none(), "no invented ground point");
    }

    #[test]
    fn zooming_toward_the_cursor_keeps_that_point_still() {
        let target = Vec2::ZERO;
        let cursor = Vec2::new(100.0, 50.0);
        let (new_target, radius) = zoom_to_cursor(target, cursor, 400.0, 0.5);
        close(radius, 200.0, 1e-3);
        // Zooming in pulls the target toward the cursor by the same factor.
        close(new_target.x, 50.0, 1e-3);
        close(new_target.y, 25.0, 1e-3);
        // Zooming back out returns to where it started (same rule, factor 2).
        let (back, radius) = zoom_to_cursor(new_target, cursor, radius, 2.0);
        close(back.x, target.x, 1e-3);
        close(back.y, target.y, 1e-3);
        close(radius, 400.0, 1e-3);
    }

    #[test]
    fn the_orthographic_swap_keeps_the_framing() {
        let radius = 1000.0;
        let fov = 54.0;
        let ortho = radius_for_fov(radius, fov, ORTHO_FOV_DEG);
        assert!(ortho > radius, "a narrower field of view needs distance");
        // Round-tripping the two fields of view returns the original radius.
        close(radius_for_fov(ortho, ORTHO_FOV_DEG, fov), radius, 0.5);
    }
}
