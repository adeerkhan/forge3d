//! Forge3D Studio entry point.
//!
//! The UI is GPUI Kit (GPUI + gpui-base + gpui-component). The engine is driven
//! exactly as before: a bridge worker (embedded Python) for certified renders,
//! exports and certificates, and a viewport worker for the interactive frame.

mod actions;
mod state;
mod ui;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::state::Studio;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    log::info!("Forge3D Studio {} starting", env!("CARGO_PKG_VERSION"));

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            actions::init(cx);

            let bounds = Bounds::centered(None, size(px(1500.0), px(950.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(Size {
                    width: px(1000.0),
                    height: px(640.0),
                }),
                ..TitleBar::window_options()
            };

            // `forge3d-studio [scene.tif | project.forge3d]` opens that file.
            let initial = std::env::args().nth(1).map(std::path::PathBuf::from);

            gpui_kit::open_window(options, cx, |window, cx| {
                let studio = cx.new(|cx| Studio::new(window, cx));
                if let Some(path) = initial.clone() {
                    studio.update(cx, |studio, cx| studio.open_path(&path, cx));
                }
                window.activate_window();
                studio
            })
            .expect("open Forge3D Studio window");
        });
}
