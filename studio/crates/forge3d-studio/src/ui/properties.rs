//! The properties editor: a Blender-style icon strip with one tab's content.
//!
//! Everything that used to be a separate dock tab (layer facts, terrain
//! parameters, render quality, world/environment, MapScene, statistics) lives
//! behind one vertical icon strip, so the right side of the window is one
//! column instead of a pile of tabs.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
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
        PropertiesTab::Scene => (IconName::Layers, "MapScene"),
        PropertiesTab::Stats => (IconName::ChartColumn, "Statistics"),
    }
}

pub fn properties(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let active = studio.read(cx).properties_tab;

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

    let weak = studio.downgrade();
    let content = match active {
        PropertiesTab::Layer => crate::ui::panels::inspector(&weak, window, cx),
        PropertiesTab::Terrain => crate::ui::params::params_pages(studio, window, cx),
        PropertiesTab::Render => crate::ui::certificates::render(studio, window, cx),
        PropertiesTab::World => crate::ui::scene::environment(studio, window, cx),
        PropertiesTab::Scene => crate::ui::scene::mapscene(studio, window, cx),
        PropertiesTab::Stats => crate::ui::panels::statistics_entity(studio, window, cx),
    };

    h_flex()
        .size_full()
        .child(strip)
        .child(
            div()
                .id("properties-content")
                .flex_1()
                .h_full()
                .overflow_y_scroll()
                .child(content),
        )
        .into_any_element()
}
