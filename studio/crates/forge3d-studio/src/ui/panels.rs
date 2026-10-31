//! The dock panels: layers, inspector, console, statistics.
//!
//! Panels are pure functions of [`Studio`] state. Reads go through
//! `Entity::read`; every edit is an action or a state method, so panels never
//! hold state of their own (the scroll position lives in the element tree).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::Studio;

type StudioRef<'a> = &'a WeakEntity<Studio>;

fn upgrade(studio: StudioRef<'_>) -> Option<Entity<Studio>> {
    studio.upgrade()
}

/// A small caps group label, as in the design language.
fn group_label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// One key/value line: label left, value right.
fn field(label: &'static str, value: impl Into<SharedString>, cx: &App) -> AnyElement {
    h_flex()
        .justify_between()
        .gap_2()
        .py(px(1.))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.foreground)
                .child(value.into()),
        )
        .into_any_element()
}

/// A stepper: `label   [-] value [+]`.
fn stepper(
    studio: StudioRef<'_>,
    id: &'static str,
    label: &'static str,
    value: String,
    dec: fn(&mut Studio, &mut Context<Studio>),
    inc: fn(&mut Studio, &mut Context<Studio>),
    cx: &App,
) -> AnyElement {
    let studio_dec = studio.clone();
    let studio_inc = studio.clone();
    h_flex()
        .justify_between()
        .gap_2()
        .py(px(2.))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(label),
        )
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    Button::new(format!("dec-{id}"))
                        .label("−")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            let _ = studio_dec.update(cx, dec);
                        }),
                )
                .child(div().min_w(px(58.)).text_xs().text_center().child(value))
                .child(
                    Button::new(format!("inc-{id}"))
                        .label("+")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            let _ = studio_inc.update(cx, inc);
                        }),
                ),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- inspector

