//! Layers panel (GUI.md III.3) — the scene document as rows.
//!
//! Rows are painted rather than assembled from widgets, because the default
//! egui checkbox/slider look is exactly what makes a panel read as a prototype.
//! A row is one interaction surface: click selects, double-click renames, drag
//! reorders, and the eye/lock glyphs are their own hit targets. Everything the
//! panel does leaves as a [`LayersCommand`]; `StudioApp` applies it to the
//! document with undo recording, so the panel holds no scene state itself.

use egui::{Align2, FontId, Rect, RichText, Sense, Ui, Vec2};
use studio_project::document::{Document, LayerId, LayerKind, Row};

use crate::icons::{Icon, Icons};
use crate::ui::theme::{self, Tokens};

/// Where a dragged layer would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSpot {
    /// Above the target row.
    Before(LayerId),
    /// Below the target row.
    After(LayerId),
    /// Inside the target group, at the top.
    Into(LayerId),
}

impl DropSpot {
    pub fn target(self) -> LayerId {
        match self {
            DropSpot::Before(id) | DropSpot::After(id) | DropSpot::Into(id) => id,
        }
    }
}

/// Transient panel state owned by the app (drag, inline rename).
#[derive(Default)]
pub struct LayersState {
    pub drag: Option<LayerId>,
    pub drop: Option<DropSpot>,
    pub renaming: Option<LayerId>,
    pub rename_buffer: String,
    /// Row layout from the last frame, for hit-testing drags.
    rows: Vec<(LayerId, Rect)>,
}

impl LayersState {
    pub fn begin_rename(&mut self, id: LayerId, current: &str) {
        self.renaming = Some(id);
        self.rename_buffer = current.to_owned();
    }

    pub fn cancel_rename(&mut self) {
        self.renaming = None;
        self.rename_buffer.clear();
    }
}

/// Everything the panel reads.
pub struct LayersView<'a> {
    pub document: &'a Document,
    pub selected: &'a [LayerId],
    pub primary: Option<LayerId>,
    pub icons: &'a Icons,
    pub tokens: &'a Tokens,
}

/// Everything the panel wants done.
#[derive(Debug, Clone, PartialEq)]
pub enum LayersCommand {
    Select {
        ids: Vec<LayerId>,
        additive: bool,
    },
    SetVisible(LayerId, bool),
    SetLocked(LayerId, bool),
    SetOpacity(LayerId, f32),
    /// U2: replace a layer's engine payload (per-kind inspector edits).
    SetStyle(LayerId, serde_json::Value),
    SetExpanded(LayerId, bool),
    BeginRename(LayerId),
    Rename(LayerId, String),
    Move {
        id: LayerId,
        spot: DropSpot,
    },
    Duplicate(LayerId),
    Delete(Vec<LayerId>),
    Group(Vec<LayerId>),
    Ungroup(LayerId),
    MoveUp(LayerId),
    MoveDown(LayerId),
    ZoomTo(LayerId),
    AddTerrain,
    ImportDataset,
}

/// Kind → glyph. One concept, one icon, everywhere in the UI.
pub fn layer_icon(kind: LayerKind) -> Icon {
    match kind {
        LayerKind::Terrain => Icon::MountainSnow,
        LayerKind::Reference => Icon::FileInput,
        LayerKind::RasterOverlay => Icon::Table,
        LayerKind::Vector => Icon::Spline,
        LayerKind::Buildings => Icon::Building,
        LayerKind::Labels => Icon::Type,
        LayerKind::PointCloud => Icon::Cloud,
        LayerKind::Mesh => Icon::Box,
        LayerKind::Furniture => Icon::Compass,
        LayerKind::Smoke => Icon::Wind,
        LayerKind::Group => Icon::Folder,
    }
}

