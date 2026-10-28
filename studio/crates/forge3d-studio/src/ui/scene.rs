//! Scene surface (GUI.md G7): the examples gallery, MapScene mode and the
//! environment panels (AETHER atmosphere, SIDERA time & place, smoke volumes).
//!
//! Every control here calls an op the bridge already exposes; nothing is
//! re-derived in the UI. Where the engine reports a limitation (a missing
//! denoiser, an unfetchable fixture) its text is shown verbatim.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::{SceneMode, Studio};

fn title(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text)
        .into_any_element()
}

fn note(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}

// ------------------------------------------------------------------ gallery

/// The examples gallery: every `examples/*.py` script with its tier.
pub fn gallery(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (entries, filter) = {
        let state = studio.read(cx);
        (state.examples.clone(), state.gallery_filter.clone())
    };

    let mut categories: Vec<String> = entries
        .iter()
        .map(|entry| entry.category.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    categories.sort();
    categories.insert(0, "All".to_owned());

    let mut chips = h_flex().flex_wrap().gap_1().px_2().py_1();
    for category in &categories {
        let selected = *category == filter;
        let studio_pick = studio.clone();
        let name = category.clone();
        chips = chips.child(
            Button::new(format!("gallery-filter-{category}"))
                .label(category.clone())
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.outline())
                .on_click(move |_, _, cx| {
                    let name = name.clone();
                    studio_pick.update(cx, |studio, cx| {
                        studio.gallery_filter = name;
                        cx.notify();
                    });
                }),
        );
    }

    let mut list = v_flex().w_full();
    if entries.is_empty() {
        list = list.child(
            div()
                .p_3()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(
                    "The gallery index was not found (studio/examples/index.json). \
                     Run Studio from the repository so its fixtures resolve.",
                ),
        );
    }
    for entry in entries
        .iter()
        .filter(|entry| filter == "All" || entry.category == filter)
    {
        let runnable = entry.tier == 'A';
        let studio_open = studio.clone();
        let entry_owned = entry.clone();
        list = list.child(
            h_flex()
                .id(format!("gallery-{}", entry.id))
                .justify_between()
                .gap_2()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(cx.theme().colors.border)
                .when(runnable, |row| row.cursor_pointer())
                .on_click(move |_, _, cx| {
                    if runnable {
                        let entry = entry_owned.clone();
                        studio_open.update(cx, |studio, cx| studio.open_example(entry, cx));
                    }
                })
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(if runnable {
                                    cx.theme().colors.foreground
                                } else {
                                    cx.theme().colors.muted_foreground
                                })
                                .child(entry.title.clone()),
                        )
                        .child(note(entry.description.clone(), cx)),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(entry.category.clone()),
                        )
                        .child(if runnable {
                            Button::new(format!("gallery-open-{}", entry.id))
                                .label("Open")
                                .xsmall()
                                .outline()
                                .into_any_element()
                        } else {
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.warning)
                                .child(format!("tier {} · {}", entry.tier, entry.unlock))
                                .into_any_element()
                        }),
                ),
        );
    }

    v_flex()
        .w_full()
        .child(chips)
        .child(
            div()
                .id("gallery-list")
                .max_h(px(320.))
                .overflow_y_scroll()
                .child(list),
        )
        .into_any_element()
}

// ----------------------------------------------------------------- mapscene

