//! Window-level UI tests: a real window, a real element tree, real clicks.
//!
//! These run headless through `gpui`'s own `TestAppContext` — not
//! `gpui-kit/test-support`, which pulls a parallel GPUI that cannot resolve
//! against the runtime the app uses (it pins `taffy =0.15.0` and
//! `unicode-properties =0.1.3` against `gpui-pre`'s `^` ranges).
//!
//! No screenshots: `window.render_to_image` needs a `PlatformHeadlessRenderer`
//! and the test platform only wires one for Metal; on this Linux box the call
//! fails with "render_to_image not available: no HeadlessRenderer configured".
//! So a window test can assert behaviour, not appearance — for the look of a
//! panel, run the app.
//!
//! The studio ones need a GPU and the engine's Python worker, so they are
//! gated on `FORGE3D_STUDIO_UI_TESTS=1`: a machine without an adapter must not
//! fail the suite, but a machine with one should prove the interaction.

use gpui::prelude::*;
use gpui::{div, px, size, TestAppContext};

use forge3d_studio::state::Studio;

/// Two Studios in one process means two engine devices and two Python workers
/// compiling shaders at once, which starves whichever test loses the race. The
/// studio tests take turns.
static STUDIO_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ui_tests_enabled() -> bool {
    std::env::var_os("FORGE3D_STUDIO_UI_TESTS").is_some()
}

#[gpui::test]
fn a_window_opens_and_paints(cx: &mut TestAppContext) {
    struct Shell;
    impl gpui::Render for Shell {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div().size_full().child("hello")
        }
    }
    cx.open_window(size(px(400.), px(300.)), |_, _| Shell);
    cx.run_until_parked();
}

#[gpui::test]
fn the_studio_window_builds_and_paints_a_frame(cx: &mut TestAppContext) {
    let _turn = STUDIO_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !ui_tests_enabled() {
        eprintln!("skipped: set FORGE3D_STUDIO_UI_TESTS=1 (needs a GPU adapter)");
        return;
    }
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .is_test(true)
        .try_init();
    // The app's own bootstrap: the kit's globals and the action table have to
    // exist before a Studio can be constructed, exactly as `main` does it.
    cx.update(|cx| {
        gpui_kit::init(cx);
        forge3d_studio::actions::init(cx);
    });
    let handle = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
        Studio::new(window, cx)
    });
    // The viewport worker starts on its own thread, so wait for it rather than
    // assuming the first parked tick is enough.
    let mut ready = false;
    for _ in 0..150 {
        // The studio's pump is a 16 ms timer on the background executor; in a
        // test that clock is virtual, so it has to be advanced by hand while
        // the worker thread gets real time to start.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        ready = cx.update(|cx| {
            handle
                .read(cx)
                .map(|studio| studio.viewport_ready)
                .unwrap_or(false)
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(ready, "the viewport worker should have initialised");
    // The library, not just the binary, must ask for the discrete adapter.
    assert_eq!(
        std::env::var("FORGE3D_GPU_POWER").ok().as_deref(),
        Some("high"),
        "constructing a Studio must request the high-performance adapter"
    );
}

#[gpui::test]
fn a_view_action_reaches_the_state_through_the_action_table(cx: &mut TestAppContext) {
    let _turn = STUDIO_TESTS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !ui_tests_enabled() {
        eprintln!("skipped: set FORGE3D_STUDIO_UI_TESTS=1 (needs a GPU adapter)");
        return;
    }
    cx.update(|cx| {
        gpui_kit::init(cx);
        forge3d_studio::actions::init(cx);
    });
    let handle = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
        Studio::new(window, cx)
    });

    let before = cx.update(|cx| handle.read(cx).map(|studio| studio.project.camera).ok());
    let before = before.expect("studio state");

    cx.dispatch_action(handle.into(), forge3d_studio::actions::ToggleOrtho);
    cx.run_until_parked();
    let ortho = cx.update(|cx| {
        handle
            .read(cx)
            .map(|studio| (studio.ortho, studio.project.camera.fov_deg))
            .ok()
    });
    let (enabled, fov) = ortho.expect("studio state");
    assert!(enabled, "the action switched to orthographic");
    assert!(
        (fov - 2.0).abs() < 1e-3,
        "ortho is a 2 degree field of view"
    );

    cx.dispatch_action(handle.into(), forge3d_studio::actions::ToggleOrtho);
    cx.run_until_parked();
    let after = cx.update(|cx| {
        handle
            .read(cx)
            .map(|studio| (studio.ortho, studio.project.camera))
            .ok()
    });
    let (enabled, camera) = after.expect("studio state");
    assert!(!enabled, "the action went back to perspective");
    assert!(
        (camera.fov_deg - before.fov_deg).abs() < 1e-3,
        "perspective restores the field of view"
    );
    assert!(
        (camera.radius - before.radius).abs() / before.radius.max(1.0) < 0.01,
        "and the framing with it"
    );
}
