//! Dock-panel bodies. Each panel is a free function over borrowed state that
//! returns the commands the user asked for, so panels stay headlessly testable
//! and hold no scene state of their own.

pub mod layers;
