//! Forge3D Studio, as a library plus a thin binary.
//!
//! The modules live here rather than in `main.rs` so integration tests can
//! build a real window and drive it (`tests/ui_window.rs`); the binary only
//! parses arguments and opens the window.

pub mod actions;
pub mod state;
pub mod ui;

/// Startup clock: from process entry to the first painted frame, reported in
/// the console and the Statistics panel so the budget is visible.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// The workbench wants the discrete GPU.
///
/// The engine defaults to `PowerPreference::LowPower`, so on a dual-adapter
/// machine the viewport renders on the integrated adapter while the UI
/// composites on the discrete one and every frame crosses PCIe (measured: first
/// frame 88-195 ms against 16.9 ms). Any explicit user choice wins.
///
/// This lives here, not in `main`, because anything that constructs a `Studio`
/// has to set it before the first engine context exists — a test run that only
/// called `Studio::new` was silently rendering on the iGPU.
pub fn ensure_gpu_power_preference() {
    if std::env::var_os("FORGE3D_GPU_POWER").is_none() {
        std::env::set_var("FORGE3D_GPU_POWER", "high");
    }
}
