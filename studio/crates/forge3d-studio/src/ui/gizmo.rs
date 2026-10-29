//! The viewport's navigation gizmo: a Blender-style axis widget in the corner.
//!
//! Six axis handles plus a centre ball. Clicking a handle snaps the camera to
//! that orthogonal view; the centre snaps to isometric. The handle positions
//! are the world axes projected into the current camera's screen basis, so the
//! widget turns with the view instead of being a static picture.
//!
//! The maths is pure and unit-tested: `axis_offsets` maps the document camera
//! to six screen-space unit vectors.

use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::Studio;

/// Radius of the widget in pixels (handles are placed on this circle * scale).
const RADIUS: f32 = 26.0;

/// One clickable axis handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisHandle {
    /// Unit vector in screen space: x right, y down.
    pub dir: (f32, f32),
    /// How much the axis points at the viewer (1 = straight at you, 0 = in
    /// the screen plane). A near axis is drawn bigger, as Blender does, so the
    /// two handles that collapse to the centre in an orthogonal view stay
    /// distinguishable.
    pub depth: f32,
    /// Camera preset this handle snaps to.
    pub phi_deg: f32,
    pub theta_deg: f32,
    pub positive: bool,
    pub axis: usize,
}

/// Project the six world axes into the camera's screen basis.
///
/// The camera orbits a Z-up scene: `phi` turns around Z, `theta` is elevation.
/// Screen right is `(-sin φ, cos φ, 0)`; screen up is the cross of forward and
/// right. A positive axis pointing at the viewer is drawn larger by the caller.
pub fn axis_offsets(phi_deg: f32, theta_deg: f32) -> [AxisHandle; 6] {
    let phi = phi_deg.to_radians();
    let theta = theta_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();
    let (sin_t, cos_t) = theta.sin_cos();

    // Forward: from the target toward the eye.
    let forward = [cos_t * cos_p, cos_t * sin_p, sin_t];
    let right = [-sin_p, cos_p, 0.0];
    // Screen up is forward × right (right × forward points the other way, which
    // draws the world upside down).
    let up = [
        forward[1] * right[2] - forward[2] * right[1],
        forward[2] * right[0] - forward[0] * right[2],
        forward[0] * right[1] - forward[1] * right[0],
    ];

    let axes = [
        ([1.0, 0.0, 0.0], 0usize),
        ([0.0, 1.0, 0.0], 1),
        ([0.0, 0.0, 1.0], 2),
    ];

    let mut handles = Vec::with_capacity(6);
    for (axis, index) in axes {
        let sx = axis[0] * right[0] + axis[1] * right[1] + axis[2] * right[2];
        let sy = axis[0] * up[0] + axis[1] * up[1] + axis[2] * up[2];
        // Positive side.
        let depth = axis[0] * forward[0] + axis[1] * forward[1] + axis[2] * forward[2];
        let (positive_phi, positive_theta) = preset_for(index, true).unwrap_or((45.0, 55.0));
        let negative = preset_for(index, false);
        handles.push(AxisHandle {
            dir: (sx, -sy),
            depth,
            phi_deg: positive_phi,
            theta_deg: positive_theta,
            positive: true,
            axis: index,
        });
        handles.push(AxisHandle {
            dir: (-sx, sy),
            depth: -depth,
            phi_deg: negative.map(|preset| preset.0).unwrap_or(45.0),
            theta_deg: negative.map(|preset| preset.1).unwrap_or(55.0),
            positive: false,
            axis: index,
        });
    }
    [
        handles[0], handles[1], handles[2], handles[3], handles[4], handles[5],
    ]
}

/// The orbit angles that look along `axis` from its side, in the engine's
/// convention (theta polar from +Z, so 89 is the horizon).
///
/// Looking from below is not possible: the viewer orbits above the ground
/// plane, so the negative Z handle is shown but does nothing.
fn preset_for(axis: usize, positive: bool) -> Option<(f32, f32)> {
    match (axis, positive) {
        // +Z looks straight down; there is no under-ground view.
        (2, true) => Some((90.0, 1.0)),
        (2, false) => None,
        // +X is the right view, -X the left.
        (0, true) => Some((0.0, 89.0)),
        (0, false) => Some((180.0, 89.0)),
        // +Y is the back view, -Y the front.
        (1, true) => Some((90.0, 89.0)),
        (1, false) => Some((270.0, 89.0)),
        _ => None,
    }
}

fn axis_color(axis: usize, positive: bool, cx: &App) -> Hsla {
    match axis {
        0 => cx.theme().colors.red,
        1 => cx.theme().colors.green,
        _ => cx.theme().colors.blue,
    }
    .opacity(if positive { 1.0 } else { 0.55 })
}

