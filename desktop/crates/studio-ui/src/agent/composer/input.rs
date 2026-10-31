//! The multiline text input: content, selection, composition, scrolling and undo.
//!
//! It is a complete text editor built on gpui's [`EntityInputHandler`], so the
//! platform input method, dead keys, selection and the clipboard work as in any
//! native field. The wrapping [`super::Composer`] adds the completion list, the
//! attachment strip and the send controls.
//!
//! The content is plain text: the string the owner reads with [`ComposerInput::text`]
//! is the string the user sees, so every byte offset in this module is also an
//! offset into what gets sent.

use std::{
    ops::Range,
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, DispatchPhase, Element,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ScrollWheelEvent, SharedString,
    Style, Task, TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, WrappedLine, div,
    fill, point, prelude::*, px, relative, size,
};
use studio_theme::Theme;
use unicode_segmentation::UnicodeSegmentation;

use super::keymap::{self, EnterOutcome, KEY_CONTEXT, enter_outcome, escape_dismisses_completion};
use super::metrics::{
    CARET_BLINK_MS, CARET_WIDTH, DRAG_SCROLL_FRAME_MS, FALLBACK_WIDTH, HIT_INSET,
    INPUT_LINE_HEIGHT, INPUT_MAX_HEIGHT, INPUT_REST_TEXT_HEIGHT, MIN_WRAP_WIDTH, PressIntent,
    caret_visible, input_drag_scroll_delta, input_max_scroll, input_scroll_offset,
    input_scroll_offset_for_cursor, press_intent, word_range,
};

mod element;
mod geometry;
mod ime;
mod layout;
mod motion;
mod pointer;
mod render;
#[cfg(test)]
mod tests;

pub use element::ComposerTextElement;

/// How long a run of single-character edits keeps merging into one undo step.
///
/// A pause longer than this starts a fresh step, so undo rewinds in the bursts
/// the user typed rather than one character at a time.
const UNDO_COALESCE: Duration = Duration::from_millis(700);

/// Retained undo steps. A long-lived composer must not grow without bound.
const UNDO_LIMIT: usize = 200;

/// A restorable point in the input's history: text plus where the caret and
/// selection sat when the edit landed.
#[derive(Clone)]
struct EditSnapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    caret_affinity: CaretAffinity,
}

/// Direction of the last edit. A run only merges with edits of its own kind.
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Insert,
    Delete,
}

/// A soft-wrap boundary is both the previous row's end and the next row's
/// start. The visual side of the caret is kept apart from its source offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CaretAffinity {
    Upstream,
    #[default]
    Downstream,
}

/// What the input tells its owner.
#[derive(Debug, Clone)]
pub enum ComposerInputEvent {
    /// Enter with no completion open.
    Submitted,
    /// The send-now chord with no completion open.
    SubmittedNow,
    /// The content changed.
    Edited,
    /// The caret or selection moved.
    CursorMoved,
    /// The measured size or the scroll position changed.
    ViewportChanged,
    /// Up or Down while a completion row is selected.
    CompletionNavigate(isize),
    /// Enter or Tab while a completion row is selected.
    CompletionAccept,
    /// Escape while a completion is open.
    CompletionDismiss,
    /// Images on the clipboard, such as a screenshot.
    PastedImages(Vec<gpui::Image>),
    /// Files copied in a file manager. Nothing here reads them.
    PastedPaths(Vec<PathBuf>),
}

/// The multiline input entity: content, selection, IME marked text and the
/// measured layout used for mouse mapping and auto-grow.
pub struct ComposerInput {
    theme: Theme,
    focus_handle: FocusHandle,
    content: String,
    edit_revision: u64,
    read_only: bool,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    caret_affinity: CaretAffinity,
    marked_range: Option<Range<usize>>,
    /// Kept through vertical moves across short rows; other moves reset it.
    preferred_column: Option<Pixels>,
    is_selecting: bool,
    drag_position: Option<Point<Pixels>>,
    drag_generation: u64,
    drag_unit: Option<(PressIntent, Range<usize>)>,
    drag_autoscroll_active: bool,
    /// Vertical scroll once the content exceeds the maximum height.
    scroll_top: f32,
    /// The tallest the text area grows before it scrolls.
    viewport_height: f32,
    needs_measure: bool,
    last_layout_key: Option<layout::InputLayoutKey>,
    last_notified_layout: Option<(Pixels, f32)>,
    #[cfg(test)]
    layout_rebuilds: usize,
    /// The runs of the last rebuilt layout. The test text system shapes every
    /// run alike, so tests check the requested fonts here.
    #[cfg(test)]
    last_runs: Vec<TextRun>,
    /// Keeps the caret visible through edits and rewraps. Wheel scrolling
    /// pauses it until the next caret move or edit.
    follow_cursor: bool,
    last_lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    content_height: f32,
    last_width: f32,
    display_is_placeholder: bool,
    /// Caret blink anchor, reset on every keystroke and caret move.
    blink_anchor: Instant,
    /// Half-period repaint driver, alive only while the input is focused.
    blink_task: Option<Task<()>>,
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    /// Kind, trailing offset and time of the last edit: the merge test that
    /// decides whether the next edit extends the current undo step.
    last_edit: Option<(EditKind, usize, Instant)>,
    /// The owner shows a completion list; Enter, Tab, Up, Down and Escape go to it.
    completion_open: bool,
    completion_has_selection: bool,
}

