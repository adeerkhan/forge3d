//! Bottom-dock Timeline: the camera-rig bake, keyframe scrub and sequence
//! render (GUI.md G6).
//!
//! The rig itself is the engine's (`rig.bake` → `camera_rigs`), rescaled by the
//! bridge into renderer world units, so scrubbing here replays exactly what an
//! export would render.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::state::Studio;

pub fn timeline(studio: &Entity<Studio>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let (kind, duration, radius, playback_fps, keyframe_count, frame, playing, sequence) = {
        let state = studio.read(cx);
        (
            state.rig.kind.clone(),
            state.rig.duration,
            state.rig.radius_rel,
            state.rig.playback_fps,
            state.rig.keyframes.len(),
            state.rig.frame,
            state.rig.playing,
            state.sequence.as_ref().map(|sequence| {
                format!("{} / {} frames written", sequence.finished, sequence.total)
            }),
        )
    };

    let mut kinds = h_flex().gap_1();
    for option in ["orbit", "rail", "follow"] {
        let selected = kind == option;
        let studio_pick = studio.clone();
        kinds = kinds.child(
            Button::new(format!("rig-kind-{option}"))
                .label(option)
                .xsmall()
                .when(selected, |button| button.primary())
                .when(!selected, |button| button.outline())
                .on_click(move |_, _, cx| {
                    let option = option.to_owned();
                    studio_pick.update(cx, |studio, cx| studio.set_rig_kind(option, cx));
                }),
        );
    }

    let studio_bake = studio.clone();
    let studio_duration_dec = studio.clone();
    let studio_duration_inc = studio.clone();
    let studio_fps_dec = studio.clone();
    let studio_fps_inc = studio.clone();
    let studio_radius_dec = studio.clone();
    let studio_radius_inc = studio.clone();
    let studio_prev = studio.clone();
    let studio_next = studio.clone();
    let studio_play = studio.clone();
    let studio_sequence = studio.clone();

    v_flex()
        .w_full()
        .p_2()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(label("Rig", cx))
                .child(kinds)
                .child(
                    Button::new("rig-bake")
                        .label("Bake rig")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_bake.update(cx, |studio, cx| studio.bake_rig(cx));
                        }),
                ),
        )
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(stepper(
                    "rig-duration",
                    "Duration",
                    format!("{duration:.1} s"),
                    studio_duration_dec,
                    |studio, cx| studio.step_rig_duration(-1.0, cx),
                    studio_duration_inc,
                    |studio, cx| studio.step_rig_duration(1.0, cx),
                    cx,
                ))
                .child(stepper(
                    "rig-radius",
                    "Radius",
                    format!("{radius:.2} ×"),
                    studio_radius_dec,
                    |studio, cx| studio.step_rig_radius(-0.05, cx),
                    studio_radius_inc,
                    |studio, cx| studio.step_rig_radius(0.05, cx),
                    cx,
                ))
                .child(stepper(
                    "rig-fps",
                    "Playback",
                    format!("{playback_fps} fps"),
                    studio_fps_dec,
                    |studio, cx| studio.step_playback_fps(-6, cx),
                    studio_fps_inc,
                    |studio, cx| studio.step_playback_fps(6, cx),
                    cx,
                )),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(label("Frames", cx))
                .child(
                    div()
                        .w(px(150.))
                        .text_xs()
                        .child(if keyframe_count == 0 {
                            "not baked".to_owned()
                        } else {
                            format!("{keyframe_count} baked · showing {}", frame + 1)
                        }),
                )
                .child(
                    Button::new("rig-prev")
                        .icon(IconName::ChevronLeft)
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            studio_prev.update(cx, |studio, cx| studio.scrub_rig(-1, cx));
                        }),
                )
                .child(
                    Button::new("rig-next")
                        .icon(IconName::ChevronRight)
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            studio_next.update(cx, |studio, cx| studio.scrub_rig(1, cx));
                        }),
                )
                .child(
                    Button::new("rig-play")
                        .icon(if playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        })
                        .label(if playing { "Pause" } else { "Play" })
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            studio_play.update(cx, |studio, cx| studio.toggle_rig_playback(cx));
                        }),
                )
                .child(
                    Button::new("sequence-render")
                        .label("Render sequence…")
                        .xsmall()
                        .outline()
                        .on_click(move |_, _, cx| {
                            studio_sequence.update(cx, |studio, cx| studio.render_sequence(cx));
                        }),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(sequence.unwrap_or_else(|| {
                    "Frames export as frame_%04d.png through the certified bridge; one rig bake is one undo-free camera replay."
                        .to_owned()
                })),
        )
        .into_any_element()
}

fn label(text: &'static str, cx: &App) -> AnyElement {
    div()
        .w(px(56.))
        .text_xs()
        .text_color(cx.theme().colors.muted_foreground)
        .child(text)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn stepper(
    id: &'static str,
    title: &'static str,
    value: String,
    dec_entity: Entity<Studio>,
    dec: fn(&mut Studio, &mut Context<Studio>),
    inc_entity: Entity<Studio>,
    inc: fn(&mut Studio, &mut Context<Studio>),
    cx: &App,
) -> AnyElement {
    h_flex()
        .gap_1()
        .items_center()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().colors.muted_foreground)
                .child(title),
        )
        .child(
            Button::new(format!("{id}-dec"))
                .label("−")
                .xsmall()
                .ghost()
                .on_click(move |_, _, cx| {
                    dec_entity.update(cx, dec);
                }),
        )
        .child(div().w(px(52.)).text_xs().text_center().child(value))
        .child(
            Button::new(format!("{id}-inc"))
                .label("+")
                .xsmall()
                .ghost()
                .on_click(move |_, _, cx| {
                    inc_entity.update(cx, inc);
                }),
        )
        .into_any_element()
}
