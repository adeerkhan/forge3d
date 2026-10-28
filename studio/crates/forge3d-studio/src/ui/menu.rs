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
            | Action::AtmosphereSettings
    )
}

/// One row of a menu: a plain action, or a labelled one-level submenu.
///
/// Submenus are declared by [`Action::submenu`] (the registry stays flat), so a
/// submenu sits where its first member does and cannot drift from it.
enum Entry {
    Action(Action),
    Submenu {
        label: &'static str,
        actions: Vec<Action>,
    },
}

/// Fold one menu's actions into rows, merging each submenu at its first member.
fn entries(menu: Menu) -> Vec<Entry> {
    let mut rows: Vec<Entry> = Vec::new();
    for action in Action::in_menu(menu) {
        let Some((_, label)) = action.submenu() else {
            rows.push(Entry::Action(action));
            continue;
        };
        if let Some(Entry::Submenu { actions, .. }) = rows
            .iter_mut()
            .find(|row| matches!(row, Entry::Submenu { label: name, .. } if *name == label))
        {
            actions.push(action);
        } else {
            rows.push(Entry::Submenu {
                label,
                actions: vec![action],
            });
        }
    }
    rows
}

/// Draw one action button; returns true when it was clicked.
fn action_button(
    ui: &mut Ui,
    action: Action,
    checked: &impl Fn(Action) -> bool,
    enabled: &impl Fn(Action) -> bool,
) -> bool {
    let mut button = egui::Button::new(action.label()).frame(false);
    if let Some(shortcut) = action.shortcut_label() {
        button = button.shortcut_text(shortcut);
    }
    if action.checkable() {
        button = button.selected(checked(action));
    }
    ui.add_enabled(enabled(action), button).clicked()
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
                for (index, row) in entries(*menu).into_iter().enumerate() {
                    let first = match &row {
                        Entry::Action(action) => *action,
                        Entry::Submenu { actions, .. } => actions[0],
                    };
                    if index > 0 && starts_group(first) {
                        ui.separator();
                    }
                    match row {
                        Entry::Action(action) => {
                            if action_button(ui, action, &checked, &enabled) {
                                chosen = Some(action);
                                ui.close_menu();
                            }
                        }
                        Entry::Submenu { label, actions } => {
                            ui.menu_button(label, |ui| {
                                for action in actions {
                                    if action_button(ui, action, &checked, &enabled) {
                                        chosen = Some(action);
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
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
            let rows = entries(*menu);
            assert!(!rows.is_empty(), "menu {} would render empty", menu.label());
            for (index, row) in rows.iter().enumerate() {
                if index == 0 {
                    let first = match row {
                        Entry::Action(action) => *action,
                        Entry::Submenu { actions, .. } => actions[0],
                    };
                    assert!(
                        !starts_group(first),
                        "{} leads with a separator",
                        menu.label()
                    );
                }
            }
        }
    }

    /// The rows cover every action exactly once: nesting must not hide one.
    #[test]
    fn every_action_renders_exactly_once() {
        for menu in Menu::ALL {
            let mut listed: Vec<Action> = Vec::new();
            for row in entries(*menu) {
                match row {
                    Entry::Action(action) => listed.push(action),
                    Entry::Submenu { actions, .. } => listed.extend(actions),
                }
            }
            assert_eq!(listed, Action::in_menu(*menu), "{} row drift", menu.label());
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
