//! The transport bar at the foot of the player card, after Cap's: the clock, playback in the
//! middle, then mute, the timeline's Split and zoom, build and cancel.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, div,
    prelude::FluentBuilder, px,
};
use studio_engine::engine::backend::Command;
use studio_theme::{METRICS, TextStyle, text_style};

use super::StudioShell;
use crate::kit::{Divider, Emphasis, Icon, IconButtonSize, Slider, with_tooltip};
use crate::timeline::TimelineControls;

impl StudioShell {
    pub(super) fn render_transport(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let palette = theme.colors;
        let enabled = !self.busy;
        let project = self.presentation.project.clone();
        let mode = transport_mode(self.player_width);
        let has_frames = self
            .displayed
            .as_ref()
            .is_some_and(|d| d.timeline.total_frames > 0);
        let playing = self.transport.playing() || self.resume_install;
        let (current, total, tip) = match &self.displayed {
            Some(d) => {
                let fps = d.timeline.fps as f64;
                let tip = format!(
                    "Displayed {} · frame {} / {} · {:.3}s{}",
                    &d.identity().source_revision[..12.min(d.identity().source_revision.len())],
                    self.preview_state.position(),
                    d.timeline.total_frames,
                    self.preview_state.position() as f64 / fps,
                    if project
                        .as_ref()
                        .is_some_and(|p| p.source.as_str() != d.identity().source_revision)
                        && !self
                            .presentation
                            .state
                            .as_ref()
                            .is_some_and(|s| s.is_current_build(d.tag()))
                    {
                        " · prior revision"
                    } else {
                        ""
                    }
                );
                (
                    format_clock(self.preview_state.position() as f64 / fps),
                    format_clock(d.timeline.total_frames as f64 / fps),
                    Some(tip),
                )
            }
            None => ("0:00.00".to_owned(), "0:00.00".to_owned(), None),
        };
        // Cap's clock: the time in `text-1`, the length after it in `text-3`.
        let clock = with_tooltip(
            text_style(div().id("time-readout"), TextStyle::Body)
                .font_family("monospace")
                .flex()
                .flex_none()
                .text_color(palette.text_1)
                .child(current)
                .when(show_total_time(self.player_width), |clock| {
                    clock.child(
                        div()
                            .text_color(palette.text_3)
                            .child(format!(" / {total}")),
                    )
                }),
            &theme,
            tip.map(Into::into),
        );
        let can_build = enabled && project.as_ref().is_some_and(|p| p.worker_available);
        let can_cancel = self.preview.is_some();
        // Build keeps its label while the bar has room; Cancel is always an icon.
        let build = if mode == TransportMode::Full {
            self.action_button(
                "build-preview",
                "Build preview",
                can_build,
                true,
                Some(Icon::Hammer),
                cx,
                |s, cx| s.dispatch(Command::Build, cx),
            )
            .into_any_element()
        } else {
            self.action_icon_button(
                "build-preview",
                "Build preview",
                Icon::Hammer,
                IconButtonSize::Md,
                can_build,
                cx,
                |s, cx| s.dispatch(Command::Build, cx),
            )
            .emphasis(Emphasis::Primary)
            .into_any_element()
        };
        let cancel = self
            .action_icon_button(
                "cancel-preview",
                "Cancel build",
                Icon::Square,
                IconButtonSize::Md,
                can_cancel,
                cx,
                |s, cx| s.dispatch(Command::CancelBuild, cx),
            )
            .emphasis(Emphasis::Neutral);
        let playback = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_3))
            .child(self.action_icon_button(
                "previous-frame",
                "Previous frame",
                Icon::StepBack,
                IconButtonSize::Sm,
                has_frames,
                cx,
                |s, cx| s.step_preview(-1, cx),
            ))
            .child(
                self.action_icon_button(
                    "play-pause",
                    if playing { "Pause" } else { "Play" },
                    if playing { Icon::Pause } else { Icon::Play },
                    IconButtonSize::Md,
                    has_frames,
                    cx,
                    |s, cx| s.toggle_playback(cx),
                )
                .round()
                .emphasis(Emphasis::Neutral),
            )
            .child(self.action_icon_button(
                "next-frame",
                "Next frame",
                Icon::StepForward,
                IconButtonSize::Sm,
                has_frames,
                cx,
                |s, cx| s.step_preview(1, cx),
            ));
        let timeline = self.timeline_controls;
        let split = self.action_button(
            "timeline-split",
            "Split",
            timeline.editable,
            false,
            Some(Icon::Scissors),
            cx,
            |s, cx| s.timeline.update(cx, |t, cx| t.split_at_playhead(cx)),
        );
        let split = if mode == TransportMode::Full {
            split
        } else {
            split.icon_only()
        };
        let mut tools = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.layout.transport_tools_gap))
            .child(self.action_icon_button(
                "mute-audio",
                if self.muted { "Unmute (M)" } else { "Mute (M)" },
                if self.muted {
                    Icon::VolumeX
                } else {
                    Icon::Volume2
                },
                IconButtonSize::Md,
                self.displayed.is_some(),
                cx,
                |s, cx| s.toggle_mute(cx),
            ))
            .child(Divider::vertical(&theme))
            .child(split)
            .child(self.action_icon_button(
                "timeline-cycle",
                "Cycle scene (S)",
                Icon::Layers,
                IconButtonSize::Md,
                timeline.enabled,
                cx,
                |s, cx| s.timeline.update(cx, |t, cx| t.cycle_scene(cx)),
            ));
        if mode != TransportMode::Narrow {
            tools = tools
                .child(Divider::vertical(&theme))
                .child(self.action_icon_button(
                    "timeline-zoom-out",
                    "Zoom out",
                    Icon::ZoomOut,
                    IconButtonSize::Md,
                    timeline.enabled,
                    cx,
                    |s, cx| s.timeline.update(cx, |t, cx| t.zoom(0.5, cx)),
                ))
                .when(mode == TransportMode::Full, |tools| {
                    tools.child(self.zoom_slider(timeline))
                })
                .child(self.action_icon_button(
                    "timeline-zoom-in",
                    "Zoom in",
                    Icon::ZoomIn,
                    IconButtonSize::Md,
                    timeline.enabled,
                    cx,
                    |s, cx| s.timeline.update(cx, |t, cx| t.zoom(2., cx)),
                ))
                .child(self.action_button(
                    "timeline-fit",
                    "Fit",
                    timeline.enabled,
                    false,
                    None,
                    cx,
                    |s, cx| s.timeline.update(cx, |t, cx| t.fit(cx)),
                ));
        }
        let tools = tools
            .child(Divider::vertical(&theme))
            .child(build)
            .child(cancel)
            .child(self.overflow_trigger(cx));
        // Two equal sides keep the playback group in the middle; the tools side does not
        // shrink below its buttons, so in a tight bar the group moves left instead.
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_3))
            .h(px(METRICS.transport_h))
            .px(px(METRICS.space_3))
            .border_t_1()
            .border_color(palette.line)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .flex_basis(Pixels::ZERO)
                    .min_w_0()
                    .overflow_hidden()
                    .items_center()
                    .child(clock),
            )
            .child(playback)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .flex_basis(Pixels::ZERO)
                    .justify_end()
                    .items_center()
                    .child(tools),
            )
            .into_any_element()
    }

    /// Cap's 72 px zoom slider: Fit at the left end, the closest zoom at the right. It drives
    /// the timeline, which reports the new zoom back through its controls.
    fn zoom_slider(&self, timeline: TimelineControls) -> gpui::Div {
        let entity = self.timeline.clone();
        // The half-thumb inset keeps the knob inside the 72 px.
        div()
            .flex()
            .flex_none()
            .w(px(METRICS.layout.zoom_slider_w))
            .px(px(METRICS.size.slider_thumb / 2.))
            .child(
                Slider::new(&self.theme, "timeline-zoom-slider", timeline.zoom_fraction)
                    .flex()
                    .label("Timeline zoom")
                    .disabled(!timeline.enabled)
                    .on_change(0., move |fraction, _, cx| {
                        entity.update(cx, |t, cx| t.set_zoom_fraction(*fraction, cx));
                    }),
            )
    }
}

