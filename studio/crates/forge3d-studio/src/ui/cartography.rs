//! Cartography: the map furniture a QGIS-style layout tab edits.
//!
//! Title, legend, scale bar, north arrow and graticule are fields of the
//! recipe's `MapFurnitureLayer`, drawn by the engine over the finished frame.
//! This panel writes those fields (through the bridge, which owns the schema)
//! and can render the result, so what you see is the engine's own output with
//! your furniture on it.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::Studio;

const POSITIONS: &[&str] = &["top-left", "top-right", "bottom-left", "bottom-right"];

fn section(title: &'static str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().colors.muted_foreground)
        .child(title)
        .into_any_element()
}

fn hint(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}

fn position_chips(studio: &Entity<Studio>, kind: &'static str, current: &str) -> AnyElement {
    let mut row = h_flex().gap_1().flex_wrap();
    for position in POSITIONS {
        let selected = current == *position;
        let studio_pick = studio.clone();
        let name = (*position).to_owned();
        row = row.child(
            Button::new(format!("furniture-{kind}-{position}"))
                .label((*position).replace('-', " "))
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.outline())
                .on_click(move |_, _, cx| {
                    let name = name.clone();
                    studio_pick.update(cx, |studio, cx| {
                        studio.set_furniture_position(kind, name, cx)
                    });
                }),
        );
    }
    row.into_any_element()
}

