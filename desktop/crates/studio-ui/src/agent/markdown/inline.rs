//! Flattening of inline runs into one text string with styled segments and link ranges.
//!
//! A paragraph is laid out as a single wrapped text element, so its runs are concatenated and the
//! styling of each run becomes a [`Seg`] of the same byte length. Links are classified here, so
//! the colour of a link run says what a click will do: only destinations the link policy would act
//! on are styled and recorded as [`LinkSpan`]s; everything else (including the placeholder of a
//! link still being streamed) is not clickable.

use std::ops::Range;

use gpui::{
    FontStyle, FontWeight, Hsla, StrikethroughStyle, TextRun, TextStyle, UnderlineStyle, px,
};
use studio_markdown::{InlineRun, mend::PENDING_LINK_URL};
use studio_theme::METRICS;

use super::links::{LinkTarget, classify};
use super::style::MdStyle;

/// A stretch of text with one look. Segments tile the text exactly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seg {
    pub len: usize,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub strike: bool,
    pub underline: bool,
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
}

/// The byte range of a clickable link and where it points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub range: Range<usize>,
    pub dest: String,
}

/// Inline runs as one text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flat {
    pub text: String,
    pub segs: Vec<Seg>,
    pub links: Vec<LinkSpan>,
}

impl Flat {
    /// Makes every segment bold (table headers).
    pub fn bold(mut self) -> Self {
        for seg in &mut self.segs {
            seg.bold = true;
        }
        self
    }
}

/// Whether a link to `dest` is shown as a link and can be clicked. `files` says whether project
/// files can be revealed at all (the row knows its project).
pub fn is_clickable(dest: &str, files: bool) -> bool {
    match classify(dest) {
        LinkTarget::External { .. } => true,
        LinkTarget::ProjectFile { .. } => files,
        LinkTarget::NotClickable(_) => false,
    }
}

pub fn flatten(runs: &[InlineRun], style: &MdStyle, files: bool) -> Flat {
    let mut flat = Flat::default();
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let start = flat.text.len();
        flat.text.push_str(&run.text);
        let mut seg = Seg {
            len: run.text.len(),
            bold: run.style.bold,
            italic: run.style.italic,
            mono: run.style.code,
            strike: run.style.strikethrough,
            background: run.style.code.then_some(style.code_bg),
            ..Seg::default()
        };
        if let Some(dest) = &run.style.link {
            if dest == PENDING_LINK_URL {
                // The destination has not streamed in yet: look like a link, do nothing.
                seg.color = Some(style.link);
                seg.underline = true;
            } else if is_clickable(dest, files) {
                seg.color = Some(style.link);
                seg.underline = true;
                let end = start + run.text.len();
                match flat.links.last_mut() {
                    Some(last) if last.range.end == start && last.dest == *dest => {
                        last.range.end = end;
                    }
                    _ => flat.links.push(LinkSpan {
                        range: start..end,
                        dest: dest.clone(),
                    }),
                }
            }
        }
        flat.segs.push(seg);
    }
    flat
}

/// The text runs of `segs` over the surrounding text style `base`.
pub fn runs_for(segs: &[Seg], base: &TextStyle, mono: &gpui::SharedString) -> Vec<TextRun> {
    segs.iter()
        .filter(|s| s.len > 0)
        .map(|seg| {
            let mut font = base.font();
            if seg.mono {
                font.family = mono.clone();
            }
            if seg.bold {
                font.weight = FontWeight::BOLD;
            }
            if seg.italic {
                font.style = FontStyle::Italic;
            }
            let color = seg.color.unwrap_or(base.color);
            TextRun {
                len: seg.len,
                font,
                color,
                background_color: seg.background,
                underline: seg.underline.then_some(UnderlineStyle {
                    thickness: px(METRICS.size.divider),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: seg.strike.then_some(StrikethroughStyle {
                    thickness: px(METRICS.size.divider),
                    color: Some(color),
                }),
            }
        })
        .collect()
}

/// Gives the runs covering `range` a background (the selection wash), splitting at its edges.
pub fn wash(runs: Vec<TextRun>, range: Range<usize>, color: Hsla) -> Vec<TextRun> {
    if range.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut pos = 0;
    for run in runs {
        let (start, end) = (pos, pos + run.len);
        pos = end;
        let (from, to) = (range.start.max(start), range.end.min(end));
        if from >= to {
            out.push(run);
            continue;
        }
        for (s, e, washed) in [(start, from, false), (from, to, true), (to, end, false)] {
            if s == e {
                continue;
            }
            let mut piece = run.clone();
            piece.len = e - s;
            if washed {
                piece.background_color = Some(color);
            }
            out.push(piece);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::font;
    use studio_markdown::InlineStyle;
    use studio_theme::Theme;

    fn style() -> MdStyle {
        MdStyle::new(&Theme::new(studio_theme::Appearance::Dark))
    }

    fn run(text: &str, link: Option<&str>, bold: bool) -> InlineRun {
        InlineRun {
            text: text.into(),
            style: InlineStyle {
                bold,
                link: link.map(str::to_owned),
                ..InlineStyle::default()
            },
        }
    }

    #[test]
    fn runs_concatenate_and_segments_tile_the_text() {
        let flat = flatten(
            &[run("Hello ", None, false), run("world", None, true)],
            &style(),
            true,
        );
        assert_eq!(flat.text, "Hello world");
        assert_eq!(
            flat.segs.iter().map(|s| s.len).sum::<usize>(),
            flat.text.len()
        );
        assert!(flat.segs[1].bold && !flat.segs[0].bold);
    }

    #[test]
    fn an_external_link_is_styled_and_recorded() {
        let flat = flatten(
            &[run("docs", Some("https://example.com/a"), false)],
            &style(),
            true,
        );
        assert_eq!(flat.links.len(), 1);
        assert_eq!(flat.links[0].range, 0..4);
        assert!(flat.segs[0].underline && flat.segs[0].color.is_some());
    }

    #[test]
    fn the_pending_placeholder_looks_like_a_link_and_is_never_recorded() {
        let flat = flatten(
            &[run("soon", Some(PENDING_LINK_URL), false)],
            &style(),
            true,
        );
        assert!(flat.links.is_empty());
        assert!(flat.segs[0].underline);
    }

    #[test]
    fn links_that_the_policy_refuses_are_plain_text() {
        for dest in [
            "javascript:alert(1)",
            "file:///c:/x",
            "mailto:a@b.c",
            "#frag",
            "//host/x",
            "",
        ] {
            let flat = flatten(&[run("x", Some(dest), false)], &style(), true);
            assert!(flat.links.is_empty(), "{dest}");
            assert!(!flat.segs[0].underline, "{dest}");
        }
    }

    #[test]
    fn project_file_links_need_a_known_project() {
        let with = flatten(&[run("f", Some("tools/x.cmd"), false)], &style(), true);
        let without = flatten(&[run("f", Some("tools/x.cmd"), false)], &style(), false);
        assert_eq!(with.links.len(), 1);
        assert!(without.links.is_empty() && !without.segs[0].underline);
    }

    #[test]
    fn the_wash_splits_runs_and_keeps_lengths() {
        let base = TextRun {
            len: 10,
            font: font("t"),
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let out = wash(vec![base], 3..6, gpui::black());
        assert_eq!(out.iter().map(|r| r.len).collect::<Vec<_>>(), vec![3, 3, 4]);
        assert_eq!(out[1].background_color, Some(gpui::black()));
        assert!(out[0].background_color.is_none() && out[2].background_color.is_none());
    }
}
