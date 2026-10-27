//! U3: viewport tools, 3D navigation gizmo, and HUD.
//!
//! Everything here is pure maths or egui painting driven by explicit state —
//! no engine calls — so the gizmo and HUD are unit-tested without a GPU, per
//! GUI.md Part VII. The engine's orbit convention is polar: `theta_deg = 0`
//! looks straight down, `90` sits on the horizon (see `terrain_demo.py`'s
//! camera documentation, which the render pipeline follows verbatim).

use glam::Vec3;

use crate::actions::Action;
use studio_project::CameraParams;

/// Which interactive viewport tool owns the left mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportTool {
    /// Left-drag orbits (default).
    Orbit,
    /// Left-drag pans the orbit target on the ground plane.
    Pan,
    /// Left-drag zooms (drag up = closer).
    Zoom,
}

impl ViewportTool {
    pub fn from_action(action: Action) -> Option<Self> {
        match action {
            Action::ToolOrbit => Some(ViewportTool::Orbit),
            Action::ToolPan => Some(ViewportTool::Pan),
            Action::ToolZoom => Some(ViewportTool::Zoom),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewportTool::Orbit => "Orbit (drag)",
            ViewportTool::Pan => "Pan (drag, or middle-drag anywhere)",
            ViewportTool::Zoom => "Zoom (drag, or scroll anywhere)",
        }
    }
}

// ---------------------------------------------------------------------------
// Camera basis (gizmo maths)
// ---------------------------------------------------------------------------

/// Camera basis in engine orbit conventions: `eye = target + radius *
/// (sin θ cos φ, cos θ, sin θ sin φ)`, view direction towards the target.
/// Returns `(right, up, forward)` where `forward` points from the scene to
/// the camera (the axes drawn towards the viewer).
pub fn camera_basis(phi_deg: f32, theta_deg: f32) -> (Vec3, Vec3, Vec3) {
    let (phi, theta) = (phi_deg.to_radians(), theta_deg.to_radians());
    let towards_camera = Vec3::new(
        theta.sin() * phi.cos(),
        theta.cos(),
        theta.sin() * phi.sin(),
    );
    let forward = towards_camera; // axis pointing at the viewer
    let world_up = Vec3::Y;
    // Near the poles the cross with Y degenerates; fall back to -Z like the
    // engine's own look-at does.
    let right = if forward.dot(world_up).abs() > 0.999 {
        forward.cross(Vec3::Z).normalize_or_zero()
    } else {
        forward.cross(world_up).normalize_or_zero()
    };
    let up = right.cross(forward).normalize_or_zero();
    (right, up, forward)
}

/// Project a world axis endpoint onto gizmo-local screen coordinates
/// (y-down, matching egui). The gizmo centre is the origin.
pub fn axis_screen_dir(basis: (Vec3, Vec3, Vec3), axis: Vec3) -> glam::Vec2 {
    let (right, up, forward) = basis;
    let x = axis.dot(right);
    let y = -axis.dot(up);
    // Depth decides draw order and dimming; direction is all the hit test
    // needs, but scale by a mild perspective term so the front hemisphere
    // reaches further than the back one (Blender's gizmo behaviour).
    let depth = axis.dot(forward);
    let scale = 1.0 + 0.35 * depth;
    glam::vec2(x, y) * scale
}

/// The clickable gizmo zones, Blender-style: six axis handles plus a centre
/// ball for isometric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoZone {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
    Center,
}

/// Hit-test the gizmo: the zone whose handle is nearest the cursor, provided
/// it is within `pick_radius_px` of the handle's screen position.
pub fn gizmo_zone_at(
    cursor: glam::Vec2,
    center_px: glam::Vec2,
    radius_px: f32,
    basis: (Vec3, Vec3, Vec3),
) -> Option<GizmoZone> {
    let axes = [
        (GizmoZone::PosX, Vec3::X),
        (GizmoZone::NegX, -Vec3::X),
        (GizmoZone::PosY, Vec3::Y),
        (GizmoZone::NegY, -Vec3::Y),
        (GizmoZone::PosZ, Vec3::Z),
        (GizmoZone::NegZ, -Vec3::Z),
    ];
    let mut best: Option<(GizmoZone, f32)> = None;
    for (zone, axis) in axes {
        let handle = center_px + axis_screen_dir(basis, axis) * radius_px;
        let dist = cursor.distance(handle);
        if dist <= 10.0 && best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((zone, dist));
        }
    }
    let center_dist = cursor.distance(center_px);
    if center_dist <= 12.0 && best.map(|(_, d)| center_dist <= d).unwrap_or(true) {
        return Some(GizmoZone::Center);
    }
    best.map(|(zone, _)| zone)
}

