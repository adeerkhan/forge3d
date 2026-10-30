//! The left editor (Blender's Outliner slot): the layer tree, and the same
//! panel switched to the examples gallery.
//!
//! The header carries the Blender-like row of icons for the two editors plus
//! the quick actions that belong to the scene (import, gallery, new group).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::{LeftTab, Studio};

pub fn outliner(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let left = studio.read(cx).left_tab;

    let studio_layers = studio.clone();
    let studio_examples = studio.clone();
    let studio_import = studio.clone();
    let studio_group = studio.clone();

    let header = h_flex()
        .w_full()
        .gap_1()
        .px_1()
        .py_1()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .child(
            tab_button(
                "outliner-layers",
                IconName::Layers,
                "Layers",
                left == LeftTab::Layers,
            )
            .on_click(move |_, _, cx| {
                studio_layers.update(cx, |studio, cx| studio.set_left_tab(LeftTab::Layers, cx));
            }),
        )
        .child(
            tab_button(
                "outliner-examples",
                IconName::Grid2x2,
                "Examples",
                left == LeftTab::Examples,
            )
            .on_click(move |_, _, cx| {
                studio_examples.update(cx, |studio, cx| studio.set_left_tab(LeftTab::Examples, cx));
            }),
        )
        .child(div().flex_1())
        .child(
            icon_button("outliner-import", IconName::Upload, "Import DEM (Ctrl+I)").on_click(
                move |_, _, cx| {
                    studio_import.update(cx, |studio, cx| studio.pick_dem(cx));
                },
            ),
        )
        .child(
            icon_button("outliner-group", IconName::FolderPlus, "Group selection").on_click(
                move |_, _, cx| {
                    studio_group.update(cx, |studio, cx| studio.group_selection(cx));
                },
            ),
        );

    let content = match left {
        LeftTab::Layers => {
            // The layers list is not virtualised; this measures what a big
            // document actually costs so the note in the docs is a number.
            let started = std::time::Instant::now();
            let element = crate::ui::layers::layers(studio, window, cx);
            if std::env::var_os("FORGE3D_STUDIO_UI_TIMING").is_some() {
                let rows = studio.read(cx).visible_rows().len();
                log::info!(
                    "layers panel: {rows} row(s) built in {:.2} ms",
                    started.elapsed().as_secs_f32() * 1000.0
                );
            }
            element
        }
        LeftTab::Examples => crate::ui::gallery::gallery(studio, window, cx),
    };

    v_flex()
        .size_full()
        .child(header)
        .child(div().flex_1().w_full().overflow_hidden().child(content))
        .into_any_element()
}

fn tab_button(id: &'static str, icon: IconName, tooltip: &'static str, selected: bool) -> Button {
    Button::new(id)
        .icon(icon)
        .xsmall()
        .when(selected, |button| button.primary())
        .when(!selected, |button| button.ghost())
        .tooltip(tooltip)
}

fn icon_button(id: &'static str, icon: IconName, tooltip: &'static str) -> Button {
    Button::new(id).icon(icon).xsmall().ghost().tooltip(tooltip)
}