/// Draw the panel and return the commands the user asked for.
pub fn panel(ui: &mut Ui, state: &mut LayersState, view: &LayersView<'_>) -> Vec<LayersCommand> {
    let mut commands = Vec::new();
    toolbar(ui, state, view, &mut commands);
    ui.add_space(4.0);
    opacity_control(ui, view, &mut commands);
    ui.add_space(4.0);
    ui.separator();

    let rows = view.document.rows();
    if rows.is_empty() {
        empty_state(ui, &mut commands);
        return commands;
    }

    let width = ui.available_width();
    let row_h = view.tokens.row_h + 4.0;
    state.rows.clear();
    let mut rename_request: Option<(LayerId, String)> = None;

    for row in &rows {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click_and_drag());
        state.rows.push((row.id, rect));

        // Drop indicator for the row currently under the pointer.
        if state.drag.is_some() {
            if let Some(spot) = state.drop {
                if spot.target() == row.id {
                    paint_drop(ui, rect, spot, view.tokens);
                }
            }
        }

        let Some(layer) = view.document.layer(row.id) else {
            continue;
        };
        let selected = view.selected.contains(&row.id);
        let primary = view.primary == Some(row.id);
        paint_row(ui, view, row, rect, selected, primary);

        // ---- interactions ------------------------------------------------
        if response.double_clicked() {
            commands.push(LayersCommand::BeginRename(row.id));
        } else if response.clicked() {
            let additive = ui.input(|input| input.modifiers.command || input.modifiers.shift);
            let ids = if ui.input(|input| input.modifiers.shift) {
                range_selection(&rows, view.primary, row.id)
            } else {
                vec![row.id]
            };
            commands.push(LayersCommand::Select { ids, additive });
        }

        if response.drag_started() {
            state.drag = Some(row.id);
        }
        if state.drag == Some(row.id) && response.dragged() {
            if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                state.drop = drop_spot(&state.rows, pointer, view, row_h);
            }
        }
        if response.drag_stopped() {
            if let (Some(id), Some(spot)) = (state.drag.take(), state.drop.take()) {
                if spot.target() != id {
                    commands.push(LayersCommand::Move { id, spot });
                }
            }
        }

        let hover = format!(
            "{}\n{} layer{}",
            layer.source.clone().unwrap_or_else(|| layer.name.clone()),
            layer.kind.label(),
            if layer.kind.renders() {
                ""
            } else {
                " — registered only; its renderer comes in a later milestone"
            }
        );
        let response = response.on_hover_text(hover);

        // Disclosure triangle for groups (own hit target, left of the icon).
        if layer.kind.is_group() && !view.document.children(Some(row.id)).is_empty() {
            let centre = egui::pos2(
                rect.left() + 6.0 + row.depth as f32 * 12.0 + 5.0,
                rect.center().y,
            );
            let hit = Rect::from_center_size(centre, Vec2::splat(row_h - 8.0));
            let toggle = ui.interact(
                hit,
                egui::Id::new(("layer_expand", row.id.0)),
                Sense::click(),
            );
            if toggle.clicked() {
                commands.push(LayersCommand::SetExpanded(row.id, !layer.expanded));
            }
        }

        // Eye / lock hit targets (painted above the row, so they win the click).
        let eye_rect = Rect::from_center_size(
            egui::pos2(rect.right() - 18.0, rect.center().y),
            Vec2::splat(row_h - 6.0),
        );
        let eye = ui.interact(
            eye_rect,
            egui::Id::new(("layer_eye", row.id.0)),
            Sense::click(),
        );
        if eye.clicked() {
            commands.push(LayersCommand::SetVisible(row.id, !layer.visible));
        }
        if !layer.kind.is_group() {
            let lock_rect = Rect::from_center_size(
                egui::pos2(rect.right() - 38.0, rect.center().y),
                Vec2::splat(row_h - 6.0),
            );
            let lock = ui.interact(
                lock_rect,
                egui::Id::new(("layer_lock", row.id.0)),
                Sense::click(),
            );
            if lock.clicked() {
                commands.push(LayersCommand::SetLocked(row.id, !layer.locked));
            }
        }

        // Context menu.
        response.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                commands.push(LayersCommand::BeginRename(row.id));
                ui.close_menu();
            }
            if ui.button("Duplicate").clicked() {
                commands.push(LayersCommand::Duplicate(row.id));
                ui.close_menu();
            }
            if ui.button("Delete").clicked() {
                commands.push(LayersCommand::Delete(view.selected.to_vec()));
                ui.close_menu();
            }
            ui.separator();
            if layer.kind.is_group() {
                if ui.button("Ungroup").clicked() {
                    commands.push(LayersCommand::Ungroup(row.id));
                    ui.close_menu();
                }
            } else if ui.button("Group selection").clicked() {
                commands.push(LayersCommand::Group(view.selected.to_vec()));
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Move up").clicked() {
                commands.push(LayersCommand::MoveUp(row.id));
                ui.close_menu();
            }
            if ui.button("Move down").clicked() {
                commands.push(LayersCommand::MoveDown(row.id));
                ui.close_menu();
            }
            if ui.button("Zoom to layer").clicked() {
                commands.push(LayersCommand::ZoomTo(row.id));
                ui.close_menu();
            }
        });

        // Inline rename, drawn last so it sits above the row's own painting.
        if state.renaming == Some(row.id) {
            let edit_rect = Rect::from_min_max(
                egui::pos2(
                    rect.left() + 22.0 + row.depth as f32 * 12.0,
                    rect.top() + 2.0,
                ),
                egui::pos2(rect.right() - 48.0, rect.bottom() - 2.0),
            );
            let response = ui.put(
                edit_rect,
                egui::TextEdit::singleline(&mut state.rename_buffer)
                    .id(egui::Id::new(("layer_rename", row.id.0)))
                    .font(egui::TextStyle::Body),
            );
            response.request_focus();
            let commit =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
            if commit || (response.lost_focus() && !state.rename_buffer.is_empty()) {
                rename_request = Some((row.id, state.rename_buffer.clone()));
            } else if cancel {
                state.cancel_rename();
            }
        }
    }

    if let Some((id, name)) = rename_request {
        let trimmed = name.trim().to_owned();
        if !trimmed.is_empty() {
            commands.push(LayersCommand::Rename(id, trimmed));
        }
        state.cancel_rename();
    }

    ui.add_space(2.0);
    footer(ui, view);
    commands
}

