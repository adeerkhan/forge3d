//! The action registry (GUI.md III.2).
//!
//! Every menu item, toolbar button and keyboard shortcut is an [`Action`]
//! declared in one table. Consequences, all test-locked:
//!
//! * an action that does not work is **absent**, never a disabled stub
//!   (except where a [`Need`] explains why it is unavailable);
//! * the menu bar, the toolbar, the layer-panel toolbar, the shortcut window
//!   and the command palette are generated from this table, so they cannot
//!   drift apart;
//! * shortcuts are unique.
//!
//! Handlers live in `app::StudioApp::run_action`, whose `match` is exhaustive:
//! adding an action without handling it does not compile.

use egui::{Key, KeyboardShortcut, Modifiers};

use crate::icons::Icon;

/// Top-level menus, in bar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    View,
    Scene,
    Layer,
    Render,
    Help,
}

impl Menu {
    pub const ALL: &'static [Menu] = &[
        Menu::File,
        Menu::Edit,
        Menu::View,
        Menu::Scene,
        Menu::Layer,
        Menu::Render,
        Menu::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::View => "View",
            Menu::Scene => "Scene",
            Menu::Layer => "Layer",
            Menu::Render => "Render",
            Menu::Help => "Help",
        }
    }
}

/// What an action needs before it can run. The UI disables it and the status
/// bar explains, so a menu never lies about what is possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Always available.
    Nothing,
    /// At least one layer in the document.
    Layers,
    /// At least one selected layer.
    Selection,
    /// Something to undo.
    Undo,
    /// Something to redo.
    Redo,
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
    // Edit
    Undo,
    Redo,
    DuplicateLayer,
    DeleteLayer,
    GroupLayers,
    UngroupLayer,
    RenameLayer,
    SelectAllLayers,
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
    // Scene
    AddTerrainLayer,
    AddGroup,
    // Layer
    ZoomToLayer,
    ToggleLayerVisibility,
    ToggleLayerLock,
    LayerMoveUp,
    LayerMoveDown,
    LayerToTop,
    LayerToBottom,
    // Render
    RenderFrame,
    // Help
    ShowExamples,
    ShowShortcuts,
    ShowAbout,
    // Palette-only
    OpenPalette,
}

/// One row of the registry.
pub struct ActionDef {
    pub action: Action,
    pub label: &'static str,
    pub icon: Option<Icon>,
    pub shortcut: Option<Shortcut>,
    pub menu: Option<Menu>,
    /// Toggle entries (theme, layout) render a check mark for the active state.
    pub checkable: bool,
    /// What must be true for the action to run.
    pub need: Need,
}

const fn file(
    action: Action,
    label: &'static str,
    icon: Option<Icon>,
    shortcut: Option<Shortcut>,
) -> ActionDef {
    ActionDef {
        action,
        label,
        icon,
        shortcut,
        menu: Some(Menu::File),
        checkable: false,
        need: Need::Nothing,
    }
}

const fn edit(
    action: Action,
    label: &'static str,
    icon: Option<Icon>,
    shortcut: Option<Shortcut>,
    need: Need,
) -> ActionDef {
    ActionDef {
        action,
        label,
        icon,
        shortcut,
        menu: Some(Menu::Edit),
        checkable: false,
        need,
    }
}

const fn view(
    action: Action,
    label: &'static str,
    icon: Option<Icon>,
    shortcut: Option<Shortcut>,
    checkable: bool,
) -> ActionDef {
    ActionDef {
        action,
        label,
        icon,
        shortcut,
        menu: Some(Menu::View),
        checkable,
        need: Need::Nothing,
    }
}

const fn scene(
    action: Action,
    label: &'static str,
    icon: Option<Icon>,
    shortcut: Option<Shortcut>,
) -> ActionDef {
    ActionDef {
        action,
        label,
        icon,
        shortcut,
        menu: Some(Menu::Scene),
        checkable: false,
        need: Need::Nothing,
    }
}

const fn layer(
    action: Action,
    label: &'static str,
    icon: Option<Icon>,
    shortcut: Option<Shortcut>,
) -> ActionDef {
    ActionDef {
        action,
        label,
        icon,
        shortcut,
        menu: Some(Menu::Layer),
        checkable: false,
        need: Need::Selection,
    }
}

