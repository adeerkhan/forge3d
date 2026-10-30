//! The action registry.
//!
//! One table declares every command: its label, its menu, its shortcut and its
//! action type. The menu bar, the keyboard map and the shortcuts window are all
//! generated from that table, so they cannot drift apart.
//!
//! The table is a macro because GPUI keybinding and menu APIs take the concrete
//! action type (not `Box<dyn Action>`); expanding per action keeps the single
//! declaration while giving each consumer a typed value.

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::{menu::AppMenuBar, ActiveTheme as _};
use gpui_kit::*;

gpui_kit::actions!(
    studio,
    [
        NewProject,
        OpenProject,
        SaveProject,
        SaveProjectAs,
        ImportDem,
        ExportPng,
        Quit,
        Undo,
        Redo,
        DuplicateLayer,
        DeleteLayer,
        ToggleLayerVisibility,
        ToggleLayerLock,
        RenameLayer,
        GroupLayers,
        UngroupLayers,
        MoveLayerUp,
        MoveLayerDown,
        MoveLayerToTop,
        MoveLayerToBottom,
        ToggleTheme,
        FitView,
        CameraTop,
        CameraFront,
        CameraRight,
        CameraBack,
        CameraLeft,
        ToggleOrtho,
        ToggleGrid,
        ToggleRigHandles,
        CameraIsometric,
        ResetCamera,
        RenderFrame,
        ToggleLiveViewport,
        UseRasterMode,
        UsePathTracedMode,
        ToggleCertificate,
        RenderSequence,
        DeterminismCheck,
        RefreshCapabilities,
        BakeRig,
        OpenExamples,
        ToggleRigPlayback,
        Shortcuts,
        About,
    ]
);

/// Which menu bar entry an action lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuGroup {
    File,
    Edit,
    View,
    Layer,
    Render,
    Tools,
    Help,
}

impl MenuGroup {
    pub const ALL: &'static [MenuGroup] = &[
        MenuGroup::File,
        MenuGroup::Edit,
        MenuGroup::View,
        MenuGroup::Layer,
        MenuGroup::Render,
        MenuGroup::Tools,
        MenuGroup::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuGroup::File => "File",
            MenuGroup::Edit => "Edit",
            MenuGroup::View => "View",
            MenuGroup::Layer => "Layer",
            MenuGroup::Render => "Render",
            MenuGroup::Tools => "Tools",
            MenuGroup::Help => "Help",
        }
    }
}

/// The single declaration every surface expands from.
///
/// `(action type, label, menu, starts a block, shortcut)`.
#[macro_export]
macro_rules! studio_action_table {
    ($callback:ident) => {
        $callback! {
            NewProject, "New project", File, false, Some("ctrl-n");
            OpenProject, "Open project…", File, false, Some("ctrl-o");
            SaveProject, "Save project", File, false, Some("ctrl-s");
            SaveProjectAs, "Save project as…", File, false, Some("ctrl-shift-s");
            ImportDem, "Import DEM…", File, true, Some("ctrl-i");
            ExportPng, "Export PNG…", File, false, Some("ctrl-e");
            Quit, "Quit", File, true, Some("ctrl-q");
            Undo, "Undo", Edit, false, Some("ctrl-z");
            Redo, "Redo", Edit, false, Some("ctrl-shift-z");
            DuplicateLayer, "Duplicate layer", Edit, true, Some("ctrl-d");
            DeleteLayer, "Delete layer", Edit, false, Some("delete");
            ToggleLayerVisibility, "Toggle visibility", Layer, false, None;
            ToggleLayerLock, "Lock / unlock", Layer, false, Some("ctrl-shift-l");
            RenameLayer, "Rename…", Layer, false, Some("f2");
            GroupLayers, "Group layers", Layer, true, Some("ctrl-g");
            UngroupLayers, "Ungroup", Layer, false, Some("ctrl-shift-g");
            MoveLayerUp, "Move up", Layer, true, Some("ctrl-shift-up");
            MoveLayerDown, "Move down", Layer, false, Some("ctrl-shift-down");
            MoveLayerToTop, "To top", Layer, false, None;
            MoveLayerToBottom, "To bottom", Layer, false, None;
            FitView, "Fit view", View, false, Some("f");
            CameraTop, "Top", View, true, Some("7");
            CameraFront, "Front", View, false, Some("1");
            CameraRight, "Right", View, false, Some("3");
            CameraBack, "Back", View, false, Some("ctrl-1");
            CameraLeft, "Left", View, false, Some("ctrl-3");
            ToggleOrtho, "Orthographic (approximate)", View, false, Some("5");
            ToggleGrid, "Grid", View, false, Some("shift-g");
            ToggleRigHandles, "Rig handles", View, false, Some("shift-r");
            CameraIsometric, "Isometric", View, false, Some("0");
            ResetCamera, "Reset camera", View, true, None;
            ToggleTheme, "Dark / light theme", View, true, None;
            RenderFrame, "Render still", Render, false, Some("ctrl-r");
            ToggleLiveViewport, "Live viewport", View, true, Some("ctrl-l");
            UseRasterMode, "Raster (PBR + POM)", Render, true, None;
            UsePathTracedMode, "Path traced", Render, false, None;
            ToggleCertificate, "Request render certificate", Render, true, None;
            RenderSequence, "Render sequence…", Render, false, None;
            DeterminismCheck, "Determinism check…", Tools, false, None;
            RefreshCapabilities, "Refresh engine capabilities", Tools, false, None;
            BakeRig, "Bake camera rig", Tools, false, None;
            OpenExamples, "Examples gallery…", Help, false, None;
            ToggleRigPlayback, "Play / pause rig", Tools, false, None;
            Shortcuts, "Keyboard shortcuts", Help, false, Some("shift-?");
            About, "About Forge3D Studio", Help, true, None;
        }
    };
}

