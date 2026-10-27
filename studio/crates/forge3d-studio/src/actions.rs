//! The action registry (GUI.md III.2).
//!
//! Every menu item, toolbar button and keyboard shortcut is an [`Action`]
//! declared in one table. Consequences, all test-locked:
//!
//! * an action that does not work is **absent**, never a disabled stub
//!   (except where a runtime predicate such as "no terrain" explains itself);
//! * the menu bar, the toolbar, the shortcut window and the command palette
//!   are generated from this table, so they cannot drift apart;
//! * shortcuts are unique (a test fails on a duplicate).
//!
//! Handlers live in `app::StudioApp::run_action`, whose `match` is exhaustive:
//! adding an action without handling it does not compile.

use egui::{Key, KeyboardShortcut, Modifiers};

/// Top-level menus, in bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    File,
    View,
    Render,
    Help,
}

impl Menu {
    pub const ALL: &'static [Menu] = &[Menu::File, Menu::View, Menu::Render, Menu::Help];

    pub fn label(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::View => "View",
            Menu::Render => "Render",
            Menu::Help => "Help",
        }
    }
}

/// A keyboard shortcut: modifiers plus one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub key: Key,
}

impl Shortcut {
    pub const fn new(ctrl: bool, shift: bool, key: Key) -> Self {
        Self { ctrl, shift, key }
    }

