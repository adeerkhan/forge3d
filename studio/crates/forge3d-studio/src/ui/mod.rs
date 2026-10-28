//! The UI shell: one generic panel host, the dock layout, and the panels.

pub mod certificates;
pub mod layers;
pub mod panels;
pub mod params;
pub mod shell;
pub mod timeline;
pub mod viewport;

use std::rc::Rc;

use gpui_kit::component::dock::{
    panel_handle, BasePanel, DockArea, DockLayout, DockPlacement, DockSkin, Panel as StyledPanel,
    PanelEvent,
};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::*;

/// What a panel draws: a function of application state.
type PanelContent = dyn Fn(&mut Window, &mut App) -> AnyElement;

/// The saved dock layout's schema version. Bump when the panel set changes in
/// a way an old layout cannot express; a saved layout from another version is
/// ignored and the default layout is built.
pub const DOCK_LAYOUT_VERSION: usize = 2;

/// Panel ids, in default-layout order.
pub const PANEL_IDS: &[&str] = &[
    "studio.layers",
    "studio.inspector",
    "studio.render",
    "studio.console",
    "studio.statistics",
    "studio.timeline",
    "studio.certificates",
    "studio.viewport",
];

/// The `Studio` entity, published as an app global so a restored dock layout can
/// rebuild its panels: the kit's panel registry is process-wide and rebuilds
/// panels by name, which needs a way back to application state.
#[derive(Clone)]
pub struct StudioHandle(pub WeakEntity<crate::state::Studio>);

impl Global for StudioHandle {}

/// How a panel draws itself, given the app state.
type PanelFn = fn(&WeakEntity<crate::state::Studio>, &mut Window, &mut App) -> AnyElement;

/// Title and content for a panel id: the single place the default layout and
/// the restored-layout registry both read.
pub fn panel_spec(id: &str) -> Option<(&'static str, PanelFn)> {
    match id {
        "studio.layers" => Some(("Layers", panels::layers)),
        "studio.inspector" => Some(("Inspector", panels::inspector)),
        "studio.render" => Some(("Render", render_panel)),
        "studio.timeline" => Some(("Timeline", timeline_panel)),
        "studio.certificates" => Some(("Certificates", certificates_panel)),
        "studio.console" => Some(("Console", panels::console)),
        "studio.statistics" => Some(("Statistics", panels::statistics)),
        "studio.viewport" => Some(("Viewport", viewport::viewport_panel)),
        _ => None,
    }
}

fn render_panel(
    studio: &WeakEntity<crate::state::Studio>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match studio.upgrade() {
        Some(entity) => certificates::render(&entity, window, cx),
        None => div().into_any_element(),
    }
}

fn timeline_panel(
    studio: &WeakEntity<crate::state::Studio>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match studio.upgrade() {
        Some(entity) => timeline::timeline(&entity, window, cx),
        None => div().into_any_element(),
    }
}

fn certificates_panel(
    studio: &WeakEntity<crate::state::Studio>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match studio.upgrade() {
        Some(entity) => certificates::certificates(&entity, window, cx),
        None => div().into_any_element(),
    }
}

/// Build one panel by id, for the default layout and for a restored one.
pub fn make_panel(
    id: &'static str,
    studio: WeakEntity<crate::state::Studio>,
    cx: &mut App,
) -> Option<Entity<PanelHost>> {
    let (title, draw) = panel_spec(id)?;
    Some(PanelHost::new(id, title, cx, move |window, cx| {
        draw(&studio, window, cx)
    }))
}

/// Register the panel builders a saved layout needs to come back.
pub fn register_panels(cx: &mut App) {
    for id in PANEL_IDS {
        gpui_kit::component::dock::register_panel(cx, id, move |_context, _window, cx| {
            let studio = cx
                .try_global::<StudioHandle>()
                .map(|handle| handle.0.clone());
            let panel = studio
                .and_then(|studio| make_panel(id, studio, cx))
                .expect("every registered panel id has a spec");
            panel_handle(panel)
        });
    }
}

/// A dock panel that draws whatever closure it was built with.
///
/// Studio panels are functions of application state, not independent widgets:
/// they read the [`Studio`](crate::state::Studio) entity and dispatch actions
/// for edits. One host type keeps the dock's `Panel`/`BasePanel` boilerplate in
/// one place instead of six.
pub struct PanelHost {
    id: &'static str,
    title: SharedString,
    focus: FocusHandle,
    content: Rc<PanelContent>,
}

impl PanelHost {
    pub fn new(
        id: &'static str,
        title: impl Into<SharedString>,
        cx: &mut App,
        content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Entity<Self> {
        cx.new(|cx| Self {
            id,
            title: title.into(),
            focus: cx.focus_handle(),
            content: Rc::new(content),
        })
    }
}

impl Focusable for PanelHost {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for PanelHost {}

impl BasePanel for PanelHost {
    fn panel_name(&self) -> &'static str {
        self.id
    }

    /// Studio panels are part of the workbench, not documents: they cannot be
    /// closed, only collapsed.
    fn closable(&self, _: &App) -> bool {
        false
    }
}

impl StyledPanel for PanelHost {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
}

impl Render for PanelHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.id)
            .size_full()
            .overflow_y_scroll()
            .track_focus(&self.focus)
            .child((self.content)(window, cx))
    }
}

/// Build the dock: Layers (left), Inspector (right), Console/Statistics
/// (bottom) and the viewport in the centre.
pub fn build_dock(
    studio: WeakEntity<crate::state::Studio>,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<DockArea>, Rc<DockSkin>, Entity<AppMenuBar>) {
    let (dock_area, skin) =
        DockSkin::dock_area("studio-dock", Some(DOCK_LAYOUT_VERSION), window, cx);

    let mut panel = |id: &'static str| {
        make_panel(id, studio.clone(), cx).expect("every default panel has a spec")
    };
    let layers = panel("studio.layers");
    let inspector = panel("studio.inspector");
    let console = panel("studio.console");
    let statistics = panel("studio.statistics");
    let viewport = panel("studio.viewport");

    let render = panel("studio.render");
    let timeline = panel("studio.timeline");
    let certificates_panel = panel("studio.certificates");

    let left = DockLayout::tabs().panel_view(panel_handle(layers), cx);
    let right = DockLayout::tabs()
        .panel_view(panel_handle(inspector), cx)
        .panel_view(panel_handle(render), cx);
    let bottom = DockLayout::tabs()
        .panel_view(panel_handle(timeline), cx)
        .panel_view(panel_handle(certificates_panel), cx)
        .panel_view(panel_handle(console), cx)
        .panel_view(panel_handle(statistics), cx);
    let center = DockLayout::tabs().panel_view(panel_handle(viewport), cx);

    dock_area.update(cx, |area, cx| {
        area.set_center(center, window, cx);
        area.set_dock(DockPlacement::Left, left, window, cx);
        area.set_dock_size(DockPlacement::Left, px(260.), window, cx);
        area.set_dock(DockPlacement::Right, right, window, cx);
        area.set_dock_size(DockPlacement::Right, px(320.), window, cx);
        area.set_dock(DockPlacement::Bottom, bottom, window, cx);
        area.set_dock_size(DockPlacement::Bottom, px(180.), window, cx);
        for placement in [
            DockPlacement::Left,
            DockPlacement::Bottom,
            DockPlacement::Right,
        ] {
            area.set_dock_collapsible(placement, true, window, cx);
        }
    });

    let menu_bar = crate::actions::menu_bar(cx);
    (dock_area, skin, menu_bar)
}
