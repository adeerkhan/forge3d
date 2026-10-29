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
    let studio_back = studio.clone();

    let mode = state.scene_mode;
    let live_viewport = state.live_viewport;
    let show_gizmo = state.show_gizmo;
    let show_hud = state.show_hud;
    let image_tier = state.viewport_image;
    let preview_ms = state.preview_ms;
    let preview_size = state.preview_size;
    let showing_render = image_tier != crate::state::ViewportImage::Live;
    let frame_size = state.frame_size;
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
                        .when(show_gizmo, |button| button.outline())
                        .when(!show_gizmo, |button| button.ghost())
                        .tooltip("Navigation gizmo")
                        .on_click(move |_, _, cx| {
                            let _ = studio_gizmo.update(cx, |studio, cx| studio.toggle_gizmo(cx));
                        }),
                )
                .child(
                    Button::new("viewport-hud-toggle")
                        .icon(IconName::Info)
                        .xsmall()
                        .when(show_hud, |button| button.outline())
                        .when(!show_hud, |button| button.ghost())
                        .tooltip("HUD")
                        .on_click(move |_, _, cx| {
                            let _ = studio_hud.update(cx, |studio, cx| {
                                studio.show_hud = !studio.show_hud;
                                cx.notify();
                            });
                        }),
                )
                .when(showing_render, |row| {
                    let studio_back = studio_back.clone();
                    row.child(
                        Button::new("viewport-back")
                            .icon(IconName::MonitorPlay)
                            .label("Render result — back to viewport")
                            .xsmall()
                            .outline()
                            .on_click(move |_, _, cx| {
                                let _ = studio_back
                                    .update(cx, |studio, cx| studio.back_to_viewport(cx));
                            }),
                    )
                })
                .child(
                    Button::new("viewport-live-toggle")
                        .icon(if live_viewport {
                            IconName::Pause
                        } else {
                            IconName::Play
                        })
                        .xsmall()
                        .when(live_viewport, |button| button.outline())
                        .when(!live_viewport, |button| button.ghost())
                        .tooltip(if live_viewport {
                            "Live: redraws while you move. Ctrl+L pauses it."
                        } else {
                            "Paused: draws once when you let go. Ctrl+L resumes."
                        })
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
                .child(live_button(studio, live_viewport)),
        )
        .child(
            // Fixed widths: a readout that changes width would resize the
            // viewport, which would request another frame, forever.
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(div().w(px(150.)).text_right().child(match image_tier {
                    crate::state::ViewportImage::Live => "live preview".to_owned(),
                    crate::state::ViewportImage::Preview => format!(
                        "preview {}×{} · {:.0} ms",
                        preview_size.0, preview_size.1, preview_ms
                    ),
                    crate::state::ViewportImage::Render => "render result".to_owned(),
                }))
                .child(
                    div()
                        .w(px(76.))
                        .text_right()
                        .child(format!("{} × {}", frame_size.0, frame_size.1)),
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
        None => {
            let studio_build = studio.clone();
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(if mode == crate::state::SceneMode::MapScene {
                            "No map rendered yet"
                        } else if has_terrain {
                            "Rendering the first frame…"
                        } else {
                            "No terrain loaded"
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(match mode {
                            crate::state::SceneMode::MapScene => {
                                "Cartography ▸ Build recipe renders the map here."
                            }
                            _ if has_terrain => "The engine is compiling the terrain pipeline.",
                            _ => "File ▸ Import DEM… to start a scene.",
                        }),
                )
                .when(mode == crate::state::SceneMode::MapScene, |body| {
                    body.child(
                        Button::new("viewport-build-map")
                            .label("Build recipe")
                            .small()
                            .primary()
                            .on_click(move |_, _, cx| {
                                let _ =
                                    studio_build.update(cx, |studio, cx| studio.build_mapscene(cx));
                            }),
                    )
                })
                .into_any_element()
        }
    };

    let studio_down = studio.clone();
    let studio_down_middle = studio.clone();
    let studio_move = studio.clone();
    let studio_up_left = studio.clone();
    let studio_up_middle = studio.clone();
    let studio_scroll = studio.clone();
    let size_for_layout = size_cell.clone();

    let hud_left = if showing_render {
        "render result — move the view to return to the live preview".to_owned()
    } else if live_viewport {
        format!(
            "Orbit {:.0}° · Elev {:.0}° · Dist {:.0}",
            camera.phi_deg, camera.theta_deg, camera.radius
        )
    } else {
        format!(
            "Orbit {:.0}° · Elev {:.0}° · Dist {:.0} · paused (draws on release)",
            camera.phi_deg, camera.theta_deg, camera.radius
        )
    };

    let body = div()
        .relative()
        .flex_1()
        .w_full()
        .h_full()
        .min_h(px(1.))
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
            // Blender's mapping: MMB orbits, Shift+MMB pans.
            let pan = event.modifiers.shift;
            let _ = studio_down_middle.update(cx, |studio, cx| {
                studio.drag = Some(if pan {
                    ViewDrag::Pan {
                        last_x: x,
                        last_y: y,
                    }
                } else {
                    ViewDrag::Orbit {
                        last_x: x,
                        last_y: y,
                    }
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
        .when(show_hud, |body| {
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
        .when(show_gizmo, |body| {
            body.child(crate::ui::gizmo::gizmo(&studio_entity, cx))
        });

    let studio_fit = studio.clone();
    let studio_zoom_in = studio.clone();
    let studio_zoom_out = studio.clone();
    let studio_iso = studio.clone();
    let studio_live_rail = studio.clone();
    let rail = v_flex()
        .w(px(32.))
        .h_full()
        .items_center()
        .gap_1()
        .py_1()
        .border_r_1()
        .border_color(cx.theme().colors.border)
        .child(
            Button::new("rail-fit")
                .icon(IconName::Maximize)
                .xsmall()
                .ghost()
                .tooltip("Fit view (F)")
                .on_click(move |_, _, cx| {
                    let _ = studio_fit.update(cx, |studio, cx| studio.fit_camera(cx));
                }),
        )
        .child(
            Button::new("rail-zoom-in")
                .icon(IconName::ZoomIn)
                .xsmall()
                .ghost()
                .tooltip("Zoom in")
                .on_click(move |_, _, cx| {
                    let _ = studio_zoom_in.update(cx, |studio, cx| studio.zoom_by(120.0, cx));
                }),
        )
        .child(
            Button::new("rail-zoom-out")
                .icon(IconName::ZoomOut)
                .xsmall()
                .ghost()
                .tooltip("Zoom out")
                .on_click(move |_, _, cx| {
                    let _ = studio_zoom_out.update(cx, |studio, cx| studio.zoom_by(-120.0, cx));
                }),
        )
        .child(
            Button::new("rail-iso")
                .icon(IconName::Box)
                .xsmall()
                .ghost()
                .tooltip("Isometric view (0)")
                .on_click(move |_, _, cx| {
                    let _ =
                        studio_iso.update(cx, |studio, cx| studio.preset_camera(45.0, 35.0, cx));
                }),
        )
        .child(div().flex_1())
        .child(
            Button::new("rail-live")
                .icon(if live_viewport {
                    IconName::Pause
                } else {
                    IconName::Play
                })
                .xsmall()
                .ghost()
                .tooltip("Live viewport (Ctrl+L)")
                .on_click(move |_, _, cx| {
                    let _ =
                        studio_live_rail.update(cx, |studio, cx| studio.toggle_live_viewport(cx));
                }),
        );

    v_flex()
        .size_full()
        .child(tools)
        .child(
            h_flex()
                .flex_1()
                .w_full()
                .min_h(px(0.))
                .items_stretch()
                .child(rail)
                .child(body),
        )
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
