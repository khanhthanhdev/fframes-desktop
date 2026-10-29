//! A real text input.
//!
//! Every field in this app used to be the same append-only stand-in: focus
//! tracking, `key_char` for the typed character, a static caret drawn at the
//! end of the string, and `Backspace` popping the last one. No caret movement,
//! no selection, no clipboard, no click-to-position. That was the single
//! blocker in front of the editor's text/caption panels, the project rename and
//! the sidebar's hex fields, and it made every field the app already had worse
//! than the `<input>` it was transcribed from.
//!
//! ## The architecture: gpui's own input-handler protocol
//!
//! gpui *does* ship the seam a text editor needs -- it is just not a widget.
//! [`gpui::EntityInputHandler`] is the trait the platform's IME talks to, and
//! [`gpui::ElementInputHandler`] adapts an `Entity<V>` implementing it into the
//! `PlatformInputHandler` that `Window::handle_input` installs during paint
//! (`gpui/src/input.rs:10-96`). On macOS that is `NSTextInputClient`: marked
//! text, dead keys, dictation and the character palette all arrive through
//! `replace_and_mark_text_in_range` / `replace_text_in_range` rather than
//! through `key_char`. gpui's own `examples/input.rs` is the reference
//! implementation and this component is built on the same three pieces
//! (`EntityInputHandler` + a custom `Element` + `window.handle_input`), with
//! wrapping, word/line motion, undo and the app's theming added.
//!
//! **The dispatch order on macOS is the thing to understand before touching
//! any of this** (`gpui_macos/src/window.rs:2143-2250`):
//!
//! 1. A key press runs gpui's own dispatch first -- key *bindings* are matched
//!    against the focused node's context stack, and a matching action is
//!    dispatched and consumes the event (`gpui/src/window.rs:5280-5296`
//!    returns before `finish_dispatch_key_event`, so no `on_key_down` listener
//!    anywhere on the path ever sees it).
//! 2. Only if nothing handled the event does AppKit hand it to the input
//!    context, which composes it and calls back into
//!    `replace_text_in_range` -- the text actually being typed.
//!
//! Two consequences shape the whole design:
//!
//! - **Every chord is a bound action in the `TextInput` key context.**
//!   Backspace, the arrows, select-all, the clipboard, undo and the rest are
//!   [`bind_keys`]'s bindings, scoped to `TextInput`. Because a matched binding
//!   consumes the keystroke, a focused field *structurally* prevents the
//!   editor's Backspace-deletes-the-selection and Cmd-Z-undoes-the-project
//!   handlers from firing. That is the key-context discipline, and it is a
//!   mechanism rather than a check.
//! - **A bare printable key can never be bound**, because a binding would
//!   consume it at step 1 and the IME at step 2 would never run -- the field
//!   would type nothing. So `s`, `c` and `space` still reach an ancestor's
//!   `on_key_down`, and the ancestor has to ask whether a field has focus.
//!   That is exactly what the Tauri app does: `useEditorShortcuts`' scope gate
//!   is `document.activeElement` being an `input`/`textarea`/contenteditable
//!   (`apps/desktop/src/routes/editor/Player.tsx:236-245`), and the timeline's
//!   own listener repeats it (`Timeline/index.tsx:960-966`).
//!   [`text_input_has_focus`] is that gate, transcribed.
//!
//! ## One state machine, one element, two shapes
//!
//! [`TextInputState`] is the whole editing model and is window-agnostic; it
//! emits [`TextInputEvent`]s and the owning window decides what a commit means.
//! [`TextInput`] is the element, with the same per-surface constructors the
//! rest of the kit uses. `multi_line` switches three things and nothing else:
//! Return inserts a newline instead of committing, the text wraps to the
//! element's width, and the element measures its own height.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontWeight, GlobalElementId, Hsla,
    InteractiveElement, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, RenderOnce, SharedString, Size,
    Style, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, WeakEntity, Window,
    WrappedLine, actions, div, fill, point, prelude::*, px, relative, size, svg,
};
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;

