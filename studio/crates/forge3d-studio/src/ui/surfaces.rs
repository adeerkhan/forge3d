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
/// A download carousel with per-card asset state; useful, but it is the first
/// thing a newcomer sees next to their layers. The Layers tab stays, and
/// `File ▸ Open example…` still reaches the gallery.
pub const GALLERY: bool = false;

/// Timeline / Console / Certificates strip.
///
/// Kept, because the console is where engine errors are shown verbatim and the
/// timeline is where a camera rig is played. Not a cluttered surface yet.
pub const BOTTOM_STRIP: bool = true;

/// The AETHER atmosphere, SIDERA sky and smoke-volume sections in World.
pub const ENVIRONMENT_EXTRAS: bool = false;

/// Certificate verification, determinism checks and sequence export.
pub const CERTIFICATE_TOOLS: bool = false;

/// Automatic low-resolution MapScene previews after an edit.
///
/// Off: a render the user did not ask for is the thing that made editing feel
/// slow. `Rendered` (or F12) asks for one still; the debounce and coalescing in
/// `schedule_preview` stay intact behind this switch.
pub const AUTO_PREVIEW: bool = false;
