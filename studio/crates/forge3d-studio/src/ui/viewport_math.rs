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

/// Normalised coordinate inside an image drawn with `ObjectFit::Contain`.
///
/// A map still is letterboxed inside the viewport, so a click has to be mapped
/// through the image's own rectangle, not the viewport's — otherwise every pick
/// drifts once the two aspect ratios differ. `None` means the click landed in
/// the letterbox, where there is no image to pick.
pub fn contained_uv(
    image: (u32, u32),
    viewport: (f32, f32),
    point: (f32, f32),
) -> Option<(f32, f32)> {
    let (image_w, image_h) = (image.0 as f32, image.1 as f32);
    let (view_w, view_h) = viewport;
    if image_w < 1.0 || image_h < 1.0 || view_w < 1.0 || view_h < 1.0 {
        return None;
    }
    let scale = (view_w / image_w).min(view_h / image_h);
    let drawn = (image_w * scale, image_h * scale);
    let origin = ((view_w - drawn.0) * 0.5, (view_h - drawn.1) * 0.5);
    let u = (point.0 - origin.0) / drawn.0;
    let v = (point.1 - origin.1) / drawn.1;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    Some((u, v))
}

/// The field of view used to approximate orthographic projection.
pub const ORTHO_FOV_DEG: f32 = 2.0;

// --------------------------------------------------------------- camera rig

/// Which part of the camera rig a handle moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigHandleKind {
    /// The pivot: drag it to slide the whole orbit across the ground.
    Pivot,
    /// The ground ring: drag it to orbit around the pivot.
    Orbit,
    /// The vertical arm above the pivot: drag it to change elevation.
    Elevation,
}

/// A handle's position in viewport pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigHandle {
    pub kind: RigHandleKind,
    pub x: f32,
    pub y: f32,
}

/// Project a world point to viewport pixels, or `None` when it is behind the
/// camera or outside the clip range.
///
/// The engine renders the terrain with `perspective_rh` at `fov_deg` and the
/// same Z-up orbit as [`eye`]/[`view_basis`], so a handle projected here lands
/// exactly where the world point is drawn.
pub fn project(
    target: Vec2,
    phi_deg: f32,
    theta_deg: f32,
    radius: f32,
    fov_deg: f32,
    viewport: (f32, f32),
    world: Vec3,
) -> Option<(f32, f32)> {
    let (width, height) = viewport;
    if width < 2.0 || height < 2.0 {
        return None;
    }
    let origin = eye(target, phi_deg, theta_deg, radius);
    let (forward, right, up) = view_basis(phi_deg, theta_deg);
    let offset = world - origin;
    let depth = offset.dot(forward);
    if depth <= 1e-3 {
        return None;
    }
    let aspect = width / height;
    let tan_half = (fov_deg.to_radians() * 0.5).tan();
    let ndc_x = offset.dot(right) / (depth * tan_half * aspect);
    let ndc_y = offset.dot(up) / (depth * tan_half);
    Some((
        (ndc_x * 0.5 + 0.5) * width,
        // NDC y is up, pixels are down.
        (0.5 - ndc_y * 0.5) * height,
    ))
}

/// The three rig handles, positioned in viewport pixels.
///
/// The ring sits a quarter of the orbit radius from the pivot along the
/// camera's own right axis, so it always reads as "around here"; the elevation
/// arm is a quarter radius above the pivot. Both shrink with the orbit, which
/// keeps them clickable at any zoom.
pub fn rig_handles(
    target: Vec2,
    phi_deg: f32,
    theta_deg: f32,
    radius: f32,
    fov_deg: f32,
    viewport: (f32, f32),
) -> Vec<RigHandle> {
    let (_, right, _) = view_basis(phi_deg, theta_deg);
    let offset = radius * 0.25;
    let pivot_world = Vec3::new(target.x, target.y, 0.0);
    let ring_world = pivot_world + right * offset;
    let arm_world = pivot_world + Vec3::Z * offset;
    [
        (RigHandleKind::Pivot, pivot_world),
        (RigHandleKind::Orbit, ring_world),
        (RigHandleKind::Elevation, arm_world),
    ]
    .into_iter()
    .filter_map(|(kind, world)| {
        project(target, phi_deg, theta_deg, radius, fov_deg, viewport, world)
            .map(|(x, y)| RigHandle { kind, x, y })
    })
    .collect()
}