fn toolbar(
    ui: &mut Ui,
    _state: &mut LayersState,
    view: &LayersView<'_>,
    commands: &mut Vec<LayersCommand>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icon_button(ui, view.icons, view.tokens, Icon::Plus, "Add layer")
            .on_hover_text("Add a layer")
            .clicked()
        {
            // The only layer kind that renders today: the terrain. Other kinds
            // appear when their renderer lands (their menu entries are absent,
            // not disabled).
            commands.push(LayersCommand::AddTerrain);
        }
        let has_selection = !view.selected.is_empty();
        if ui
            .add_enabled(
                has_selection,
                egui::Button::image_and_text(
                    egui::Image::new(&view.icons.texture(ui.ctx(), Icon::FolderPlus))
                        .fit_to_exact_size(Vec2::splat(14.0)),
                    RichText::new("Group").size(11.5),
                )
                .frame(false),
            )
            .on_hover_text("Group the selected layers (Ctrl+G)")
            .clicked()
        {
            commands.push(LayersCommand::Group(view.selected.to_vec()));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    has_selection,
                    egui::Button::image(
                        egui::Image::new(&view.icons.texture(ui.ctx(), Icon::Trash))
                            .fit_to_exact_size(Vec2::splat(14.0)),
                    )
                    .frame(false),
                )
                .on_hover_text("Delete the selected layers")
                .clicked()
            {
                commands.push(LayersCommand::Delete(view.selected.to_vec()));
            }
            if ui
                .add_enabled(
                    has_selection,
                    egui::Button::image(
                        egui::Image::new(&view.icons.texture(ui.ctx(), Icon::Copy))
                            .fit_to_exact_size(Vec2::splat(14.0)),
                    )
                    .frame(false),
                )
                .on_hover_text("Duplicate the selected layers")
                .clicked()
            {
                if let Some(id) = view.primary {
                    commands.push(LayersCommand::Duplicate(id));
                }
            }
        });
    });
}

