//! UI shell modules (GUI.md Part III).
//!
//! U0 splits the shell into modules with one job each: tokens, dock state and
//! chrome, the registry-driven menu/toolbar/palette/shortcut surfaces, and the
//! status bar. Panel bodies move here as they are rewritten in U1 (layers) and
//! U2 (inspector) — the render-parameter pages already live as a free function
//! so they can be rendered headlessly.

pub mod dock;
pub mod examples;
pub mod menu;
pub mod palette;
pub mod panels;
pub mod shortcuts;
pub mod status;
pub mod theme;
pub mod toolbar;
