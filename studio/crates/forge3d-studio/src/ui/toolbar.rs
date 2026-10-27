//! Toolbar: the six most-used actions, generated from the registry.

use egui::{Image, RichText, Ui};

use crate::actions::Action;
use crate::icons::Icons;

/// Draw the toolbar; returns the action the user clicked, if any.
///
/// `is_active` highlights checkable actions (viewport tools, view toggles)
/// so the toolbar shows state, Blender-style.
pub fn toolbar(
    ui: &mut Ui,
    icons: &Icons,
    icon_size: f32,
    enabled: impl Fn(Action) -> bool,
    is_active: impl Fn(Action) -> bool,
) -> Option<Action> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let actions = Action::toolbar();
        for (index, action) in actions.iter().enumerate() {
            // Group file operations away from viewport operations.
            if index == 4 {
                ui.separator();
            }
            let Some(icon) = action.icon() else { continue };
            let texture = icons.texture(ui.ctx(), icon);
            let tooltip = match action.shortcut_label() {
                Some(shortcut) => format!("{} ({shortcut})", action.label()),
                None => action.label().to_owned(),
            };
            let mut button = egui::Button::image(
                Image::new(&texture).fit_to_exact_size(egui::vec2(icon_size, icon_size)),
            )
            .frame(false);
            if is_active(*action) {
                button = button.fill(egui::Color32::from_white_alpha(30));
            }
            let response = ui
                .add_enabled(enabled(*action), button)
                .on_hover_text(tooltip);
            if response.clicked() {
                chosen = Some(*action);
            }
        }
    });
    chosen
}

/// Right-aligned project identity (path or "unsaved") plus a busy chip.
pub fn project_chip(ui: &mut Ui, project: Option<&str>, busy: bool) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if busy {
            ui.add(egui::Spinner::new().size(13.0));
            ui.label(RichText::new("working…").size(11.5).weak());
        }
        match project {
            Some(path) => {
                ui.label(RichText::new(path).size(11.5).weak())
                    .on_hover_text(path);
            }
            None => {
                ui.label(RichText::new("unsaved project").size(11.5).weak());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_renders_headlessly_with_every_action_enabled() {
        let ctx = egui::Context::default();
        let icons = Icons::load(&ctx, egui::Color32::WHITE);
        let mut frame = 0;
        while frame < 2 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert!(
                        toolbar(ui, &icons, 16.0, |_| true, |_| false).is_none(),
                        "no click, no action"
                    );
                });
            });
            frame += 1;
        }
    }
}
