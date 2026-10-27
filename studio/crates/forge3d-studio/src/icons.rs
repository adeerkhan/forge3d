//! Lucide-style icon support.
//!
//! Icons are Lucide SVG assets (see `assets/icons/LICENSE.md`), rasterized at
//! startup with resvg and uploaded as egui textures. `currentColor` in each
//! asset is replaced with the requested foreground color so icons follow the
//! active theme. Icons are rasterized at 2x the logical size for crispness.

use std::collections::HashMap;

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

const ICON_SIZE_PX: u32 = 32;

const ICON_SOURCES: &[(&str, &str)] = &[
    (
        "folder-open",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/folder-open.svg"
        )),
    ),
    (
        "save",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/save.svg"
        )),
    ),
    (
        "file-input",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/file-input.svg"
        )),
    ),
    (
        "file-output",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/file-output.svg"
        )),
    ),
    (
        "layers",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/layers.svg"
        )),
    ),
    (
        "sliders-horizontal",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/sliders-horizontal.svg"
        )),
    ),
    (
        "camera",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/camera.svg"
        )),
    ),
    (
        "orbit",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/orbit.svg"
        )),
    ),
    (
        "image",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/image.svg"
        )),
    ),
    (
        "settings-2",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/settings-2.svg"
        )),
    ),
    (
        "activity",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/activity.svg"
        )),
    ),
    (
        "cpu",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/cpu.svg"
        )),
    ),
    (
        "circle-alert",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/circle-alert.svg"
        )),
    ),
    (
        "circle-check",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/circle-check.svg"
        )),
    ),
    (
        "mountain-snow",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/mountain-snow.svg"
        )),
    ),
    (
        "sun",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/sun.svg"
        )),
    ),
    (
        "rotate-ccw",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/icons/rotate-ccw.svg"
        )),
    ),
];

/// Rasterized icon set keyed by Lucide asset name.
pub struct Icons {
    textures: HashMap<&'static str, TextureHandle>,
}

impl Icons {
    /// Rasterize every bundled icon in `color` and upload them to `ctx`.
    pub fn load(ctx: &egui::Context, color: Color32) -> Self {
        let mut textures = HashMap::new();
        for (name, svg) in ICON_SOURCES {
            match rasterize(svg, color) {
                Ok(image) => {
                    let handle = ctx.load_texture(
                        format!("icon-{name}-{color:?}"),
                        image,
                        TextureOptions::LINEAR,
                    );
                    textures.insert(*name, handle);
                }
                Err(err) => {
                    log::warn!("failed to rasterize icon '{name}': {err}");
                }
            }
        }
        Self { textures }
    }

    /// Texture for a Lucide icon name; falls back to a placeholder if the
    /// asset failed to rasterize.
    pub fn get(&self, ctx: &egui::Context, name: &str) -> TextureHandle {
        self.textures.get(name).cloned().unwrap_or_else(|| {
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
