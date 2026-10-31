//! Forge3D Studio entry point.
//!
//! The UI is GPUI Kit (GPUI + gpui-base + gpui-component). The engine is driven
//! exactly as before: a bridge worker (embedded Python) for certified renders,
//! exports and certificates, and a viewport worker for the interactive frame.

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use forge3d_studio::state::Studio;

fn main() {
    let _ = forge3d_studio::STARTED.set(std::time::Instant::now());

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

    // Discrete GPU before any engine context exists (the library does this too
    // for anything that builds a Studio without going through `main`).
    forge3d_studio::ensure_gpu_power_preference();

    log::info!("Forge3D Studio {} starting", env!("CARGO_PKG_VERSION"));

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            forge3d_studio::actions::init(cx);

            // `FORGE3D_STUDIO_WINDOW=1920x1200` sets the initial size, so the
            // viewport can be measured at the sizes a target actually uses.
            let (window_width, window_height) = std::env::var("FORGE3D_STUDIO_WINDOW")
                .ok()
                .and_then(|value| {
                    let (w, h) = value.split_once('x')?;
                    Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
                })
                .unwrap_or((1500.0, 950.0));
            let bounds = Bounds::centered(
                None,
                size(px(window_width.max(1000.0)), px(window_height.max(640.0))),
                cx,
            );
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