impl EventEmitter<ComposerInputEvent> for ComposerInput {}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ComposerInput {
    pub fn new(placeholder: impl Into<SharedString>, theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            focus_handle: cx.focus_handle().tab_stop(true),
            content: String::new(),
            edit_revision: 0,
            read_only: false,
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            caret_affinity: CaretAffinity::Downstream,
            marked_range: None,
            preferred_column: None,
            is_selecting: false,
            drag_position: None,
            drag_generation: 0,
            drag_unit: None,
            drag_autoscroll_active: false,
            scroll_top: 0.0,
            viewport_height: INPUT_MAX_HEIGHT,
            needs_measure: true,
            last_layout_key: None,
            last_notified_layout: None,
            #[cfg(test)]
            layout_rebuilds: 0,
            #[cfg(test)]
            last_runs: Vec::new(),
            follow_cursor: true,
            last_lines: Vec::new(),
            line_starts: vec![0],
            last_bounds: None,
            line_height: px(INPUT_LINE_HEIGHT),
            content_height: INPUT_LINE_HEIGHT,
            last_width: 0.0,
            display_is_placeholder: true,
            blink_anchor: Instant::now(),
            blink_task: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: None,
            completion_open: false,
            completion_has_selection: false,
        }
    }

    /// The tallest the text area grows before it scrolls, in pixels.
    pub fn with_max_height(mut self, height: f32) -> Self {
        self.viewport_height = height.max(INPUT_LINE_HEIGHT);
        self
    }

    /// Recolors the input for another appearance.
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme != theme {
            self.theme = theme;
            self.needs_measure = true;
            cx.notify();
        }
    }

    /// The content, exactly as it would be sent.
    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Whether an input method composition is in progress.
    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    /// The caret's byte offset.
    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// The selected byte range.
    pub fn selection(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    /// Counts every content change, so an async result can tell it is stale.
    pub fn revision(&self) -> u64 {
        self.edit_revision
    }

    /// The height the text area wants at the last measured width.
    pub fn measured_content_height(&self) -> f32 {
        self.content_height
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        self.needs_measure = true;
        cx.notify();
    }

    /// Replaces the whole draft. This is a new document, not an edit, so undo
    /// does not reach back past it.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.marked_range = None;
        self.scroll_top = 0.0;
        self.follow_cursor = true;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    /// Tells the input whether the owner's completion list is open and has a
    /// row selected. While one does, Enter, Tab, Up and Down belong to the list.
    pub fn set_completion_controls(
        &mut self,
        open: bool,
        has_selection: bool,
        cx: &mut Context<Self>,
    ) {
        if self.completion_open == open && self.completion_has_selection == has_selection {
            return;
        }
        self.completion_open = open;
        self.completion_has_selection = has_selection;
        cx.notify();
    }

    /// Replaces `range` with `replacement` as one undo step that never merges
    /// with typing around it. A completion uses this, so accepting one is a
    /// single undo.
    pub fn replace_range_as_step(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        cx: &mut Context<Self>,
    ) {
        if self.read_only
            || self.marked_range.is_some()
            || range.start > range.end
            || !self.content.is_char_boundary(range.start)
            || !self.content.is_char_boundary(range.end)
        {
            return;
        }
        self.last_edit = None;
        self.record_edit(&range, replacement);
        self.last_edit = None;
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content.replace_range(range.clone(), replacement);
        let cursor = range.start + replacement.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    /// Resets the caret blink phase to solid, as typing does in any textarea.
    fn reset_blink(&mut self) {
        self.blink_anchor = Instant::now();
    }

    /// Whether to paint the caret now: a focused input in an active window, in
    /// the "on" blink phase. Arms the half-period repaint driver while focused
    /// and drops it on blur, so an unfocused input schedules no frames.
    fn caret_shown(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let focused = self.focus_handle.is_focused(window);
        if !focused || !window.is_window_active() {
            self.blink_task = None;
            return false;
        }
        if self.blink_task.is_none() {
            self.blink_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(CARET_BLINK_MS))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
        caret_visible(self.blink_anchor.elapsed().as_millis() as u64)
    }

    // ---- undo history ----

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
            caret_affinity: self.caret_affinity,
        }
    }

    /// Called with the range about to be replaced, BEFORE the content changes,
    /// so the pushed snapshot is the pre-edit state.
    fn record_edit(&mut self, range: &Range<usize>, new_text: &str) {
        let kind = if new_text.is_empty() {
            EditKind::Delete
        } else {
            EditKind::Insert
        };
        // A run merges only while it stays single-character, contiguous with
        // the previous edit, of the same kind, and inside the idle window. A
        // pause, a word break, a paste, or a caret jump all break the run so
        // undo lands on a boundary the user recognizes.
        let coalescible = match kind {
            EditKind::Insert => range.is_empty() && new_text.chars().count() == 1,
            EditKind::Delete => self
                .content
                .get(range.clone())
                .is_some_and(|text| text.graphemes(true).count() == 1),
        };
        let mergeable = coalescible
            && match (kind, &self.last_edit) {
                (EditKind::Insert, Some((EditKind::Insert, at, when))) => {
                    range.is_empty()
                        && range.start == *at
                        && new_text.chars().count() == 1
                        && !new_text.starts_with(['\n', ' ', '\t'])
                        && when.elapsed() < UNDO_COALESCE
                }
                (EditKind::Delete, Some((EditKind::Delete, at, when))) => {
                    range.end == *at && when.elapsed() < UNDO_COALESCE
                }
                _ => false,
            };
        if !mergeable {
            self.undo_stack.push(self.snapshot());
            if self.undo_stack.len() > UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
        }
        // Any fresh edit invalidates the redo branch.
        self.redo_stack.clear();
        let tail = match kind {
            EditKind::Insert => range.start + new_text.len(),
            EditKind::Delete => range.start,
        };
        self.last_edit = coalescible.then(|| (kind, tail, Instant::now()));
    }

    fn restore(&mut self, snapshot: EditSnapshot, cx: &mut Context<Self>) {
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content = snapshot.content;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.caret_affinity = snapshot.caret_affinity;
        self.preferred_column = None;
        self.marked_range = None;
        self.follow_cursor = true;
        // Never merge a subsequent edit into a step that undo just crossed.
        self.last_edit = None;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn undo(&mut self, _: &keymap::Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(previous) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore(previous, cx);
    }

    fn redo(&mut self, _: &keymap::Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore(next, cx);
    }

    // ---- clipboard and line breaks ----

    fn clipboard_selection(&self) -> Option<String> {
        if self.selected_range.is_empty() {
            return None;
        }
        Some(self.content[self.selected_range.clone()].to_string())
    }

    fn copy(&mut self, _: &keymap::Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.clipboard_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &keymap::Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(text) = self.clipboard_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.last_edit = None;
            self.replace_text_in_range(None, "", window, cx);
            self.last_edit = None;
        }
    }

    /// Image data and copied files go to the owner; text is inserted.
    ///
    /// Nothing here reads a file or decodes an image: the owner decides whether
    /// the attachment is allowed.
    fn paste(&mut self, _: &keymap::Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut images: Vec<gpui::Image> = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in &item.entries {
            match entry {
                ClipboardEntry::Image(image) => images.push(image.clone()),
                ClipboardEntry::ExternalPaths(files) => paths.extend(files.paths().iter().cloned()),
                ClipboardEntry::String(_) => {}
            }
        }
        if !images.is_empty() {
            cx.emit(ComposerInputEvent::PastedImages(images));
            return;
        }
        if !paths.is_empty() {
            cx.emit(ComposerInputEvent::PastedPaths(paths));
            return;
        }
        if let Some(text) = item.text() {
            // A paste is its own undo step, even one character long.
            self.last_edit = None;
            self.replace_text_in_range(None, &text, window, cx);
            self.last_edit = None;
        }
    }

    /// Shift+Enter. A line that already ends in CRLF keeps its ending style.
    fn newline(&mut self, _: &keymap::Newline, window: &mut Window, cx: &mut Context<Self>) {
        let line_end = self.line_range_at(self.cursor_offset()).end;
        let newline = if self.line_content_end_at(self.cursor_offset()) < line_end {
            "\r\n"
        } else {
            "\n"
        };
        self.replace_text_in_range(None, newline, window, cx);
    }

    /// Enter: sends, or accepts the selected completion row.
    fn submit(&mut self, _: &keymap::Submit, _: &mut Window, cx: &mut Context<Self>) {
        // Enter and Escape during a composition belong to the input method.
        if self.marked_range.is_some() {
            return;
        }
        match enter_outcome(self.completion_has_selection, EnterOutcome::Submit) {
            EnterOutcome::AcceptCompletion => cx.emit(ComposerInputEvent::CompletionAccept),
            EnterOutcome::Submit => cx.emit(ComposerInputEvent::Submitted),
            EnterOutcome::Newline => unreachable!("submit never inserts a line break"),
        }
    }

    /// The send-now chord: sends, or accepts the selected completion row.
    fn submit_now(&mut self, _: &keymap::SubmitNow, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.is_some() {
            return;
        }
        match enter_outcome(self.completion_has_selection, EnterOutcome::Submit) {
            EnterOutcome::AcceptCompletion => cx.emit(ComposerInputEvent::CompletionAccept),
            EnterOutcome::Submit => cx.emit(ComposerInputEvent::SubmittedNow),
            EnterOutcome::Newline => unreachable!("submit never inserts a line break"),
        }
    }

    /// Tab accepts a selected completion row and otherwise moves focus on.
    fn accept_completion(
        &mut self,
        _: &keymap::AcceptCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion_has_selection {
            cx.emit(ComposerInputEvent::CompletionAccept);
        } else {
            cx.propagate();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if escape_dismisses_completion(&event.keystroke.key, self.completion_open) {
            cx.emit(ComposerInputEvent::CompletionDismiss);
            cx.stop_propagation();
        }
    }
}