fn icon_button(
    ui: &mut Ui,
    icons: &Icons,
    tokens: &Tokens,
    icon: Icon,
    tooltip: &str,
) -> egui::Response {
    ui.add(
        egui::Button::image(
            egui::Image::new(&icons.texture(ui.ctx(), icon))
                .fit_to_exact_size(Vec2::splat(tokens.icon)),
        )
        .frame(false),
    )
    .on_hover_text(tooltip)
}

fn opacity_control(ui: &mut Ui, view: &LayersView<'_>, commands: &mut Vec<LayersCommand>) {
    let Some(id) = view.primary else {
        return;
    };
    let Some(layer) = view.document.layer(id) else {
        return;
    };
    if layer.kind.is_group() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(theme::meta("Opacity"));
        let mut opacity = layer.opacity;
        let response = ui.add(
            egui::DragValue::new(&mut opacity)
                .speed(0.01)
                .clamp_range(0.0..=1.0)
                .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
                .custom_parser(|text| {
                    text.trim()
                        .trim_end_matches('%')
                        .parse::<f64>()
                        .ok()
                        .map(|value| value / 100.0)
                }),
        );
        if response.changed() {
            commands.push(LayersCommand::SetOpacity(id, opacity.clamp(0.0, 1.0)));
        }
        ui.add(
            egui::Slider::new(&mut opacity, 0.0..=1.0)
                .show_value(false)
                .trailing_fill(true),
        )
        .changed()
        .then(|| commands.push(LayersCommand::SetOpacity(id, opacity)));
    });
}

fn empty_state(ui: &mut Ui, commands: &mut Vec<LayersCommand>) {
    ui.add_space(6.0);
    ui.weak("No layers yet.");
    if ui.button("Add terrain layer…").clicked() {
        commands.push(LayersCommand::AddTerrain);
    }
    if ui.button("Import dataset…").clicked() {
        commands.push(LayersCommand::ImportDataset);
    }
}

fn footer(ui: &mut Ui, view: &LayersView<'_>) {
    let total = view.document.len();
    let hidden = view
        .document
        .layers
        .iter()
        .filter(|layer| !view.document.effective_visible(layer.id))
        .count();
    let mut line = format!("{total} layer{}", if total == 1 { "" } else { "s" });
    if hidden > 0 {
        line.push_str(&format!(" · {hidden} hidden"));
    }
    if !view.selected.is_empty() {
        line.push_str(&format!(" · {} selected", view.selected.len()));
    }
    ui.label(theme::meta(line));
    let registered_only = view
        .document
        .layers
        .iter()
        .filter(|layer| !layer.kind.renders() && !layer.kind.is_group())
        .count();
    if registered_only > 0 {
        ui.weak(format!(
            "{registered_only} registered layer(s) have no renderer in Studio yet"
        ));
    }
}

