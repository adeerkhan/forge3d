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

/// Startup clock: from process entry to the first painted frame, reported in
/// the console and the Statistics panel so the budget is visible.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    let _ = STARTED.set(std::time::Instant::now());

    // `--version` / `--help` print and exit: an executable a person launches
    // should answer those without opening a window.
    let mut initial: Option<std::path::PathBuf> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!("forge3d-studio {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => {
                println!(
                    "forge3d-studio {} — Forge3D Studio\n\n\
                     Usage: forge3d-studio [options] [scene.tif | project.forge3d]\n\n\
                     Arguments:\n  \
                     <file>            GeoTIFF DEM, .npy grid, .forge3d project or .f3dz\n\n\
                     Options:\n  \
                     -h, --help        Print this help and exit\n  \
                     -V, --version     Print the version and exit\n\n\
                     Environment:\n  \
                     FORGE3D_GPU_POWER=high|low   adapter preference (default: high)\n  \
                     RUST_LOG=info                enable the console log",
                    env!("CARGO_PKG_VERSION")
                );
                return;
            }
            other if initial.is_none() => initial = Some(std::path::PathBuf::from(other)),
            other => log::warn!("ignoring extra argument {other}"),
        }
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    // The workbench wants the discrete GPU: the engine's default LOW power
    // preference otherwise renders the viewport on the integrated adapter while
    // the UI composites on the discrete one, and every frame crosses PCIe.
    // Set before any engine context exists (the bridge and the viewport worker
    // both read it), and never override an explicit user choice.
    if std::env::var_os("FORGE3D_GPU_POWER").is_none() {
        std::env::set_var("FORGE3D_GPU_POWER", "high");
    }

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
