//! Examples gallery (GUI.md III.7 / W10) — every `examples/*.py` as a scene.
//!
//! The index (`studio/examples/index.json`) maps each engine example script to
//! the Studio scene that reproduces its headline result. Tiers are the honesty
//! contract: **A** runs today through registered ops and is locked by
//! `studio-engine/tests/examples_gallery.rs`, which renders every A entry and
//! fails if one stops working; **B/C** name the milestone that unlocks them and
//! are not clickable until then.
//!
//! Thumbnails are real renders: the test writes each A entry to
//! `studio/target/example-output/<id>.png` and the gallery shows that image when
//! it exists. A card without one draws a placeholder instead of inventing a
//! screenshot.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{Color32, Context, Rect, RichText, Sense, Stroke, Ui, Vec2};
use serde::Deserialize;

use crate::icons::{Icon, Icons};
use crate::ui::theme::Tokens;

const INDEX_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/index.json"
));

/// Where `examples_gallery.rs` writes the verified renders.
pub fn output_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/example-output")
        .canonicalize()
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/example-output")
        })
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExampleEntry {
    pub id: String,
    pub title: String,
    pub category: String,
    /// `A` runs today; `B`/`C` wait for `unlock`.
    pub tier: String,
    pub example: String,
    pub description: String,
    #[serde(default)]
    pub unlock: Option<String>,
    /// The scene Studio applies when the card is opened (absent for B/C).
    #[serde(default)]
    pub scene: Option<serde_json::Value>,
}

impl ExampleEntry {
    pub fn runs_today(&self) -> bool {
        self.tier == "A" && self.scene.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct Index {
    entries: Vec<ExampleEntry>,
}

/// Parse the bundled index once.
pub fn entries() -> &'static [ExampleEntry] {
    static INDEX: OnceLock<Vec<ExampleEntry>> = OnceLock::new();
    INDEX.get_or_init(|| match serde_json::from_str::<Index>(INDEX_JSON) {
        Ok(index) => index.entries,
        Err(err) => {
            log::error!("examples index is invalid: {err}");
            Vec::new()
        }
    })
}

/// Categories in index order, deduplicated.
pub fn categories() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in entries() {
        if !out.contains(&entry.category) {
            out.push(entry.category.clone());
        }
    }
    out
}

/// Category → glyph, so a card is recognisable before its thumbnail loads.
fn category_icon(category: &str) -> Icon {
    match category {
        "Terrain" => Icon::MountainSnow,
        "Cartography" => Icon::Table,
        "Buildings & cities" => Icon::Building,
        "Labels" => Icon::Type,
        "Atmosphere & time" => Icon::Sun,
        "Smoke & volumes" => Icon::Cloud,
        "Path tracing" => Icon::Image,
        "Globe & astro" => Icon::Orbit,
        "Point clouds" => Icon::Cloud,
        "Data prep" => Icon::Table,
        "Provenance" => Icon::CircleCheck,
        "Analysis" => Icon::Compass,
        _ => Icon::Layers,
    }
}

#[derive(Default)]
pub struct GalleryState {
    pub open: bool,
    category: Option<String>,
    thumbnails: HashMap<String, egui::TextureHandle>,
    failed: HashMap<String, bool>,
}

impl GalleryState {
    /// Load (once) the verified render for an entry, if the harness produced one.
    fn thumbnail(&mut self, ctx: &Context, entry: &ExampleEntry) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.thumbnails.get(&entry.id) {
            return Some(texture.clone());
        }
        if self.failed.contains_key(&entry.id) {
            return None;
        }
        let path = output_dir().join(format!("{}.png", entry.id));
        let image = match image::open(&path) {
            Ok(image) => image,
            Err(_) => {
                self.failed.insert(entry.id.clone(), true);
                return None;
            }
        };
        let rgba = image.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        let texture = ctx.load_texture(
            format!("example-thumb-{}", entry.id),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.thumbnails.insert(entry.id.clone(), texture.clone());
        Some(texture)
    }
}

/// Draw the gallery; returns the entry the user opened.
pub fn window(
    ctx: &Context,
    state: &mut GalleryState,
    icons: &Icons,
    tokens: &Tokens,
) -> Option<ExampleEntry> {
    if !state.open {
        return None;
    }
    let mut opened = None;
    let mut open = state.open;
    egui::Window::new("Examples")
        .id(egui::Id::new("studio_examples"))
        .open(&mut open)
        .default_size(Vec2::new(880.0, 620.0))
        .min_width(640.0)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut choose_all = false;
                if ui
                    .selectable_label(state.category.is_none(), "All")
                    .clicked()
                {
                    choose_all = true;
                }
                if choose_all {
                    state.category = None;
                }
                for category in categories() {
                    let selected = state.category.as_deref() == Some(category.as_str());
                    if ui.selectable_label(selected, &category).clicked() {
                        state.category = Some(category);
                    }
                }
            });
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "Tier A cards run their example's headline result through the engine. \
                     Tier B/C cards name the milestone that unlocks them — nothing pretends to work.",
                )
                .size(11.0)
                .weak(),
            );
            ui.separator();

            let visible: Vec<&ExampleEntry> = entries()
                .iter()
                .filter(|entry| {
                    state
                        .category
                        .as_deref()
                        .map(|category| entry.category == category)
                        .unwrap_or(true)
                })
                .collect();

            egui::ScrollArea::vertical()
                .id_source("examples_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let columns = ((ui.available_width() / 270.0).floor() as usize).max(1);
                    for chunk in visible.chunks(columns) {
                        ui.horizontal_top(|ui| {
                            for entry in chunk {
                                if let Some(entry) = card(ui, state, icons, tokens, entry) {
                                    opened = Some(entry);
                                }
                            }
                        });
                        ui.add_space(6.0);
                    }
                });
        });
    state.open = open;
    opened
}

