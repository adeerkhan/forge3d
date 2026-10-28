//! The Layers panel (GUI.md G5): the document made visible.
//!
//! The tree is the document. Rows are drawn (not stacked widgets) so the panel
//! can show a disclosure arrow, an icon, the name, opacity and the eye/lock hit
//! targets in one 24 px line, with drag reordering, inline rename and a context
//! menu on top.
//!
//! Only the terrain layer and groups render today; every other kind says so in
//! its row instead of pretending to draw.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use studio_project::document::{LayerId, LayerKind};

use crate::state::Studio;

/// Where a dragged row would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Before,
    Into,
    After,
}

/// What a row carries while it is being dragged.
#[derive(Clone, Debug)]
pub struct LayerDrag {
    pub id: LayerId,
}

/// The ghost that follows the pointer during a layer drag.
pub struct LayerDragGhost {
    pub name: String,
    pub icon: IconName,
}

impl Render for LayerDragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().colors.popover)
            .border_1()
            .border_color(cx.theme().colors.border)
            .text_xs()
            .child(self.icon)
            .child(self.name.clone())
    }
}

/// Panel-local UI state: selection anchor, the row being renamed, the drop
/// preview. Never a copy of the document.
#[derive(Default)]
pub struct LayerPanel {
    pub anchor: Option<LayerId>,
    /// The row being dragged, and the undo snapshot taken at drag start.
    pub dragging: Option<LayerId>,
    pub drag_before: Option<studio_project::Document>,
    pub renaming: Option<LayerId>,
    pub rename_input: Option<Entity<InputState>>,
    pub drop_target: Option<(LayerId, DropZone)>,
    pub collapsed: std::collections::HashSet<LayerId>,
    /// Kept alive while the rename box is open.
    pub rename_subscription: Option<Subscription>,
}

pub fn kind_icon(kind: LayerKind) -> IconName {
    match kind {
        LayerKind::Terrain => IconName::Mountain,
        LayerKind::Reference => IconName::File,
        LayerKind::RasterOverlay => IconName::Image,
        LayerKind::Vector => IconName::Spline,
        LayerKind::Buildings => IconName::Building,
        LayerKind::Labels => IconName::Tag,
        LayerKind::PointCloud => IconName::Cloud,
        LayerKind::Mesh => IconName::Box,
        LayerKind::Furniture => IconName::Map,
        LayerKind::Smoke => IconName::Cloudy,
        LayerKind::Group => IconName::Folder,
    }
}

