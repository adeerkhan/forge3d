//! Command palette (`Ctrl+K`): every registered action, one keystroke away.

use egui::Context;

use crate::actions::{Action, ACTIONS};

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
}

impl Palette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
        }
    }

    fn matches(&self) -> Vec<Action> {
        let query = self.query.trim().to_lowercase();
        ACTIONS
            .iter()
            .filter(|def| {
                if query.is_empty() {
                    return true;
                }
                def.label.to_lowercase().contains(&query)
                    || def
                        .menu
                        .map(|menu| menu.label().to_lowercase().contains(&query))
                        .unwrap_or(false)
            })
            .map(|def| def.action)
            .collect()
    }
}

/// Draw the palette; returns the action the user ran.
pub fn window(ctx: &Context, palette: &mut Palette) -> Option<Action> {
    if !palette.open {
        return None;
    }
    let mut chosen = None;
    let mut open = palette.open;
    egui::Window::new("Command palette")
        .id(egui::Id::new("studio_command_palette"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut palette.query)
                    .hint_text("Type a command…")
                    .desired_width(f32::INFINITY),
            );
            response.request_focus();
            let matches = palette.matches();
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                chosen = matches.first().copied();
            }
            ui.add_space(4.0);
            if matches.is_empty() {
                ui.weak("No matching command");
                return;
            }
            let mut run = None;
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for action in &matches {
                        let mut label = action.label().to_owned();
                        if let Some(menu) = action.menu() {
                            label.push_str(&format!("   · {}", menu.label()));
                        }
                        let shortcut = action.shortcut_label().unwrap_or_default();
                        let button = egui::Button::new(label)
                            .shortcut_text(shortcut)
                            .frame(false)
                            .min_size(egui::vec2(ui.available_width(), 0.0));
                        if ui.add(button).clicked() {
                            run = Some(*action);
                        }
                    }
                });
            if let Some(action) = run {
                chosen = Some(action);
            }
            ui.add_space(2.0);
            ui.weak("Enter runs the first match · Esc closes");
        });
    if !open || chosen.is_some() {
        palette.open = false;
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_filters_the_registry() {
        let palette = Palette {
            open: true,
            query: "export".to_owned(),
        };
        let matches = palette.matches();
        assert!(matches.contains(&Action::ExportPng));
        assert!(!matches.contains(&Action::ZoomIn));
    }

    #[test]
    fn empty_query_lists_everything() {
        let palette = Palette {
            open: true,
            query: String::new(),
        };
        assert_eq!(palette.matches().len(), ACTIONS.len());
    }

    #[test]
    fn menu_names_are_searchable() {
        use crate::actions::Menu;
        let palette = Palette {
            open: true,
            query: "help".to_owned(),
        };
        let matches = palette.matches();
        assert!(matches.contains(&Action::ShowAbout));
        assert_eq!(
            Menu::Help.label().to_lowercase(),
            "help",
            "menu names feed the search"
        );
    }

    #[test]
    fn palette_renders_headlessly() {
        let ctx = egui::Context::default();
        let mut palette = Palette {
            open: true,
            query: "zoom".to_owned(),
        };
        // `window` may close the palette; the frame itself must not.
        let _ = ctx.run(Default::default(), |ctx| {
            assert!(window(ctx, &mut palette).is_none());
        });
    }
}
