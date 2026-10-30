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

fn section(title: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().colors.muted_foreground)
        .child(title.into())
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
    let selected = studio
        .read(cx)
        .recipe_selected
        .and_then(|index| layers.iter().find(|layer| layer.index == index).cloned());

    let mut body = v_flex().w_full().gap_3();

    // ---- recipe layers: the scene's paint order, editable
    body = body.child(section(
        if layer_count == 0 {
            "Recipe layers".to_owned()
        } else {
            format!("Recipe layers ({layer_count})")
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
        let studio_eye = studio.clone();
        let studio_select = studio.clone();
        let opacity = layer.opacity;
        let visible = layer.visible;
        let is_selected = selected.as_ref().map(|row| row.index) == Some(index);
        rows = rows.child(
            h_flex()
                .justify_between()
                .gap_2()
                .py(px(1.))
                .when(is_selected, |row| row.bg(cx.theme().colors.secondary))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new(format!("layer-eye-{index}"))
                                .icon(if visible {
                                    IconName::Eye
                                } else {
                                    IconName::EyeOff
                                })
                                .xsmall()
                                .ghost()
                                .tooltip(if visible {
                                    "Hide this layer (it stays in the recipe)"
                                } else {
                                    "Show this layer"
                                })
                                .on_click(move |_, _, cx| {
                                    studio_eye.update(cx, |studio, cx| {
                                        studio.set_recipe_layer_visible(index, !visible, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new(format!("layer-select-{index}"))
                                .label(layer.kind_label())
                                .xsmall()
                                .when(is_selected, |button| button.primary())
                                .when(!is_selected, |button| button.ghost())
                                .tooltip("Select to style this layer")
                                .on_click(move |_, _, cx| {
                                    studio_select.update(cx, |studio, cx| {
                                        studio.select_recipe_layer(index, cx)
                                    });
                                }),
                        )
                        .when(!visible, |row| row.opacity(0.55))
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

    // ---- per-layer style (post-build), for the selected layer
    if let Some(layer) = selected.as_ref() {
        let index = layer.index;
        body = body.child(section(format!("Style · {}", layer.label()), cx));
        match (layer.line_color.clone(), layer.line_width) {
            (Some(color), Some(width)) => {
                let mut chips = h_flex().flex_wrap().gap_1();
                for (name, value) in [
                    ("white", "#ffffff"),
                    ("ink", "#111827"),
                    ("orange", "#ff8800"),
                    ("cyan", "#22d3ee"),
                    ("rose", "#f43f5e"),
                ] {
                    let studio_chip = studio.clone();
                    let value_owned = value.to_owned();
                    let active = color.eq_ignore_ascii_case(value);
                    chips = chips.child(
                        Button::new(format!("style-{index}-{name}"))
                            .label(name)
                            .xsmall()
                            .when(active, |button| button.primary())
                            .when(!active, |button| button.outline())
                            .tooltip(format!("Line colour {value}"))
                            .on_click(move |_, _, cx| {
                                let value = value_owned.clone();
                                studio_chip.update(cx, |studio, cx| {
                                    studio.set_recipe_vector_style(index, Some(value), None, cx)
                                });
                            }),
                    );
                }
                let studio_thin = studio.clone();
                let studio_thick = studio.clone();
                body = body
                    .child(chips)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(div().text_xs().child("Width"))
                            .child(
                                Button::new(format!("style-{index}-thin"))
                                    .label("−")
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Thinner line")
                                    .on_click(move |_, _, cx| {
                                        studio_thin.update(cx, |studio, cx| {
                                            studio.set_recipe_vector_style(
                                                index,
                                                None,
                                                Some((width - 0.5).max(0.5)),
                                                cx,
                                            )
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(48.))
                                    .text_xs()
                                    .text_center()
                                    .child(format!("{width:.1} px")),
                            )
                            .child(
                                Button::new(format!("style-{index}-thick"))
                                    .label("+")
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Thicker line")
                                    .on_click(move |_, _, cx| {
                                        studio_thick.update(cx, |studio, cx| {
                                            studio.set_recipe_vector_style(
                                                index,
                                                None,
                                                Some((width + 0.5).min(24.0)),
                                                cx,
                                            )
                                        });
                                    }),
                            ),
                    )
                    .child(hint(
                        "Written into the recipe's Mapbox line paint; the next preview redraws with it.",
                        cx,
                    ));
            }
            _ => {
                body = body.child(hint(
                    "This layer kind has no line paint to edit — raster opacity is in the layer row above.",
                    cx,
                ));
            }
        }
    }

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
            .child({
                let studio_svg = studio.clone();
                Button::new("map-export-svg")
                    .label("Export SVG…")
                    .xsmall()
                    .outline()
                    .disabled(!has_recipe)
                    .tooltip(if has_recipe {
                        "Write the recipe's vector and label layers as print SVG (Pro)"
                    } else {
                        "Build the recipe first"
                    })
                    .on_click(move |_, _, cx| {
                        studio_svg.update(cx, |studio, cx| studio.export_svg(cx));
                    })
            })
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
