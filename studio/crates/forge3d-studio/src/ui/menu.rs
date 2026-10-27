//! Menu bar generated from the action registry (GUI.md III.2).

use egui::Ui;

use crate::actions::{Action, Menu};

/// A separator marks the start of a new group inside a menu.
fn starts_group(action: Action) -> bool {
    matches!(
        action,
        Action::ImportDataset
            | Action::Quit
            | Action::CameraTop
            | Action::LayoutDefault
            | Action::ResetLayout
            | Action::ThemeDark
    )
}

/// Draw the menu bar; returns the action the user picked, if any.
pub fn menu_bar(
    ui: &mut Ui,
    checked: impl Fn(Action) -> bool,
    enabled: impl Fn(Action) -> bool,
) -> Option<Action> {
    let mut chosen = None;
    egui::menu::bar(ui, |ui| {
        for menu in Menu::ALL {
            ui.menu_button(menu.label(), |ui| {
                for (index, action) in Action::in_menu(*menu).into_iter().enumerate() {
                    if index > 0 && starts_group(action) {
                        ui.separator();
                    }
                    let mut button = egui::Button::new(action.label()).frame(false);
                    if let Some(shortcut) = action.shortcut_label() {
                        button = button.shortcut_text(shortcut);
                    }
                    if action.checkable() {
                        button = button.selected(checked(action));
                    }
                    let response = ui.add_enabled(enabled(action), button);
                    if response.clicked() {
                        chosen = Some(action);
                        ui.close_menu();
                    }
                }
            });
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registered action must render inside exactly one menu, and each
    /// menu's first entry must not start with a separator.
    #[test]
    fn menu_groups_never_lead_with_a_separator() {
        for menu in Menu::ALL {
            for (index, action) in Action::in_menu(*menu).into_iter().enumerate() {
                if index == 0 {
                    assert!(
                        !starts_group(action),
                        "{} leads with a separator",
                        menu.label()
                    );
                }
            }
        }
    }

    #[test]
    fn menu_bar_renders_headlessly() {
        let ctx = egui::Context::default();
        let mut picked = None;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                picked = menu_bar(ui, |_| false, |_| true);
            });
        });
        assert!(picked.is_none(), "no click, no action");
    }
}
