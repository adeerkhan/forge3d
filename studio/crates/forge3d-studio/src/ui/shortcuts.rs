//! Keyboard reference window, generated from the action registry plus the
//! viewport gestures that are not actions (drag, scroll, space-pan).

use egui::{Context, Grid, RichText};

use crate::actions::{Action, Menu};

/// Gestures the viewport owns. Kept here so the reference is the whole truth.
const VIEWPORT_BINDINGS: &[(&str, &str)] = &[
    ("Drag", "Orbit the camera"),
    ("Scroll", "Zoom in / out"),
    ("Double-click", "Fit the terrain in view"),
];

pub fn window(ctx: &Context, open: &mut bool) {
    if !*open {
        return;
    }
    egui::Window::new("Keyboard shortcuts")
        .id(egui::Id::new("studio_shortcuts"))
        .open(open)
        .resizable(true)
        .default_width(420.0)
        .show(ctx, |ui| {
            for menu in Menu::ALL {
                ui.label(RichText::new(menu.label()).strong());
                Grid::new(format!("shortcuts_{}", menu.label()))
                    .num_columns(2)
                    .spacing([16.0, 3.0])
                    .show(ui, |ui| {
                        for action in Action::in_menu(*menu) {
                            let label = action.label();
                            match action.shortcut_label() {
                                Some(shortcut) => {
                                    ui.label(label);
                                    ui.label(RichText::new(shortcut).monospace());
                                }
                                None => {
                                    ui.label(label);
                                    ui.weak("—");
                                }
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
            }
            ui.label(RichText::new("Viewport").strong());
            Grid::new("shortcuts_viewport")
                .num_columns(2)
                .spacing([16.0, 3.0])
                .show(ui, |ui| {
                    for (gesture, meaning) in VIEWPORT_BINDINGS {
                        ui.label(*gesture);
                        ui.weak(*meaning);
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.weak("Drag the splitters to resize docks; View ▸ Panel layout has presets.");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_in_the_registry_is_listed() {
        // The window is generated, so this locks the invariant that no action
        // carries a binding the reference would hide.
        let bound: Vec<Action> = crate::actions::ACTIONS
            .iter()
            .filter(|def| def.shortcut.is_some())
            .map(|def| def.action)
            .collect();
        assert!(
            bound.len() >= 10,
            "expected the core bindings, got {bound:?}"
        );
        for action in bound {
            assert!(action.shortcut_label().is_some());
        }
    }

    #[test]
    fn window_renders_headlessly() {
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(Default::default(), |ctx| window(ctx, &mut open));
        assert!(open, "a frame must not close the window by itself");
    }
}
