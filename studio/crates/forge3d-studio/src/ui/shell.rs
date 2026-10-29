//! The window shell: title bar with the menu bar, the dock area, the status
//! bar, and the modal dialogs the registry can open.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _, TitleBar,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions;
use crate::state::Studio;

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = std::env::var_os("FORGE3D_STUDIO_UI_TIMING")
            .is_some()
            .then(std::time::Instant::now);
        let title = format!(
            "Forge3D Studio — {}{}",
            self.project.name,
            if self.project_dirty { " •" } else { "" }
        );
        window.set_window_title(&title);

        let theme_button = Button::new("theme-toggle")
            .icon(if cx.theme().mode.is_dark() {
                IconName::Sun
            } else {
                IconName::Moon
            })
            .xsmall()
            .ghost()
            .on_click(|_, _, cx| {
                let next = if cx.theme().mode.is_dark() {
                    gpui_kit::component::theme::ThemeMode::Light
                } else {
                    gpui_kit::component::theme::ThemeMode::Dark
                };
                gpui_kit::component::theme::Theme::change(next, None, cx);
            });

        let about_button = Button::new("about")
            .icon(IconName::Info)
            .xsmall()
            .ghost()
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(actions::About), cx);
            });

        let title_bar = TitleBar::new()
            .child(self.menu_bar.clone())
            .child(div().flex_1())
            .child(h_flex().gap_1().child(theme_button).child(about_button));

        // Blender-style workspace row: one flat tab per working arrangement.
        let mut workspaces = h_flex()
            .w_full()
            .gap_1()
            .px_2()
            .py_0p5()
            .border_b_1()
            .border_color(cx.theme().colors.border);
        for workspace in crate::state::Workspace::ALL {
            let selected = self.workspace == *workspace;
            let entity = cx.entity();
            workspaces = workspaces.child(
                Button::new(format!("workspace-{}", workspace.label()))
                    .label(workspace.label())
                    .xsmall()
                    .when(selected, |button| button.primary())
                    .when(!selected, |button| button.ghost())
                    .on_click(move |_, _, cx| {
                        let workspace = *workspace;
                        entity.update(cx, |studio, cx| studio.set_workspace(workspace, cx));
                    }),
            );
        }

        let root = v_flex()
            .size_full()
            .bg(cx.theme().colors.background)
            .child(title_bar)
            .child(workspaces)
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.))
                    .child(self.dock_area.clone()),
            )
            .child(self.status_bar(cx));

        // Dev hook: how long the UI tree takes to rebuild. If this is
        // milliseconds per pointer event, the CPU — not the GPU — is what
        // makes an orbit feel slow.
        if let Some(started) = started {
            self.ui_timing
                .record(started.elapsed().as_secs_f32() * 1000.0);
        }
        root
    }
}

impl Studio {
    fn status_bar(&self, cx: &App) -> impl IntoElement {
        let mut right = h_flex()
            .gap_3()
            .text_xs()
            .text_color(cx.theme().colors.muted_foreground);

        if let Some(layer) = self
            .primary_layer()
            .and_then(|id| self.project.document.layer(id))
        {
            right = right.child(format!("{} · {}", layer.name, layer.kind.label()));
        }
        right = right.child(format!("{:.1} ms", self.frame_ms));
        if !self.adapter.is_empty() {
            right = right.child(format!("{} · {}", self.adapter, self.backend));
        }
        if self.software {
            right = right.child("software");
        }

        let left = h_flex()
            .gap_2()
            .text_xs()
            .when(self.busy, |row| {
                row.child(IconName::Loader).child("working")
            })
            .child(self.status.clone());

        StatusBar::new().left(left).right(right)
    }
}

impl Studio {
    /// Open a dialog on the window this entity lives in.
    fn with_dialog(
        cx: &mut Context<Self>,
        build: impl Fn(
                gpui_kit::component::dialog::Dialog,
                &mut Window,
                &mut App,
            ) -> gpui_kit::component::dialog::Dialog
            + 'static,
    ) {
        crate::ui::open_dialog(&mut *cx, build);
    }

    /// `Help ▸ Keyboard shortcuts`: generated from the action registry.
    pub fn show_shortcuts(&mut self, cx: &mut Context<Self>) {
        let groups: Vec<(String, Vec<(String, String)>)> = actions::shortcut_rows()
            .into_iter()
            .map(|(menu, rows)| {
                (
                    menu.to_owned(),
                    rows.into_iter()
                        .map(|(label, keys)| (label.to_owned(), keys))
                        .collect(),
                )
            })
            .collect();
        Self::with_dialog(cx, move |dialog, _, cx| {
            let mut list = v_flex().gap_2();
            for (menu, rows) in groups.clone() {
                let mut block = v_flex().gap_1().child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(menu),
                );
                for (label, keys) in rows {
                    block = block.child(
                        h_flex()
                            .justify_between()
                            .gap_8()
                            .child(div().text_xs().child(label))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child(keys),
                            ),
                    );
                }
                list = list.child(block);
            }
            dialog
                .title("Keyboard shortcuts")
                .child(list)
                .width(px(400.))
        });
    }

    /// `Help ▸ About Forge3D Studio`.
    pub fn show_about(&mut self, cx: &mut Context<Self>) {
        let version = env!("CARGO_PKG_VERSION").to_owned();
        let adapter = self.adapter.clone();
        let backend = self.backend.clone();
        let ops = "31 bridge ops";
        Self::with_dialog(cx, move |dialog, _, cx| {
            dialog.title("Forge3D Studio").child(
                v_flex()
                    .gap_1()
                    .text_xs()
                    .child(format!("Version {version}"))
                    .child(ops)
                    .child(format!("Device: {adapter} · {backend}"))
                    .child(div().text_color(cx.theme().colors.muted_foreground).child(
                        "Renders come from the Forge3D engine: the certified Python \
                                 pipeline for exports and certificates, the native TerrainScene \
                                 for the interactive frame.",
                    )),
            )
        });
    }
}