/// Camera for a gizmo snap zone: angles change, everything else is kept.
/// Engine convention: theta 0 = top-down, 90 = horizon; near-pole values are
/// clamped the way the engine's own validation clamps them.
pub fn snap_camera(zone: GizmoZone, current: &CameraParams) -> CameraParams {
    let mut camera = *current;
    match zone {
        GizmoZone::PosY => {
            camera.phi_deg = current.phi_deg; // keep spin when dropping on top
            camera.theta_deg = 2.0;
        }
        GizmoZone::NegY => {
            camera.phi_deg = current.phi_deg;
            camera.theta_deg = 178.0; // bottom view: past the horizon
        }
        GizmoZone::PosX => {
            camera.phi_deg = 0.0;
            camera.theta_deg = 84.0;
        }
        GizmoZone::NegX => {
            camera.phi_deg = 180.0;
            camera.theta_deg = 84.0;
        }
        GizmoZone::PosZ => {
            camera.phi_deg = 90.0;
            camera.theta_deg = 84.0;
        }
        GizmoZone::NegZ => {
            camera.phi_deg = 270.0;
            camera.theta_deg = 84.0;
        }
        GizmoZone::Center => {
            camera.phi_deg = 135.0;
            camera.theta_deg = 45.0;
        }
    }
    camera
}

/// Camera angles for the menu/keyboard presets — the same mapping the gizmo
/// uses, so the two cannot drift.
pub fn preset_camera(action: Action, current: &CameraParams) -> Option<CameraParams> {
    let zone = match action {
        Action::CameraTop => GizmoZone::PosY,
        Action::CameraFront => GizmoZone::NegZ,
        Action::CameraRight => GizmoZone::PosX,
        Action::CameraIso => GizmoZone::Center,
        Action::CameraReset => GizmoZone::Center,
        _ => return None,
    };
    Some(snap_camera(zone, current))
}

// ---------------------------------------------------------------------------
// HUD maths
// ---------------------------------------------------------------------------

/// World-space width of the viewport at the orbit target's plane, from the
/// orbit camera maths: `width = 2 * radius * tan(fov/2)`.
pub fn world_width_at_target(camera: &CameraParams) -> f32 {
    2.0 * camera.radius * (camera.fov_deg.to_radians() * 0.5).tan()
}

/// A "nice" scale-bar length (1/2/5 × 10^n) that fits within `max_fraction`
/// of the viewport's world width. Returns `(world_length, label)`.
pub fn scale_bar(world_width: f32, max_fraction: f32) -> (f32, String) {
    let budget = world_width * max_fraction.clamp(0.05, 1.0);
    // Largest nice value <= budget.
    let exponent = budget.abs().log10().floor();
    for e in [exponent, exponent - 1.0] {
        for mantissa in [5.0, 2.0, 1.0] {
            let value = mantissa * 10f32.powf(e);
            if value <= budget && value > 0.0 {
                return (value, format_length(value));
            }
        }
    }
    (budget.max(0.0), format_length(budget))
}

/// 3-significant-digit length formatting (Rust has no `g` format trait).
fn format_length(value: f32) -> String {
    let digits = |v: f32| {
        if v.abs() >= 100.0 {
            0
        } else if v.abs() >= 10.0 {
            1
        } else {
            2
        }
    };
    if value >= 1000.0 {
        let km = value / 1000.0;
        format!("{:.*} km", digits(km), km)
    } else {
        format!("{:.*} m", digits(value), value)
    }
}