/// The panel body.
pub fn layers(studio: &Entity<Studio>, window: &mut Window, cx: &mut App) -> AnyElement {
    let (rows, selection, anchor, renaming, drop_target, collapsed, layer_count, opacity_of) = {
        let state = studio.read(cx);
        let rows = state.visible_rows();
        let selection = state.selection.clone();
        let layer_count = state.project.document.len();
        let mut names = HashMap::new();
        for row in &rows {
            if let Some(layer) = state.project.document.layer(row.id) {
                names.insert(row.id, layer.opacity);
            }
        }
        (
            rows,
            selection,
            state.layers.anchor,
            state.layers.renaming,
            state.layers.drop_target,
            state.layers.collapsed.clone(),
            layer_count,
            names,
        )
    };

    let mut list = v_flex().w_full();
    if rows.is_empty() {
        list = list.child(
            div()
                .p_3()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child("No layers yet. File ▸ Import DEM… starts a scene."),
        );
    }

    for row in rows {
        if collapsed
            .iter()
            .any(|id| *id != row.id && is_ancestor(studio, row.id, *id, cx))
        {
            continue;
        }
        let Some(layer) = studio.read(cx).project.document.layer(row.id).cloned() else {
            continue;
        };
        let selected = selection.contains(&row.id);
        let effective = studio.read(cx).project.document.effective_visible(row.id);
        let drop_zone = drop_target
            .filter(|(target, _)| *target == row.id)
            .map(|(_, zone)| zone);
        let is_renaming = renaming == Some(row.id);
        if is_renaming && studio.read(cx).layers.rename_input.is_none() {
            let name = layer.name.clone();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Layer name")
                    .default_value(name)
            });
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            let id = row.id;
            studio.update(cx, |studio, cx| studio.attach_rename_input(id, input, cx));
        }
        let is_group = layer.kind.is_group();
        let has_children = !studio
            .read(cx)
            .project
            .document
            .children(Some(row.id))
            .is_empty();
        let expanded = !collapsed.contains(&row.id);

        let row_el = h_flex()
            .id(format!("layer-row-{}", row.id.0))
            .relative()
            .h(px(24.))
            .w_full()
            .gap_2()
            .pl(px(6.0 + 12.0 * row.depth as f32))
            .pr_2()
            .bg(if selected {
                cx.theme().colors.selection
            } else {
                cx.theme().colors.background
            })
            .border_b_1()
            .border_color(if matches!(drop_zone, Some(DropZone::After)) {
                cx.theme().colors.primary
            } else {
                cx.theme().colors.background
            })
            .when(matches!(drop_zone, Some(DropZone::Before)), |row| {
                row.border_t_1().border_color(cx.theme().colors.primary)
            })
            .when(matches!(drop_zone, Some(DropZone::Into)), |row| {
                row.border_1().border_color(cx.theme().colors.primary)
            })
            .cursor_pointer();

        // Disclosure arrow for groups.
        let row_el = if has_children {
            let studio_toggle = studio.clone();
            let id = row.id;
            row_el.child(
                div()
                    .id(format!("layer-disclosure-{}", id.0))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let _ = studio_toggle
                            .update(cx, |studio, cx| studio.toggle_layer_collapsed(id, cx));
                    })
                    .child(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    }),
            )
        } else {
            row_el.child(div().w(px(12.)))
        };

        // Name, or the inline rename input.
        let name_el: AnyElement = if is_renaming {
            let input = studio.read(cx).layers.rename_input.clone();
            match input {
                Some(input) => Input::new(&input).small().w(px(150.)).into_any_element(),
                None => div().text_xs().child(layer.name.clone()).into_any_element(),
            }
        } else {
            div()
                .flex_1()
                .text_xs()
                .text_ellipsis()
                .text_color(if effective {
                    cx.theme().colors.foreground
                } else {
                    cx.theme().colors.muted_foreground
                })
                .when(selected, |name| name.font_semibold())
                .child(layer.name.clone())
                .into_any_element()
        };

        let studio_click = studio.clone();
        let studio_lock = studio.clone();
        let studio_eye = studio.clone();
        let studio_menu = studio.clone();
        let studio_drag_move = studio.clone();
        let studio_drag_start = studio.clone();
        let id = row.id;
        let layer_name = layer.name.clone();
        let layer_icon = kind_icon(layer.kind);
        let drag_name = layer_name.clone();

        let row_el = row_el
            .on_click(move |event, _, cx| {
                let modifiers = event.modifiers();
                let range = modifiers.shift;
                let additive = modifiers.control || modifiers.platform;
                let double = event.click_count() >= 2;
                let _ = studio_click.update(cx, |studio, cx| {
                    if double {
                        studio.begin_rename(id, cx);
                    } else {
                        studio.select_layer_row(id, range, additive, cx);
                    }
                });
            })
            .on_drag(LayerDrag { id }, move |drag, _, _, cx| {
                let ghost = LayerDragGhost {
                    name: drag_name.clone(),
                    icon: layer_icon,
                };
                let _ = studio_drag_start.update(cx, |studio, cx| studio.begin_layer_drag(id, cx));
                cx.new(|_| ghost)
            })
            // Reordering previews live while the pointer is over a target: the
            // document moves immediately, and the first move records the undo
            // step, so one drag is one undo.
            .on_drag_move(move |event: &DragMoveEvent<LayerDrag>, _, cx| {
                let zone = drop_zone_for(event);
                let _ = studio_drag_move.update(cx, |studio, cx| studio.preview_drop(id, zone, cx));
            })
            .context_menu(move |menu, _, _| {
                let mut menu = menu
                    .item(PopupMenuItem::new("Rename").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ = studio.update(cx, |studio, cx| studio.begin_rename(id, cx));
                        }
                    }))
                    .separator()
                    .item(PopupMenuItem::new("Duplicate").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ = studio.update(cx, |studio, cx| studio.duplicate_layer(cx));
                        }
                    }))
                    .item(PopupMenuItem::new("Delete").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ = studio.update(cx, |studio, cx| studio.delete_layer(cx));
                        }
                    }))
                    .separator()
                    .item(PopupMenuItem::new("Move up").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ = studio.update(cx, |studio, cx| studio.move_layer(-1, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("Move down").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ = studio.update(cx, |studio, cx| studio.move_layer(1, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("To top").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ =
                                studio.update(cx, |studio, cx| studio.move_layer_to_edge(true, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("To bottom").on_click({
                        let studio = studio_menu.clone();
                        move |_, _, cx| {
                            let _ = studio
                                .update(cx, |studio, cx| studio.move_layer_to_edge(false, cx));
                        }
                    }));
                if is_group && has_children {
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("Ungroup").on_click({
                            let studio = studio_menu.clone();
                            move |_, _, cx| {
                                let _ =
                                    studio.update(cx, |studio, cx| studio.ungroup_selection(cx));
                            }
                        }));
                }
                menu
            })
            .child(IconName::from(layer_icon))
            .child(name_el)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .when(
                        opacity_of.get(&id).copied().unwrap_or(1.0) < 0.999,
                        |value| {
                            value.child(format!(
                                "{:.0}%",
                                opacity_of.get(&id).copied().unwrap_or(1.0) * 100.0
                            ))
                        },
                    ),
            )
            .child(
                Button::new(format!("layer-lock-{}", id.0))
                    .icon(if layer.locked {
                        IconName::Lock
                    } else {
                        IconName::LockOpen
                    })
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        let _ =
                            studio_lock.update(cx, |studio, cx| studio.toggle_layer_lock(id, cx));
                    }),
            )
            .child(
                Button::new(format!("layer-eye-{}", id.0))
                    .icon(if layer.visible {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        let _ =
                            studio_eye.update(cx, |studio, cx| studio.toggle_layer_visible(id, cx));
                    }),
            );

        list = list.child(row_el);
    }

    // Selection footer: opacity for the selection, and layer count.
    let opacity = selection
        .last()
        .and_then(|id| opacity_of.get(id).copied())
        .unwrap_or(1.0);
    let studio_dec = studio.clone();
    let studio_inc = studio.clone();
    let studio_group = studio.clone();
    let studio_ungroup = studio.clone();

    v_flex()
        .w_full()
        .child(
            h_flex()
                .justify_between()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(cx.theme().colors.border)
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(format!("{layer_count} layer(s)")),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("layers-group")
                                .icon(IconName::FolderPlus)
                                .xsmall()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    let _ = studio_group
                                        .update(cx, |studio, cx| studio.group_selection(cx));
                                }),
                        )
                        .child(
                            Button::new("layers-ungroup")
                                .icon(IconName::FolderMinus)
                                .xsmall()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    let _ = studio_ungroup
                                        .update(cx, |studio, cx| studio.ungroup_selection(cx));
                                }),
                        ),
                ),
        )
        .child(
            div()
                .id("layers-list")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        )
        .child(
            h_flex()
                .justify_between()
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(cx.theme().colors.border)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(if selection.is_empty() {
                            "No selection".to_owned()
                        } else {
                            format!("{} selected", selection.len())
                        }),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .when(!selection.is_empty(), |row| {
                            row.child(
                                Button::new("opacity-dec")
                                    .label("−")
                                    .xsmall()
                                    .ghost()
                                    .on_click(move |_, _, cx| {
                                        let _ = studio_dec.update(cx, |studio, cx| {
                                            studio.step_selection_opacity(-0.1, cx)
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(44.))
                                    .text_xs()
                                    .text_center()
                                    .child(format!("Opacity {:.0}%", opacity * 100.0)),
                            )
                            .child(
                                Button::new("opacity-inc")
                                    .label("+")
                                    .xsmall()
                                    .ghost()
                                    .on_click(move |_, _, cx| {
                                        let _ = studio_inc.update(cx, |studio, cx| {
                                            studio.step_selection_opacity(0.1, cx)
                                        });
                                    }),
                            )
                        })
                        .when(anchor.is_some(), |row| row.child("")),
                ),
        )
        .into_any_element()
}

fn is_ancestor(studio: &Entity<Studio>, id: LayerId, maybe_parent: LayerId, cx: &App) -> bool {
    studio
        .read(cx)
        .project
        .document
        .ancestors(id)
        .contains(&maybe_parent)
}

fn drop_zone_for(event: &DragMoveEvent<LayerDrag>) -> DropZone {
    let top = event.bounds.origin.y;
    let height = event.bounds.size.height;
    let y = event.event.position.y;
    let relative = (y - top) / height.max(px(1.));
    if relative < 0.3 {
        DropZone::Before
    } else if relative > 0.7 {
        DropZone::After
    } else {
        DropZone::Into
    }
}
