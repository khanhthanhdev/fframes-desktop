//! Review of what an agent changed: unified and split diffs of each changed file.
//!
//! [`Changes`] is a standalone gpui view. It takes the changed files as [`DiffFile`]s (full text
//! before and after), diffs them by lines, and paints a file list beside the diff. It never
//! applies anything: choices leave as [`ChangesEvent`]s for the owner to act on.
//!
//! Rows are virtualised with a `gpui::list`, so a diff of any size builds only the rows on
//! screen. Code is clipped at the column edge. Highlighting is paint-only: the text is laid out
//! as is and `agent::syntax` spans only change the colour of its runs.

mod file_list;
mod host;
mod model;
mod paint;
mod source;
mod split;
mod strips;
#[cfg(test)]
mod tests;
mod unified;

use std::collections::{BTreeSet, HashSet};

use gpui::{
    AnyElement, ClickEvent, Context, EventEmitter, InteractiveElement, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, list, px,
};
use studio_theme::{METRICS, MONO_FAMILY, TextStyle, Theme, text_style};

pub use host::{ReviewHost, ReviewHostEvent, ReviewRequest, ReviewTarget};
pub use model::{DiffFile, DiffMode, NotShown};
pub use source::Read;
pub use source::{Facts, Loaded, ReviewBackend, facts, map_diffs};

use model::{DiffRow, FileDiff, flatten_rows};
use paint::{Colors, FILE_HEADER_H, HUNK_HEADER_H, Highlights, NOTICE_H, RowStyle};

use crate::kit::{Button, ButtonSize, Emphasis, Icon, SegmentOption, SegmentedControl, icon};

/// Rows kept built beyond the viewport edges.
const OVERDRAW_PX: f32 = 600.;

/// Where the changes are in their life, which decides what a reviewer can do with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Review {
    /// Not applied yet: each file has an accept toggle and rejected files can be dropped.
    #[default]
    Pending,
    /// Already applied: each file can be reverted.
    Applied,
}

/// What the viewer asks its owner to do. The viewer performs none of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesEvent {
    /// A file's accept toggle was flipped.
    FileToggled { path: String, accepted: bool },
    /// Drop these files (every file currently toggled off) from the candidate.
    Reject(Vec<String>),
    /// Undo the change to this one file after apply.
    RevertFile(String),
    /// Every file is toggled off and the reviewer confirmed: drop the whole candidate. This is
    /// not a Reject, which keeps the files that stay on.
    Discard,
    /// Leave the viewer.
    Close,
}

impl EventEmitter<ChangesEvent> for Changes {}

/// The diffs and syntax highlights of a listing: the expensive part of showing it, done once
/// and movable between threads.
pub struct Prepared {
    diffs: Vec<FileDiff>,
    highlights: Vec<Highlights>,
}

impl Prepared {
    pub fn compute(files: &[DiffFile]) -> Self {
        Self {
            diffs: files.iter().map(FileDiff::compute).collect(),
            highlights: files
                .iter()
                .map(|file| {
                    Highlights::compute(&file.path, file.before.as_deref(), file.after.as_deref())
                })
                .collect(),
        }
    }
}

pub struct Changes {
    files: Vec<FileDiff>,
    highlights: Vec<Highlights>,
    mode: DiffMode,
    /// Long lines wrap onto more rows instead of clipping at the column edge.
    wrap: bool,
    review: Review,
    collapsed: HashSet<String>,
    rejected: BTreeSet<String>,
    /// Files changed that the owner could not list, shown as one notice and never as a row.
    more: usize,
    /// An operation is under way: nothing can be rejected or reverted until it settles.
    busy: bool,
    /// Asking whether to discard the whole candidate, because every file is toggled off.
    confirm_discard: bool,
    /// Files already reverted, which show as such and offer no Revert.
    reverted: HashSet<String>,
    rows: Vec<DiffRow>,
    /// Row index of each file's header.
    headers: Vec<usize>,
    list: ListState,
}