// The breakpoints are `METRICS.layout`'s. The bar keeps playback centred while each side
// fits half of it: the tools measure 566 px with the slider and the Split and Build labels,
// 333 px without them and 214 px without the zoom group, beside about 100 px of playback,
// 24 px of padding and two 12 px gaps. Below `transport_compact_below` the slider and the
// labels go; below `transport_narrow_below` the zoom buttons and Fit go too (a 1280x800
// window gives the player about 824 px, so the zoom stays there); below
// `transport_total_hidden_below` the clock drops the total. The tools' gap,
// `transport_tools_gap`, is tighter than the bar's so they fit in a 1280 px window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransportMode {
    Full,
    Compact,
    Narrow,
}

pub(super) fn transport_mode(player_width: f32) -> TransportMode {
    let layout = METRICS.layout;
    if player_width < layout.transport_narrow_below {
        TransportMode::Narrow
    } else if player_width < layout.transport_compact_below {
        TransportMode::Compact
    } else {
        TransportMode::Full
    }
}

pub(super) fn show_total_time(player_width: f32) -> bool {
    player_width >= METRICS.layout.transport_total_hidden_below
}

/// `m:ss.cc`, truncated to centiseconds.
fn format_clock(seconds: f64) -> String {
    let centis = (seconds.max(0.) * 100.).floor() as u64;
    format!(
        "{}:{:02}.{:02}",
        centis / 6000,
        centis / 100 % 60,
        centis % 100
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_mode_drops_the_slider_then_the_zoom_buttons() {
        assert_eq!(transport_mode(1500.), TransportMode::Full);
        assert_eq!(transport_mode(1290.), TransportMode::Full);
        assert_eq!(transport_mode(1289.9), TransportMode::Compact);
        assert_eq!(transport_mode(820.), TransportMode::Compact);
        assert_eq!(transport_mode(819.9), TransportMode::Narrow);
        assert!(show_total_time(620.));
        assert!(!show_total_time(619.));
    }

    #[test]
    fn clock_formats_minutes_seconds_centiseconds() {
        assert_eq!(format_clock(0.), "0:00.00");
        assert_eq!(format_clock(1.2), "0:01.20");
        assert_eq!(format_clock(5.5), "0:05.50");
        assert_eq!(format_clock(125.999), "2:05.99");
        assert_eq!(format_clock(-3.), "0:00.00");
    }
}
