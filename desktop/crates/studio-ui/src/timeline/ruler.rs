//! Cap's `render_ruler`: marks on the engine's resolution grid, taller and labelled on
//! whole seconds. A label at either end is anchored to its mark so it stays inside the
//! axis, and a label that would sit on a slide stop's flag is left out.
use gpui::{ParentElement, Styled, div, prelude::FluentBuilder, px};
use studio_engine::TimelineTick;
use studio_theme::{METRICS, TextStyle, Theme, text_style};

/// How a tick's label lines up with its mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LabelAnchor {
    /// Starts at the mark: near the start of the axis.
    Start,
    Centre,
    /// Ends at the mark: near the end of the axis.
    End,
}

/// The label box of a tick at `x` on a ruler `width` wide: its left edge and how the text
/// lines up in it. The box is centred on the mark unless that puts it past an end.
pub(super) fn label_box(x: f32, width: f32) -> (f32, LabelAnchor) {
    let label_w = METRICS.layout.ruler_label_w;
    let half = label_w / 2.;
    if x - half < 0. {
        (x, LabelAnchor::Start)
    } else if x + half > width {
        (x - label_w, LabelAnchor::End)
    } else {
        (x - half, LabelAnchor::Centre)
    }
}

/// The left edge of the flag of a slide stop at `x` on a ruler `width` wide: it points
/// right, or back where it would pass the end, so the last flag stays inside.
pub(super) fn flag_left(x: f32, width: f32) -> f32 {
    let flag_w = METRICS.layout.stop_flag_w;
    if x + flag_w > width { x - flag_w } else { x }
}

/// Whether a label box at `left` overlaps the flag of a stop at any of `stops` (the
/// stops' x on a ruler `width` wide).
pub(super) fn label_hits_flag(left: f32, stops: &[f32], width: f32) -> bool {
    let (label_w, flag_w) = (METRICS.layout.ruler_label_w, METRICS.layout.stop_flag_w);
    stops.iter().any(|&stop| {
        let flag = flag_left(stop, width);
        flag < left + label_w && left < flag + flag_w
    })
}

/// The ruler over the time axis, `inset` from the left of the tracks and `width` wide,
/// with the slide stops at `stops` (x on the axis) drawn over it.
pub(super) fn render_ruler(
    theme: &Theme,
    ticks: Vec<TimelineTick>,
    inset: f32,
    width: f32,
    stops: &[f32],
) -> gpui::Div {
    let colors = theme.colors;
    let layout = METRICS.layout;
    let mut ruler = div()
        .absolute()
        .left(px(inset))
        .right_0()
        .top_0()
        .h(px(METRICS.ruler_h))
        .overflow_hidden();
    for tick in ticks {
        let x = tick.x as f32;
        ruler = ruler.child(
            div()
                .absolute()
                .left(px(x))
                .bottom_0()
                .w(px(METRICS.size.divider))
                .h(px(if tick.major {
                    layout.ruler_major_tick_h
                } else {
                    layout.ruler_minor_tick_h
                }))
                .bg(colors.line_strong),
        );
        let Some(label) = tick.label else {
            continue;
        };
        let (left, anchor) = label_box(x, width);
        if label_hits_flag(left, stops, width) {
            continue;
        }
        ruler = ruler.child(
            // Times are mono so the digits keep their width as the view scrolls.
            text_style(div(), TextStyle::CodeSmall)
                .absolute()
                .left(px(left))
                .bottom(px(layout.ruler_label_bottom))
                .w(px(layout.ruler_label_w))
                .flex()
                .when(anchor == LabelAnchor::Centre, |label| {
                    label.justify_center()
                })
                .when(anchor == LabelAnchor::End, |label| label.justify_end())
                .whitespace_nowrap()
                .text_color(colors.text_3)
                .child(label),
        );
    }
    ruler
}
