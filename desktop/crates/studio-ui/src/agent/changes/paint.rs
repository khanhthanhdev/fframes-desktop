//! Colours, row metrics and the cells both layouts share: gutters, markers and code text with
//! paint-only syntax colours.

use std::ops::Range;

use gpui::prelude::FluentBuilder as _;
use gpui::{Div, Hsla, ParentElement, SharedString, Styled, StyledText, TextRun, div, font, px};
use studio_theme::{METRICS, MONO_FAMILY, TextStyle, Theme, text_style};

use super::model::{DiffLine, LineKind, push_expanded};
use crate::agent::syntax::{self, HighlightSpan};

pub const LINE_H: f32 = 20.0;
pub const FILE_HEADER_H: f32 = 32.0;
pub const HUNK_HEADER_H: f32 = 24.0;
pub const NOTICE_H: f32 = 24.0;
pub const ACCENT_W: f32 = 3.0;
pub const MARKER_W: f32 = 22.0;
pub const SPLIT_MARKER_W: f32 = 16.0;
pub const UNIFIED_CODE_PAD: f32 = 8.0;
pub const SPLIT_CODE_PAD: f32 = 4.0;
const WASH_ALPHA: f32 = 0.10;
const HUNK_HEADER_ALPHA: f32 = 0.5;

/// The resolved colours the viewer draws with.
#[derive(Clone, Copy)]
pub struct Colors {
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub add: Hsla,
    pub del: Hsla,
    pub add_wash: Hsla,
    pub del_wash: Hsla,
    pub line: Hsla,
    pub header: Hsla,
    pub hunk_header: Hsla,
    pub filler: Hsla,
}

impl Colors {
    pub fn new(theme: &Theme) -> Self {
        let add = Hsla::from(theme.syntax.added);
        let del = Hsla::from(theme.syntax.removed);
        Self {
            text: theme.colors.text_1.into(),
            muted: theme.colors.text_2.into(),
            faint: theme.colors.text_3.into(),
            add,
            del,
            add_wash: add.opacity(WASH_ALPHA),
            del_wash: del.opacity(WASH_ALPHA),
            line: theme.colors.line.into(),
            header: theme.colors.card_2.into(),
            hunk_header: Hsla::from(theme.colors.card_2).opacity(HUNK_HEADER_ALPHA),
            filler: theme.colors.card_2.into(),
        }
    }

    /// Marker glyph, marker colour, row wash and accent bar for a line kind.
    fn kind(&self, kind: LineKind) -> (&'static str, Hsla, Option<Hsla>, Option<Hsla>) {
        match kind {
            LineKind::Add => ("+", self.add, Some(self.add_wash), Some(self.add)),
            LineKind::Del => ("-", self.del, Some(self.del_wash), Some(self.del)),
            _ => (" ", self.faint, None, None),
        }
    }
}

/// Per-line syntax spans for both sides of a file. Empty for a language without a grammar.
#[derive(Default, Clone)]
pub struct Highlights {
    old: Vec<Vec<HighlightSpan>>,
    new: Vec<Vec<HighlightSpan>>,
}

impl Highlights {
    /// Highlights each side by the file extension of `path`.
    pub fn compute(path: &str, before: Option<&str>, after: Option<&str>) -> Self {
        let Some((_, extension)) = path.rsplit_once('.') else {
            return Self::default();
        };
        let side = |text: Option<&str>| {
            text.map(|text| per_line(text, syntax::highlight(extension, text)))
                .unwrap_or_default()
        };
        Self {
            old: side(before),
            new: side(after),
        }
    }

    /// The spans of `line`, relative to the start of its text.
    pub fn spans(&self, line: &DiffLine) -> &[HighlightSpan] {
        let (lines, number) = match line.kind {
            LineKind::Del => (&self.old, line.old_no),
            LineKind::Add | LineKind::Context => (&self.new, line.new_no),
            LineKind::Meta => return &[],
        };
        number
            .and_then(|n| lines.get(n as usize - 1))
            .map_or(&[], Vec::as_slice)
    }
}