use studio_theme::{METRICS, TextStyle, Theme};

mod element;
mod ime;
mod state;
#[cfg(test)]
mod tests;
mod text;

pub use element::TextInput;
use text::*;

// -- Actions and key bindings -------------------------------------------------

actions!(
    studio_text_input,
    [
        /// Delete backwards one grapheme, or the selection.
        Backspace,
        /// Delete forwards one grapheme, or the selection.
        DeleteForward,
        /// alt-backspace on macOS, ctrl-backspace elsewhere.
        DeleteWordLeft,
        /// alt-delete on macOS, ctrl-delete elsewhere.
        DeleteWordRight,
        /// cmd-backspace on macOS.
        DeleteToLineStart,
        /// cmd-delete on macOS.
        DeleteToLineEnd,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveWordLeft,
        MoveWordRight,
        MoveToLineStart,
        MoveToLineEnd,
        MoveToStart,
        MoveToEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectToLineStart,
        SelectToLineEnd,
        SelectToStart,
        SelectToEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        /// Return: a newline in a multi-line field, a commit in a single-line one.
        Confirm,
        /// Escape: the owner decides what reverting means.
        Cancel,
        ShowCharacterPalette,
    ]
);

/// The key context every text input carries. Bindings scoped to it are matched
/// at the field's own node, which is deeper than any window root, so they win
/// and consume the keystroke before an ancestor's `on_key_down` runs.
pub const KEY_CONTEXT: &str = "TextInput";

/// Register the field's bindings. Called once at start.
///
/// Nothing here is a bare printable key -- see the module docs: a binding is
/// matched *before* the IME composes the character, so binding `a` would mean
/// the letter `a` could never be typed.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(key_bindings(cfg!(target_os = "macos")));
}

/// The field's bindings on macOS, or on Windows and Linux. The two differ in
/// more than the command key: on macOS cmd-arrows go to the line ends and
/// alt-arrows move by word; elsewhere ctrl-arrows move by word and home/end go
/// to the line ends.
fn key_bindings(macos: bool) -> Vec<KeyBinding> {
    let ctx = Some(KEY_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", DeleteForward, ctx),
        KeyBinding::new("left", MoveLeft, ctx),
        KeyBinding::new("right", MoveRight, ctx),
        KeyBinding::new("up", MoveUp, ctx),
        KeyBinding::new("down", MoveDown, ctx),
        KeyBinding::new("home", MoveToLineStart, ctx),
        KeyBinding::new("end", MoveToLineEnd, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("shift-home", SelectToLineStart, ctx),
        KeyBinding::new("shift-end", SelectToLineEnd, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("escape", Cancel, ctx),
    ];
    if macos {
        bindings.extend([
            KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("alt-delete", DeleteWordRight, ctx),
            KeyBinding::new("cmd-backspace", DeleteToLineStart, ctx),
            KeyBinding::new("cmd-delete", DeleteToLineEnd, ctx),
            KeyBinding::new("alt-left", MoveWordLeft, ctx),
            KeyBinding::new("alt-right", MoveWordRight, ctx),
            KeyBinding::new("cmd-left", MoveToLineStart, ctx),
            KeyBinding::new("cmd-right", MoveToLineEnd, ctx),
            KeyBinding::new("cmd-up", MoveToStart, ctx),
            KeyBinding::new("cmd-down", MoveToEnd, ctx),
            KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
            KeyBinding::new("cmd-shift-left", SelectToLineStart, ctx),
            KeyBinding::new("cmd-shift-right", SelectToLineEnd, ctx),
            KeyBinding::new("cmd-shift-up", SelectToStart, ctx),
            KeyBinding::new("cmd-shift-down", SelectToEnd, ctx),
            KeyBinding::new("cmd-a", SelectAll, ctx),
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-x", Cut, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("cmd-z", Undo, ctx),
            KeyBinding::new("cmd-shift-z", Redo, ctx),
            KeyBinding::new("cmd-y", Redo, ctx),
            KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, ctx),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("ctrl-delete", DeleteWordRight, ctx),
            KeyBinding::new("ctrl-left", MoveWordLeft, ctx),
            KeyBinding::new("ctrl-right", MoveWordRight, ctx),
            KeyBinding::new("ctrl-home", MoveToStart, ctx),
            KeyBinding::new("ctrl-end", MoveToEnd, ctx),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, ctx),
            KeyBinding::new("ctrl-shift-home", SelectToStart, ctx),
            KeyBinding::new("ctrl-shift-end", SelectToEnd, ctx),
            KeyBinding::new("ctrl-a", SelectAll, ctx),
            KeyBinding::new("ctrl-c", Copy, ctx),
            KeyBinding::new("ctrl-x", Cut, ctx),
            KeyBinding::new("ctrl-v", Paste, ctx),
            KeyBinding::new("shift-insert", Paste, ctx),
            KeyBinding::new("ctrl-insert", Copy, ctx),
            KeyBinding::new("shift-delete", Cut, ctx),
            KeyBinding::new("ctrl-z", Undo, ctx),
            KeyBinding::new("ctrl-y", Redo, ctx),
            KeyBinding::new("ctrl-shift-z", Redo, ctx),
        ]);
    }
    bindings
}

