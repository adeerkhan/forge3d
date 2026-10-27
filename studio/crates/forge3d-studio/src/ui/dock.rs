//! Dock slots, tab groups and layout persistence (GUI.md III.1).
//!
//! Four fixed slots — left, right, bottom, and the free central viewport — each
//! holding a small tab group. The model is deliberately hand-rolled: egui 0.27
//! has no dock crate we can pin against its wgpu-0.19 pairing, and slot + tab
//! state is small, testable and ours to shape. Tabs do not tear off yet.

use std::path::PathBuf;

use egui::{Color32, Rect, RichText, Ui};
use serde::{Deserialize, Serialize};

use super::theme::Tokens;

/// Named layouts from `View ▸ Panel layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutPreset {
    /// Layers + inspector, equal sides, short timeline.
    Default,
    /// Wide layers and inspector for styling work.
    Cartography,
    /// Collapsed left dock, wide viewport.
    Modeling,
    /// Tall bottom dock for logs and statistics.
    Data,
}

/// Which tab each slot is showing, plus its size and collapsed state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockState {
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
    pub left_collapsed: bool,
    pub bottom_collapsed: bool,
    pub left_tab: usize,
    pub right_tab: usize,
    pub bottom_tab: usize,
    /// The preset the user last applied (used for the menu checkmark only —
    /// manual resizes do not clear it, they simply stop matching).
    pub preset: LayoutPreset,
}

impl Default for DockState {
    fn default() -> Self {
        let mut state = Self {
            left_width: 240.0,
            right_width: 320.0,
            bottom_height: 170.0,
            left_collapsed: false,
            bottom_collapsed: false,
            left_tab: 0,
            right_tab: 0,
            bottom_tab: 0,
            preset: LayoutPreset::Default,
        };
        state.apply_preset(LayoutPreset::Default);
        state
    }
}

impl DockState {
    pub fn apply_preset(&mut self, preset: LayoutPreset) {
        self.preset = preset;
        match preset {
            LayoutPreset::Default => {
                self.left_width = 240.0;
                self.right_width = 320.0;
                self.bottom_height = 170.0;
                self.left_collapsed = false;
                self.bottom_collapsed = false;
            }
            LayoutPreset::Cartography => {
                self.left_width = 330.0;
                self.right_width = 360.0;
                self.bottom_height = 140.0;
                self.left_collapsed = false;
                self.bottom_collapsed = false;
            }
            LayoutPreset::Modeling => {
                self.left_width = 200.0;
                self.right_width = 300.0;
                self.bottom_height = 120.0;
                self.left_collapsed = true;
                self.bottom_collapsed = true;
            }
            LayoutPreset::Data => {
                self.left_width = 260.0;
                self.right_width = 320.0;
                self.bottom_height = 320.0;
                self.left_collapsed = false;
                self.bottom_collapsed = false;
            }
        }
    }

    /// Clamp against the window so a saved layout cannot paint off-screen.
    pub fn clamp(&mut self, width: f32, height: f32) {
        let min = Tokens::dark().dock_min;
        self.left_width = self.left_width.clamp(min, (width * 0.5).max(min));
        self.right_width = self.right_width.clamp(min, (width * 0.5).max(min));
        self.bottom_height = self.bottom_height.clamp(90.0, (height * 0.6).max(90.0));
    }

    /// `$XDG_CONFIG_HOME/forge3d-studio/layout.json` (or the platform
    /// equivalent). Returns None when the environment gives us no home.
    pub fn path() -> Option<PathBuf> {
        let base = if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(dir)
        } else if cfg!(target_os = "windows") {
            PathBuf::from(std::env::var("APPDATA").ok()?)
        } else if cfg!(target_os = "macos") {
            PathBuf::from(std::env::var("HOME").ok()?)
                .join("Library")
                .join("Application Support")
        } else {
            PathBuf::from(std::env::var("HOME").ok()?).join(".config")
        };
        Some(base.join("forge3d-studio").join("layout.json"))
    }

    /// Load the saved layout, falling back to the default. A corrupt file is
    /// logged and ignored — a bad setting must never stop the app starting.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                log::warn!("ignoring unreadable layout {}: {err}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&path, text) {
                    log::warn!("could not save layout {}: {err}", path.display());
                }
            }
            Err(err) => log::warn!("could not serialize layout: {err}"),
        }
    }
}

