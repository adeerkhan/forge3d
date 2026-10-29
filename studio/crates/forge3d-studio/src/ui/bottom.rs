//! Bottom strip (Blender's timeline slot): one row of icon tabs over the
//! timeline, the console or the certificate list.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::{BottomTab, Studio};

fn tab_icon(tab: BottomTab) -> IconName {
    match tab {
        BottomTab::Timeline => IconName::Film,
        BottomTab::Console => IconName::Terminal,
        BottomTab::Certificates => IconName::ShieldCheck,
    }
}

pub fn bottom(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let active = studio.read(cx).bottom_tab;

    let mut tabs = h_flex()
        .gap_1()
        .px_1()
        .py_1()
        .border_b_1()
        .border_color(cx.theme().colors.border);
    for tab in BottomTab::ALL {
        let selected = active == *tab;
        let studio_pick = studio.clone();
        let label = tab.label();
        tabs = tabs.child(
            Button::new(format!("bottom-tab-{label}"))
                .icon(tab_icon(*tab))
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.ghost())
                .tooltip(label)
                .on_click(move |_, _, cx| {
                    let tab = *tab;
                    studio_pick.update(cx, |studio, cx| studio.set_bottom_tab(tab, cx));
                }),
        );
    }

    let content = match active {
        BottomTab::Timeline => crate::ui::timeline::timeline(studio, window, cx),
        BottomTab::Console => crate::ui::panels::console_entity(studio, window, cx),
        BottomTab::Certificates => crate::ui::certificates::certificates(studio, window, cx),
    };

    v_flex()
        .size_full()
        .child(tabs)
        .child(
            div()
                .id("bottom-content")
                .flex_1()
                .overflow_y_scroll()
                .child(content),
        )
        .into_any_element()
}