/// The single source of truth for the UI's command surface.
pub const ACTIONS: &[ActionDef] = &[
    // ---------------------------------------------------------------- File
    file(
        Action::NewProject,
        "New project",
        Some(Icon::Plus),
        Some(Shortcut::new(true, false, Key::N)),
    ),
    file(
        Action::OpenProject,
        "Open project…",
        Some(Icon::FolderOpen),
        Some(Shortcut::new(true, false, Key::O)),
    ),
    file(
        Action::SaveProject,
        "Save project",
        Some(Icon::Save),
        Some(Shortcut::new(true, false, Key::S)),
    ),
    file(
        Action::SaveProjectAs,
        "Save project as…",
        None,
        Some(Shortcut::new(true, true, Key::S)),
    ),
    file(
        Action::ImportDataset,
        "Import dataset…",
        Some(Icon::FileInput),
        Some(Shortcut::new(true, false, Key::I)),
    ),
    file(
        Action::ExportPng,
        "Export PNG…",
        Some(Icon::FileOutput),
        Some(Shortcut::new(true, false, Key::E)),
    ),
    file(Action::Quit, "Quit", None, None),
    // ---------------------------------------------------------------- Edit
    edit(
        Action::Undo,
        "Undo",
        Some(Icon::RotateCcw),
        Some(Shortcut::new(true, false, Key::Z)),
        Need::Undo,
    ),
    edit(
        Action::Redo,
        "Redo",
        Some(Icon::RotateCw),
        Some(Shortcut::new(true, true, Key::Z)),
        Need::Redo,
    ),
    edit(
        Action::DuplicateLayer,
        "Duplicate layer",
        Some(Icon::Copy),
        Some(Shortcut::new(true, false, Key::D)),
        Need::Selection,
    ),
    edit(
        Action::DeleteLayer,
        "Delete layer",
        Some(Icon::Trash),
        Some(Shortcut::new(false, false, Key::Delete)),
        Need::Selection,
    ),
    edit(
        Action::GroupLayers,
        "Group layers",
        Some(Icon::FolderPlus),
        Some(Shortcut::new(true, false, Key::G)),
        Need::Selection,
    ),
    edit(
        Action::UngroupLayer,
        "Ungroup",
        Some(Icon::Folder),
        Some(Shortcut::new(true, true, Key::G)),
        Need::Selection,
    ),
    edit(
        Action::RenameLayer,
        "Rename layer",
        Some(Icon::Type),
        Some(Shortcut::new(false, false, Key::F2)),
        Need::Selection,
    ),
    edit(
        Action::SelectAllLayers,
        "Select all layers",
        None,
        Some(Shortcut::new(true, false, Key::A)),
        Need::Layers,
    ),
    // ---------------------------------------------------------------- View
    view(
        Action::ZoomIn,
        "Zoom in",
        None,
        Some(Shortcut::new(true, false, Key::Equals)),
        false,
    ),
    view(
        Action::ZoomOut,
        "Zoom out",
        None,
        Some(Shortcut::new(true, false, Key::Minus)),
        false,
    ),
    view(
        Action::FitView,
        "Fit view",
        Some(Icon::Camera),
        Some(Shortcut::new(false, false, Key::F)),
        false,
    ),
    view(
        Action::CameraTop,
        "Top",
        None,
        Some(Shortcut::new(true, false, Key::Num7)),
        false,
    ),
    view(
        Action::CameraFront,
        "Front",
        None,
        Some(Shortcut::new(true, false, Key::Num1)),
        false,
    ),
    view(
        Action::CameraRight,
        "Right",
        None,
        Some(Shortcut::new(true, false, Key::Num3)),
        false,
    ),
    view(
        Action::CameraIso,
        "Isometric",
        None,
        Some(Shortcut::new(true, false, Key::Num5)),
        false,
    ),
    view(
        Action::CameraReset,
        "Reset camera",
        Some(Icon::RotateCcw),
        Some(Shortcut::new(true, false, Key::Num0)),
        false,
    ),
    view(
        Action::LayoutDefault,
        "Panel layout: Default",
        None,
        None,
        true,
    ),
    view(
        Action::LayoutCartography,
        "Panel layout: Cartography",
        None,
        None,
        true,
    ),
    view(
        Action::LayoutModeling,
        "Panel layout: Modeling",
        None,
        None,
        true,
    ),
    view(Action::LayoutData, "Panel layout: Data", None, None, true),
    view(Action::ResetLayout, "Reset layout", None, None, false),
    view(Action::ThemeDark, "Theme: Dark", None, None, true),
    view(Action::ThemeLight, "Theme: Light", None, None, true),
    // --------------------------------------------------------------- Scene
    scene(
        Action::AddTerrainLayer,
        "Add terrain layer…",
        Some(Icon::MountainSnow),
        None,
    ),
    scene(Action::AddGroup, "New group", Some(Icon::FolderPlus), None),
    // --------------------------------------------------------------- Layer
    layer(
        Action::ZoomToLayer,
        "Zoom to layer",
        Some(Icon::Camera),
        None,
    ),
    layer(
        Action::ToggleLayerVisibility,
        "Toggle visibility",
        Some(Icon::Eye),
        Some(Shortcut::new(true, false, Key::H)),
    ),
    layer(
        Action::ToggleLayerLock,
        "Lock / unlock",
        Some(Icon::Lock),
        Some(Shortcut::new(true, false, Key::L)),
    ),
    layer(
        Action::LayerMoveUp,
        "Move up",
        None,
        Some(Shortcut::new(true, false, Key::CloseBracket)),
    ),
    layer(
        Action::LayerMoveDown,
        "Move down",
        None,
        Some(Shortcut::new(true, false, Key::OpenBracket)),
    ),
    layer(
        Action::LayerToTop,
        "Move to top",
        None,
        Some(Shortcut::new(true, true, Key::CloseBracket)),
    ),
    layer(
        Action::LayerToBottom,
        "Move to bottom",
        None,
        Some(Shortcut::new(true, true, Key::OpenBracket)),
    ),
    // -------------------------------------------------------------- Render
    ActionDef {
        action: Action::RenderFrame,
        label: "Render frame",
        icon: Some(Icon::Image),
        shortcut: Some(Shortcut::new(true, false, Key::R)),
        menu: Some(Menu::Render),
        checkable: false,
        need: Need::Nothing,
    },
    // ---------------------------------------------------------------- Help
    ActionDef {
        action: Action::ShowExamples,
        label: "Examples gallery",
        icon: Some(Icon::Layers),
        shortcut: None,
        menu: Some(Menu::Help),
        checkable: false,
        need: Need::Nothing,
    },
    ActionDef {
        action: Action::ShowShortcuts,
        label: "Keyboard shortcuts",
        icon: None,
        shortcut: Some(Shortcut::new(false, false, Key::F1)),
        menu: Some(Menu::Help),
        checkable: false,
        need: Need::Nothing,
    },
    ActionDef {
        action: Action::ShowAbout,
        label: "About Forge3D Studio",
        icon: None,
        shortcut: None,
        menu: Some(Menu::Help),
        checkable: false,
        need: Need::Nothing,
    },
    // ------------------------------------------------------- palette only
    ActionDef {
        action: Action::OpenPalette,
        label: "Command palette…",
        icon: Some(Icon::Settings2),
        shortcut: Some(Shortcut::new(true, false, Key::K)),
        menu: None,
        checkable: false,
        need: Need::Nothing,
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

    pub fn icon(self) -> Option<Icon> {
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

    pub fn need(self) -> Need {
        self.def().need
    }

    /// Actions shown in the global toolbar, in order.
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
            assert!(!shortcut.label().is_empty());
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
    fn destructive_layer_actions_require_a_selection() {
        for action in [
            Action::DeleteLayer,
            Action::DuplicateLayer,
            Action::GroupLayers,
            Action::RenameLayer,
            Action::ToggleLayerVisibility,
            Action::LayerMoveUp,
        ] {
            assert_eq!(action.need(), Need::Selection, "{action:?}");
        }
        assert_eq!(Action::Undo.need(), Need::Undo);
        assert_eq!(Action::Redo.need(), Need::Redo);
        assert_eq!(Action::SelectAllLayers.need(), Need::Layers);
        assert_eq!(Action::SaveProject.need(), Need::Nothing);
    }

    #[test]
    fn toolbar_actions_are_registered_with_icons() {
        for action in Action::toolbar() {
            assert!(action.icon().is_some(), "{action:?} has no icon");
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
        let plain = Shortcut::new(true, false, Key::S).to_egui();
        assert!(plain.modifiers.ctrl && !plain.modifiers.shift);
    }
}