/// The widget itself, absolutely positioned by the caller.
pub fn gizmo(studio: &Entity<Studio>, cx: &mut App) -> AnyElement {
    let camera = studio.read(cx).project.camera;
    let handles = axis_offsets(camera.phi_deg, camera.theta_deg);

    let mut body = div()
        .relative()
        .w(px(RADIUS * 2.0 + 16.0))
        .h(px(RADIUS * 2.0 + 16.0))
        .rounded_full()
        .bg(cx.theme().colors.overlay.opacity(0.35));

    for handle in handles {
        let studio_click = studio.clone();
        let (phi, theta) = (handle.phi_deg, handle.theta_deg);
        let enabled = preset_for(handle.axis, handle.positive).is_some();
        let size = 10.0 + 5.0 * (handle.depth.clamp(-1.0, 1.0) + 1.0) / 2.0;
        let left = RADIUS + 8.0 + handle.dir.0 * RADIUS - size / 2.0;
        let top = RADIUS + 8.0 + handle.dir.1 * RADIUS - size / 2.0;
        body = body.child(
            div()
                .id(format!(
                    "gizmo-{}{}",
                    ["x", "y", "z"][handle.axis],
                    if handle.positive { "+" } else { "-" }
                ))
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(size))
                .h(px(size))
                .rounded_full()
                .bg(axis_color(handle.axis, handle.positive, cx))
                .border_1()
                .border_color(cx.theme().colors.border)
                .when(enabled, |dot| dot.cursor_pointer())
                .when(!enabled, |dot| dot.opacity(0.25))
                .on_click(move |_, _, cx| {
                    if enabled {
                        studio_click.update(cx, |studio, cx| studio.preset_camera(phi, theta, cx));
                    }
                }),
        );
    }

    // Centre: isometric.
    let studio_iso = studio.clone();
    body = body.child(
        div()
            .id("gizmo-iso")
            .absolute()
            .left(px(RADIUS + 8.0 - 9.0))
            .top(px(RADIUS + 8.0 - 9.0))
            .w(px(18.))
            .h(px(18.))
            .rounded_full()
            .bg(cx.theme().colors.foreground.opacity(0.18))
            .border_1()
            .border_color(cx.theme().colors.border)
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                studio_iso.update(cx, |studio, cx| studio.preset_camera(45.0, 35.0, cx));
            }),
    );

    v_flex()
        .absolute()
        .right(px(10.))
        .top(px(10.))
        .gap_1()
        .items_center()
        .child(body)
        .child(
            h_flex()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(IconName::MousePointer2)
                .child("drag to orbit"),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn find(handles: &[AxisHandle; 6], axis: usize, positive: bool) -> AxisHandle {
        *handles
            .iter()
            .find(|handle| handle.axis == axis && handle.positive == positive)
            .expect("handle exists")
    }

    #[test]
    fn top_view_collapses_z_and_lays_the_ground_plane_out() {
        let handles = axis_offsets(0.0, 89.0);
        let z = find(&handles, 2, true);
        assert!(
            z.dir.0.abs() < 0.1 && z.dir.1.abs() < 0.1,
            "looking straight down, +Z points at the viewer and collapses to the centre, got {:?}",
            z.dir
        );
        assert!(z.depth > 0.9, "+Z is the near axis in a top view");
        let x = find(&handles, 0, true);
        let y = find(&handles, 1, true);
        assert!(
            x.dir.1.abs() > 0.9,
            "top view: +X runs vertically on screen, got {:?}",
            x.dir
        );
        assert!(
            y.dir.0.abs() > 0.9,
            "top view: +Y runs horizontally on screen, got {:?}",
            y.dir
        );
    }

    #[test]
    fn a_horizon_view_puts_z_up() {
        let handles = axis_offsets(90.0, 2.0);
        let z = find(&handles, 2, true);
        assert!(
            z.dir.1 < -0.9,
            "near the horizon +Z must point up on screen, got {:?}",
            z.dir
        );
    }

    #[test]
    fn opposite_handles_are_antiparallel() {
        let handles = axis_offsets(33.0, 41.0);
        for axis in 0..3 {
            let pos = find(&handles, axis, true).dir;
            let neg = find(&handles, axis, false).dir;
            assert!((pos.0 + neg.0).abs() < 1e-5 && (pos.1 + neg.1).abs() < 1e-5);
        }
    }

    #[test]
    fn handles_snap_to_the_matching_orthogonal_view() {
        // theta is polar from +Z here, so "top" is theta 1, not 89.
        let handles = axis_offsets(12.0, 30.0);
        let top = find(&handles, 2, true);
        assert_eq!(top.phi_deg, 90.0);
        assert!(
            top.theta_deg < 5.0,
            "top view looks down: {}",
            top.theta_deg
        );
        let right = find(&handles, 0, true);
        assert_eq!(right.phi_deg, 0.0);
        assert!(right.theta_deg > 80.0, "a side view sits on the horizon");
        // Looking from below is impossible; the handle is inert.
        assert_eq!(preset_for(2, false), None);
    }
}