/// One row of the registry, for the shortcuts window and the tests.
pub struct ActionSpec {
    pub label: &'static str,
    pub group: MenuGroup,
    pub shortcut: Option<&'static str>,
}

impl ActionSpec {
    /// The shortcut as a menu label reads it (`Ctrl+O`).
    pub fn shortcut_label(&self) -> Option<String> {
        self.shortcut.map(|keys| {
            keys.split('-')
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
    }
}

macro_rules! build_specs {
    ($($action:ident, $label:literal, $group:ident, $block:literal, $shortcut:expr;)*) => {
        /// Every action, in menu order.
        pub static ACTIONS: &[ActionSpec] = &[
            $(ActionSpec {
                label: $label,
                group: MenuGroup::$group,
                shortcut: $shortcut,
            }),*
        ];
    };
}

crate::studio_action_table!(build_specs);

macro_rules! build_bindings {
    ($($action:ident, $label:literal, $group:ident, $block:literal, $shortcut:expr;)*) => {
        fn keybindings() -> Vec<KeyBinding> {
            let mut bindings = Vec::new();
            $(
                if let Some(keys) = $shortcut {
                    bindings.push(KeyBinding::new(keys, $action, None));
                }
            )*
            bindings
        }

        fn push_group_items(group: MenuGroup, items: &mut Vec<MenuItem>) {
            $(
                if group == MenuGroup::$group {
                    if $block && !items.is_empty() {
                        items.push(MenuItem::Separator);
                    }
                    items.push(MenuItem::action($label, $action));
                }
            )*
        }
    };
}

crate::studio_action_table!(build_bindings);

/// What an action does.
///
/// One impl per action type: the table macro expands `run` for every entry, so
/// an action added to the registry without behaviour does not compile.
pub trait StudioAction: Action + Clone {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>);
}

impl StudioAction for NewProject {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.new_project(cx);
    }
}

impl StudioAction for OpenProject {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.open_project(cx);
    }
}

impl StudioAction for SaveProject {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.save_project(cx, false);
    }
}

impl StudioAction for SaveProjectAs {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.save_project(cx, true);
    }
}

impl StudioAction for ImportDem {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.pick_dem(cx);
    }
}

impl StudioAction for ExportPng {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.export_png(cx);
    }
}

impl StudioAction for Quit {
    fn run(_studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        cx.quit();
    }
}

impl StudioAction for Undo {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.undo(cx);
    }
}

impl StudioAction for Redo {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.redo(cx);
    }
}

impl StudioAction for DuplicateLayer {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.duplicate_layer(cx);
    }
}

