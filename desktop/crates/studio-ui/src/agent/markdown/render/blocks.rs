//! Blocks of a markdown tree: the pure `prepare` step and the element `build` step.
//!
//! [`prepare`] turns a parsed block into a [`Prepared`] value: flattened text, highlighted code
//! segments, nothing that depends on the window. It is the expensive step, and the row caches its
//! result per block. [`build`] then makes this frame's elements from a prepared block, which is
//! cheap and carries the per-frame state (veil opacities, selection, link handling).

use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use studio_markdown::{Block, InlineRun, TableAlign};

use super::super::inline::{Flat, flatten};
use studio_theme::{METRICS, text_style};

use super::super::style::{MdStyle, heading_style};
use super::super::text_element::{MdText, SharedSelection};
use super::super::veil::RowVeil;
use super::{code, table};

/// The bar down the side of a quote.
const QUOTE_BAR_W: f32 = 2.;

#[derive(Clone, Debug, PartialEq)]
pub enum Prepared {
    Paragraph(Flat),
    Heading(u8, Flat),
    Code {
        language: Option<String>,
        raw: String,
        flat: Flat,
    },
    Quote(Vec<Prepared>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Prepared>>,
    },
    Table {
        header: Vec<Flat>,
        rows: Vec<Vec<Flat>>,
        align: Vec<TableAlign>,
        widths: table::WidthCache,
    },
    Rule,
}

impl Prepared {
    /// The name of the block kind, for element selectors and diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Paragraph(_) => "paragraph",
            Self::Heading(..) => "heading",
            Self::Code { .. } => "code",
            Self::Quote(_) => "quote",
            Self::List { .. } => "list",
            Self::Table { .. } => "table",
            Self::Rule => "rule",
        }
    }
}

#[cfg(test)]
impl Prepared {
    /// The destination of every link in the block, in order.
    pub fn link_dests(&self) -> Vec<String> {
        let flats = |flats: &[Flat]| -> Vec<String> {
            flats
                .iter()
                .flat_map(|f| f.links.iter().map(|l| l.dest.clone()))
                .collect()
        };
        match self {
            Self::Paragraph(flat) | Self::Heading(_, flat) | Self::Code { flat, .. } => {
                flats(std::slice::from_ref(flat))
            }
            Self::Quote(children) => children.iter().flat_map(Self::link_dests).collect(),
            Self::List { items, .. } => items.iter().flatten().flat_map(Self::link_dests).collect(),
            Self::Table { header, rows, .. } => {
                let mut out = flats(header);
                out.extend(rows.iter().flat_map(|row| flats(row)));
                out
            }
            Self::Rule => Vec::new(),
        }
    }
}

fn cells(row: &[Vec<InlineRun>], style: &MdStyle, files: bool, bold: bool) -> Vec<Flat> {
    row.iter()
        .map(|cell| {
            let flat = flatten(cell, style, files);
            if bold { flat.bold() } else { flat }
        })
        .collect()
}

pub fn prepare(block: &Block, style: &MdStyle, files: bool) -> Prepared {
    match block {
        Block::Paragraph { runs } => Prepared::Paragraph(flatten(runs, style, files)),
        Block::Heading { level, runs } => Prepared::Heading(*level, flatten(runs, style, files)),
        Block::CodeBlock { language, code } => {
            let code = code.strip_suffix('\n').unwrap_or(code);
            Prepared::Code {
                language: language.clone(),
                raw: code.to_owned(),
                flat: code::highlighted(language.as_deref(), code, style),
            }
        }
        Block::BlockQuote { children } => Prepared::Quote(
            children
                .iter()
                .map(|child| prepare(child, style, files))
                .collect(),
        ),
        Block::List {
            ordered_start,
            items,
        } => Prepared::List {
            start: *ordered_start,
            items: items
                .iter()
                .map(|item| item.iter().map(|b| prepare(b, style, files)).collect())
                .collect(),
        },
        Block::Table {
            header,
            rows,
            align,
        } => Prepared::Table {
            header: cells(header, style, files, true),
            rows: rows
                .iter()
                .map(|row| cells(row, style, files, false))
                .collect(),
            align: align.clone(),
            widths: table::WidthCache::default(),
        },
        Block::Rule => Prepared::Rule,
    }
}

