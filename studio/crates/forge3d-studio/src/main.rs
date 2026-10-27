//! Forge3D Studio entry point.

mod actions;
mod app;
mod icons;
mod ui;

use eframe::NativeOptions;
use egui::ViewportBuilder;

fn main() -> Result<(), eframe::Error> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    log::info!("Forge3D Studio {} starting", env!("CARGO_PKG_VERSION"));

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([1500.0, 950.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_title("Forge3D Studio"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Forge3D Studio",
        options,
        Box::new(|cc| Box::new(app::StudioApp::new(cc))),
    )
}
