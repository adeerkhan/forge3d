//! The viewport panel: the engine's frame, the navigation tools and the HUD.
//!
//! The frame is an image because the UI runtime and the engine sit on
//! different wgpu majors (see `studio_engine::native_viewport`). Everything
//! else is the same interaction model as before: drag to orbit, middle-drag to
//! pan, wheel to zoom, one render per gesture step.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::{Studio, ViewDrag};

type StudioRef<'a> = &'a WeakEntity<Studio>;

pub fn viewport_panel(studio: StudioRef<'_>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(studio_entity) = studio.upgrade() else {
        return div().into_any_element();
    };
    let state = studio_entity.read(cx);
    let frame = state.frame.clone();
    let size_cell = state.viewport_size.clone();
    let camera = state.project.camera;
    let frame_ms = state.frame_ms;
    let has_terrain = state.project.terrain.is_some();

    // Toolbar: the six actions a viewport needs, plus the readout.
    let studio_terrain_mode = studio.clone();
    let studio_mapscene_mode = studio.clone();
    let studio_gizmo = studio.clone();
    let studio_hud = studio.clone();
    let studio_live = studio.clone();

    let mode = state.scene_mode;
    let tools = h_flex()
        .w_full()
        .justify_between()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("viewport-mode-terrain")
                        .icon(IconName::Mountain)
                        .label("Terrain")
                        .xsmall()
                        .when(mode == crate::state::SceneMode::Terrain, |button| {
                            button.primary()
                        })
                        .when(mode != crate::state::SceneMode::Terrain, |button| {
                            button.ghost()
                        })
                        .on_click(move |_, _, cx| {
                            let _ = studio_terrain_mode.update(cx, |studio, cx| {
                                studio.set_scene_mode(crate::state::SceneMode::Terrain, cx)
                            });
                        }),
                )
                .child(
                    Button::new("viewport-mode-mapscene")
                        .icon(IconName::Layers)
                        .label("MapScene")
                        .xsmall()
                        .when(mode == crate::state::SceneMode::MapScene, |button| {
                            button.primary()
                        })
                        .when(mode != crate::state::SceneMode::MapScene, |button| {
                            button.ghost()
                        })
                        .on_click(move |_, _, cx| {
                            let _ = studio_mapscene_mode.update(cx, |studio, cx| {
                                studio.set_scene_mode(crate::state::SceneMode::MapScene, cx)
                            });
                        }),
                )
                .child(div().w(px(8.)))
                .child(
                    Button::new("viewport-gizmo-toggle")
                        .icon(IconName::Compass)
                        .xsmall()
                        .when(state.show_gizmo, |button| button.outline())
                        .when(!state.show_gizmo, |button| button.ghost())
                        .tooltip("Navigation gizmo")
                        .on_click(move |_, _, cx| {
                            let _ = studio_gizmo.update(cx, |studio, cx| studio.toggle_gizmo(cx));
                        }),
                )
                .child(
                    Button::new("viewport-hud-toggle")
                        .icon(IconName::Info)
                        .xsmall()
                        .when(state.show_hud, |button| button.outline())
                        .when(!state.show_hud, |button| button.ghost())
                        .tooltip("HUD")
                        .on_click(move |_, _, cx| {
                            let _ = studio_hud.update(cx, |studio, cx| {
                                studio.show_hud = !studio.show_hud;
                                cx.notify();
                            });
                        }),
                )
                .child(
                    Button::new("viewport-live-toggle")
                        .icon(if state.live_viewport {
                            IconName::Pause
                        } else {
                            IconName::Play
                        })
                        .xsmall()
                        .when(state.live_viewport, |button| button.outline())
                        .when(!state.live_viewport, |button| button.ghost())
                        .tooltip("Live viewport (Ctrl+L)")
                        .on_click(move |_, _, cx| {
                            let _ = studio_live
                                .update(cx, |studio, cx| studio.toggle_live_viewport(cx));
                        }),
                ),
        )
        .child(
            h_flex()
                .gap_1()
                .child(tool_button(
                    studio,
                    "fit",
                    "Fit",
                    IconName::Maximize,
                    |studio, cx| studio.fit_camera(cx),
                ))
                .child(tool_button(
                    studio,
                    "top",
                    "Top",
                    IconName::ArrowUp,
                    |studio, cx| studio.preset_camera(90.0, 89.0, cx),
                ))
                .child(tool_button(
                    studio,
                    "front",
                    "Front",
                    IconName::Square,
                    |studio, cx| studio.preset_camera(0.0, 1.0, cx),
                ))
                .child(tool_button(
                    studio,
                    "right",
                    "Right",
                    IconName::SquareArrowRight,
                    |studio, cx| studio.preset_camera(90.0, 1.0, cx),
                ))
                .child(tool_button(
                    studio,
                    "iso",
                    "Isometric",
                    IconName::Box,
                    |studio, cx| studio.preset_camera(45.0, 35.0, cx),
                ))
                .child(live_button(studio, state.live_viewport)),
        )
        .child(
            // Fixed widths: a readout that changes width would resize the
            // viewport, which would request another frame, forever.
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(
                    div()
                        .w(px(76.))
                        .text_right()
                        .child(format!("{} × {}", state.frame_size.0, state.frame_size.1)),
                )
                .child(
                    div()
                        .w(px(52.))
                        .text_right()
                        .child(format!("{frame_ms:.1} ms")),
                ),
        );

    // The body: the frame, or an honest empty state.
    let body_content: AnyElement = match frame {
        Some(image) => img(image)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(if has_terrain {
                        "Rendering the first frame…"
                    } else {
                        "No terrain loaded"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(if has_terrain {
                        "The engine is compiling the terrain pipeline."
                    } else {
                        "File ▸ Import DEM… to start a scene."
                    }),
            )
            .into_any_element(),
    };

    let studio_down = studio.clone();
    let studio_down_middle = studio.clone();
    let studio_move = studio.clone();
    let studio_up_left = studio.clone();
    let studio_up_middle = studio.clone();
    let studio_scroll = studio.clone();
    let size_for_layout = size_cell.clone();

    let hud_left = format!(
        "Orbit {:.0}° · Elev {:.0}° · Dist {:.0}",
        camera.phi_deg, camera.theta_deg, camera.radius
    );

    let body = div()
        .relative()
        .flex_1()
        .w_full()
        .overflow_hidden()
        .bg(cx.theme().colors.background)
        .on_children_prepainted(move |bounds, _, _| {
            if let Some(bounds) = bounds.first() {
                let size = bounds.size;
                size_for_layout.set((f32::from(size.width), f32::from(size.height)));
            }
        })
        .id("viewport-body")
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
            let x = f32::from(event.position.x);
            let y = f32::from(event.position.y);
            let _ = studio_down.update(cx, |studio, cx| {
                studio.drag = Some(ViewDrag::Orbit {
                    last_x: x,
                    last_y: y,
                });
                cx.notify();
            });
        })
        .on_mouse_down(MouseButton::Middle, move |event, _, cx| {
            let x = f32::from(event.position.x);
            let y = f32::from(event.position.y);
            let _ = studio_down_middle.update(cx, |studio, cx| {
                studio.drag = Some(ViewDrag::Pan {
                    last_x: x,
                    last_y: y,
                });
                cx.notify();
            });
        })
        .on_mouse_move(move |event, _, cx| {
            let x = f32::from(event.position.x);
            let y = f32::from(event.position.y);
            let _ = studio_move.update(cx, |studio, cx| studio.drag_to(x, y, cx));
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            let _ = studio_up_left.update(cx, |studio, cx| {
                studio.end_gesture(cx);
                cx.notify();
            });
        })
        .on_mouse_up(MouseButton::Middle, move |_, _, cx| {
            let _ = studio_up_middle.update(cx, |studio, cx| {
                studio.end_gesture(cx);
                cx.notify();
            });
        })
        .on_scroll(move |event, _, cx| {
            let delta = match event.delta {
                ScrollDelta::Pixels(point) => f32::from(point.y),
                ScrollDelta::Lines(point) => point.y * 30.0,
            };
            let _ = studio_scroll.update(cx, |studio, cx| studio.zoom_by(delta, cx));
        })
        .child(body_content)
        .when(state.show_hud, |body| {
            body.child(
                h_flex()
                    .absolute()
                    .left_2()
                    .bottom_1()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(hud_left),
            )
        })
        .when(state.show_gizmo, |body| {
            body.child(crate::ui::gizmo::gizmo(&studio_entity, cx))
        });

    v_flex()
        .size_full()
        .child(tools)
        .child(body)
        .into_any_element()
}

/// The live-render switch: the escape hatch for a slow GPU or a heavy DEM.
fn live_button(studio: StudioRef<'_>, live: bool) -> AnyElement {
    let studio = studio.clone();
    Button::new("live-viewport")
        .icon(if live {
            IconName::Pause
        } else {
            IconName::Play
        })
        .label(if live { "Live" } else { "Paused" })
        .xsmall()
        .when(live, |button| button.outline())
        .when(!live, |button| button.ghost())
        .on_click(move |_, _, cx| {
            let _ = studio.update(cx, |studio, cx| studio.toggle_live_viewport(cx));
        })
        .into_any_element()
}

fn tool_button(
    studio: StudioRef<'_>,
    id: &'static str,
    label: &'static str,
    icon: IconName,
    action: fn(&mut Studio, &mut Context<Studio>),
) -> AnyElement {
    let studio = studio.clone();
    Button::new(id)
        .icon(icon)
        .label(label)
        .xsmall()
        .ghost()
        .on_click(move |_, _, cx| {
            let _ = studio.update(cx, action);
        })
        .into_any_element()
}
