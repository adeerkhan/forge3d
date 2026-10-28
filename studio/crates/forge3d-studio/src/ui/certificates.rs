//! Bottom-dock Certificates and the render-quality panel (GUI.md G6).
//!
//! Certificates are the engine's signed attestation of a render. Studio lists
//! the ones this session produced, shows the payload hash, and asks the engine
//! to verify one — a tampered file must come back rejected, not "probably fine".

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use studio_project::RenderMode;

use crate::state::Studio;

pub fn certificates(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (signing, certificates, last_export) = {
        let state = studio.read(cx);
        (
            state.sign_renders,
            state.certificates.clone(),
            state.last_export.clone(),
        )
    };

    let studio_sign = studio.clone();
    let studio_refresh = studio.clone();

    let mut body = v_flex().w_full().p_2().gap_2();
    body = body.child(
        h_flex()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Checkbox::new("certificate-signing")
                            .checked(signing)
                            .on_click(move |checked, _, cx| {
                                studio_sign
                                    .update(cx, |studio, cx| studio.set_sign_renders(*checked, cx));
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Sign every render (Render ▸ Request certificate)"),
                    ),
            )
            .child(
                Button::new("certs-refresh")
                    .label("Diagnostics snapshot")
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        studio_refresh.update(cx, |studio, cx| studio.refresh_diagnostics(cx));
                    }),
            ),
    );

    if certificates.is_empty() {
        body = body.child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(
                    "No certificates yet — turn on signing, then render. A certificate is \
                     written next to the frame (or the export) it attests.",
                ),
        );
    }

    for (index, certificate) in certificates.iter().enumerate() {
        let studio_verify = studio.clone();
        let verified = certificate.verified;
        body = body.child(
            h_flex()
                .justify_between()
                .gap_2()
                .py(px(2.))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(if matches!(verified, Some(true)) {
                            IconName::CircleCheck
                        } else if matches!(verified, Some(false)) {
                            IconName::CircleX
                        } else {
                            IconName::Shield
                        })
                        .child(div().text_xs().child(certificate.source.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(
                                    certificate
                                        .payload_sha256
                                        .as_deref()
                                        .map(|sha| sha.chars().take(20).collect::<String>())
                                        .unwrap_or_else(|| "no payload hash".to_owned()),
                                ),
                        ),
                )
                .child(match verified {
                    Some(true) => div()
                        .text_xs()
                        .text_color(cx.theme().colors.success)
                        .child("verified")
                        .into_any_element(),
                    Some(false) => div()
                        .text_xs()
                        .text_color(cx.theme().colors.danger)
                        .child("INVALID")
                        .into_any_element(),
                    None => Button::new(format!("cert-verify-{index}"))
                        .label("Verify")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_verify
                                .update(cx, |studio, cx| studio.verify_certificate(index, cx));
                        })
                        .into_any_element(),
                }),
        );
    }

    if let Some(path) = last_export {
        body = body.child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(format!("Last export: {}", path.display())),
        );
    }

    body.into_any_element()
}