impl StudioAction for DeleteLayer {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.delete_layer(cx);
    }
}

impl StudioAction for ToggleLayerVisibility {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.toggle_selection_visibility(cx);
    }
}

impl StudioAction for ToggleTheme {
    fn run(_studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        let next = if cx.theme().mode.is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(next, None, cx);
    }
}

impl StudioAction for FitView {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.fit_camera(cx);
    }
}

impl StudioAction for CameraTop {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        // The UI speaks elevation; the engine's theta is polar from +Z.
        studio.preset_view(90.0, 90.0, cx);
    }
}

impl StudioAction for CameraBack {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.preset_view(90.0, 1.0, cx);
    }
}

impl StudioAction for CameraLeft {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.preset_view(180.0, 1.0, cx);
    }
}

impl StudioAction for ToggleRigHandles {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.toggle_rig_handles(cx);
    }
}

impl StudioAction for ToggleGrid {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.toggle_grid(cx);
    }
}

impl StudioAction for ToggleOrtho {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.toggle_ortho(cx);
    }
}

impl StudioAction for CameraFront {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.preset_view(270.0, 1.0, cx);
    }
}

impl StudioAction for CameraRight {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.preset_view(0.0, 1.0, cx);
    }
}

impl StudioAction for CameraIsometric {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.preset_view(45.0, 35.0, cx);
    }
}

impl StudioAction for ResetCamera {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.reset_camera(cx);
    }
}

impl StudioAction for ToggleLayerLock {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        if let Some(id) = studio.primary_layer() {
            studio.toggle_layer_lock(id, cx);
        }
    }
}

impl StudioAction for RenameLayer {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        if let Some(id) = studio.primary_layer() {
            studio.begin_rename(id, cx);
        }
    }
}

impl StudioAction for GroupLayers {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.group_selection(cx);
    }
}

impl StudioAction for UngroupLayers {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.ungroup_selection(cx);
    }
}

impl StudioAction for MoveLayerUp {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.move_layer(-1, cx);
    }
}

impl StudioAction for MoveLayerDown {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.move_layer(1, cx);
    }
}

impl StudioAction for MoveLayerToTop {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.move_layer_to_edge(true, cx);
    }
}

impl StudioAction for MoveLayerToBottom {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.move_layer_to_edge(false, cx);
    }
}

impl StudioAction for UseRasterMode {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.set_render_mode(studio_project::RenderMode::Raster, cx);
    }
}

impl StudioAction for UsePathTracedMode {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.set_render_mode(studio_project::RenderMode::PathTraced, cx);
    }
}

impl StudioAction for ToggleCertificate {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        let next = !studio.sign_renders;
        studio.set_sign_renders(next, cx);
    }
}

impl StudioAction for RenderSequence {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.render_sequence(cx);
    }
}

impl StudioAction for DeterminismCheck {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.run_determinism_check(cx);
    }
}

impl StudioAction for RefreshCapabilities {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.refresh_capabilities(cx);
    }
}

impl StudioAction for OpenExamples {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.focus_examples(cx);
        let entity = cx.entity();
        // A menu action has no window in hand; the focused one is the target.
        let Some(window) = cx.active_window() else {
            log::warn!("examples gallery: no active window to open the carousel in");
            cx.notify();
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            crate::ui::gallery::open_carousel(&entity, window, cx);
        });
    }
}

impl StudioAction for BakeRig {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.bake_rig(cx);
    }
}

impl StudioAction for ToggleRigPlayback {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.toggle_rig_playback(cx);
    }
}

impl StudioAction for ToggleLiveViewport {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.toggle_live_viewport(cx);
    }
}

impl StudioAction for RenderFrame {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.request_still(cx);
    }
}

impl StudioAction for Shortcuts {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.show_shortcuts(cx);
    }
}

impl StudioAction for About {
    fn run(studio: &mut crate::state::Studio, cx: &mut Context<crate::state::Studio>) {
        studio.show_about(cx);
    }
}

/// Which menu items carry a check mark. Rebuilt whenever one toggles, so the
/// menus never show a stale state.
#[derive(Debug, Clone, Copy)]
pub struct MenuState {
    pub live_viewport: bool,
    pub certificate: bool,
    pub path_traced: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            live_viewport: true,
            certificate: false,
            path_traced: false,
        }
    }
}

