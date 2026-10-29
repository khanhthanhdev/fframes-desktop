//! Split layout: old on the left, new on the right, paired per hunk.

use gpui::{Div, ParentElement, Styled, div, px};
use studio_theme::METRICS;

use super::model::{DiffLine, Hunk, LineKind, gutter_width};
use super::paint::{
    ACCENT_W, Cell, Gutter, LINE_H, RowStyle, SPLIT_CODE_PAD, SPLIT_MARKER_W, meta_row,
};

/// Which half of the row a line is drawn in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

fn half(line: Option<&DiffLine>, side: Side, style: &RowStyle) -> Div {
    let Some(line) = line else {
        return div()
            .flex_1()
            .min_w_0()
            .self_stretch()
            .bg(style.colors.filler);
    };
    let gutters = [Gutter {
        number: match side {
            Side::Old => line.old_no,
            Side::New => line.new_no,
        },
        changed: line.kind != LineKind::Context,
    }];
    Cell {
        line,
        gutters: &gutters,
        gutter_w: gutter_width(style.max_line),
        marker_w: SPLIT_MARKER_W,
        code_pad: SPLIT_CODE_PAD,
        highlights: style.highlights,
        wrap: style.wrap,
    }
    .render(style.theme, style.colors)
}

/// One split row: the hunk lines at `left` and `right`, either of which may be empty. A
/// no-newline marker on either side spans the whole row.
pub fn row(hunk: &Hunk, left: Option<u32>, right: Option<u32>, style: &RowStyle) -> Div {
    let get = |ix: Option<u32>| ix.and_then(|ix| hunk.lines.get(ix as usize));
    let (left, right) = (get(left), get(right));
    if let Some(marker) = [left, right]
        .into_iter()
        .flatten()
        .find(|line| line.kind == LineKind::Meta)
    {
        return meta_row(&marker.text, ACCENT_W + 8.0, style.colors);
    }
    div()
        .w_full()
        .min_h(px(LINE_H))
        .flex()
        .flex_row()
        .child(half(left, Side::Old, style))
        .child(
            div()
                .w(px(METRICS.size.divider))
                .self_stretch()
                .flex_none()
                .bg(style.colors.line),
        )
        .child(half(right, Side::New, style))
}