/// The handle under the pointer, within `radius_px`.
///
/// The pivot wins ties: it is the one a person grabs most, and it sits under
/// the other two at some angles.
pub fn hit_rig_handle(
    handles: &[RigHandle],
    point: (f32, f32),
    radius_px: f32,
) -> Option<RigHandleKind> {
    let mut best: Option<(f32, RigHandleKind)> = None;
    for handle in handles {
        let distance = ((handle.x - point.0).powi(2) + (handle.y - point.1).powi(2)).sqrt();
        if distance > radius_px {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_distance, best_kind)) => {
                // Pivot wins unless another handle is much closer.
                (handle.kind == RigHandleKind::Pivot && best_kind != RigHandleKind::Pivot)
                    || distance + 4.0 < best_distance
            }
        };
        if better {
            best = Some((distance, handle.kind));
        }
    }
    best.map(|(_, kind)| kind)
}

/// The orbit a rig handle manipulates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigOrbit {
    pub target: Vec2,
    pub phi_deg: f32,
    pub theta_deg: f32,
    pub radius: f32,
    pub fov_deg: f32,
}

/// Apply a drag of a rig handle, returning the new orbit.
///
/// `delta` is in pixels. Orbiting and elevation are rate-based (pixels per
/// degree), the pivot slides in the camera's ground plane so the drag follows
/// the pointer rather than the axes.
pub fn rig_drag(
    kind: RigHandleKind,
    orbit: RigOrbit,
    viewport: (f32, f32),
    delta: (f32, f32),
) -> RigOrbit {
    match kind {
        RigHandleKind::Orbit => RigOrbit {
            phi_deg: (orbit.phi_deg - delta.0 * 0.4).rem_euclid(360.0),
            ..orbit
        },
        RigHandleKind::Elevation => RigOrbit {
            theta_deg: (orbit.theta_deg - delta.1 * 0.3).clamp(1.0, 89.0),
            ..orbit
        },
        RigHandleKind::Pivot => {
            // Pixels per world unit at the pivot's depth. The vertical field of
            // view sets the scale; aspect does not enter it.
            let (_, height) = viewport;
            let tan_half = (orbit.fov_deg.to_radians() * 0.5).tan().max(1e-4);
            let per_pixel = 2.0 * orbit.radius * tan_half / height.max(1.0);
            let (_, right, up) = view_basis(orbit.phi_deg, orbit.theta_deg);
            let slide = right * (-delta.0 * per_pixel) + up * (delta.1 * per_pixel);
            // Keep the pivot on the ground plane: only its horizontal part
            // moves the orbit, which is what "drag the rig across the map"
            // means for a terrain.
            RigOrbit {
                target: Vec2::new(orbit.target.x + slide.x, orbit.target.y + slide.y),
                ..orbit
            }
        }
    }
}

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
    fn a_letterboxed_image_maps_clicks_through_its_own_rectangle() {
        // A 2:1 image in a square viewport: bars above and below.
        let image = (640, 320);
        let viewport = (800.0, 800.0);
        // Scale is min(1.25, 2.5) = 1.25, so the image is 800x400 and spans
        // y 200..600: the letterbox bars are 200 px tall.
        let (u, v) = contained_uv(image, viewport, (400.0, 400.0)).expect("centre is inside");
        close(u, 0.5, 1e-4);
        close(v, 0.5, 1e-4);
        // The top-left pixel of the image, not of the viewport.
        let (u, v) = contained_uv(image, viewport, (0.0, 200.0)).expect("corner is inside");
        close(u, 0.0, 1e-4);
        close(v, 0.0, 1e-4);
        // The letterbox is not the image.
        assert!(contained_uv(image, viewport, (400.0, 100.0)).is_none());
        assert!(contained_uv(image, viewport, (400.0, 700.0)).is_none());
        // Just inside the image is still inside.
        let (_, v) = contained_uv(image, viewport, (400.0, 201.0)).expect("one px inside works");
        close(v, 0.0025, 1e-3);
        // A degenerate frame has nothing to pick.
        assert!(contained_uv((0, 0), viewport, (400.0, 400.0)).is_none());
    }

    #[test]
    fn the_pivot_projects_to_the_middle_of_the_view() {
        let target = Vec2::new(120.0, -80.0);
        let (x, y) = project(
            target,
            30.0,
            55.0,
            2000.0,
            54.0,
            (800.0, 500.0),
            Vec3::new(target.x, target.y, 0.0),
        )
        .expect("the pivot is in front of the camera");
        close(x, 400.0, 0.5);
        close(y, 250.0, 0.5);
        // A point behind the camera is not projectable.
        let behind = project(
            target,
            30.0,
            55.0,
            2000.0,
            54.0,
            (800.0, 500.0),
            Vec3::new(target.x, target.y, 1e6),
        );
        assert!(behind.is_none(), "a point behind the eye has no pixel");
    }

    #[test]
    fn the_rig_offers_a_pivot_an_orbit_and_an_elevation_handle() {
        let handles = rig_handles(Vec2::ZERO, 20.0, 50.0, 4000.0, 54.0, (800.0, 500.0));
        let kinds: Vec<RigHandleKind> = handles.iter().map(|handle| handle.kind).collect();
        assert!(kinds.contains(&RigHandleKind::Pivot));
        assert!(kinds.contains(&RigHandleKind::Orbit));
        assert!(kinds.contains(&RigHandleKind::Elevation));
        // The elevation arm sits above the pivot on screen.
        let pivot = handles
            .iter()
            .find(|h| h.kind == RigHandleKind::Pivot)
            .unwrap();
        let arm = handles
            .iter()
            .find(|h| h.kind == RigHandleKind::Elevation)
            .unwrap();
        assert!(arm.y < pivot.y, "the arm is above the pivot");
    }

    #[test]
    fn a_click_finds_the_nearest_handle_and_the_pivot_wins() {
        let handles = vec![
            RigHandle {
                kind: RigHandleKind::Pivot,
                x: 400.0,
                y: 250.0,
            },
            RigHandle {
                kind: RigHandleKind::Orbit,
                x: 500.0,
                y: 250.0,
            },
            RigHandle {
                kind: RigHandleKind::Elevation,
                x: 400.0,
                y: 150.0,
            },
        ];
        assert_eq!(
            hit_rig_handle(&handles, (405.0, 252.0), 14.0),
            Some(RigHandleKind::Pivot)
        );
        assert_eq!(
            hit_rig_handle(&handles, (498.0, 251.0), 14.0),
            Some(RigHandleKind::Orbit)
        );
        // Nothing within reach.
        assert_eq!(hit_rig_handle(&handles, (700.0, 480.0), 14.0), None);
        // The pivot wins the tie at the same spot.
        let stacked = vec![
            RigHandle {
                kind: RigHandleKind::Orbit,
                x: 400.0,
                y: 250.0,
            },
            RigHandle {
                kind: RigHandleKind::Pivot,
                x: 401.0,
                y: 250.0,
            },
        ];
        assert_eq!(
            hit_rig_handle(&stacked, (400.5, 250.0), 14.0),
            Some(RigHandleKind::Pivot)
        );
    }

    #[test]
    fn dragging_each_handle_moves_only_what_it_owns() {
        let target = Vec2::ZERO;
        let orbit = RigOrbit {
            target,
            phi_deg: 40.0,
            theta_deg: 50.0,
            radius: 1000.0,
            fov_deg: 54.0,
        };
        let viewport = (800.0, 500.0);

        let orbited = rig_drag(RigHandleKind::Orbit, orbit, viewport, (10.0, 0.0));
        close(orbited.target.x, 0.0, 1e-4);
        close(orbited.phi_deg, 36.0, 1e-3);
        close(orbited.theta_deg, 50.0, 1e-4);

        let raised = rig_drag(RigHandleKind::Elevation, orbit, viewport, (0.0, 10.0));
        close(raised.phi_deg, 40.0, 1e-4);
        close(raised.theta_deg, 47.0, 1e-3);

        let slid = rig_drag(RigHandleKind::Pivot, orbit, viewport, (20.0, 0.0));
        close(slid.phi_deg, 40.0, 1e-4);
        close(slid.theta_deg, 50.0, 1e-4);
        close(slid.radius, 1000.0, 1e-4);
        assert!(
            slid.target.length() > 1.0,
            "the pivot slid across the ground"
        );
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
