//! Design tokens — the single place the Studio look is defined.
//!
//! GUI.md III.8: dark, dense, neutral chrome, one accent used only for
//! selection/focus/primary actions, monospace for paths and numbers. Panels
//! and widgets read from [`Tokens`] instead of hard-coding sizes, so density
//! and colour stay consistent as panels are added.

use egui::{Color32, Context, FontFamily, FontId, Rounding, Stroke, TextStyle, Vec2, Visuals};

/// Logical-pixel layout and colour tokens.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    /// Selection, focus and primary-action colour.
    pub accent: Color32,
    /// Viewport backdrop (darker than the panels so the image reads as content).
    pub viewport: Color32,
    /// Panel background.
    pub panel: Color32,
    /// Row height in lists (layers, jobs).
    pub row_h: f32,
    /// Icon size in tool buttons and list rows.
    pub icon: f32,
    /// Smallest a dock slot may be dragged to.
    pub dock_min: f32,
}

impl Tokens {
    pub fn dark() -> Self {
        Self {
            accent: Color32::from_rgb(88, 156, 255),
            viewport: Color32::from_rgb(10, 10, 12),
            panel: Color32::from_rgb(30, 31, 34),
            row_h: 22.0,
            icon: 16.0,
            dock_min: 180.0,
        }
    }

    pub fn light() -> Self {
        Self {
            accent: Color32::from_rgb(19, 96, 210),
            viewport: Color32::from_rgb(228, 228, 230),
            panel: Color32::from_rgb(242, 242, 244),
            row_h: 22.0,
            icon: 16.0,
            dock_min: 180.0,
        }
    }

    pub fn for_theme(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

/// Apply the Studio style for `dark` (or light) to the whole context.
pub fn apply(ctx: &Context, dark: bool) {
    let tokens = Tokens::for_theme(dark);
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = tokens.panel;
    visuals.window_fill = tokens.panel;
    visuals.extreme_bg_color = tokens.viewport;
    visuals.selection.bg_fill = tokens.accent.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0_f32, tokens.accent);
    visuals.hyperlink_color = tokens.accent;
    visuals.widgets.hovered.bg_fill = if dark {
        Color32::from_rgb(44, 46, 51)
    } else {
        Color32::from_rgb(226, 228, 232)
    };
    visuals.widgets.active.bg_fill = tokens.accent.gamma_multiply(0.35);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(
        1.0_f32,
        if dark {
            Color32::from_rgb(48, 50, 55)
        } else {
            Color32::from_rgb(206, 208, 212)
        },
    );
    visuals.window_rounding = Rounding::same(6.0);
    visuals.menu_rounding = Rounding::same(6.0);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.rounding = Rounding::same(4.0);
    }
    ctx.set_visuals(visuals);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(6.0, 4.0);
        style.spacing.button_padding = Vec2::new(6.0, 3.0);
        style.spacing.menu_margin = egui::Margin::same(6.0);
        style.spacing.indent = 14.0;
        style.spacing.interact_size = Vec2::new(20.0, tokens.row_h);
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, FontId::proportional(15.0)),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// A dimmed label used for metadata inside panels.
pub fn meta(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(11.5).weak()
}

/// The small caps-style group label used above groups of controls.
pub fn group_label(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).size(11.0).strong()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_stay_in_sync_between_themes() {
        let dark = Tokens::dark();
        let light = Tokens::light();
        // Density is a property of the app, not of the theme.
        assert_eq!(dark.row_h, light.row_h);
        assert_eq!(dark.icon, light.icon);
        assert_ne!(dark.panel, light.panel);
        assert_ne!(dark.viewport, light.viewport);
    }

    #[test]
    fn applying_a_theme_sets_spacing_and_panel_fill() {
        let ctx = Context::default();
        apply(&ctx, true);
        let dark_panel = Tokens::dark().panel;
        assert_eq!(ctx.style().visuals.panel_fill, dark_panel);
        apply(&ctx, false);
        assert_eq!(ctx.style().visuals.panel_fill, Tokens::light().panel);
        assert_eq!(ctx.style().spacing.item_spacing.y, 4.0);
    }
}