/// Everything one build of a row needs.
pub struct BuildCx<'a> {
    pub style: Rc<MdStyle>,
    pub sel: SharedSelection,
    pub window: &'a Window,
    pub row_id: u64,
    /// The fade state, when new text fades in; `None` leaves the text as it is.
    pub veil: Option<&'a mut RowVeil>,
    pub now: Instant,
    ix: usize,
    texts: Vec<SharedString>,
}

impl<'a> BuildCx<'a> {
    pub fn new(
        style: Rc<MdStyle>,
        sel: SharedSelection,
        window: &'a Window,
        row_id: u64,
        veil: Option<&'a mut RowVeil>,
        now: Instant,
    ) -> Self {
        Self {
            style,
            sel,
            window,
            row_id,
            veil,
            now,
            ix: 0,
            texts: Vec::new(),
        }
    }

    /// The next text element of the row, in document order.
    pub fn text(&mut self, flat: &Flat) -> MdText {
        let ix = self.ix;
        self.ix += 1;
        let text = SharedString::from(flat.text.clone());
        self.texts.push(text.clone());
        let veil = match self.veil.as_deref_mut() {
            Some(veil) => veil.advance(ix, &flat.text, self.now),
            None => Vec::new(),
        };
        MdText::new(
            ix,
            text,
            flat.segs.clone(),
            veil,
            Rc::new(flat.links.clone()),
            self.sel.clone(),
            self.style.clone(),
        )
    }

    /// The index the next text element will get.
    pub fn peek_ix(&self) -> usize {
        self.ix
    }

    /// The texts of the build, handed to the selection once it is done.
    pub fn finish(self) -> Vec<SharedString> {
        self.texts
    }
}

pub fn build(block: &Prepared, cx: &mut BuildCx) -> AnyElement {
    match block {
        Prepared::Paragraph(flat) => div().child(cx.text(flat)).into_any_element(),
        Prepared::Heading(level, flat) => text_style(div(), heading_style(*level))
            .when(*level <= 2, |heading| heading.pt(px(METRICS.space_1)))
            .child(cx.text(flat))
            .into_any_element(),
        Prepared::Code {
            language,
            raw,
            flat,
        } => code::build(language.as_deref(), raw, flat, cx),
        Prepared::Quote(children) => div()
            .flex()
            .flex_row()
            .gap(px(METRICS.space_3))
            .child(
                div()
                    .w(px(QUOTE_BAR_W))
                    .flex_none()
                    .rounded_full()
                    .bg(cx.style.quote_bar),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(METRICS.space_2))
                    .text_color(cx.style.muted)
                    .children(
                        children
                            .iter()
                            .map(|child| build(child, cx))
                            .collect::<Vec<_>>(),
                    ),
            )
            .into_any_element(),
        Prepared::List { start, items } => {
            let marker_color = cx.style.muted;
            div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_1))
                .children(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| {
                            let marker = match start {
                                Some(first) => format!("{}.", first + i as u64),
                                None => "\u{2022}".to_owned(),
                            };
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(METRICS.space_1))
                                .child(
                                    div()
                                        .min_w(px(METRICS.space_4))
                                        .flex_none()
                                        .text_right()
                                        .text_color(marker_color)
                                        .child(marker),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .gap(px(METRICS.space_1))
                                        .children(
                                            item.iter().map(|b| build(b, cx)).collect::<Vec<_>>(),
                                        ),
                                )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_any_element()
        }
        Prepared::Table {
            header,
            rows,
            align,
            widths,
        } => table::build(header, rows, align, widths, cx),
        Prepared::Rule => div()
            .h(px(METRICS.size.divider))
            .my(px(METRICS.space_1))
            .bg(cx.style.rule)
            .into_any_element(),
    }
}

/// Wraps a built top-level block in a node a test can find by its kind and position.
pub fn tagged(block: &Prepared, ix: usize, element: AnyElement) -> AnyElement {
    let kind = block.kind();
    div()
        .debug_selector(move || format!("md-{kind}-{ix}"))
        .child(element)
        .into_any_element()
}