pub fn inspector(studio: StudioRef<'_>, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(studio_entity) = upgrade(studio) else {
        return div().into_any_element();
    };
    let state = studio_entity.read(cx);

    let mut body = v_flex().w_full().p_2().gap_3();

    // Selection header.
    match state
        .primary_layer()
        .and_then(|id| state.project.document.layer(id))
    {
        Some(layer) => {
            body = body.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_ellipsis()
                            .child(layer.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(layer.kind.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(if layer.kind.renders() {
                                "renders in this build".to_owned()
                            } else {
                                "registered only — no renderer yet".to_owned()
                            }),
                    ),
            );
        }
        None => {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child("Select a layer to edit its properties."),
            );
        }
    }

    // Terrain: the frame's source, the palette, and the parameters that are
    // not yet editable (said plainly instead of showing disabled fields).
    if let Some(terrain) = &state.project.terrain {
        body = body.child(
            v_flex()
                .gap_1()
                .child(group_label("Terrain", cx))
                .child(field(
                    "File",
                    terrain
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    cx,
                ))
                .child(field(
                    "Grid",
                    format!("{} × {}", terrain.width, terrain.height),
                    cx,
                ))
                .child(field(
                    "Elevation",
                    format!("{:.0} – {:.0} m", terrain.height_min, terrain.height_max),
                    cx,
                )),
        );

        let mut palette = v_flex().gap_1();
        let mut chips = h_flex().flex_wrap().gap_1();
        chips = chips.child(
            Button::new("colormap-default")
                .label("Default")
                .xsmall()
                .outline()
                .on_click({
                    let studio = studio.clone();
                    move |_, _, cx| {
                        let _ = studio.update(cx, |studio, cx| studio.set_colormap(None, cx));
                    }
                }),
        );
        for name in state.colormaps.iter().take(48) {
            let selected = state.project.colormap.as_deref() == Some(name.as_str());
            let studio = studio.clone();
            let name_owned = name.clone();
            chips = chips.child(
                Button::new(format!("colormap-{name}"))
                    .label(name.clone())
                    .xsmall()
                    .when(selected, |button| button.primary())
                    .when(!selected, |button| button.outline())
                    .on_click(move |_, _, cx| {
                        let name = name_owned.clone();
                        let _ = studio.update(cx, |studio, cx| studio.set_colormap(Some(name), cx));
                    }),
            );
        }
        palette = palette
            .child(group_label("Colormap", cx))
            .child(if state.colormaps.is_empty() {
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child("Palettes load from the engine…")
                    .into_any_element()
            } else {
                chips.into_any_element()
            });
        body = body.child(palette);
    }

    // Camera.
    let camera = state.project.camera;
    body = body.child(
        v_flex()
            .gap_1()
            .child(group_label("Camera", cx))
            .child(stepper(
                studio,
                "cam-phi",
                "Azimuth",
                format!("{:.0}°", camera.phi_deg),
                |studio, cx| studio.step_camera(-5.0, 0.0, 1.0, 1.0, cx),
                |studio, cx| studio.step_camera(5.0, 0.0, 1.0, 1.0, cx),
                cx,
            ))
            .child(stepper(
                studio,
                "cam-theta",
                // The document stores the engine's polar theta; people think
                // in elevation, so the control and its label convert.
                "Elevation",
                format!(
                    "{:.0}°",
                    crate::ui::viewport_math::elevation_from_theta(camera.theta_deg)
                ),
                |studio, cx| studio.step_camera(0.0, 2.0, 1.0, 1.0, cx),
                |studio, cx| studio.step_camera(0.0, -2.0, 1.0, 1.0, cx),
                cx,
            ))
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .py(px(1.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Target"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.foreground)
                            .child(format!(
                                "x {:.0} · y {:.0}",
                                camera.target_x, camera.target_z
                            )),
                    ),
            )
            .child(stepper(
                studio,
                "cam-radius",
                "Distance",
                format!("{:.0}", camera.radius),
                |studio, cx| studio.step_camera(0.0, 0.0, 1.0 / 1.15, 1.0, cx),
                |studio, cx| studio.step_camera(0.0, 0.0, 1.15, 1.0, cx),
                cx,
            ))
            .child(stepper(
                studio,
                "cam-fov",
                "Field of view",
                format!("{:.0}°", camera.fov_deg),
                |studio, cx| studio.step_camera(0.0, 0.0, 1.0, 1.0 / 1.05, cx),
                |studio, cx| studio.step_camera(0.0, 0.0, 1.0, 1.05, cx),
                cx,
            ))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("fit")
                            .label("Fit")
                            .xsmall()
                            .outline()
                            .on_click({
                                let studio = studio.clone();
                                move |_, _, cx| {
                                    let _ = studio.update(cx, |studio, cx| studio.fit_camera(cx));
                                }
                            }),
                    )
                    .child(
                        Button::new("reset-camera")
                            .label("Reset")
                            .xsmall()
                            .ghost()
                            .on_click({
                                let studio = studio.clone();
                                move |_, _, cx| {
                                    let _ = studio.update(cx, |studio, cx| studio.reset_camera(cx));
                                }
                            }),
                    ),
            ),
    );

    // Sun.
    let sun = state.project.sun;
    body = body.child(
        v_flex()
            .gap_1()
            .child(group_label("Sun", cx))
            .child(stepper(
                studio,
                "sun-az",
                "Azimuth",
                format!("{:.0}°", sun.azimuth_deg),
                |studio, cx| studio.step_sun(-5.0, 0.0, 1.0, cx),
                |studio, cx| studio.step_sun(5.0, 0.0, 1.0, cx),
                cx,
            ))
            .child(stepper(
                studio,
                "sun-el",
                "Elevation",
                format!("{:.0}°", sun.elevation_deg),
                |studio, cx| studio.step_sun(0.0, -2.5, 1.0, cx),
                |studio, cx| studio.step_sun(0.0, 2.5, 1.0, cx),
                cx,
            ))
            .child(stepper(
                studio,
                "sun-int",
                "Intensity",
                format!("{:.2}", sun.intensity),
                |studio, cx| studio.step_sun(0.0, 0.0, 1.0 / 1.1, cx),
                |studio, cx| studio.step_sun(0.0, 0.0, 1.1, cx),
                cx,
            )),
    );

    // Render parameters: the engine's own schema, rendered field by field.
    let schema_ready = state.params_schema.is_some();
    if schema_ready {
        body = body.child(crate::ui::params::params_pages(&studio_entity, window, cx));
    }

    body.into_any_element()
}

