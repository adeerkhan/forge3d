//! The examples gallery and its download carousel (G10).
//!
//! Every `examples/*.py` workflow is a card. Clicking a card opens a carousel
//! that discloses *everything the scene needs from disk* — DEM, rasters, the
//! thumbnail — with its size and status, and only then offers to open it. A
//! scene whose assets are missing says so and can fetch them through git LFS;
//! a scene that needs an engine milestone says which one, instead of offering a
//! button that cannot work.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::{AssetKind, AssetStatus, CardState, Studio};

fn muted(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}

fn bytes_human(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

/// The gallery panel body: filter row plus the card list.
pub fn gallery(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (entries, filter, runnable_only, cards) = {
        let state = studio.read(cx);
        (
            state.visible_examples(),
            state.gallery_filter.clone(),
            state.gallery_runnable_only,
            state.gallery_cards.clone(),
        )
    };
    let categories = {
        let state = studio.read(cx);
        let mut list: Vec<String> = state
            .examples
            .iter()
            .map(|entry| entry.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        list.sort();
        list.insert(0, "All".to_owned());
        list
    };

    let mut chips = h_flex().flex_wrap().gap_1().px_2().pt_1();
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
                .on_click(move |_, _, c| {
                    let name = name.clone();
                    studio_pick.update(c, |studio, cx| {
                        studio.gallery_filter = name;
                        cx.notify();
                    });
                }),
        );
    }
    let studio_only = studio.clone();
    chips = chips.child(
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Checkbox::new("gallery-runnable-only")
                    .checked(runnable_only)
                    .on_click(move |checked, _, c| {
                        let checked = *checked;
                        studio_only.update(c, |studio, cx| {
                            studio.gallery_runnable_only = checked;
                            cx.notify();
                        });
                    }),
            )
            .child(muted("only what runs today", cx)),
    );

    let mut list = v_flex().w_full();
    if entries.is_empty() {
        list = list.child(
            div()
                .p_3()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child("No examples match this filter."),
        );
    }

    for entry in &entries {
        let runnable = entry.tier == 'A';
        let missing_ops = studio.update(cx, |studio, _| studio.missing_ops(entry));
        let state = cards.get(&entry.id).cloned().unwrap_or(CardState::Idle);
        let thumbnail = studio.update(cx, |studio, cx| studio.example_thumbnail(entry, cx));
        let studio_card = studio.clone();
        let id = entry.id.clone();

        let mut card = h_flex()
            .id(format!("gallery-card-{}", entry.id))
            .w_full()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().colors.border)
            .cursor_pointer();

        if let Some(image) = thumbnail {
            card = card.child(
                div()
                    .w(px(72.))
                    .h(px(44.))
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().colors.border)
                    .child(img(image).size_full().object_fit(ObjectFit::Cover)),
            );
        } else {
            card = card.child(
                div()
                    .w(px(72.))
                    .h(px(44.))
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().colors.muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(IconName::Image),
            );
        }

        let badge = match (&state, entry.tier) {
            (CardState::Loading, _) => ("opening…", cx.theme().colors.warning),
            (CardState::Loaded, _) => ("opened", cx.theme().colors.success),
            (CardState::Failed(_), _) => ("failed", cx.theme().colors.danger),
            (_, 'A') if missing_ops.is_empty() => ("runs today", cx.theme().colors.success),
            (_, 'A') => ("engine op missing", cx.theme().colors.danger),
            (_, tier) => (
                if tier == 'B' {
                    "needs a milestone"
                } else {
                    "needs a download"
                },
                cx.theme().colors.muted_foreground,
            ),
        };

        card = card
            .on_click(move |_, window, c| {
                let id = id.clone();
                log::info!("gallery: card clicked ({id})");
                let entity = studio_card.clone();
                entity.update(c, |studio, cx| studio.preview_example(id, cx));
                carousel_dialog(entity, window, c);
            })
            .child(
                v_flex()
                    .flex_1()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
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
                            .child(div().text_xs().text_color(badge.1).child(badge.0)),
                    )
                    .child(muted(entry.description.clone(), cx))
                    .when(matches!(&state, CardState::Failed(_)), |card| {
                        card.child(div().text_xs().text_color(cx.theme().colors.danger).child(
                            match &state {
                                CardState::Failed(message) => message.clone(),
                                _ => String::new(),
                            },
                        ))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(if runnable {
                        entry.category.clone()
                    } else {
                        format!("{} · {}", entry.category, entry.unlock)
                    }),
            );

        list = list.child(card);
    }

    v_flex()
        .w_full()
        .child(chips)
        .child(
            div()
                .id("gallery-list")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        )
        .into_any_element()
}

