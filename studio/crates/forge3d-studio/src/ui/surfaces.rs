//! Which surfaces the shell shows in this pass.
//!
//! The app grew faster than it was read. These switches hide whole surfaces
//! without deleting anything: the code stays compiled (so it cannot rot into
//! something that no longer type-checks) and flipping a flag brings it back.
//!
//! Prefer a switch over commented-out code: commented-out code is never
//! compiled, so it silently stops matching the rest of the tree, and nobody
//! notices until the day it is needed.

/// The right-hand panel's icon strip.
///
/// It is a second navigation for the same pages the workspace tabs already
/// open — `Data` and `Layout` even pointed at the same page. The tabs stay (one
/// navigation, and `Data` now opens Statistics, which the strip used to own).
pub const PROPERTIES_STRIP: bool = false;

/// The examples gallery in the outliner.
///
/// On again, but the list defaults to **runnable-only**: an example that cannot
/// run today is a promise, not a card. The catalogue keeps every entry (the
/// contract test still requires it); the gallery just stops showing the ones
/// that need a milestone.
pub const GALLERY: bool = true;

/// Certificate verification, determinism checks and sequence export in the
/// Render page.
///
/// The page's own controls (output size, quality, render mode) are core and stay;
/// what is hidden is the machinery around the artifact — verifying a signature,
/// re-running determinism, writing 240 PNG frames. None of it is exercised while
/// the look of the app is still changing.
pub const CERTIFICATE_TOOLS: bool = false;

/// Automatic low-resolution MapScene previews after an edit.
///
/// Off: a render the user did not ask for is the thing that made editing feel
/// slow. `Rendered` (or F12) asks for one still; the debounce and coalescing in
/// `schedule_preview` stay intact behind this switch.
pub const AUTO_PREVIEW: bool = false;