// -- The focus gate for bare printable keys -----------------------------------

/// Every live [`TextInputState`], weakly. Registered on construction and
/// pruned when the entity goes.
#[derive(Default)]
struct LiveInputs(RefCell<Vec<WeakEntity<TextInputState>>>);

impl gpui::Global for LiveInputs {}

/// Does any text input in this window currently hold focus?
///
/// This is `getScopeActive()` inverted (`Player.tsx:236-245`): the shipping app
/// suppresses its editor shortcuts while `document.activeElement` is an
/// `input`, a `textarea` or a contenteditable, and so does every window here
/// that handles bare printable keys in an `on_key_down` listener. Chords do not
/// need it -- their bindings are scoped to the [`KEY_CONTEXT`] and consume the
/// keystroke outright -- but a bare `s` cannot be bound at all without breaking
/// typing, so it is checked instead.
pub fn text_input_has_focus(window: &Window, cx: &App) -> bool {
    let Some(live) = cx.try_global::<LiveInputs>() else {
        return false;
    };
    let inputs = live.0.borrow();
    inputs.iter().any(|weak| {
        weak.upgrade()
            .is_some_and(|entity| entity.read(cx).focus.is_focused(window))
    })
}

fn register_live_input(weak: WeakEntity<TextInputState>, cx: &mut App) {
    if !cx.has_global::<LiveInputs>() {
        cx.set_global(LiveInputs::default());
    }
    let live = cx.global::<LiveInputs>();
    let mut inputs = live.0.borrow_mut();
    inputs.retain(|weak| weak.upgrade().is_some());
    inputs.push(weak);
}

// -- Events -------------------------------------------------------------------

/// What the field tells its owner. The owner keeps the meaning: `Cancel` is
/// "clear the filter, then close the panel" in the main window and "revert to
/// the stored value" in settings, and neither belongs in a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The text changed -- a keystroke, a paste, a delete, an undo.
    Changed,
    /// Return in a single-line field.
    Confirmed,
    /// Escape.
    Cancelled,
    /// Focus left the field. Commit-on-blur lives here.
    Blurred,
}

impl EventEmitter<TextInputEvent> for TextInputState {}

// -- Layout -------------------------------------------------------------------

/// One visual row: a soft-wrapped slice of a paragraph, or a whole short one.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    /// Byte range within the whole content.
    range: Range<usize>,
    /// Index of the paragraph (hard line) this row belongs to.
    paragraph: usize,
    /// Byte offset of the row's start within its paragraph.
    local_start: usize,
    /// The row's laid-out width, which is what the aligner centres on
    /// (`aligned_origin_x`, `gpui/src/text_system/line.rs`).
    width: Pixels,
}