pub fn furniture_panel(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let (furniture, has_recipe, status) = {
        let state = studio.read(cx);
        (
            state.furniture.clone(),
            state.mapscene.recipe_json.is_some(),
            state.furniture.status.clone(),
        )
    };

    let title_input = studio.update(cx, |studio, cx| {
        studio.furniture_input("title", &furniture.title, window, cx)
    });
    let legend_input = studio.update(cx, |studio, cx| {
        studio.furniture_input("legend_items", &furniture.legend_items, window, cx)
    });

    let studio_title_toggle = studio.clone();
    let studio_legend_toggle = studio.clone();
    let studio_scale_toggle = studio.clone();
    let studio_arrow_toggle = studio.clone();
    let studio_graticule_toggle = studio.clone();
    let studio_units_km = studio.clone();
    let studio_units_mi = studio.clone();
    let studio_interval_dec = studio.clone();
    let studio_interval_inc = studio.clone();
    let studio_apply = studio.clone();
    let studio_render = studio.clone();
    let studio_export = studio.clone();

    let layers = studio.read(cx).recipe_layers();
    let layer_count = studio
        .read(cx)
        .mapscene
        .recipe_json
        .as_deref()
        .map(crate::ui::recipe::layer_count)
        .unwrap_or(0);
    let output = studio
        .read(cx)
        .mapscene
        .recipe_json
        .as_deref()
        .and_then(crate::ui::recipe::output_size)
        .unwrap_or((0, 0));
    let has_raster = studio.read(cx).mapscene.raster_path.is_some();

    let mut body = v_flex().w_full().gap_3();

    // ---- recipe layers: the scene's paint order, editable
    body = body.child(section(
        if layer_count == 0 {
            "Recipe layers"
        } else if layer_count == 1 {
            "Recipe layers (1)"
        } else {
            "Recipe layers (2+)"
        },
        cx,
    ));
    if layers.is_empty() {
        body = body.child(hint(
            "No layers yet — build the recipe, then add overlays by choosing a raster.",
            cx,
        ));
    }
    let mut rows = v_flex().w_full().gap_1();
    for layer in &layers {
        let index = layer.index;
        let studio_up = studio.clone();
        let studio_down = studio.clone();
        let studio_remove = studio.clone();
        let studio_less = studio.clone();
        let studio_more = studio.clone();
        let opacity = layer.opacity;
        rows = rows.child(
            h_flex()
                .justify_between()
                .gap_2()
                .py(px(1.))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(div().text_xs().child(layer.kind_label()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(layer.label()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .when(layer.opacity < 0.999, |row| {
                            row.child(
                                div()
                                    .w(px(40.))
                                    .text_xs()
                                    .text_right()
                                    .child(format!("{:.0}%", opacity * 100.0)),
                            )
                        })
                        .when(layer.opacity < 0.999, |row| {
                            let studio_less = studio_less.clone();
                            let studio_more = studio_more.clone();
                            row.child(
                                Button::new(format!("layer-less-{index}"))
                                    .label("−")
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Less opaque")
                                    .on_click(move |_, _, cx| {
                                        studio_less.update(cx, |studio, cx| {
                                            studio.set_recipe_layer_opacity(
                                                index,
                                                (opacity - 0.1).max(0.0),
                                                cx,
                                            )
                                        });
                                    }),
                            )
                            .child(
                                Button::new(format!("layer-more-{index}"))
                                    .label("+")
                                    .xsmall()
                                    .ghost()
                                    .tooltip("More opaque")
                                    .on_click(move |_, _, cx| {
                                        studio_more.update(cx, |studio, cx| {
                                            studio.set_recipe_layer_opacity(
                                                index,
                                                (opacity + 0.1).min(1.0),
                                                cx,
                                            )
                                        });
                                    }),
                            )
                        })
                        .child(
                            Button::new(format!("layer-up-{index}"))
                                .icon(IconName::ArrowUp)
                                .xsmall()
                                .ghost()
                                .tooltip("Move earlier in paint order")
                                .on_click(move |_, _, cx| {
                                    studio_up.update(cx, |studio, cx| {
                                        studio.move_recipe_layer(index, -1, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new(format!("layer-down-{index}"))
                                .icon(IconName::ArrowDown)
                                .xsmall()
                                .ghost()
                                .tooltip("Move later in paint order")
                                .on_click(move |_, _, cx| {
                                    studio_down.update(cx, |studio, cx| {
                                        studio.move_recipe_layer(index, 1, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new(format!("layer-remove-{index}"))
                                .icon(IconName::Trash)
                                .xsmall()
                                .ghost()
                                .tooltip("Remove this layer from the recipe")
                                .on_click(move |_, _, cx| {
                                    studio_remove.update(cx, |studio, cx| {
                                        studio.remove_recipe_layer(index, cx)
                                    });
                                }),
                        ),
                ),
        );
    }
    body = body.child(rows);

    // ---- output size
    body = body.child(section("Output", cx));
    let mut presets = h_flex().flex_wrap().gap_1();
    for (label, width, height) in [
        ("960×600", 960u32, 600u32),
        ("1280×720", 1280, 720),
        ("1920×1080", 1920, 1080),
        ("2560×1440", 2560, 1440),
    ] {
        let selected = output == (width, height);
        let studio_size = studio.clone();
        presets = presets.child(
            Button::new(format!("output-{label}"))
                .label(label)
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.outline())
                .disabled(!has_recipe)
                .tooltip(if has_recipe {
                    "Render size for this map"
                } else {
                    "Build the recipe first"
                })
                .on_click(move |_, _, cx| {
                    studio_size.update(cx, |studio, cx| studio.set_output_size(width, height, cx));
                }),
        );
    }
    body = body.child(
        h_flex()
            .gap_2()
            .items_center()
            .child(presets)
            .when(has_recipe, |row| {
                row.child(hint(format!("now {}×{}", output.0, output.1), cx))
            }),
    );

    body = body.child(section("Map furniture", cx));
    body = body.child(hint(
        "Drawn by the engine over the finished frame; labels keep out of the way.",
        cx,
    ));

    // Title
    body = body.child(
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Checkbox::new("furniture-title-toggle")
                            .checked(!furniture.title.trim().is_empty())
                            .on_click(move |checked, _, cx| {
                                let checked = *checked;
                                studio_title_toggle.update(cx, |studio, cx| {
                                    if !checked {
                                        studio.set_furniture_title(String::new(), cx);
                                    } else {
                                        cx.notify();
                                    }
                                });
                            }),
                    )
                    .child(div().text_xs().child("Title")),
            )
            .child(Input::new(&title_input).small()),
    );

    // Legend
    body = body
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Checkbox::new("furniture-legend-toggle")
                        .checked(furniture.legend_enabled)
                        .on_click(move |checked, _, cx| {
                            let checked = *checked;
                            studio_legend_toggle.update(cx, |studio, cx| {
                                studio.toggle_furniture("legend", checked, cx)
                            });
                        }),
                )
                .child(div().text_xs().child("Legend")),
        )
        .child(Input::new(&legend_input).small())
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(hint("Comma-separated swatches, up to eight.", cx))
                .child(div().flex_1())
                .child(
                    Button::new("legend-from-raster")
                        .label("From raster…")
                        .xsmall()
                        .outline()
                        .disabled(!has_raster)
                        .tooltip(if has_raster {
                            "Classify the raster overlay and fill the legend"
                        } else {
                            "Choose a raster overlay first"
                        })
                        .on_click({
                            let studio_legend = studio.clone();
                            move |_, _, cx| {
                                studio_legend
                                    .update(cx, |studio, cx| studio.legend_from_raster(cx));
                            }
                        }),
                ),
        );

    // Scale bar
    body = body
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Checkbox::new("furniture-scale-toggle")
                        .checked(furniture.scale_bar_enabled)
                        .on_click(move |checked, _, cx| {
                            let checked = *checked;
                            studio_scale_toggle.update(cx, |studio, cx| {
                                studio.toggle_furniture("scale_bar", checked, cx)
                            });
                        }),
                )
                .child(div().text_xs().child("Scale bar")),
        )
        .child(position_chips(
            studio,
            "scale_bar",
            &furniture.scale_bar_position,
        ))
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("furniture-scale-km")
                        .label("km")
                        .xsmall()
                        .when(furniture.scale_bar_units == "km", |b| b.primary())
                        .when(furniture.scale_bar_units != "km", |b| b.outline())
                        .on_click(move |_, _, cx| {
                            studio_units_km.update(cx, |studio, cx| {
                                studio.set_furniture_units("km".to_owned(), cx)
                            });
                        }),
                )
                .child(
                    Button::new("furniture-scale-mi")
                        .label("mi")
                        .xsmall()
                        .when(furniture.scale_bar_units == "mi", |b| b.primary())
                        .when(furniture.scale_bar_units != "mi", |b| b.outline())
                        .on_click(move |_, _, cx| {
                            studio_units_mi.update(cx, |studio, cx| {
                                studio.set_furniture_units("mi".to_owned(), cx)
                            });
                        }),
                ),
        );

    // North arrow
    body = body
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Checkbox::new("furniture-arrow-toggle")
                        .checked(furniture.north_arrow_enabled)
                        .on_click(move |checked, _, cx| {
                            let checked = *checked;
                            studio_arrow_toggle.update(cx, |studio, cx| {
                                studio.toggle_furniture("north_arrow", checked, cx)
                            });
                        }),
                )
                .child(div().text_xs().child("North arrow")),
        )
        .child(position_chips(
            studio,
            "north_arrow",
            &furniture.north_arrow_position,
        ));

    // Graticule
    body = body.child(
        h_flex()
            .gap_2()
            .items_center()
            .child(
                Checkbox::new("furniture-graticule-toggle")
                    .checked(furniture.graticule_enabled)
                    .on_click(move |checked, _, cx| {
                        let checked = *checked;
                        studio_graticule_toggle.update(cx, |studio, cx| {
                            studio.toggle_furniture("graticule", checked, cx)
                        });
                    }),
            )
            .child(div().text_xs().child("Graticule"))
            .child(div().flex_1())
            .child(
                Button::new("graticule-dec")
                    .label("−")
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_interval_dec.update(cx, |studio, cx| {
                            let next = studio.furniture.graticule_interval_deg / 2.0;
                            studio.set_furniture_graticule(next, cx)
                        });
                    }),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_xs()
                    .text_center()
                    .child(format!("{:.2}°", furniture.graticule_interval_deg)),
            )
            .child(
                Button::new("graticule-inc")
                    .label("+")
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_interval_inc.update(cx, |studio, cx| {
                            let next = studio.furniture.graticule_interval_deg * 2.0;
                            studio.set_furniture_graticule(next, cx)
                        });
                    }),
            ),
    );

    // Actions
    body = body.child(
        h_flex()
            .gap_1()
            .child(
                Button::new("furniture-apply")
                    .label("Apply")
                    .xsmall()
                    .outline()
                    .on_click(move |_, _, cx| {
                        studio_apply.update(cx, |studio, cx| studio.apply_furniture(false, cx));
                    }),
            )
            .child(
                Button::new("furniture-render")
                    .label("Apply & render")
                    .xsmall()
                    .primary()
                    .on_click(move |_, _, cx| {
                        studio_render.update(cx, |studio, cx| studio.apply_furniture(true, cx));
                    }),
            )
            .child(
                Button::new("map-export")
                    .label("Export map…")
                    .xsmall()
                    .outline()
                    .disabled(!has_recipe)
                    .tooltip(if has_recipe {
                        "Render the recipe straight to a PNG"
                    } else {
                        "Build the recipe first"
                    })
                    .on_click(move |_, _, cx| {
                        studio_export.update(cx, |studio, cx| studio.export_map(cx));
                    }),
            )
            .when(!has_recipe, |row| {
                row.child(hint("build a recipe first", cx))
            }),
    );

    if let Some(status) = status {
        body = body.child(
            h_flex()
                .gap_1()
                .items_center()
                .child(IconName::CircleCheck)
                .child(hint(status, cx)),
        );
    }

    body.into_any_element()
}