fn paint_row(ui: &Ui, view: &LayersView<'_>, row: &Row, rect: Rect, selected: bool, primary: bool) {
    let Some(layer) = view.document.layer(row.id) else {
        return;
    };
    let painter = ui.painter();
    let visuals = ui.visuals();
    let hovered = ui.rect_contains_pointer(rect);

    // Background: selection > hover > zebra-free.
    if selected {
        painter.rect_filled(rect, 3.0, visuals.selection.bg_fill);
    } else if hovered {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    if primary && selected {
        let bar = Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + 2.0),
            egui::pos2(rect.left() + 2.5, rect.bottom() - 2.0),
        );
        painter.rect_filled(bar, 1.0, visuals.selection.stroke.color);
    }

    let indent = 6.0 + row.depth as f32 * 12.0;
    let mut x = rect.left() + indent;

    // Disclosure triangle for groups with children.
    if layer.kind.is_group() && !view.document.children(Some(row.id)).is_empty() {
        let centre = egui::pos2(x + 5.0, rect.center().y);
        let points = if layer.expanded {
            vec![
                egui::pos2(centre.x - 4.0, centre.y - 2.0),
                egui::pos2(centre.x + 4.0, centre.y - 2.0),
                egui::pos2(centre.x, centre.y + 3.0),
            ]
        } else {
            vec![
                egui::pos2(centre.x - 2.0, centre.y - 4.0),
                egui::pos2(centre.x + 3.0, centre.y),
                egui::pos2(centre.x - 2.0, centre.y + 4.0),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            points,
            visuals.weak_text_color(),
            egui::Stroke::NONE,
        ));
    }
    x += 12.0;

    // Kind glyph.
    let icon_size = 14.0;
    let icon_rect = Rect::from_min_size(
        egui::pos2(x, rect.center().y - icon_size / 2.0),
        Vec2::splat(icon_size),
    );
    let tint = if !view.document.effective_visible(row.id) {
        visuals.weak_text_color()
    } else if layer.kind.renders() {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let texture = view.icons.texture(ui.ctx(), layer_icon(layer.kind));
    painter.image(
        texture.id(),
        icon_rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        tint,
    );
    x += icon_size + 6.0;

    // Name (clipped so it never runs under the eye/lock glyphs).
    let name_rect = Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2(rect.right() - 52.0, rect.bottom()),
    );
    let name_color = if layer.visible {
        if selected {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        }
    } else {
        visuals.weak_text_color()
    };
    let font = FontId::proportional(12.5);
    let mut name = layer.name.clone();
    if layer.locked {
        name.push_str("  (locked)");
    }
    if (layer.opacity - 1.0).abs() > 0.01 {
        name.push_str(&format!("  {}%", (layer.opacity * 100.0).round() as i32));
    }
    ui.painter().with_clip_rect(name_rect).text(
        egui::pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        font,
        name_color,
    );

    // Eye + lock.
    let eye_icon = if view.document.effective_visible(row.id) {
        Icon::Eye
    } else {
        Icon::EyeOff
    };
    let eye_tint = if view.document.effective_visible(row.id) {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let eye_rect = Rect::from_center_size(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        Vec2::splat(13.0),
    );
    let eye_texture = view.icons.texture(ui.ctx(), eye_icon);
    painter.image(
        eye_texture.id(),
        eye_rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        eye_tint,
    );
    if layer.locked {
        let lock_rect = Rect::from_center_size(
            egui::pos2(rect.right() - 38.0, rect.center().y),
            Vec2::splat(12.0),
        );
        let lock_texture = view.icons.texture(ui.ctx(), Icon::Lock);
        painter.image(
            lock_texture.id(),
            lock_rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            visuals.weak_text_color(),
        );
    }
}

fn paint_drop(ui: &Ui, rect: Rect, spot: DropSpot, tokens: &Tokens) {
    let color = ui.visuals().selection.stroke.color;
    match spot {
        DropSpot::Before(_) => {
            ui.painter().rect_filled(
                Rect::from_min_max(rect.left_top(), egui::pos2(rect.right(), rect.top() + 2.0)),
                0.0,
                color,
            );
        }
        DropSpot::After(_) => {
            ui.painter().rect_filled(
                Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - 2.0),
                    rect.right_bottom(),
                ),
                0.0,
                color,
            );
        }
        DropSpot::Into(_) => {
            ui.painter()
                .rect_stroke(rect.shrink(1.0), 3.0, egui::Stroke::new(1.5_f32, color));
            let _ = tokens;
        }
    }
}