/// MapScene mode: templates, validation, the certified render and its camera.
pub fn mapscene(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (mode, mapscene, has_terrain, export) = {
        let state = studio.read(cx);
        (
            state.scene_mode,
            state.mapscene.clone(),
            state.project.terrain.is_some(),
            state.project.export,
        )
    };

    let studio_terrain = studio.clone();
    let studio_mapscene = studio.clone();
    let studio_build = studio.clone();
    let studio_validate = studio.clone();
    let studio_render = studio.clone();

    let mut body = v_flex().w_full().p_2().gap_2();

    body = body.child(
        h_flex()
            .gap_1()
            .child(
                Button::new("scene-terrain")
                    .label("Terrain")
                    .xsmall()
                    .when(mode == SceneMode::Terrain, |button| button.primary())
                    .when(mode != SceneMode::Terrain, |button| button.outline())
                    .on_click(move |_, _, cx| {
                        studio_terrain.update(cx, |studio, cx| {
                            studio.set_scene_mode(SceneMode::Terrain, cx)
                        });
                    }),
            )
            .child(
                Button::new("scene-mapscene")
                    .label("MapScene")
                    .xsmall()
                    .when(mode == SceneMode::MapScene, |button| button.primary())
                    .when(mode != SceneMode::MapScene, |button| button.outline())
                    .on_click(move |_, _, cx| {
                        studio_mapscene.update(cx, |studio, cx| {
                            studio.set_scene_mode(SceneMode::MapScene, cx)
                        });
                    }),
            ),
    );

    if mode == SceneMode::Terrain {
        body = body.child(note(
            "Terrain mode paints the interactive viewport on the engine device. MapScene mode renders the compiled recipe through the certified bridge.",
            cx,
        ));
        return body.into_any_element();
    }

    let mut templates = h_flex().flex_wrap().gap_1();
    for template in &mapscene.templates {
        let selected = &mapscene.template == template;
        let studio_pick = studio.clone();
        let name = template.clone();
        templates = templates.child(
            Button::new(format!("mapscene-template-{template}"))
                .label(template.clone())
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.outline())
                .on_click(move |_, _, cx| {
                    let name = name.clone();
                    studio_pick.update(cx, |studio, cx| {
                        studio.mapscene.template = name;
                        cx.notify();
                    });
                }),
        );
    }

    body = body
        .child(title("Template", cx))
        .child(templates)
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("mapscene-build")
                        .label("Build recipe")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_build.update(cx, |studio, cx| studio.build_mapscene(cx));
                        }),
                )
                .child(
                    Button::new("mapscene-validate")
                        .label("Validate")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_validate.update(cx, |studio, cx| studio.validate_mapscene(cx));
                        }),
                )
                .child(
                    Button::new("mapscene-render")
                        .label("Render")
                        .xsmall()
                        .primary()
                        .on_click(move |_, _, cx| {
                            studio_render.update(cx, |studio, cx| studio.render_mapscene(cx));
                        }),
                )
                .when(!has_terrain, |row| row.child(note("load a DEM first", cx))),
        )
        .child(note(
            format!(
                "Renders through MapScene.render at {}×{} (Project ▸ export size).",
                export.width, export.height
            ),
            cx,
        ));

    if let Some(status) = &mapscene.last_status {
        body = body.child(note(status.clone(), cx));
    }
    if let Some(sha) = &mapscene.certificate_sha {
        body = body.child(
            h_flex()
                .gap_1()
                .items_center()
                .child(IconName::Shield)
                .child(note(
                    format!("certificate payload {}", &sha[..sha.len().min(24)]),
                    cx,
                )),
        );
    }

    body.into_any_element()
}

// --------------------------------------------------------------- environment

