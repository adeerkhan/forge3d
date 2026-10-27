//! Status bar: adapter, frame time, selection, jobs and the latest message.

use egui::{Color32, Image, RichText, Ui};

use crate::icons::Icons;

/// Everything the status bar shows, passed by value so it stays a pure widget.
#[derive(Default)]
pub struct StatusParts<'a> {
    pub adapter: Option<&'a str>,
    pub backend: Option<&'a str>,
    pub software: bool,
    pub frame_ms: Option<f32>,
    pub busy: bool,
    pub selection: Option<&'a str>,
    pub status: Option<&'a str>,
    pub latest_error: Option<&'a str>,
    pub error_count: usize,
}

/// Draw the status bar; returns true when the user asked to clear the errors.
pub fn status_bar(ui: &mut Ui, icons: &Icons, parts: &StatusParts<'_>) -> bool {
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if let Some(adapter) = parts.adapter {
            let icon = icons.get(ui.ctx(), "cpu");
            ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(12.0, 12.0)));
            let label = match parts.backend {
                Some(backend) if parts.software => format!("{adapter} ({backend}, software)"),
                Some(backend) => format!("{adapter} ({backend})"),
                None => adapter.to_owned(),
            };
            let color = if parts.software {
                ui.visuals().warn_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(RichText::new(label).size(11.5).color(color))
                .on_hover_text("Active engine adapter (GPU device probe)");
            ui.separator();
        }
        if let Some(ms) = parts.frame_ms {
            ui.label(RichText::new(format!("{ms:.0} ms")).size(11.5).weak())
                .on_hover_text("Last viewport frame, engine time included");
        }
        if parts.busy {
            ui.add(egui::Spinner::new().size(11.0));
            ui.label(RichText::new("working…").size(11.5).weak());
        }
        if let Some(selection) = parts.selection {
            ui.separator();
            ui.label(RichText::new(selection).size(11.5));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(error) = parts.latest_error {
                if ui
                    .small_button("clear")
                    .on_hover_text(format!("Dismiss {} message(s)", parts.error_count))
                    .clicked()
                {
                    clear = true;
                }
                let icon = icons.get(ui.ctx(), "circle-alert");
                ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(12.0, 12.0)));
                ui.label(
                    RichText::new(error)
                        .size(11.5)
                        .color(Color32::from_rgb(230, 120, 120)),
                )
                .on_hover_text(error);
            } else if let Some(status) = parts.status {
                ui.label(RichText::new(status).size(11.5).weak());
                let icon = icons.get(ui.ctx(), "circle-check");
                ui.add(Image::new(&icon).fit_to_exact_size(egui::vec2(12.0, 12.0)));
            }
        });
    });
    clear
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_bar_renders_every_state_headlessly() {
        let ctx = egui::Context::default();
        let icons = Icons::load(&ctx, egui::Color32::WHITE);
        let states = [
            StatusParts::default(),
            StatusParts {
                adapter: Some("NVIDIA GeForce RTX 3070"),
                backend: Some("Vulkan"),
                frame_ms: Some(128.0),
                busy: true,
                selection: Some("Terrain"),
                ..Default::default()
            },
            StatusParts {
                latest_error: Some("msaa_samples must be 1, 2, 4, or 8"),
                error_count: 3,
                ..Default::default()
            },
        ];
        for parts in &states {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                    assert!(!status_bar(ui, &icons, parts));
                });
            });
        }
    }
}