// ------------------------------------------------------------------ console

pub fn console(studio: StudioRef<'_>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(studio_entity) = upgrade(studio) else {
        return div().into_any_element();
    };
    let state = studio_entity.read(cx);

    let mut lines = v_flex().w_full().p_2().gap_1();
    if state.log.is_empty() {
        lines = lines.child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child("No engine messages yet — imports, renders and errors appear here."),
        );
    }
    for entry in &state.log {
        lines = lines.child(
            div()
                .text_xs()
                .text_color(if entry.error {
                    cx.theme().colors.danger
                } else {
                    cx.theme().colors.muted_foreground
                })
                .child(entry.text.clone()),
        );
    }

    lines.into_any_element()
}

// --------------------------------------------------------------- statistics

pub fn statistics(studio: StudioRef<'_>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(studio_entity) = upgrade(studio) else {
        return div().into_any_element();
    };
    let state = studio_entity.read(cx);
    let (undo, redo) = state.history.depth();

    v_flex()
        .w_full()
        .p_2()
        .gap_3()
        .child(
            v_flex()
                .gap_1()
                .child(group_label("Device", cx))
                .child(field("Adapter", state.adapter.clone(), cx))
                .child(field("Backend", state.backend.clone(), cx))
                .child(field(
                    "Software fallback",
                    if state.software { "yes" } else { "no" },
                    cx,
                ))
                .child(field(
                    "Viewport",
                    if state.viewport_ready {
                        "engine device (readback)"
                    } else {
                        "building…"
                    },
                    cx,
                )),
        )
        .child(
            v_flex()
                .gap_1()
                .child(group_label("Frame", cx))
                .child(field(
                    "Viewport size",
                    format!("{} × {}", state.frame_size.0, state.frame_size.1),
                    cx,
                ))
                .child(field(
                    "Engine render",
                    format!("{:.1} ms", state.frame_ms),
                    cx,
                ))
                .child(field(
                    "Frame interval",
                    format!("{:.1} ms", state.frame_interval_ms),
                    cx,
                ))
                .child(field(
                    "Request → present",
                    format!("{:.1} ms", state.frame_e2e_ms),
                    cx,
                ))
                .child(field(
                    "Still",
                    if state.viewport_dirty {
                        "stale"
                    } else {
                        "current"
                    },
                    cx,
                ))
                .child(field(
                    "Startup",
                    match state.startup_ms {
                        Some(ms) => format!("{ms:.0} ms to first frame"),
                        None => "building…".to_owned(),
                    },
                    cx,
                )),
        )
        .child(
            v_flex()
                .gap_1()
                .child(group_label("Document", cx))
                .child(field(
                    "Layers",
                    state.project.document.len().to_string(),
                    cx,
                ))
                .child(field(
                    "Selection",
                    if state.selection.is_empty() {
                        "none".to_owned()
                    } else {
                        state.selection.len().to_string()
                    },
                    cx,
                ))
                .child(field("History", format!("{undo} undo / {redo} redo"), cx))
                .child(field(
                    "Project",
                    if state.project_dirty {
                        "unsaved changes"
                    } else {
                        "saved"
                    },
                    cx,
                )),
        )
        .child(
            v_flex()
                .gap_1()
                .child(group_label("Engine", cx))
                .child(field(
                    "State",
                    if state.busy { "working" } else { "idle" },
                    cx,
                ))
                .child(field(
                    "Progress",
                    state
                        .progress
                        .map(|fraction| format!("{:.0}%", fraction * 100.0))
                        .unwrap_or_else(|| "—".to_owned()),
                    cx,
                ))
                .child(field(
                    "Last export",
                    state
                        .last_export
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "—".to_owned()),
                    cx,
                )),
        )
        .into_any_element()
}

/// `console` for callers that hold the entity (the bottom strip).
pub fn console_entity(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let weak = studio.downgrade();
    console(&weak, window, cx)
}

/// `statistics` for callers that hold the entity (the properties strip).
pub fn statistics_entity(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let weak = studio.downgrade();
    statistics(&weak, window, cx)
}