/// The shaped picture of the content, rebuilt only when its key changes.
struct InputLayout {
    lines: SmallVec<[WrappedLine; 1]>,
    /// Byte offset of each paragraph's first character in the whole content.
    paragraph_starts: Vec<usize>,
    /// Index of each paragraph's first visual row.
    paragraph_rows: Vec<usize>,
    rows: Vec<Row>,
    key: LayoutKey,
}

/// What makes a cached layout stale.
#[derive(Debug, Clone, PartialEq)]
struct LayoutKey {
    text: SharedString,
    wrap_width: Option<Pixels>,
    font_size: Pixels,
    line_height: Pixels,
    weight: FontWeight,
    marked: Option<Range<usize>>,
    /// The text colour is shaped into the runs, so a theme switch must reshape.
    color: Hsla,
}

impl InputLayout {
    fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The row a caret at `offset` paints on.
    ///
    /// The later row wins at a soft-wrap boundary, which is where a caret
    /// belongs after moving right onto it. gpui's own `position_for_index`
    /// resolves such an index onto the *previous* row, so the x inside that row
    /// is taken as zero rather than asked for.
    fn row_for_offset(&self, offset: usize) -> usize {
        match self
            .rows
            .binary_search_by(|row| row.range.start.cmp(&offset))
        {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        }
    }

    /// The x of a caret at `offset`, relative to its row's start.
    fn x_for_offset(&self, offset: usize, line_height: Pixels) -> Pixels {
        let row_index = self.row_for_offset(offset);
        let Some(row) = self.rows.get(row_index) else {
            return px(0.);
        };
        if offset <= row.range.start {
            return px(0.);
        }
        let Some(line) = self.lines.get(row.paragraph) else {
            return px(0.);
        };
        let paragraph_start = self.paragraph_starts[row.paragraph];
        let local = offset.saturating_sub(paragraph_start).min(line.len());
        line.position_for_index(local, line_height)
            .map(|position| position.x)
            .unwrap_or(px(0.))
    }

    /// The offset closest to a point given in *text-local* coordinates: x is
    /// already un-aligned and un-scrolled, y is measured from the first row's
    /// top.
    fn offset_for_point(&self, position: Point<Pixels>, line_height: Pixels) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let row_index = if position.y < px(0.) {
            0
        } else {
            ((f32::from(position.y) / f32::from(line_height)) as usize).min(self.rows.len() - 1)
        };
        let row = &self.rows[row_index];
        let Some(line) = self.lines.get(row.paragraph) else {
            return row.range.start;
        };
        let local_row = row_index - self.paragraph_rows[row.paragraph];
        let local_point = point(position.x, line_height * local_row as f32);
        let local = match line.closest_index_for_position(local_point, line_height) {
            Ok(index) => index,
            Err(index) => index,
        };
        self.paragraph_starts[row.paragraph] + local.min(line.len())
    }
}