/// The carousel dialog for the selected example: assets, provenance, actions.
///
/// It rebuilds from live state on every paint, so Prev/Next page in place.
pub fn carousel_dialog(studio: Entity<Studio>, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let Some(id) = studio.read(cx).gallery_selected.clone() else {
            return dialog
                .title("Examples")
                .child(muted("No example selected.", cx));
        };
        let entries = studio.read(cx).visible_examples();
        let index = entries.iter().position(|entry| entry.id == id).unwrap_or(0);
        let entry = entries.get(index).cloned();
        let Some(entry) = entry else {
            return dialog
                .title("Examples")
                .child(muted("No example selected.", cx));
        };

        let assets = studio.read(cx).example_assets(&entry);
        let missing_ops = studio.update(cx, |studio, _| studio.missing_ops(&entry));
        let declared_ops = entry.ops.clone();
        let thumbnail = studio.update(cx, |studio, cx| studio.example_thumbnail(&entry, cx));
        let card_state = studio
            .read(cx)
            .gallery_cards
            .get(&entry.id)
            .cloned()
            .unwrap_or(CardState::Idle);
        let runnable = entry.tier == 'A' && missing_ops.is_empty();
        let ready = assets.iter().all(|asset| asset.ready());
        let fetchable = assets
            .iter()
            .any(|asset| matches!(asset.kind, AssetKind::LfsPointer | AssetKind::Missing));

        let studio_open = studio.clone();
        let studio_prev = studio.clone();
        let studio_next = studio.clone();
        let studio_download = studio.clone();
        let entry_for_open = entry.clone();
        let entry_for_download = entry.clone();

        let mut body = v_flex().w_full().gap_3();
        body = body.child(
            h_flex()
                .gap_3()
                .items_start()
                .child(if let Some(image) = thumbnail {
                    div()
                        .w(px(240.))
                        .h(px(150.))
                        .rounded(cx.theme().radius)
                        .overflow_hidden()
                        .border_1()
                        .border_color(cx.theme().colors.border)
                        .child(img(image).size_full().object_fit(ObjectFit::Cover))
                        .into_any_element()
                } else {
                    div()
                        .w(px(240.))
                        .h(px(150.))
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().colors.muted)
                        .flex()
                        .flex_col()
                        .gap_1()
                        .items_center()
                        .justify_center()
                        .px_2()
                        .child(muted(
                            if entry.tier == 'A' {
                                "thumbnail not generated yet".to_owned()
                            } else {
                                format!("no thumbnail — this example needs {}", entry.unlock)
                            },
                            cx,
                        ))
                        .into_any_element()
                })
                .child(
                    v_flex()
                        .flex_1()
                        .gap_1()
                        .child(div().text_sm().font_semibold().child(entry.title.clone()))
                        .child(muted(entry.description.clone(), cx))
                        .child(muted(
                            format!("{} · {} · {}", entry.category, entry.example, entry.unlock),
                            cx,
                        ))
                        .child(match &card_state {
                            CardState::Failed(message) => div()
                                .text_xs()
                                .text_color(cx.theme().colors.danger)
                                .child(message.clone())
                                .into_any_element(),
                            CardState::Loading => muted("opening…", cx),
                            CardState::Loaded => div()
                                .text_xs()
                                .text_color(cx.theme().colors.success)
                                .child("opened")
                                .into_any_element(),
                            CardState::Idle => muted("", cx),
                        }),
                ),
        );

        body = body.child(
            v_flex()
                .gap_1()
                .child(div().text_xs().font_semibold().child(if assets.is_empty() {
                    "Nothing to download".to_owned()
                } else {
                    format!("Needs from disk ({})", assets.len())
                }))
                .child(asset_rows(&assets, cx)),
        );

        if !declared_ops.is_empty() {
            let mut ops = v_flex().gap_1().child(
                div()
                    .text_xs()
                    .font_semibold()
                    .child(format!("Engine ops ({})", declared_ops.len())),
            );
            for op in &declared_ops {
                let missing = missing_ops.contains(op);
                ops = ops.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(if missing {
                            IconName::CircleX
                        } else {
                            IconName::CircleCheck
                        })
                        .child(div().text_xs().child(op.clone()))
                        .child(muted(
                            if missing {
                                "missing from this bridge"
                            } else {
                                ""
                            },
                            cx,
                        )),
                );
            }
            body = body.child(ops);
        }

        if !runnable {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.warning)
                    .child(if missing_ops.is_empty() {
                        format!(
                            "This workflow needs {} — its engine surface is not in Studio yet, so it cannot open.",
                            entry.unlock
                        )
                    } else {
                        format!(
                            "This bridge does not implement {} — the card stays closed until it does.",
                            missing_ops.join(", ")
                        )
                    }),
            );
        }

        let open_label = if ready { "Open" } else { "Download and open" };
        let mut footer = h_flex().gap_2().items_center();
        footer = footer
            .child(
                Button::new("carousel-prev")
                    .icon(IconName::ChevronLeft)
                    .label(format!("{} / {}", index + 1, entries.len()))
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_prev.update(cx, |studio, cx| studio.step_carousel(-1, cx));
                    }),
            )
            .child(
                Button::new("carousel-next")
                    .icon(IconName::ChevronRight)
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_next.update(cx, |studio, cx| studio.step_carousel(1, cx));
                    }),
            )
            .child(div().flex_1());

        if runnable {
            let open = Button::new("carousel-open").label(open_label).small();
            footer = footer.child(if ready {
                open.primary().on_click(move |_, _, cx| {
                    let entry = entry_for_open.clone();
                    studio_open.update(cx, |studio, cx| studio.open_example(entry, cx));
                })
            } else if fetchable {
                open.on_click(move |_, _, cx| {
                    let entry = entry_for_download.clone();
                    studio_download.update(cx, |studio, cx| studio.download_and_open(entry, cx));
                })
            } else {
                open.disabled(true)
            });
        }

        dialog
            .title(format!("Example {}", index + 1))
            .child(body)
            .width(px(560.))
            .child(DialogFooter::new().child(footer))
    });
}