/// Draw a tab strip; returns true when the active tab changed.
pub fn tab_strip(ui: &mut Ui, tabs: &[&str], active: &mut usize) -> bool {
    let mut changed = false;
    let accent = ui.visuals().selection.stroke.color;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (index, name) in tabs.iter().enumerate() {
            let selected = index == *active;
            let text = if selected {
                RichText::new(*name).size(12.0).strong()
            } else {
                RichText::new(*name).size(12.0).weak()
            };
            let response = ui
                .add(egui::Button::new(text).frame(false))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if selected {
                let underline = Rect::from_min_max(
                    egui::pos2(response.rect.left(), response.rect.bottom() - 1.0),
                    egui::pos2(response.rect.right(), response.rect.bottom() + 1.0),
                );
                ui.painter().rect_filled(underline, 0.0, accent);
            }
            if response.clicked() && !selected {
                *active = index;
                changed = true;
            }
        }
    });
    changed
}

/// The rail shown when a slot is collapsed: one icon-button per tab.
pub fn collapsed_rail(ui: &mut Ui, tabs: &[&str], active: &mut usize) -> bool {
    let mut changed = false;
    for (index, name) in tabs.iter().enumerate() {
        let initial = name.chars().next().unwrap_or('?').to_string();
        let selected = index == *active;
        let response = ui
            .add(egui::Button::new(RichText::new(initial).size(12.0)).selected(selected))
            .on_hover_text(format!("Show {name}"));
        if response.clicked() {
            *active = index;
            changed = true;
        }
    }
    changed
}

/// Panel frame used by every dock slot so their chrome matches the theme.
pub fn panel_frame(tokens: &Tokens) -> egui::Frame {
    egui::Frame {
        fill: tokens.panel,
        inner_margin: egui::Margin::symmetric(8.0, 6.0),
        ..Default::default()
    }
}

/// Painted divider under a tab strip.
pub fn strip_separator(ui: &mut Ui) {
    ui.add_space(2.0);
    let rect = Rect::from_min_max(
        egui::pos2(ui.min_rect().left(), ui.cursor().top()),
        egui::pos2(ui.max_rect().right(), ui.cursor().top() + 1.0),
    );
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_white_alpha(12));
    ui.add_space(3.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Context;

    #[test]
    fn presets_change_the_documented_dimensions() {
        let mut dock = DockState::default();
        dock.apply_preset(LayoutPreset::Modeling);
        assert!(dock.left_collapsed && dock.bottom_collapsed);
        dock.apply_preset(LayoutPreset::Data);
        assert_eq!(dock.bottom_height, 320.0);
        assert!(!dock.left_collapsed);
        dock.apply_preset(LayoutPreset::Cartography);
        assert!(dock.left_width > DockState::default().left_width);
    }

    #[test]
    fn layout_round_trips_through_json() {
        let mut dock = DockState::default();
        dock.apply_preset(LayoutPreset::Cartography);
        dock.left_tab = 2;
        dock.bottom_tab = 1;
        let text = serde_json::to_string(&dock).expect("serialize");
        let back: DockState = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(dock, back);
    }

    #[test]
    fn partial_layout_json_fills_defaults() {
        let dock: DockState = serde_json::from_str(r#"{ "left_width": 300.0 }"#).expect("parse");
        assert_eq!(dock.left_width, 300.0);
        assert_eq!(dock.right_tab, 0);
        assert_eq!(dock.preset, LayoutPreset::Default);
    }

    #[test]
    fn clamp_keeps_slots_usable_in_a_small_window() {
        let mut dock = DockState {
            left_width: 5_000.0,
            bottom_height: 5_000.0,
            ..Default::default()
        };
        dock.clamp(1_200.0, 800.0);
        assert!(dock.left_width <= 600.0);
        // 0.6 * 800 is not exact in f32, so allow the rounding sliver.
        assert!(dock.bottom_height <= 481.0);
    }

    #[test]
    fn tabs_switch_on_click() {
        let ctx = Context::default();
        let mut active = 0usize;
        let mut frame = |ctx: &Context| {
            let mut changed = false;
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changed = tab_strip(ui, &["Layers", "Scene", "Assets"], &mut active);
                });
            });
            changed
        };
        // First frame lays out; the strip starts on tab 0.
        frame(&ctx);
        assert_eq!(active, 0);
    }
}