/// Splits whole-text spans at line ends, giving every line its spans relative to the line start.
/// The result has one entry per `\n`-separated line; a `\r` before the `\n` is not part of it.
fn per_line(text: &str, spans: Vec<HighlightSpan>) -> Vec<Vec<HighlightSpan>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut first = 0;
    for raw in text.split('\n') {
        let end = start + raw.len();
        let line_end = if raw.ends_with('\r') { end - 1 } else { end };
        while first < spans.len() && spans[first].range.end <= start {
            first += 1;
        }
        let mut line = Vec::new();
        for span in spans[first..]
            .iter()
            .take_while(|span| span.range.start < line_end)
        {
            let from = span.range.start.max(start);
            let to = span.range.end.min(line_end);
            if from < to {
                line.push(HighlightSpan {
                    range: (from - start)..(to - start),
                    class: span.class,
                });
            }
        }
        out.push(line);
        start = end + 1;
    }
    out
}

/// The text of a line with tabs expanded, and the runs that colour it.
pub fn line_runs(
    text: &str,
    spans: &[HighlightSpan],
    theme: &Theme,
    colors: &Colors,
) -> (SharedString, Vec<TextRun>) {
    let mono = font(MONO_FAMILY);
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: mono.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut out = String::with_capacity(text.len());
    let mut runs: Vec<TextRun> = Vec::new();
    let mut column = 0;
    let mut pos = 0;
    let mut push = |segment: Range<usize>, color: Hsla, out: &mut String| {
        let before = out.len();
        push_expanded(&text[segment], &mut column, out);
        let len = out.len() - before;
        match runs.last_mut() {
            Some(last) if last.color == color => last.len += len,
            _ if len == 0 => {}
            _ => runs.push(run(len, color)),
        }
    };
    for span in spans {
        let range = span.range.clone();
        if range.start < pos
            || range.end > text.len()
            || range.is_empty()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            continue;
        }
        if range.start > pos {
            push(pos..range.start, colors.text, &mut out);
        }
        push(
            range.clone(),
            Hsla::from(syntax::token_color(span.class, theme)),
            &mut out,
        );
        pos = range.end;
    }
    if pos < text.len() {
        push(pos..text.len(), colors.text, &mut out);
    }
    (SharedString::from(out), runs)
}

/// A line-number column.
fn gutter(number: Option<u32>, width: f32, color: Hsla) -> Div {
    text_style(div(), TextStyle::CodeSmall)
        .w(px(width))
        .flex_none()
        .flex()
        .justify_end()
        .pr(px(METRICS.space_2))
        .line_height(px(LINE_H))
        .text_color(color)
        .child(SharedString::from(
            number.map(|n| n.to_string()).unwrap_or_default(),
        ))
}

/// The code of one line, clipped at the column edge.
fn code(
    line: &DiffLine,
    highlights: &Highlights,
    pad: f32,
    wrap: bool,
    theme: &Theme,
    colors: &Colors,
) -> Div {
    let (text, runs) = line_runs(&line.text, highlights.spans(line), theme, colors);
    text_style(div(), TextStyle::Code)
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .when(wrap, |el| el.whitespace_normal())
        .when(!wrap, |el| el.whitespace_nowrap())
        .pl(px(pad))
        .line_height(px(LINE_H))
        .text_color(colors.text)
        .when(!text.is_empty(), |el| {
            el.child(StyledText::new(text.clone()).with_runs(runs))
        })
}

/// What a row needs besides its lines: how it is coloured, sized and wrapped.
pub struct RowStyle<'a> {
    pub highlights: &'a Highlights,
    pub max_line: u32,
    pub wrap: bool,
    pub theme: &'a Theme,
    pub colors: &'a Colors,
}

/// One line-number column of a cell; `changed` colours the number like the marker.
pub struct Gutter {
    pub number: Option<u32>,
    pub changed: bool,
}