fn asset_rows(assets: &[AssetStatus], cx: &App) -> AnyElement {
    let mut rows = v_flex().w_full().gap_1();
    for asset in assets {
        let (icon, tone, detail) = match &asset.kind {
            AssetKind::Present(bytes) => (
                IconName::CircleCheck,
                cx.theme().colors.success,
                bytes_human(*bytes),
            ),
            AssetKind::LfsPointer => (
                IconName::CircleAlert,
                cx.theme().colors.warning,
                "git LFS pointer — needs a fetch".to_owned(),
            ),
            AssetKind::Missing => (
                IconName::CircleX,
                cx.theme().colors.danger,
                "missing".to_owned(),
            ),
            AssetKind::Remote(url) => (
                IconName::Cloud,
                cx.theme().colors.muted_foreground,
                format!("remote · {url}"),
            ),
        };
        rows = rows.child(
            h_flex()
                .justify_between()
                .gap_2()
                .py(px(1.))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(icon)
                        .child(div().text_xs().child(asset.label.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(asset.declared.clone()),
                        ),
                )
                .child(div().text_xs().text_color(tone).child(detail)),
        );
    }
    rows.into_any_element()
}

/// Open the carousel for the current selection (called by the panel's card
/// click, and by the Examples menu action).
pub fn open_carousel(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) {
    let entity = studio.clone();
    // Default to the first visible card when nothing is selected.
    studio.update(cx, |studio, cx| {
        if studio.gallery_selected.is_none() {
            if let Some(first) = studio.visible_examples().first() {
                studio.gallery_selected = Some(first.id.clone());
            }
        }
        cx.notify();
    });
    carousel_dialog(entity, window, cx);
}