/// Right-dock Render panel: the still's quality, mode and trust switches.
pub fn render(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (quality, caps, determinism, progress, stale) = {
        let state = studio.read(cx);
        (
            state.project.render_quality.clone(),
            state.offline_caps.clone(),
            state.determinism.clone(),
            state.progress.map(|fraction| fraction * 100.0),
            state.viewport_dirty,
        )
    };

    let tracing_available = caps
        .as_ref()
        .and_then(|caps| caps.get("path_tracing"))
        .and_then(|value| value.as_bool())
        .unwrap_or(true);

    let studio_raster = studio.clone();
    let studio_traced = studio.clone();
    let studio_samples_dec = studio.clone();
    let studio_samples_inc = studio.clone();
    let studio_offline = studio.clone();
    let studio_spp_dec = studio.clone();
    let studio_spp_inc = studio.clone();
    let studio_frames_dec = studio.clone();
    let studio_frames_inc = studio.clone();
    let studio_variance_dec = studio.clone();
    let studio_variance_inc = studio.clone();
    let studio_seed = studio.clone();
    let studio_env_dec = studio.clone();
    let studio_env_inc = studio.clone();
    let studio_still = studio.clone();
    let studio_determinism = studio.clone();
    let studio_caps = studio.clone();

    let denoisers: Vec<String> = caps
        .as_ref()
        .and_then(|caps| caps.get("denoisers"))
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| vec![quality.offline.denoiser.clone()]);

    let mut denoiser_row = h_flex().gap_1();
    for denoiser in &denoisers {
        let selected = &quality.offline.denoiser == denoiser;
        let studio_pick = studio.clone();
        let name = denoiser.clone();
        denoiser_row = denoiser_row.child(
            Button::new(format!("denoiser-{name}"))
                .label(name.clone())
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.outline())
                .on_click(move |_, _, cx| {
                    let name = name.clone();
                    studio_pick.update(cx, |studio, cx| studio.set_denoiser(name, cx));
                }),
        );
    }

    let mut body = v_flex().w_full().p_2().gap_3();

    body = body.child(
        v_flex()
            .gap_1()
            .child(group_label("Render mode", cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("mode-raster")
                            .label("Raster (PBR + POM)")
                            .xsmall()
                            .when(quality.mode == RenderMode::Raster, |button| {
                                button.primary()
                            })
                            .when(quality.mode != RenderMode::Raster, |button| button.outline())
                            .on_click(move |_, _, cx| {
                                studio_raster.update(cx, |studio, cx| {
                                    studio.set_render_mode(RenderMode::Raster, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new("mode-traced")
                            .label("Path traced")
                            .xsmall()
                            .when(quality.mode == RenderMode::PathTraced, |button| {
                                button.primary()
                            })
                            .when(quality.mode != RenderMode::PathTraced, |button| {
                                button.outline()
                            })
                            .on_click(move |_, _, cx| {
                                studio_traced.update(cx, |studio, cx| {
                                    studio.set_render_mode(RenderMode::PathTraced, cx)
                                });
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if tracing_available {
                        cx.theme().colors.muted_foreground
                    } else {
                        cx.theme().colors.warning
                    })
                    .child(if tracing_available {
                        "A quality render is a still: camera motion marks it stale, Ctrl+R draws it."
                            .to_owned()
                    } else {
                        "This forge3d build has no native hybrid tracer; path tracing is unavailable."
                            .to_owned()
                    }),
            ),
    );

    if quality.mode == RenderMode::Raster {
        body = body.child(
            v_flex()
                .gap_1()
                .child(group_label("Offline accumulation", cx))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Checkbox::new("offline-enabled")
                                .checked(quality.offline.enabled)
                                .on_click(move |checked, _, cx| {
                                    studio_offline.update(cx, |studio, cx| {
                                        studio.set_offline_enabled(*checked, cx)
                                    });
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .child(format!("{} samples", quality.offline.samples)),
                        )
                        .child(
                            Button::new("samples-dec")
                                .label("−")
                                .xsmall()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    studio_samples_dec.update(cx, |studio, cx| {
                                        studio.step_offline_samples(-4, cx)
                                    });
                                }),
                        )
                        .child(
                            Button::new("samples-inc")
                                .label("+")
                                .xsmall()
                                .ghost()
                                .on_click(move |_, _, cx| {
                                    studio_samples_inc.update(cx, |studio, cx| {
                                        studio.step_offline_samples(4, cx)
                                    });
                                }),
                        ),
                )
                .child(denoiser_row),
        );
    } else {
        let trace = &quality.path_trace;
        body = body.child(
            v_flex()
                .gap_1()
                .child(group_label("Path tracing", cx))
                .child(numeric_row(
                    "Samples / pixel",
                    format!("{}", trace.spp),
                    studio_spp_dec,
                    |studio, cx| studio.step_path_trace(-1, 0, 1.0, 1.0, 0, cx),
                    studio_spp_inc,
                    |studio, cx| studio.step_path_trace(1, 0, 1.0, 1.0, 0, cx),
                ))
                .child(numeric_row(
                    "Min frames",
                    format!("{}", trace.min_frames),
                    studio_frames_dec,
                    |studio, cx| studio.step_path_trace(0, -1, 1.0, 1.0, 0, cx),
                    studio_frames_inc,
                    |studio, cx| studio.step_path_trace(0, 1, 1.0, 1.0, 0, cx),
                ))
                .child(numeric_row(
                    "Variance target",
                    format!("{:.3}", trace.variance_threshold),
                    studio_variance_dec,
                    |studio, cx| studio.step_path_trace(0, 0, 0.8, 1.0, 0, cx),
                    studio_variance_inc,
                    |studio, cx| studio.step_path_trace(0, 0, 1.25, 1.0, 0, cx),
                ))
                .child(numeric_row(
                    "Seed",
                    format!("{}", trace.seed),
                    studio_seed.clone(),
                    |studio, cx| studio.step_path_trace(0, 0, 1.0, 1.0, 1, cx),
                    studio_seed,
                    |studio, cx| studio.step_path_trace(0, 0, 1.0, 1.0, -1, cx),
                ))
                .child(numeric_row(
                    "Environment",
                    format!("{:.2}", trace.env_intensity),
                    studio_env_dec,
                    |studio, cx| studio.step_path_trace(0, 0, 1.0, 1.0 / 1.1, 0, cx),
                    studio_env_inc,
                    |studio, cx| studio.step_path_trace(0, 0, 1.0, 1.1, 0, cx),
                )),
        );
    }

    body = body
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new("render-still")
                        .label("Render still")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_still.update(cx, |studio, cx| studio.request_still(cx));
                        }),
                )
                .child(
                    Button::new("render-caps")
                        .label("Refresh capabilities")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            studio_caps.update(cx, |studio, cx| studio.refresh_capabilities(cx));
                        }),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(match progress {
                    Some(percent) => format!("Rendering… {percent:.0}%"),
                    None if stale => "Still is stale — Ctrl+R draws the current view".to_owned(),
                    None => "Still is current".to_owned(),
                }),
        )
        .child(
            v_flex()
                .gap_1()
                .child(group_label("Determinism", cx))
                .child(
                    Button::new("determinism-check")
                        .label("Check twice, compare SHA-256")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_determinism
                                .update(cx, |studio, cx| studio.run_determinism_check(cx));
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(match determinism {
                            Some(report) if report.matched => format!(
                                "identical: {}… (two runs, one process, one backend)",
                                report.sha_a.chars().take(16).collect::<String>()
                            ),
                            Some(report) => format!(
                                "DIFFERENT: {}… vs {}…",
                                report.sha_a.chars().take(12).collect::<String>(),
                                report.sha_b.chars().take(12).collect::<String>()
                            ),
                            None => "No check run yet.".to_owned(),
                        }),
                ),
        );

    body.into_any_element()
}

fn group_label(text: &'static str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn numeric_row(
    title: &'static str,
    value: String,
    dec_entity: Entity<Studio>,
    dec: fn(&mut Studio, &mut Context<Studio>),
    inc_entity: Entity<Studio>,
    inc: fn(&mut Studio, &mut Context<Studio>),
) -> AnyElement {
    h_flex()
        .justify_between()
        .gap_2()
        .child(div().text_xs().child(title))
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    Button::new(format!("num-dec-{title}"))
                        .label("−")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            dec_entity.update(cx, dec);
                        }),
                )
                .child(div().w(px(48.)).text_xs().text_center().child(value))
                .child(
                    Button::new(format!("num-inc-{title}"))
                        .label("+")
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            inc_entity.update(cx, inc);
                        }),
                ),
        )
        .into_any_element()
}