/// Zoom percentage relative to the fit radius (100 % = framed).
pub fn zoom_percent(radius: f32, fit_radius: f32) -> f32 {
    if radius <= 0.0 {
        return 100.0;
    }
    (fit_radius / radius * 100.0).clamp(1.0, 10_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn camera_basis_is_orthonormal_and_matches_engine_conventions() {
        // theta 0 = top-down: forward points straight up towards the camera.
        let (right, up, forward) = camera_basis(0.0, 0.0);
        assert!(approx(forward.y, 1.0), "theta 0 forward is +Y");
        assert!(approx(up.length(), 1.0) && approx(right.length(), 1.0));
        // All pairs orthogonal.
        assert!(right.dot(up).abs() < 1e-4);
        assert!(right.dot(forward).abs() < 1e-4);
        assert!(up.dot(forward).abs() < 1e-4);
        // phi 0/theta 90: camera on +X horizon, forward is +X.
        let (_, _, forward) = camera_basis(0.0, 90.0);
        assert!(approx(forward.x, 1.0) && approx(forward.y, 0.0));
        // phi 90/theta 90: camera on +Z horizon.
        let (_, _, forward) = camera_basis(90.0, 90.0);
        assert!(approx(forward.z, 1.0) && approx(forward.x, 0.0));
    }

    #[test]
    fn gizmo_zone_at_hits_the_front_handle_not_the_back_one() {
        // Generic oblique camera so the six handles land at distinct screen
        // positions (a camera exactly on an axis collapses the ring).
        let basis = camera_basis(45.0, 60.0);
        let center = glam::vec2(100.0, 100.0);
        let radius = 40.0;
        // Cursor at a handle's projected position hits that handle.
        for (zone, axis) in [
            (GizmoZone::PosX, Vec3::X),
            (GizmoZone::NegX, -Vec3::X),
            (GizmoZone::PosZ, Vec3::Z),
            (GizmoZone::NegZ, -Vec3::Z),
        ] {
            let handle = center + {
                let d = axis_screen_dir(basis, axis);
                glam::vec2(d.x, d.y)
            } * radius;
            assert_eq!(
                gizmo_zone_at(handle, center, radius, basis),
                Some(zone),
                "handle for {zone:?} must be hit"
            );
        }
        // Center ball wins at the middle and ties against handles.
        assert_eq!(
            gizmo_zone_at(center, center, radius, basis),
            Some(GizmoZone::Center)
        );
        // Nothing nearby: no zone.
        assert_eq!(
            gizmo_zone_at(glam::vec2(500.0, 500.0), center, radius, basis),
            None
        );
    }

    #[test]
    fn snap_camera_preserves_zoom_and_target() {
        let mut current = CameraParams::default();
        current.radius = 1234.0;
        current.target_x = 5.0;
        current.target_z = -7.0;
        let snapped = snap_camera(GizmoZone::PosY, &current);
        assert!(approx(snapped.theta_deg, 2.0), "top is theta ~0");
        assert!(approx(snapped.radius, 1234.0));
        assert!(approx(snapped.target_x, 5.0));
        let front = snap_camera(GizmoZone::NegZ, &current);
        assert!(approx(front.phi_deg, 270.0), "front looks from -Z");
        assert!(approx(front.theta_deg, 84.0), "front is near-horizon");
    }

    #[test]
    fn presets_and_gizmo_share_one_mapping() {
        let current = CameraParams::default();
        let top = preset_camera(Action::CameraTop, &current).expect("top preset");
        assert!(top.theta_deg < 5.0, "CameraTop must be top-down (theta ~0)");
        let front = preset_camera(Action::CameraFront, &current).expect("front preset");
        assert!(front.theta_deg > 80.0, "CameraFront must be near-horizon");
        let right = preset_camera(Action::CameraRight, &current).expect("right");
        assert!(
            approx(right.phi_deg, 0.0)
                || approx(right.phi_deg, 360.0 - 0.0)
                || right.phi_deg.abs() < 1e-4
        );
        let iso = preset_camera(Action::CameraIso, &current).expect("iso");
        assert!(approx(iso.theta_deg, 45.0) && approx(iso.phi_deg, 135.0));
    }

    #[test]
    fn scale_bar_picks_nice_values() {
        // A 2000 m wide view: the bar should be a clean 200 m (20 %).
        let (value, label) = scale_bar(2000.0, 0.2);
        assert!(approx(value, 200.0), "got {value}");
        assert_eq!(label, "200 m");
        // Kilometres label past 1000 m.
        let (value, label) = scale_bar(20_000.0, 0.2);
        assert!(approx(value, 2000.0), "got {value}");
        assert_eq!(label, "2.00 km");
    }

    #[test]
    fn world_width_and_zoom_percent_match_camera_maths() {
        let mut camera = CameraParams::default();
        camera.radius = 100.0;
        camera.fov_deg = 60.0;
        let width = world_width_at_target(&camera);
        assert!(approx(width, 2.0 * 100.0 * (30f32).to_radians().tan()));
        assert!(approx(zoom_percent(100.0, 100.0), 100.0));
        assert!(approx(zoom_percent(50.0, 100.0), 200.0));
        assert!(approx(zoom_percent(200.0, 100.0), 50.0));
    }
}