/// The byte offset each visual row of a wrapped paragraph starts at.
///
/// `WrappedLineLayout::position_for_index` maps an index onto the row it
/// *ends* -- the loop returns as soon as `index <= line_end_ix`, so a wrap
/// boundary resolves onto the row before it. Row `k`'s start is therefore the
/// greatest character boundary whose y is still below `k * line_height`, which
/// is what this binary search looks for.
///
/// Split out from the shaping so it can be exercised without a window:
/// `y_of` is the only thing the text system contributes.
pub(crate) fn row_starts(
    boundaries: &[usize],
    row_count: usize,
    line_height: f32,
    y_of: impl Fn(usize) -> f32,
) -> Vec<usize> {
    let mut starts = Vec::with_capacity(row_count);
    starts.push(0usize);
    if boundaries.is_empty() {
        return starts;
    }
    for row in 1..row_count {
        let target = line_height * row as f32;
        let mut low = 0usize;
        let mut high = boundaries.len() - 1;
        while low < high {
            let mid = low.midpoint(high + 1);
            if y_of(boundaries[mid]) < target {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        starts.push(boundaries[low]);
    }
    starts
}

/// Build the flat row table for one shaped paragraph.
fn rows_for_paragraph(
    line: &WrappedLine,
    line_height: Pixels,
    paragraph: usize,
    paragraph_start: usize,
    rows: &mut Vec<Row>,
) {
    let boundaries: Vec<usize> = line
        .text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(line.text.len()))
        .collect();
    let row_count = line.wrap_boundaries().len() + 1;
    let starts = row_starts(&boundaries, row_count, f32::from(line_height), |index| {
        line.position_for_index(index, line_height)
            .map(|position| f32::from(position.y))
            .unwrap_or(0.)
    });

    for row in 0..row_count {
        let local_start = starts[row];
        let local_end = starts.get(row + 1).copied().unwrap_or(line.text.len());
        // The aligner centres on the row's laid-out width, which is the x of
        // its end index -- `aligned_origin_x` measures exactly the same span
        // (`gpui/src/text_system/line.rs`).
        let width = line
            .position_for_index(local_end, line_height)
            .map(|position| position.x)
            .unwrap_or(px(0.));
        rows.push(Row {
            range: paragraph_start + local_start..paragraph_start + local_end,
            paragraph,
            local_start,
            width,
        });
    }
}

// -- Undo ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Snapshot {
    content: SharedString,
    selection: Range<usize>,
    reversed: bool,
}

/// Whether `next` joins the step `previous` opened rather than starting a new
/// one. A webview `<input>` groups a run of typed characters into one undo
/// step and breaks the group on anything else.
pub(crate) fn undo_coalesces(previous: Option<UndoGroup>, next: UndoGroup) -> bool {
    next != UndoGroup::Discrete && previous == Some(next)
}

/// How a mutation coalesces into the undo stack.
///
/// A webview `<input>` groups a run of typed characters into one undo step and
/// breaks the group on anything else, which is what these three arms are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UndoGroup {
    /// A single typed character. Coalesces with the previous one.
    Typing,
    /// Deleting. Coalesces with previous deletes.
    Deleting,
    /// A paste, a cut, a replaced selection -- always its own step.
    Discrete,
}

// -- The state machine --------------------------------------------------------

/// How far a drag extends the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragGranularity {
    Character,
    Word,
    Line,
}

pub struct TextInputState {
    focus: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    /// utf-8 byte range. `selection_reversed` says which end the caret is on.
    selected_range: Range<usize>,
    selection_reversed: bool,
    /// The IME's composition range, drawn underlined.
    marked_range: Option<Range<usize>>,
    multi_line: bool,
    disabled: bool,

    /// Shared with the element, for the reason gpui's own `TextLayout` is an
    /// `Rc<RefCell<..>>`: `WrappedLine::paint` needs `&mut App`, and a layout
    /// living inside the entity could not be read while the entity is being
    /// updated through that same `App`.
    layout: Rc<RefCell<Option<InputLayout>>>,
    /// Bounds of the *text* (inside the field's padding), in window space.
    text_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    /// Single-line fields scroll horizontally to keep the caret visible.
    scroll_x: Pixels,

    drag: Option<(DragGranularity, Range<usize>)>,
    /// The x an Up/Down run walks along, so a short line does not shorten it.
    goal_x: Option<Pixels>,
    /// Written back by the element: `offset_for_position` has to undo whatever
    /// shift the aligner applied before it can ask the layout for an index.
    align: TextAlign,

    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    last_group: Option<UndoGroup>,

    /// Fires `Blurred` so commit-on-blur has somewhere to live.
    _blur: gpui::Subscription,
}