/// One side of a row: accent bar, line numbers, the +/- marker and the code. A unified row has
/// two gutters (old, new); a split half has just its own.
pub struct Cell<'a> {
    pub line: &'a DiffLine,
    pub gutters: &'a [Gutter],
    pub gutter_w: f32,
    pub marker_w: f32,
    pub code_pad: f32,
    pub highlights: &'a Highlights,
    /// Wrap long code onto more lines instead of clipping it.
    pub wrap: bool,
}

impl Cell<'_> {
    pub fn render(self, theme: &Theme, colors: &Colors) -> Div {
        let (marker, marker_color, wash, accent) = colors.kind(self.line.kind);
        let mut cell = div()
            .flex_1()
            .min_w_0()
            .min_h(px(LINE_H))
            .when(!self.wrap, |el| el.h(px(LINE_H)))
            .overflow_hidden()
            .flex()
            .flex_row()
            .when_some(wash, |el, wash| el.bg(wash))
            .child(
                div()
                    .w(px(ACCENT_W))
                    .self_stretch()
                    .flex_none()
                    .when_some(accent, |el, accent| el.bg(accent)),
            );
        for gutter_cell in self.gutters {
            let color = if gutter_cell.changed {
                marker_color
            } else {
                colors.faint
            };
            cell = cell.child(gutter(gutter_cell.number, self.gutter_w, color));
        }
        cell.child(
            text_style(div(), TextStyle::Code)
                .w(px(self.marker_w))
                .flex_none()
                .flex()
                .justify_center()
                .line_height(px(LINE_H))
                .text_color(marker_color)
                .child(marker),
        )
        .child(code(
            self.line,
            self.highlights,
            self.code_pad,
            self.wrap,
            theme,
            colors,
        ))
    }
}
/// The no-newline marker: a note, indented past the columns and never tinted.
pub fn meta_row(text: &str, pad_left: f32, colors: &Colors) -> Div {
    text_style(div(), TextStyle::Small)
        .h(px(LINE_H))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .pl(px(pad_left))
        .text_color(colors.faint)
        .italic()
        .child(SharedString::from(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::syntax::TokenClass;

    fn span(range: Range<usize>) -> HighlightSpan {
        HighlightSpan {
            range,
            class: TokenClass::Keyword,
        }
    }

    #[test]
    fn spans_are_cut_at_line_ends_and_shifted_to_the_line() {
        // "ab\ncd\n": one span over "b\nc".
        let lines = per_line("ab\ncd\n", vec![span(1..4)]);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], [span(1..2)]);
        assert_eq!(lines[1], [span(0..1)]);
        assert!(lines[2].is_empty());
    }

    #[test]
    fn a_carriage_return_is_not_part_of_the_line() {
        let lines = per_line("ab\r\ncd", vec![span(0..4)]);
        assert_eq!(lines[0], [span(0..2)]);
        assert_eq!(lines[1], [] as [HighlightSpan; 0]);
    }

    #[test]
    fn rust_files_get_colours_and_unknown_extensions_do_not() {
        let code = "fn main() {}\n";
        let rust = Highlights::compute("a.rs", None, Some(code));
        let line = DiffLine {
            kind: LineKind::Add,
            old_no: None,
            new_no: Some(1),
            text: "fn main() {}".to_owned(),
        };
        assert_eq!(
            rust.spans(&line).is_empty(),
            !cfg!(feature = "syntax-grammars")
        );
        assert!(
            Highlights::compute("a.xyz", None, Some(code))
                .spans(&line)
                .is_empty()
        );
        assert!(
            Highlights::compute("Makefile", None, Some(code))
                .spans(&line)
                .is_empty()
        );
    }

    #[test]
    fn runs_cover_the_expanded_text_exactly() {
        let theme = Theme::new(studio_theme::Appearance::Dark);
        let colors = Colors::new(&theme);
        let (text, runs) = line_runs("\tfn x", &[span(1..3)], &theme, &colors);
        assert_eq!(text.as_ref(), "    fn x");
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), text.len());
        assert_eq!(runs.len(), 3);
        assert_eq!(
            runs[0].len, 4,
            "the tab belongs to the plain run before the keyword"
        );
    }
}