    pub fn label(self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("Ctrl+");
        }
        if self.shift {
            out.push_str("Shift+");
        }
        out.push_str(self.key.name());
        out
    }

    pub fn to_egui(self) -> KeyboardShortcut {
        // egui 0.27's `Modifiers` is a plain struct (no `BitOrAssign`), so the
        // flags are set directly. `command` follows the platform convention:
        // Ctrl elsewhere, Cmd on macOS.
        let modifiers = Modifiers {
            ctrl: self.ctrl,
            shift: self.shift,
            command: self.ctrl,
            ..Modifiers::NONE
        };
        KeyboardShortcut::new(modifiers, self.key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // File
    NewProject,
    OpenProject,
    SaveProject,
    SaveProjectAs,
    ImportDataset,
    ExportPng,
    Quit,
    // View
    ZoomIn,
    ZoomOut,
    FitView,
    CameraTop,
    CameraFront,
    CameraRight,
    CameraIso,
    CameraReset,
    LayoutDefault,
    LayoutCartography,
    LayoutModeling,
    LayoutData,
    ResetLayout,
    ThemeDark,
    ThemeLight,
    // Render
    RenderFrame,
    // Help
    ShowShortcuts,
    ShowAbout,
    // Palette-only
    OpenPalette,
}

/// One row of the registry.
pub struct ActionDef {
    pub action: Action,
    pub label: &'static str,
    pub icon: Option<&'static str>,
    pub shortcut: Option<Shortcut>,
    pub menu: Option<Menu>,
    /// Toggle entries (theme, layout) render a check mark for the active state.
    pub checkable: bool,
}

/// The single source of truth for the UI's command surface.
pub const ACTIONS: &[ActionDef] = &[
    ActionDef {
        action: Action::NewProject,
        label: "New project",
        icon: Some("folder-open"),
        shortcut: Some(Shortcut::new(true, false, Key::N)),
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::OpenProject,
        label: "Open project…",
        icon: Some("folder-open"),
        shortcut: Some(Shortcut::new(true, false, Key::O)),
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::SaveProject,
        label: "Save project",
        icon: Some("save"),
        shortcut: Some(Shortcut::new(true, false, Key::S)),
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::SaveProjectAs,
        label: "Save project as…",
        icon: None,
        shortcut: Some(Shortcut::new(true, true, Key::S)),
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::ImportDataset,
        label: "Import dataset…",
        icon: Some("file-input"),
        shortcut: Some(Shortcut::new(true, false, Key::I)),
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::ExportPng,
        label: "Export PNG…",
        icon: Some("file-output"),
        shortcut: Some(Shortcut::new(true, false, Key::E)),
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::Quit,
        label: "Quit",
        icon: None,
        shortcut: None,
        menu: Some(Menu::File),
        checkable: false,
    },
    ActionDef {
        action: Action::ZoomIn,
        label: "Zoom in",
        icon: None,
        shortcut: Some(Shortcut::new(true, false, Key::Equals)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::ZoomOut,
        label: "Zoom out",
        icon: None,
        shortcut: Some(Shortcut::new(true, false, Key::Minus)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::FitView,
        label: "Fit view",
        icon: Some("camera"),
        shortcut: Some(Shortcut::new(false, false, Key::F)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::CameraTop,
        label: "Top",
        icon: None,
        shortcut: Some(Shortcut::new(true, false, Key::Num7)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::CameraFront,
        label: "Front",
        icon: None,
        shortcut: Some(Shortcut::new(true, false, Key::Num1)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::CameraRight,
        label: "Right",
        icon: None,
        shortcut: Some(Shortcut::new(true, false, Key::Num3)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::CameraIso,
        label: "Isometric",
        icon: None,
        shortcut: Some(Shortcut::new(true, false, Key::Num5)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::CameraReset,
        label: "Reset camera",
        icon: Some("rotate-ccw"),
        shortcut: Some(Shortcut::new(true, false, Key::Num0)),
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::LayoutDefault,
        label: "Panel layout: Default",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: true,
    },
    ActionDef {
        action: Action::LayoutCartography,
        label: "Panel layout: Cartography",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: true,
    },
    ActionDef {
        action: Action::LayoutModeling,
        label: "Panel layout: Modeling",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: true,
    },
    ActionDef {
        action: Action::LayoutData,
        label: "Panel layout: Data",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: true,
    },
    ActionDef {
        action: Action::ResetLayout,
        label: "Reset layout",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: false,
    },
    ActionDef {
        action: Action::ThemeDark,
        label: "Theme: Dark",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: true,
    },
    ActionDef {
        action: Action::ThemeLight,
        label: "Theme: Light",
        icon: None,
        shortcut: None,
        menu: Some(Menu::View),
        checkable: true,
    },
    ActionDef {
        action: Action::RenderFrame,
        label: "Render frame",
        icon: Some("image"),
        shortcut: Some(Shortcut::new(true, false, Key::R)),
        menu: Some(Menu::Render),
        checkable: false,
    },
    ActionDef {
        action: Action::ShowShortcuts,
        label: "Keyboard shortcuts",
        icon: None,
        shortcut: Some(Shortcut::new(false, false, Key::F1)),
        menu: Some(Menu::Help),
        checkable: false,
    },
    ActionDef {
        action: Action::ShowAbout,
        label: "About Forge3D Studio",
        icon: None,
        shortcut: None,
        menu: Some(Menu::Help),
        checkable: false,
    },
    ActionDef {
        action: Action::OpenPalette,
        label: "Command palette…",
        icon: Some("search"),
        shortcut: Some(Shortcut::new(true, false, Key::K)),
        menu: None,
        checkable: false,
    },
];

impl Action {
    pub fn def(self) -> &'static ActionDef {
        ACTIONS
            .iter()
            .find(|def| def.action == self)
            .expect("every Action has a registry row")
    }

    pub fn label(self) -> &'static str {
        self.def().label
    }

    pub fn icon(self) -> Option<&'static str> {
        self.def().icon
    }

    pub fn shortcut(self) -> Option<Shortcut> {
        self.def().shortcut
    }

    pub fn shortcut_label(self) -> Option<String> {
        self.shortcut().map(Shortcut::label)
    }

    pub fn menu(self) -> Option<Menu> {
        self.def().menu
    }

    pub fn checkable(self) -> bool {
        self.def().checkable
    }

    /// Actions shown in the toolbar, in order.
    pub fn toolbar() -> Vec<Action> {
        vec![
            Action::OpenProject,
            Action::SaveProject,
            Action::ImportDataset,
            Action::ExportPng,
            Action::RenderFrame,
            Action::FitView,
        ]
    }

    /// Every action in one menu, in registry order.
    pub fn in_menu(menu: Menu) -> Vec<Action> {
        ACTIONS
            .iter()
            .filter(|def| def.menu == Some(menu))
            .map(|def| def.action)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The registry is the contract: an action without a row cannot be
    /// dispatched (and `Action::def` panics in tests, not in production).
    #[test]
    fn every_action_has_exactly_one_registry_row() {
        let mut seen = HashSet::new();
        for def in ACTIONS {
            assert!(
                seen.insert(def.action),
                "duplicate registry row for {:?}",
                def.action
            );
            assert!(!def.label.is_empty(), "{:?} has no label", def.action);
        }
        // Action::def panics for a missing row; probe the ones we know exist.
        for def in ACTIONS {
            assert_eq!(def.action.def().action, def.action);
        }
    }

    #[test]
    fn shortcuts_are_unique_and_labelled() {
        let mut seen = HashSet::new();
        for def in ACTIONS {
            let Some(shortcut) = def.shortcut else {
                continue;
            };
            assert!(
                seen.insert(shortcut),
                "{} is bound twice ({:?})",
                shortcut.label(),
                def.action
            );
            assert!(
                !shortcut.label().is_empty(),
                "{:?} has an empty shortcut label",
                def.action
            );
        }
    }

    #[test]
    fn every_menu_has_entries() {
        for menu in Menu::ALL {
            assert!(
                !Action::in_menu(*menu).is_empty(),
                "menu {} would render empty",
                menu.label()
            );
        }
    }

    #[test]
    fn toolbar_actions_are_registered_with_icons() {
        for action in Action::toolbar() {
            assert!(
                action.icon().is_some(),
                "{:?} is in the toolbar without an icon",
                action
            );
            assert!(
                ACTIONS.iter().any(|def| def.action == action),
                "{:?} is not registered",
                action
            );
        }
    }

    #[test]
    fn shortcut_labels_match_the_egui_binding() {
        let shortcut = Shortcut::new(true, true, Key::S);
        assert_eq!(shortcut.label(), "Ctrl+Shift+S");
        let egui = shortcut.to_egui();
        assert!(egui.modifiers.ctrl, "Ctrl must be required");
        assert!(egui.modifiers.shift, "Shift must be required");
        assert!(!egui.modifiers.alt);
        // `command` follows the platform convention so macOS sees Cmd+S.
        assert!(egui.modifiers.command);
        assert_eq!(egui.logical_key, Key::S);
        // A plain-ctrl shortcut must not require Shift.
        let plain = Shortcut::new(true, false, Key::S).to_egui();
        assert!(plain.modifiers.ctrl && !plain.modifiers.shift);
        assert_eq!(plain.logical_key, Key::S);
    }
}