/// Which spot the pointer is over: the top/bottom eighth of a row is a
/// before/after drop, the middle of a group row is "into".
fn drop_spot(
    rows: &[(LayerId, Rect)],
    pointer: egui::Pos2,
    view: &LayersView<'_>,
    row_h: f32,
) -> Option<DropSpot> {
    let (id, rect) = rows
        .iter()
        .find(|(_, rect)| rect.contains(pointer))
        .copied()?;
    let group = view
        .document
        .layer(id)
        .map(|layer| layer.kind.is_group())
        .unwrap_or(false);
    let top = pointer.y - rect.top();
    if group && top > row_h * 0.3 && top < row_h * 0.7 {
        return Some(DropSpot::Into(id));
    }
    if top < rect.height() / 2.0 {
        Some(DropSpot::Before(id))
    } else {
        Some(DropSpot::After(id))
    }
}

fn range_selection(rows: &[Row], primary: Option<LayerId>, clicked: LayerId) -> Vec<LayerId> {
    let Some(primary) = primary else {
        return vec![clicked];
    };
    let from = rows.iter().position(|row| row.id == primary);
    let to = rows.iter().position(|row| row.id == clicked);
    match (from, to) {
        (Some(from), Some(to)) => {
            let (start, end) = if from <= to { (from, to) } else { (to, from) };
            rows[start..=end].iter().map(|row| row.id).collect()
        }
        _ => vec![clicked],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;
    use studio_project::document::{Document, LayerKind};

    fn view_document() -> Document {
        let mut document = Document::new();
        document.push(LayerKind::Reference, "alpha", None);
        document.push(LayerKind::Reference, "beta", None);
        document.push(LayerKind::Reference, "gamma", None);
        document
    }

    #[test]
    fn panel_renders_a_document_headlessly() {
        let ctx = egui::Context::default();
        let icons = Icons::load(&ctx, Color32::WHITE);
        let tokens = Tokens::dark();
        let document = view_document();
        let selected = vec![document.layers[1].id];
        let primary = Some(document.layers[1].id);
        let mut state = LayersState::default();
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let commands = panel(
                        ui,
                        &mut state,
                        &LayersView {
                            document: &document,
                            selected: &selected,
                            primary,
                            icons: &icons,
                            tokens: &tokens,
                        },
                    );
                    assert!(commands.is_empty(), "laying out must not invent edits");
                });
            });
        }
    }

    #[test]
    fn panel_handles_a_hundred_layers_and_groups() {
        let ctx = egui::Context::default();
        let icons = Icons::load(&ctx, Color32::WHITE);
        let tokens = Tokens::dark();
        let mut document = Document::new();
        let mut ids = Vec::new();
        for index in 0..100 {
            ids.push(document.push(LayerKind::Reference, format!("layer {index}"), None));
        }
        document.group(&ids[10..20], "group").expect("group");
        let mut state = LayersState::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let commands = panel(
                        ui,
                        &mut state,
                        &LayersView {
                            document: &document,
                            selected: &[],
                            primary: None,
                            icons: &icons,
                            tokens: &tokens,
                        },
                    );
                    assert!(commands.is_empty());
                });
            });
        });
        assert!(document.rows().len() >= 100);
    }

    #[test]
    fn range_selection_spans_the_visible_rows() {
        let document = view_document();
        let rows = document.rows();
        let span = range_selection(&rows, Some(rows[2].id), rows[0].id);
        assert_eq!(span.len(), 3, "backwards ranges work too");
        let span = range_selection(&rows, None, rows[1].id);
        assert_eq!(span, vec![rows[1].id]);
    }

    #[test]
    fn every_kind_has_its_own_glyph() {
        let mut seen = std::collections::HashSet::new();
        for kind in LayerKind::ALL {
            assert!(
                seen.insert(layer_icon(*kind)),
                "{kind:?} shares a glyph with another kind"
            );
        }
    }
}
