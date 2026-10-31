//! The properties editor: a Blender-style icon strip with one tab's content.
//!
//! Everything that used to be a separate dock tab (layer facts, terrain
//! parameters, render quality, world/environment, MapScene, statistics) lives
//! behind one vertical icon strip, so the right side of the window is one
//! column instead of a pile of tabs.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::{PropertiesTab, Studio};

/// Icon and tooltip per properties tab, in strip order.
pub fn tab_icon(tab: PropertiesTab) -> (IconName, &'static str) {
    match tab {
        PropertiesTab::Layer => (IconName::SlidersHorizontal, "Layer"),
        PropertiesTab::Terrain => (IconName::Mountain, "Terrain parameters"),
        PropertiesTab::Render => (IconName::Aperture, "Render"),
        PropertiesTab::World => (IconName::Globe, "World & environment"),
        PropertiesTab::Cartography => (IconName::Map, "Cartography"),
        PropertiesTab::Stats => (IconName::ChartColumn, "Statistics"),
    }
}

/// The properties editor: one page, with the icon strip when it is enabled.
///
/// The strip is a second navigation for the same pages the workspace tabs open
/// (`Workspace::properties_tab`). It is off in this pass — see
/// `crate::ui::surfaces` — so the page gets the whole column; the strip code is
/// still compiled and still drawn when the switch is turned back on.
pub fn properties(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let active = studio.read(cx).properties_tab;
    let (_, active_title) = tab_icon(active);
    let content = content_for(studio, active, window, cx);

    let header = h_flex()
        .justify_between()
        .gap_2()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(cx.theme().colors.border)
        .child(div().text_xs().font_semibold().child(active_title))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(format!(
                    "{} / {}",
                    crate::state::Workspace::ALL
                        .iter()
                        .find(|workspace| workspace.properties_tab() == active)
                        .map(|workspace| workspace.label())
                        .unwrap_or("Custom"),
                    active.label()
                )),
        );

    let mut panel = h_flex().size_full();
    if crate::ui::surfaces::PROPERTIES_STRIP {
        panel = panel.child(icon_strip(studio, active, cx));
    }
    panel
        .child(
            v_flex()
                .flex_1()
                .h_full()
                .min_w(px(0.))
                .child(header)
                .child(
                    div()
                        .id("properties-content")
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_y_scroll()
                        .child(content),
                ),
        )
        .into_any_element()
}

/// The vertical icon strip: kept for the switch, not drawn while it is off.
fn icon_strip(studio: &Entity<Studio>, active: PropertiesTab, cx: &App) -> AnyElement {
    let mut strip = v_flex()
        .id("properties-strip")
        .w(px(38.))
        .h_full()
        .items_center()
        .gap_1()
        .py_1()
        .border_r_1()
        .border_color(cx.theme().colors.border);
    for tab in PropertiesTab::ALL {
        let (icon, tooltip) = tab_icon(*tab);
        let selected = active == *tab;
        let studio_pick = studio.clone();
        strip = strip.child(
            Button::new(format!("properties-tab-{}", tab.label()))
                .icon(icon)
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.ghost())
                .tooltip(tooltip)
                .on_click(move |_, _, cx| {
                    let tab = *tab;
                    studio_pick.update(cx, |studio, cx| studio.set_properties_tab(tab, cx));
                }),
        );
    }
    strip.into_any_element()
}

/// The page for `active`.
fn content_for(
    studio: &Entity<Studio>,
    active: PropertiesTab,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match active {
        PropertiesTab::Layer => crate::ui::panels::inspector_entity(studio, window, cx),
        PropertiesTab::Terrain => crate::ui::params::params_pages(studio, window, cx),
        PropertiesTab::Render => crate::ui::certificates::render(studio, window, cx),
        PropertiesTab::World => crate::ui::scene::environment(studio, window, cx),
        PropertiesTab::Cartography => {
            let mut body = v_flex().w_full().gap_3();
            body = body.child(crate::ui::scene::mapscene(studio, window, cx));
            body = body.child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().colors.border)
                    .child(crate::ui::cartography::furniture_panel(studio, window, cx)),
            );
            body.into_any_element()
        }
        PropertiesTab::Stats => crate::ui::panels::statistics_entity(studio, window, cx),
    }
}