/// AETHER atmosphere, SIDERA time & place, and smoke volumes.
pub fn environment(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (atmosphere, observation, smoke, is_dark_meta) = {
        let state = studio.read(cx);
        (
            state.project.atmosphere.clone(),
            state.project.observation.clone(),
            state.project.smoke.clone(),
            state.atmosphere_meta.clone(),
        )
    };

    let studio_toggle = studio.clone();
    let studio_int_desc = studio.clone();
    let studio_int_inc = studio.clone();
    let studio_turb_desc = studio.clone();
    let studio_turb_inc = studio.clone();
    let studio_mie_desc = studio.clone();
    let studio_mie_inc = studio.clone();
    let studio_bake = studio.clone();
    let studio_smoke_toggle = studio.clone();
    let studio_smoke_pick = studio.clone();
    let studio_smoke_clear = studio.clone();
    let studio_smoke_op_desc = studio.clone();
    let studio_smoke_op_inc = studio.clone();
    let studio_astro = studio.clone();
    let studio_place_dec = studio.clone();
    let studio_place_inc = studio.clone();

    let mut body = v_flex().w_full().p_2().gap_3();

    // ---- AETHER
    body = body
        .child(title("Atmosphere (AETHER)", cx))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Checkbox::new("atmo-enabled")
                        .checked(atmosphere.enabled)
                        .on_click(move |checked, _, cx| {
                            studio_toggle.update(cx, |studio, cx| {
                                studio.set_atmosphere_enabled(*checked, cx)
                            });
                        }),
                )
                .child(note("generate the sky and light the terrain with it", cx)),
        )
        .child(numeric(
            "Intensity",
            format!("{:.2}", atmosphere.intensity),
            studio_int_desc,
            |studio, cx| studio.step_atmosphere(0.0, 0.9, 0.0, cx),
            studio_int_inc,
            |studio, cx| studio.step_atmosphere(0.0, 1.1, 0.0, cx),
        ))
        .child(numeric(
            "Turbidity",
            format!("{:.1}", atmosphere.turbidity),
            studio_turb_desc,
            |studio, cx| studio.step_atmosphere(-0.5, 1.0, 0.0, cx),
            studio_turb_inc,
            |studio, cx| studio.step_atmosphere(0.5, 1.0, 0.0, cx),
        ))
        .child(numeric(
            "Mie g",
            format!("{:.2}", atmosphere.mie_g),
            studio_mie_desc,
            |studio, cx| studio.step_atmosphere(0.0, 1.0, -0.02, cx),
            studio_mie_inc,
            |studio, cx| studio.step_atmosphere(0.0, 1.0, 0.02, cx),
        ))
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("atmo-bake")
                        .label("Bake sky")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_bake.update(cx, |studio, cx| studio.bake_atmosphere(cx));
                        }),
                )
                .child(note(
                    is_dark_meta
                        .as_ref()
                        .and_then(|meta| meta.get("hdr_sha256"))
                        .and_then(|value| value.as_str())
                        .map(|sha| format!("hdr {}", &sha[..sha.len().min(16)]))
                        .unwrap_or_else(|| "no sky baked yet".to_owned()),
                    cx,
                )),
        );

    // ---- SIDERA
    body = body.child(title("Time & place (SIDERA)", cx)).child(
        h_flex()
            .gap_1()
            .child(note(
                format!(
                    "{} · lat {:.2} · lon {:.2}",
                    observation.datetime_utc, observation.latitude_deg, observation.longitude_deg
                ),
                cx,
            ))
            .child(
                Button::new("place-lat-dec")
                    .label("lat −")
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_place_dec
                            .update(cx, |studio, cx| studio.step_observation(-0.5, 0.0, cx));
                    }),
            )
            .child(
                Button::new("place-lat-inc")
                    .label("lat +")
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_place_inc
                            .update(cx, |studio, cx| studio.step_observation(0.5, 0.0, cx));
                    }),
            )
            .child(
                Button::new("place-observe")
                    .label("Sun from place")
                    .xsmall()
                    .outline()
                    .on_click(move |_, _, cx| {
                        studio_astro.update(cx, |studio, cx| studio.observe_sun(cx));
                    }),
            ),
    );

    // ---- smoke
    let smoke_label = smoke
        .path
        .as_ref()
        .map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        })
        .unwrap_or_else(|| "no volume".to_owned());
    body = body
        .child(title("Smoke volume", cx))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Checkbox::new("smoke-enabled")
                        .checked(smoke.enabled)
                        .on_click(move |checked, _, cx| {
                            studio_smoke_toggle
.update(cx, |studio, cx| {
                                studio.set_smoke_enabled(*checked, cx)
                            });
                        }),
                )
                .child(note(smoke_label, cx))
                .child(
                    Button::new("smoke-load")
                        .label("Load .npy/.npz…")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_smoke_pick
.update(cx, |studio, cx| studio.pick_smoke(cx));
                        }),
                )
                .child(
                    Button::new("smoke-clear")
                        .label("Clear")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            studio_smoke_clear
.update(cx, |studio, cx| studio.clear_smoke(cx));
                        }),
                ),
        )
        .child(numeric(
            "Plate exposure",
            format!("{:.2}", smoke.exposure),
            studio_smoke_op_desc,
            |studio, cx| studio.step_smoke_exposure(-0.1, cx),
            studio_smoke_op_inc,
            |studio, cx| studio.step_smoke_exposure(0.1, cx),
        ))
        .child(note(
            "A smoke plate is composited over the finished frame; its certificate covers the terrain pass only.",
            cx,
        ));

    body.into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn numeric(
    label: &'static str,
    value: String,
    dec_entity: Entity<Studio>,
    dec: fn(&mut Studio, &mut Context<Studio>),
    inc_entity: Entity<Studio>,
    inc: fn(&mut Studio, &mut Context<Studio>),
) -> AnyElement {
    h_flex()
        .justify_between()
        .gap_2()
        .child(div().text_xs().child(label))
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    Button::new(format!("num-dec-{label}"))
                        .label("−")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            dec_entity.update(cx, dec);
                        }),
                )
                .child(div().w(px(56.)).text_xs().text_center().child(value))
                .child(
                    Button::new(format!("num-inc-{label}"))
                        .label("+")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            inc_entity.update(cx, inc);
                        }),
                ),
        )
        .into_any_element()
}