/// Register the keyboard map and the application menus.
pub fn init(cx: &mut App) {
    let mut bindings = keybindings();
    // F12 is what a DCC user reaches for; Ctrl+R stays as the discoverable one.
    bindings.push(KeyBinding::new("f12", RenderFrame, None));
    cx.bind_keys(bindings);

    apply_menus(MenuState::default(), cx);
}

/// Publish the menu tree: the kit's menu bar reads it from `GlobalState`, and
/// the same list is handed to the platform (macOS uses it as the real menu bar).
pub fn apply_menus(state: MenuState, cx: &mut App) {
    let menus = build_menus(state);
    let owned: Vec<gpui_kit::OwnedMenu> = build_menus(state)
        .into_iter()
        .map(|menu| menu.owned())
        .collect();
    cx.set_menus(menus);
    gpui_kit::component::GlobalState::global_mut(cx).set_app_menus(owned);
}

/// The menu tree, derived from the action table.
pub fn build_menus(state: MenuState) -> Vec<Menu> {
    let mut menus = Vec::new();
    for group in MenuGroup::ALL {
        let mut items = Vec::new();
        push_group_items(*group, &mut items);
        if *group == MenuGroup::View {
            items.push(
                MenuItem::action("Live viewport", ToggleLiveViewport).checked(state.live_viewport),
            );
        }
        if *group == MenuGroup::Render {
            items.push(
                MenuItem::action("Raster (PBR + POM)", UseRasterMode).checked(!state.path_traced),
            );
            items.push(
                MenuItem::action("Path traced", UsePathTracedMode).checked(state.path_traced),
            );
            items.push(
                MenuItem::action("Request render certificate", ToggleCertificate)
                    .checked(state.certificate),
            );
        }
        menus.push(Menu {
            name: group.label().into(),
            items,
            disabled: false,
        });
    }
    menus
}

/// The shortcuts window content: one block per menu, label / keys pairs.
pub fn shortcut_rows() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    MenuGroup::ALL
        .iter()
        .map(|group| {
            let rows = ACTIONS
                .iter()
                .filter(|spec| spec.group == *group)
                .filter_map(|spec| {
                    spec.shortcut
                        .map(|_| (spec.label, spec.shortcut_label().unwrap_or_default()))
                })
                .collect();
            (group.label(), rows)
        })
        .filter(|(_, rows): &(&str, Vec<(&str, String)>)| !rows.is_empty())
        .collect()
}

/// A menu bar entity for the title bar.
pub fn menu_bar(cx: &mut App) -> Entity<AppMenuBar> {
    AppMenuBar::new(cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    // The gpui glob also re-exports GPUI's `test` attribute; keep the built-in one.
    use core::prelude::v1::test;
    use std::collections::HashSet;

    #[test]
    fn every_action_has_one_shortcut_and_one_menu_entry() {
        let mut seen = HashSet::new();
        for spec in ACTIONS {
            if let Some(keys) = spec.shortcut {
                assert!(seen.insert(keys), "duplicate shortcut {keys}");
            }
            assert!(
                !spec.label.is_empty() && !spec.label.ends_with('!'),
                "labels are verbs without exclamation marks: {}",
                spec.label
            );
            assert!(
                MenuGroup::ALL.contains(&spec.group),
                "{} is in no menu",
                spec.label
            );
        }
    }

    #[test]
    fn menu_tree_covers_every_group() {
        let menus = build_menus(MenuState::default());
        assert_eq!(menus.len(), MenuGroup::ALL.len());
        for (menu, group) in menus.iter().zip(MenuGroup::ALL) {
            assert_eq!(menu.name.as_ref(), group.label());
            assert!(
                menu.items
                    .iter()
                    .any(|item| matches!(item, MenuItem::Action { .. })),
                "{:?} has no actions",
                group
            );
        }
    }

    #[test]
    fn shortcut_labels_read_like_menus() {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.label == "Save project as…")
            .expect("save as is registered");
        assert_eq!(spec.shortcut_label().as_deref(), Some("Ctrl+Shift+S"));
    }
}
