//! GFM tables: columns sized to their widest cell, scrolling sideways when wider than the row.

use std::cell::RefCell;

use gpui::{
    AnyElement, Font, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, TextRun, Window, div, px,
};
use studio_markdown::TableAlign;

use super::super::inline::Flat;
use super::blocks::BuildCx;

const CELL_PAD_X: f32 = 10.0;
const CELL_PAD_Y: f32 = 5.0;
const MIN_COL: f32 = 48.0;
const MAX_COL: f32 = 360.0;
const BODY_SIZE: f32 = studio_theme::TextStyle::Body.size();

/// Column widths measured with a font, kept in the prepared table so the cells are shaped once
/// per font rather than on every frame. A clone starts empty and every cache compares equal:
/// the cache is not part of what the table shows.
#[derive(Debug, Default)]
pub struct WidthCache(RefCell<Option<(Font, Vec<Pixels>)>>);

impl Clone for WidthCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for WidthCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// The width of one column from the natural widths (text only, in pixels) of its cells.
pub fn column_width(natural: impl IntoIterator<Item = f32>) -> f32 {
    let widest = natural.into_iter().fold(0.0_f32, f32::max);
    (widest + 2.0 * CELL_PAD_X + 2.0).clamp(MIN_COL, MAX_COL)
}

/// Natural width of the longest line of `flat`, shaped with the surrounding font.
fn natural_width(window: &Window, flat: &Flat) -> f32 {
    let base = window.text_style();
    let mut font = base.font();
    if flat.segs.iter().any(|s| s.bold) {
        font.weight = FontWeight::BOLD;
    }
    flat.text
        .split('\n')
        .map(|line| {
            let run = TextRun {
                len: line.len(),
                font: font.clone(),
                color: base.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(line.to_owned()),
                px(BODY_SIZE),
                &[run],
                None,
            );
            f32::from(shaped.width)
        })
        .fold(0.0, f32::max)
}

pub fn build(
    header: &[Flat],
    rows: &[Vec<Flat>],
    align: &[TableAlign],
    cache: &WidthCache,
    cx: &mut BuildCx,
) -> AnyElement {
    let font = cx.window.text_style().font();
    let cached = cache
        .0
        .borrow()
        .as_ref()
        .filter(|(measured, _)| *measured == font)
        .map(|(_, widths)| widths.clone());
    let widths = cached.unwrap_or_else(|| {
        let columns = header
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));
        let widths: Vec<Pixels> = (0..columns)
            .map(|c| {
                let cells = std::iter::once(header.get(c))
                    .chain(rows.iter().map(|r| r.get(c)))
                    .flatten();
                px(column_width(cells.map(|f| natural_width(cx.window, f))))
            })
            .collect();
        *cache.0.borrow_mut() = Some((font, widths.clone()));
        widths
    });
    let total = widths.iter().fold(Pixels::ZERO, |a, w| a + *w);
    let (line, head_fill, id) = (cx.style.rule, cx.style.code_bg, cx.row_id);
    let table_ix = cx.peek_ix();

    let mut line_element = |cells: &[Flat], is_header: bool, last: bool| {
        let mut row = div().flex().flex_row().w(total);
        if is_header {
            row = row.bg(head_fill);
        }
        if !last {
            row = row.border_b_1().border_color(line);
        }
        for (c, width) in widths.iter().enumerate() {
            let cell = div()
                .w(*width)
                .flex_none()
                .px(px(CELL_PAD_X))
                .py(px(CELL_PAD_Y));
            let cell = match align.get(c).copied().unwrap_or_default() {
                TableAlign::Left => cell,
                TableAlign::Center => cell.text_center(),
                TableAlign::Right => cell.text_right(),
            };
            row = row.child(match cells.get(c) {
                Some(flat) => cell.child(cx.text(flat)),
                None => cell,
            });
        }
        row
    };

    let mut table = div().flex().flex_col().w(total);
    table = table.child(line_element(header, true, rows.is_empty()));
    for (r, row) in rows.iter().enumerate() {
        table = table.child(line_element(row, false, r + 1 == rows.len()));
    }
    div()
        .id(SharedString::from(format!("md-table-{id}-{}", table_ix)))
        .overflow_x_scroll()
        // The frame hugs the columns (plus its border) and scrolls when the row is narrower.
        .max_w(total + px(studio_theme::METRICS.size.divider * 2.))
        .rounded(crate::agent::ui::CARD_RADIUS)
        .border_1()
        .border_color(line)
        .child(table)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_is_as_wide_as_its_widest_cell_within_bounds() {
        assert_eq!(column_width([]), MIN_COL);
        assert_eq!(
            column_width([10.0, 120.0, 30.0]),
            120.0 + 2.0 * CELL_PAD_X + 2.0
        );
        assert_eq!(column_width([1000.0]), MAX_COL);
    }
}