fn card(
    ui: &mut Ui,
    state: &mut GalleryState,
    icons: &Icons,
    tokens: &Tokens,
    entry: &ExampleEntry,
) -> Option<ExampleEntry> {
    let mut opened = None;
    let width = 258.0;
    let thumb_height = 126.0;
    let runs = entry.runs_today();

    let response = ui.allocate_ui(Vec2::new(width, thumb_height + 74.0), |ui| {
        ui.vertical(|ui| {
            // ---- thumbnail ------------------------------------------------
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, thumb_height), Sense::hover());
            let thumbnail = state.thumbnail(ui.ctx(), entry);
            match thumbnail {
                Some(texture) => {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    // Placeholder: a subtle gradient plus the category glyph —
                    // never a fabricated screenshot.
                    let steps = 12;
                    for step in 0..steps {
                        let t = step as f32 / steps as f32;
                        let color = if runs {
                            Color32::from_rgb(
                                (22.0 + 10.0 * t) as u8,
                                (28.0 + 16.0 * t) as u8,
                                (38.0 + 22.0 * t) as u8,
                            )
                        } else {
                            Color32::from_rgb(26, 27, 30)
                        };
                        let band = Rect::from_min_max(
                            egui::pos2(rect.left(), rect.top() + rect.height() * t),
                            egui::pos2(
                                rect.right(),
                                rect.top() + rect.height() * (t + 1.0 / steps as f32),
                            ),
                        );
                        ui.painter().rect_filled(band, 0.0, color);
                    }
                    let texture = icons.texture(ui.ctx(), category_icon(&entry.category));
                    let glyph = Rect::from_center_size(rect.center(), Vec2::splat(30.0));
                    ui.painter().image(
                        texture.id(),
                        glyph,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        if runs {
                            Color32::from_white_alpha(150)
                        } else {
                            Color32::from_white_alpha(70)
                        },
                    );
                }
            }
            if runs {
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    Stroke::new(1.0_f32, tokens.accent.gamma_multiply(0.5)),
                );
            }

            // ---- title + tier --------------------------------------------
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&entry.title).strong().size(12.5));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = match entry.tier.as_str() {
                        "A" => ("runs", Color32::from_rgb(120, 200, 140)),
                        "B" => ("needs milestone", Color32::from_rgb(220, 180, 110)),
                        _ => ("external asset", Color32::from_rgb(170, 170, 175)),
                    };
                    ui.label(RichText::new(label).size(10.0).color(color));
                });
            });
            ui.label(
                RichText::new(format!("{} · {}", entry.category, entry.id))
                    .size(10.5)
                    .weak(),
            );
            let summary = if runs {
                entry.description.clone()
            } else {
                format!(
                    "{} Unlocks with {}.",
                    entry.description,
                    entry.unlock.as_deref().unwrap_or("a later milestone")
                )
            };
            ui.label(RichText::new(summary).size(10.5).weak());
        });
    });

    let hover = if runs {
        format!(
            "{}\n\n{}\nOpen: apply this scene through Studio",
            entry.example, entry.description
        )
    } else {
        format!(
            "{}\n\n{}\nNot runnable yet — unlocks with {}.",
            entry.example,
            entry.description,
            entry.unlock.as_deref().unwrap_or("a later milestone")
        )
    };
    let response = response.response.on_hover_text(hover);
    if runs {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.interact(Sense::click()).clicked() {
            opened = Some(entry.clone());
        }
    }
    // Card frame line, drawn last so it sits above the thumbnail.
    ui.painter().rect_stroke(
        egui::Rect::from_min_size(
            egui::pos2(response.rect.left() - 4.0, response.rect.top() - 4.0),
            response.rect.size() + Vec2::splat(8.0),
        ),
        5.0,
        Stroke::new(1.0_f32, Color32::from_white_alpha(14)),
    );
    opened
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn index_covers_every_example_script() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut missing = Vec::new();
        for entry in entries() {
            if !root.join(&entry.example).is_file() {
                missing.push(entry.example.clone());
            }
        }
        assert!(
            missing.is_empty(),
            "index names missing scripts: {missing:?}"
        );

        let mut ids = HashSet::new();
        for entry in entries() {
            assert!(ids.insert(entry.id.clone()), "duplicate id {}", entry.id);
        }
        assert!(entries().len() >= 50, "expected every example in the index");
    }

    #[test]
    fn tier_a_entries_carry_a_scene_and_b_c_name_their_unlock() {
        for entry in entries() {
            match entry.tier.as_str() {
                "A" => assert!(
                    entry.scene.is_some(),
                    "tier A entry {} has no scene",
                    entry.id
                ),
                "B" | "C" => assert!(
                    entry
                        .unlock
                        .as_deref()
                        .is_some_and(|unlock| !unlock.is_empty()),
                    "{} must name the milestone that unlocks it",
                    entry.id
                ),
                other => panic!("unknown tier {other:?} on {}", entry.id),
            }
        }
    }

    #[test]
    fn every_category_has_at_least_one_entry() {
        assert!(categories().len() >= 8);
    }

    #[test]
    fn gallery_renders_headlessly() {
        let ctx = egui::Context::default();
        let icons = Icons::load(&ctx, Color32::WHITE);
        let tokens = Tokens::dark();
        let mut state = GalleryState {
            open: true,
            ..GalleryState::default()
        };
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                assert!(window(ctx, &mut state, &icons, &tokens).is_none());
            });
        }
        assert!(state.open, "a frame must not close the gallery");
    }
}
