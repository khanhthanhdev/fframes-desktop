//! Track rows and segment boxes (Cap's `render_row` and segment), tinted per track kind.
use gpui::{ParentElement, Styled, div, px};
use studio_theme::{
    METRICS, TextStyle, Theme, TrackKind, segment_colors_for, text_style, track_color,
};

use crate::kit::{Icon, icon};

/// The gap between a segment's label and its length.
const LABEL_GAP: f32 = 6.;

/// Where a segment label sits: `left` from the segment's left edge, `width` available.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LabelSlice {
    pub left: f32,
    pub width: f32,
}

/// Anchors a label to the part of a segment (at `x`, `width` wide) that is inside
/// a viewport `view_width` wide, as Cap's `visible_box` keeps labels on screen.
pub(super) fn label_slice(x: f32, width: f32, view_width: f32) -> LabelSlice {
    let visible_start = (-x).clamp(0., width);
    let visible_end = (view_width - x).min(width).max(visible_start);
    let (pad_left, pad_right) = (
        METRICS.layout.segment_label_pad_left,
        METRICS.layout.segment_label_pad_right,
    );
    LabelSlice {
        left: visible_start + pad_left,
        width: (visible_end - visible_start - pad_left - pad_right).max(0.),
    }
}

pub(super) fn lane_name(base: &str, index: usize, count: usize) -> String {
    if count > 1 {
        format!("{base} {}", index + 1)
    } else {
        base.to_owned()
    }
}

/// The y of track row `row` below the ruler.
pub(super) fn row_top(row: usize) -> f32 {
    row as f32 * (METRICS.track_h + METRICS.track_gap)
}

/// A track row: the gutter with the lane's tile and name, then the clipped band for
/// segments.
pub(super) fn track_row(
    theme: &Theme,
    top: f32,
    name: String,
    glyph: Icon,
    segments: Vec<gpui::AnyElement>,
) -> gpui::Div {
    let metrics = METRICS;
    let (tile_radius, band_radius) = (px(metrics.radius.item), px(metrics.radius.segment));
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(top))
        .h(px(metrics.track_h))
        .flex()
        .items_center()
        .gap(px(metrics.space_2))
        .child(
            text_style(div(), TextStyle::Label)
                .w(px(metrics.track_gutter_w))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(metrics.space_2))
                .overflow_hidden()
                .text_color(theme.colors.text_2)
                .child(
                    div()
                        .size(px(metrics.track_tile))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(tile_radius)
                        .bg(theme.colors.ctl)
                        .child(icon(glyph, metrics.icon_sm, theme.colors.text_2)),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(name),
                ),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .rounded(band_radius)
                .bg(theme.colors.card_2)
                .children(segments),
        )
}

/// Segment box with its accent bar and clipped label, `detail` (the length) after the
/// label in mono `text-3`. The caller adds ids and handlers.
#[allow(clippy::too_many_arguments)]
pub(super) fn segment(
    theme: &Theme,
    kind: TrackKind,
    selected: bool,
    x: f32,
    width: f32,
    view_width: f32,
    label: String,
    detail: String,
) -> gpui::Div {
    let metrics = METRICS;
    let colors = segment_colors_for(theme.appearance, kind, selected);
    let slice = label_slice(x, width, view_width);
    let radius = px(metrics.radius.segment);
    div()
        .absolute()
        .left(px(x))
        .top_0()
        .w(px(width))
        .h(px(metrics.track_h))
        .overflow_hidden()
        .rounded(radius)
        .border_1()
        // Selected: the track colour itself, over the fill that is already stronger.
        .border_color(if selected {
            track_color(kind)
        } else {
            colors.border
        })
        .bg(colors.fill)
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(metrics.layout.segment_accent_bar_w))
                .bg(track_color(kind)),
        )
        .child(
            text_style(div(), TextStyle::Small)
                .absolute()
                .left(px(slice.left))
                .top_0()
                .bottom_0()
                .w(px(slice.width))
                .flex()
                .items_center()
                .gap(px(LABEL_GAP))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(
                    text_style(div(), TextStyle::Label)
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(colors.label)
                        .child(label),
                )
                .child(
                    text_style(div(), TextStyle::CodeSmall)
                        .flex_none()
                        .text_color(theme.colors.text_3)
                        .child(detail),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_label_anchors_to_visible_slice() {
        const LABEL_PAD_LEFT: f32 = METRICS.layout.segment_label_pad_left;
        let inner = LABEL_PAD_LEFT + METRICS.layout.segment_label_pad_right;
        // Fully visible: label starts after the left padding.
        let whole = label_slice(10., 100., 500.);
        assert_eq!(whole.left, LABEL_PAD_LEFT);
        assert_eq!(whole.width, 100. - inner);
        // Scrolled past the left edge: label follows the visible part.
        let left_cut = label_slice(-50., 100., 500.);
        assert_eq!(left_cut.left, 50. + LABEL_PAD_LEFT);
        assert_eq!(left_cut.width, 50. - inner);
        // Cut by the right edge of the viewport.
        let right_cut = label_slice(450., 100., 500.);
        assert_eq!(right_cut.left, LABEL_PAD_LEFT);
        assert_eq!(right_cut.width, 50. - inner);
        // Too little visible: no width, never negative.
        assert_eq!(label_slice(490., 100., 500.).width, 0.);
        // Longer than the viewport on both sides.
        let both = label_slice(-1000., 5000., 400.);
        assert_eq!(both.left, 1000. + LABEL_PAD_LEFT);
        assert_eq!(both.width, 400. - inner);
    }

    #[test]
    fn lanes_are_numbered_only_when_there_are_several() {
        assert_eq!(lane_name("Audio", 0, 1), "Audio");
        assert_eq!(lane_name("Audio", 1, 2), "Audio 2");
        assert_eq!(row_top(2), 2. * (METRICS.track_h + METRICS.track_gap));
    }
}