impl Changes {
    pub fn new(files: Vec<DiffFile>) -> Self {
        let mut changes = Self {
            files: Vec::new(),
            highlights: Vec::new(),
            mode: DiffMode::default(),
            wrap: true,
            review: Review::default(),
            collapsed: HashSet::new(),
            rejected: BTreeSet::new(),
            more: 0,
            busy: false,
            confirm_discard: false,
            reverted: HashSet::new(),
            rows: Vec::new(),
            headers: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX)),
        };
        changes.load(files);
        changes
    }

    /// Replaces the files. Collapse and accept choices survive for paths that are still there.
    pub fn set_files(&mut self, files: Vec<DiffFile>, cx: &mut Context<Self>) {
        self.load(files);
        cx.notify();
    }

    /// Replaces the files with diffs that were computed elsewhere, so a large listing does
    /// not hold the UI thread.
    pub fn set_prepared(&mut self, prepared: Prepared, cx: &mut Context<Self>) {
        self.load_prepared(prepared);
        cx.notify();
    }

    fn load(&mut self, files: Vec<DiffFile>) {
        self.load_prepared(Prepared::compute(&files));
    }

    fn load_prepared(&mut self, prepared: Prepared) {
        self.files = prepared.diffs;
        self.highlights = prepared.highlights;
        let present: HashSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        self.collapsed
            .retain(|path| present.contains(path.as_str()));
        self.rejected.retain(|path| present.contains(path.as_str()));
        self.confirm_discard = false;
        self.rebuild();
    }

    pub fn set_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.rebuild();
            cx.notify();
        }
    }

    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.list.reset(self.rows.len());
            cx.notify();
        }
    }

    pub fn wrap(&self) -> bool {
        self.wrap
    }

    pub fn set_review(&mut self, review: Review, cx: &mut Context<Self>) {
        if self.review != review {
            self.review = review;
            cx.notify();
        }
    }

    /// How many changed files are not listed (the owner's listing was cut short).
    pub fn set_more(&mut self, more: usize, cx: &mut Context<Self>) {
        if self.more != more {
            self.more = more;
            cx.notify();
        }
    }

    pub fn more(&self) -> usize {
        self.more
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// While an operation is under way the toggles, Reject and Revert are disabled.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            self.confirm_discard = false;
            cx.notify();
        }
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// The files that were reverted already.
    pub fn set_reverted(&mut self, reverted: HashSet<String>, cx: &mut Context<Self>) {
        if self.reverted != reverted {
            self.reverted = reverted;
            cx.notify();
        }
    }

    /// Whether the reviewer asked to drop every listed file, which is a discard of the whole
    /// candidate rather than a rejection of some files. Not so while some changed files are
    /// not listed: then the owner decides what the rest means.
    pub fn rejects_everything(&self) -> bool {
        !self.files.is_empty() && self.more == 0 && self.rejected.len() == self.files.len()
    }

    pub fn confirming_discard(&self) -> bool {
        self.confirm_discard
    }

    /// The files toggled off, in file order.
    pub fn rejected(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|file| self.rejected.contains(&file.path))
            .map(|file| file.path.clone())
            .collect()
    }

    fn rebuild(&mut self) {
        let (rows, headers) = flatten_rows(&self.files, self.mode, |ix| {
            self.collapsed.contains(&self.files[ix].path)
        });
        self.rows = rows;
        self.headers = headers;
        self.list.reset(self.rows.len());
    }

    /// Scrolls so the file's header is at the top.
    fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        let header = self
            .files
            .iter()
            .position(|file| file.path == path)
            .and_then(|ix| self.headers.get(ix));
        if let Some(&item_ix) = header {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: Pixels::ZERO,
            });
            cx.notify();
        }
    }

    fn toggle_collapsed(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_owned());
        }
        self.rebuild();
        self.reveal(path, cx);
    }

    fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = if collapsed {
            self.files.iter().map(|file| file.path.clone()).collect()
        } else {
            HashSet::new()
        };
        self.rebuild();
        cx.notify();
    }

    fn set_accepted(&mut self, path: &str, accepted: bool, cx: &mut Context<Self>) {
        self.confirm_discard = false;
        if accepted {
            self.rejected.remove(path);
        } else {
            self.rejected.insert(path.to_owned());
        }
        cx.emit(ChangesEvent::FileToggled {
            path: path.to_owned(),
            accepted,
        });
        cx.notify();
    }

    fn file_header(
        &self,
        file_ix: usize,
        theme: &Theme,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let file = &self.files[file_ix];
        let path = file.path.clone();
        let collapsed = self.collapsed.contains(&file.path);
        let revert_path = file.path.clone();
        let mut header = text_style(div(), TextStyle::Label)
            .id(SharedString::from(format!("changes-file-{file_ix}")))
            .w_full()
            .h(px(FILE_HEADER_H))
            .px(px(METRICS.space_3))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(METRICS.space_2))
            .bg(colors.header)
            .border_b_1()
            .border_color(colors.line)
            .text_color(colors.text)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.toggle_collapsed(&path, cx);
            }))
            .child(icon(
                if collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                },
                METRICS.size.icon_sm,
                colors.muted,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(MONO_FAMILY)
                    .child(SharedString::from(file.path.clone())),
            );
        if file.not_shown.is_none() {
            header = header
                .child(
                    div()
                        .text_color(colors.add)
                        .child(SharedString::from(format!("+{}", file.additions))),
                )
                .child(
                    div()
                        .text_color(colors.del)
                        .child(SharedString::from(format!("-{}", file.deletions))),
                );
        }
        if self.review == Review::Applied && self.reverted.contains(&file.path) {
            header = header.child(
                div()
                    .debug_selector(move || format!("changes-reverted-{file_ix}"))
                    .text_color(colors.muted)
                    .child("Reverted"),
            );
        } else if self.review == Review::Applied {
            let busy = self.busy;
            header = header.child(
                div()
                    .debug_selector(move || format!("changes-revert-{file_ix}"))
                    .child(
                        Button::new(
                            theme,
                            SharedString::from(format!("changes-revert-button-{file_ix}")),
                        )
                        .emphasis(Emphasis::Neutral)
                        .size(ButtonSize::Sm)
                        .label("Revert")
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |_, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                cx.emit(ChangesEvent::RevertFile(revert_path.clone()));
                            },
                        )),
                    ),
            );
        }
        header.into_any_element()
    }

    fn row_style<'a>(&'a self, file: u32, theme: &'a Theme, colors: &'a Colors) -> RowStyle<'a> {
        RowStyle {
            highlights: &self.highlights[file as usize],
            max_line: self.files[file as usize].max_line,
            wrap: self.wrap,
            theme,
            colors,
        }
    }

    fn row(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(&row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let theme = Theme::for_window(window, cx);
        let colors = Colors::new(&theme);
        match row {
            DiffRow::FileHeader { file } => self.file_header(file as usize, &theme, &colors, cx),
            DiffRow::Notice { file, notice } => {
                let text = self.files[file as usize].notices[notice as usize].clone();
                text_style(div(), TextStyle::Label)
                    .debug_selector(move || format!("changes-notice-{file}-{notice}"))
                    .w_full()
                    .h(px(NOTICE_H))
                    .px(px(METRICS.space_3))
                    .flex()
                    .items_center()
                    .text_color(colors.muted)
                    .child(SharedString::from(text))
                    .into_any_element()
            }
            DiffRow::HunkHeader { file, hunk } => {
                let header = self.files[file as usize].hunks[hunk as usize]
                    .header
                    .clone();
                text_style(div(), TextStyle::CodeSmall)
                    .w_full()
                    .h(px(HUNK_HEADER_H))
                    .px(px(METRICS.space_3))
                    .flex()
                    .items_center()
                    .bg(colors.hunk_header)
                    .text_color(colors.faint)
                    .child(SharedString::from(header))
                    .into_any_element()
            }
            DiffRow::Line { file, hunk, line } => {
                let f = &self.files[file as usize];
                let line = &f.hunks[hunk as usize].lines[line as usize];
                let style = self.row_style(file, &theme, &colors);
                unified::row(line, &style)
                    .debug_selector(move || format!("changes-line-{ix}"))
                    .into_any_element()
            }
            DiffRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                let hunk = &self.files[file as usize].hunks[hunk as usize];
                let style = self.row_style(file, &theme, &colors);
                split::row(hunk, left, right, &style)
                    .debug_selector(move || format!("changes-split-{ix}"))
                    .into_any_element()
            }
        }
    }

    fn toolbar(&self, theme: &Theme, colors: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let (files, additions, deletions) = file_list::totals(&self.files);
        let rejected = self.rejected();
        let all_collapsed = !self.files.is_empty() && self.collapsed.len() == self.files.len();
        let mode = self.mode;
        let wrap = self.wrap;
        let mut bar = text_style(div(), TextStyle::Label)
            .w_full()
            .h(px(METRICS.layout.toolbar_h))
            .flex_none()
            .px(px(METRICS.space_3))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(METRICS.space_2))
            .border_b_1()
            .border_color(colors.line)
            .text_color(colors.text)
            .child(SharedString::from(format!(
                "{files} {}",
                if files == 1 { "file" } else { "files" }
            )))
            .child(
                div()
                    .text_color(colors.add)
                    .child(SharedString::from(format!("+{additions}"))),
            )
            .child(
                div()
                    .text_color(colors.del)
                    .child(SharedString::from(format!("-{deletions}"))),
            )
            .child(div().flex_1())
            .child(
                div().debug_selector(|| "changes-mode".into()).child(
                    SegmentedControl::new(
                        theme,
                        "changes-mode-control",
                        vec![
                            SegmentOption::new("Unified", mode == DiffMode::Unified),
                            SegmentOption::new("Split", mode == DiffMode::Split),
                        ],
                    )
                    .on_select(cx.listener(|this, ix: &usize, _, cx| {
                        let mode = if *ix == 0 {
                            DiffMode::Unified
                        } else {
                            DiffMode::Split
                        };
                        this.set_mode(mode, cx);
                    })),
                ),
            )
            .child(
                Button::new(theme, "changes-collapse")
                    .emphasis(Emphasis::Neutral)
                    .label(if all_collapsed {
                        "Expand all"
                    } else {
                        "Collapse all"
                    })
                    .disabled(self.files.is_empty())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_all_collapsed(!all_collapsed, cx);
                    })),
            )
            .child(
                div().debug_selector(|| "changes-wrap".into()).child(
                    Button::new(theme, "changes-wrap")
                        .emphasis(Emphasis::Neutral)
                        .label(if wrap { "No wrap" } else { "Wrap" })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.set_wrap(!wrap, cx);
                        })),
                ),
            );
        if self.review == Review::Pending {
            bar = bar.child(self.reject_button(rejected, theme, cx));
        }
        bar.child(
            div().debug_selector(|| "changes-close".into()).child(
                Button::new(theme, "changes-close-button")
                    .emphasis(Emphasis::Neutral)
                    .label("Close")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                        cx.emit(ChangesEvent::Close);
                    })),
            ),
        )
        .into_any_element()
    }
}

impl Render for Changes {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let colors = Colors::new(&theme);
        let body = if self.files.is_empty() {
            text_style(div(), TextStyle::Body)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.muted)
                .child("No changes")
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .child(self.file_list(&theme, &colors, cx))
                .child(
                    div().flex_1().min_w_0().h_full().child(
                        list(
                            self.list.clone(),
                            cx.processor(|this, ix: usize, window, cx| this.row(ix, window, cx)),
                        )
                        .size_full(),
                    ),
                )
                .into_any_element()
        };
        div()
            .id("changes")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.card)
            .child(self.toolbar(&theme, &colors, cx))
            .children(self.confirm_strip(&theme, &colors, cx))
            .child(body)
            .children(self.more_footer(&colors))
    }
}
