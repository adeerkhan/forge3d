//! Lucide-style icon support.
//!
//! Icons are SVG assets (see `assets/icons/LICENSE.md`), rasterized at startup
//! with resvg and uploaded as egui textures. `currentColor` in each asset is
//! replaced with the requested foreground color so icons follow the active
//! theme. Icons are rasterized at 2x the logical size for crispness.
//!
//! The set is a typed [`Icon`] enum: a menu or a layer row cannot name an icon
//! that does not exist, and a test asserts every variant rasterizes.

use std::collections::HashMap;

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

const ICON_SIZE_PX: u32 = 32;

macro_rules! icon_set {
    ($(($variant:ident, $name:literal)),+ $(,)?) => {
        /// One bundled glyph. Variant names are the API; the asset name is
        /// what the licence ledger refers to.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant),+
        }

        impl Icon {
            pub fn name(self) -> &'static str {
                match self {
                    $(Icon::$variant => $name),+
                }
            }
        }

        const ICON_SOURCES: &[(Icon, &str)] = &[
            $(
                (
                    Icon::$variant,
                    include_str!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../assets/icons/",
                        $name,
                        ".svg"
                    )),
                ),
            )+
        ];
    };
}

icon_set![
    (FolderOpen, "folder-open"),
    (Save, "save"),
    (FileInput, "file-input"),
    (FileOutput, "file-output"),
    (Layers, "layers"),
    (SlidersHorizontal, "sliders-horizontal"),
    (Camera, "camera"),
    (Orbit, "orbit"),
    (Image, "image"),
    (Settings2, "settings-2"),
    (Activity, "activity"),
    (Cpu, "cpu"),
    (CircleAlert, "circle-alert"),
    (CircleCheck, "circle-check"),
    (MountainSnow, "mountain-snow"),
    (Sun, "sun"),
    (RotateCcw, "rotate-ccw"),
    (RotateCw, "rotate-cw"),
    (Eye, "eye"),
    (EyeOff, "eye-off"),
    (Lock, "lock"),
    (LockOpen, "lock-open"),
    (Folder, "folder"),
    (FolderPlus, "folder-plus"),
    (Type, "type"),
    (Box, "box"),
    (Building, "building"),
    (Cloud, "cloud"),
    (Spline, "spline"),
    (Table, "table"),
    (Compass, "compass"),
    (Plus, "plus"),
    (Trash, "trash"),
    (Copy, "copy"),
    (GripVertical, "grip-vertical"),
];

/// Rasterized icon set keyed by [`Icon`].
pub struct Icons {
    textures: HashMap<Icon, TextureHandle>,
}

impl Icons {
    /// Rasterize every bundled icon in `color` and upload them to `ctx`.
    pub fn load(ctx: &egui::Context, color: Color32) -> Self {
        let mut textures = HashMap::new();
        for (icon, svg) in ICON_SOURCES {
            match rasterize(svg, color) {
                Ok(image) => {
                    let handle = ctx.load_texture(
                        format!("icon-{}-{color:?}", icon.name()),
                        image,
                        TextureOptions::LINEAR,
                    );
                    textures.insert(*icon, handle);
                }
                Err(err) => {
                    log::warn!("failed to rasterize icon '{}': {err}", icon.name());
                }
            }
        }
        Self { textures }
    }

    /// Texture for an icon; falls back to a transparent placeholder if the
    /// asset failed to rasterize (a broken asset must not panic a frame).
    pub fn texture(&self, ctx: &egui::Context, icon: Icon) -> TextureHandle {
        self.textures.get(&icon).cloned().unwrap_or_else(|| {
            ctx.load_texture(
                "icon-placeholder",
                ColorImage::from_rgba_unmultiplied([4, 4], &[0, 0, 0, 0]),
                TextureOptions::LINEAR,
            )
        })
    }
}

fn rasterize(svg: &str, color: Color32) -> Result<ColorImage, String> {
    let colored = svg.replace("currentColor", &hex_color(color));
    let options = usvg_options();
    let tree = usvg::Tree::from_str(&colored, &options).map_err(|err| err.to_string())?;

    let mut pixmap = tiny_skia::Pixmap::new(ICON_SIZE_PX, ICON_SIZE_PX)
        .ok_or_else(|| "failed to allocate icon pixmap".to_owned())?;
    // Lucide assets have a 24x24 viewBox; scale to the raster size.
    let scale = ICON_SIZE_PX as f32 / 24.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let pixels = pixmap.take();
    Ok(ColorImage::from_rgba_unmultiplied(
        [ICON_SIZE_PX as usize, ICON_SIZE_PX as usize],
        &pixels,
    ))
}

fn usvg_options() -> usvg::Options<'static> {
    usvg::Options::default()
}

fn hex_color(color: Color32) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        color.r(),
        color.g(),
        color.b(),
        color.a()
    )
}

// resvg re-exports usvg and tiny-skia; pin callers to the same versions.
pub use resvg::tiny_skia;
pub use resvg::usvg;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_asset_rasterizes() {
        let ctx = egui::Context::default();
        let icons = Icons::load(&ctx, Color32::WHITE);
        for (icon, _) in ICON_SOURCES {
            assert!(
                icons.textures.contains_key(icon),
                "icon '{}' is missing or malformed",
                icon.name()
            );
        }
        assert_eq!(icons.textures.len(), ICON_SOURCES.len());
    }

    #[test]
    fn icon_names_are_unique_and_non_empty() {
        let mut names = std::collections::HashSet::new();
        for (icon, _) in ICON_SOURCES {
            assert!(names.insert(icon.name()), "duplicate asset {}", icon.name());
            assert!(!icon.name().is_empty());
        }
    }
}
