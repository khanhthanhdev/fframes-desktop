//! The editing model: selection, motion, edits, undo and the clipboard.

use super::*;

impl TextInputState {
    pub(super) fn new(multi_line: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        register_live_input(cx.weak_entity(), cx);
        // A field is a tab stop until it is disabled.
        let focus = cx.focus_handle().tab_stop(true);
        let blur = cx.on_blur(&focus, window, |this: &mut Self, _window, cx| {
            this.drag = None;
            cx.emit(TextInputEvent::Blurred);
        });
        Self {
            focus,
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            multi_line,
            disabled: false,
            layout: Rc::new(RefCell::new(None)),
            text_bounds: None,
            line_height: px(16.),
            scroll_x: px(0.),
            drag: None,
            goal_x: None,
            align: TextAlign::Left,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_group: None,
            _blur: blur,
        }
    }

    /// A one-line field: Return commits, no wrapping, horizontal scroll.
    pub fn single_line(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(false, window, cx)
    }

    /// A wrapping field: Return inserts a newline and the element measures its
    /// own height from the row count.
    pub fn multi_line(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(true, window, cx)
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(super) fn invalidate_layout(&self) {
        *self.layout.borrow_mut() = None;
    }

    pub(super) fn shared_layout(&self) -> Rc<RefCell<Option<InputLayout>>> {
        self.layout.clone()
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn shared_text(&self) -> SharedString {
        self.content.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.focus = self.focus.clone().tab_stop(!disabled);
            cx.notify();
        }
    }

    /// An IME composition is in progress: Return and Escape belong to the IME,
    /// and so do Tab and the other keys an owner routes itself.
    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    /// Where the text (inside the field's padding) was last painted, in window
    /// coordinates. `None` until the first paint.
    pub fn text_bounds(&self) -> Option<Bounds<Pixels>> {
        self.text_bounds
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Replace the whole value from outside -- a Reset button, a store value
    /// arriving, a project reload. Emits nothing: the owner already knows.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text: SharedString = text.into();
        if text == self.content {
            return;
        }
        self.content = text;
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.invalidate_layout();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_group = None;
        cx.notify();
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn move_to_end(&mut self, cx: &mut Context<Self>) {
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn has_selection(&self) -> bool {
        !self.selected_range.is_empty()
    }

    pub fn selection(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    /// Focus this field and select everything, the way tabbing into an
    /// `<input>` does.
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.select_all_text(cx);
    }

    pub(super) fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    // -- Motion primitives ----------------------------------------------------

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selection: self.selected_range.clone(),
            reversed: self.selection_reversed,
        }
    }

    /// Push an undo step unless it coalesces with the previous one.
    pub(super) fn push_undo(&mut self, group: UndoGroup) {
        let coalesces = undo_coalesces(self.last_group, group);
        if !coalesces {
            self.undo_stack.push(self.snapshot());
            // A stack this long is a field someone has been typing in for a
            // very long time; the browser's own history is bounded too.
            if self.undo_stack.len() > 256 {
                self.undo_stack.remove(0);
            }
        }
        self.redo_stack.clear();
        self.last_group = Some(group);
    }

    pub(super) fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp_to_boundary(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.last_group = None;
        cx.notify();
    }

    pub(super) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp_to_boundary(offset);
        let (range, reversed) =
            extend_selection(self.selected_range.clone(), self.selection_reversed, offset);
        self.selected_range = range;
        self.selection_reversed = reversed;
        self.last_group = None;
        cx.notify();
    }

    pub(super) fn clamp_to_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        if self.content.is_char_boundary(offset) {
            offset
        } else {
            previous_grapheme(&self.content, offset)
        }
    }

    pub(super) fn previous_boundary(&self, offset: usize) -> usize {
        previous_grapheme(&self.content, offset)
    }

    pub(super) fn next_boundary(&self, offset: usize) -> usize {
        next_grapheme(&self.content, offset)
    }

    /// The offset one visual row up or down, keeping the goal x.
    pub(super) fn vertical(&mut self, delta: isize) -> Option<usize> {
        let layout = self.layout.clone();
        let layout = layout.borrow();
        let layout = layout.as_ref()?;
        let offset = self.cursor_offset();
        let row = layout.row_for_offset(offset);
        let target = row as isize + delta;
        if target < 0 || target as usize >= layout.row_count() {
            return None;
        }
        let goal = self
            .goal_x
            .unwrap_or_else(|| layout.x_for_offset(offset, self.line_height));
        let position = point(goal, self.line_height * target as f32);
        let next = layout.offset_for_point(position, self.line_height);
        self.goal_x = Some(goal);
        Some(next)
    }

    // -- Editing --------------------------------------------------------------

    /// The single place text is replaced. `range` is a byte range in the
    /// content; `None` means the marked range, then the selection.
    pub(super) fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        group: UndoGroup,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        let range = self.clamp_to_boundary(range.start)..self.clamp_to_boundary(range.end);
        let range = range.start..range.end.max(range.start);

        // A single-line field never holds a newline: the same flattening the
        // reference implementation does on paste.
        let text = if self.multi_line {
            text.to_string()
        } else {
            flatten_single_line(text)
        };

        self.push_undo(group);
        let mut next = String::with_capacity(self.content.len() + text.len());
        next.push_str(&self.content[..range.start]);
        next.push_str(&text);
        next.push_str(&self.content[range.end..]);
        self.content = next.into();

        let caret = range.start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        self.invalidate_layout();
        self.goal_x = None;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    pub(super) fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.selected_range = snapshot.selection.start.min(self.content.len())
            ..snapshot.selection.end.min(self.content.len());
        self.selection_reversed = snapshot.reversed;
        self.marked_range = None;
        self.invalidate_layout();
        self.goal_x = None;
        self.last_group = None;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    // -- Action handlers ------------------------------------------------------

    pub(super) fn on_backspace(
        &mut self,
        _: &Backspace,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let previous = self.previous_boundary(self.cursor_offset());
            if previous == self.cursor_offset() {
                return;
            }
            let range = previous..self.cursor_offset();
            self.replace(Some(range), "", UndoGroup::Deleting, cx);
        } else {
            self.replace(None, "", UndoGroup::Discrete, cx);
        }
    }

    pub(super) fn on_delete_forward(
        &mut self,
        _: &DeleteForward,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if next == self.cursor_offset() {
                return;
            }
            let range = self.cursor_offset()..next;
            self.replace(Some(range), "", UndoGroup::Deleting, cx);
        } else {
            self.replace(None, "", UndoGroup::Discrete, cx);
        }
    }

    pub(super) fn on_delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_range.is_empty() {
            return self.replace(None, "", UndoGroup::Discrete, cx);
        }
        let caret = self.cursor_offset();
        let start = word_start_before(&self.content, caret);
        if start < caret {
            self.replace(Some(start..caret), "", UndoGroup::Discrete, cx);
        }
    }

    pub(super) fn on_delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_range.is_empty() {
            return self.replace(None, "", UndoGroup::Discrete, cx);
        }
        let caret = self.cursor_offset();
        let end = word_end_after(&self.content, caret);
        if end > caret {
            self.replace(Some(caret..end), "", UndoGroup::Discrete, cx);
        }
    }

    pub(super) fn on_delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caret = self.cursor_offset();
        let start = paragraph_start(&self.content, caret);
        if start < caret {
            self.replace(Some(start..caret), "", UndoGroup::Discrete, cx);
        }
    }

    pub(super) fn on_delete_to_line_end(
        &mut self,
        _: &DeleteToLineEnd,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caret = self.cursor_offset();
        let end = paragraph_end(&self.content, caret);
        if end > caret {
            self.replace(Some(caret..end), "", UndoGroup::Discrete, cx);
        }
    }

    pub(super) fn on_move_left(
        &mut self,
        _: &MoveLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        if self.selected_range.is_empty() {
            let previous = self.previous_boundary(self.cursor_offset());
            self.move_to(previous, cx);
        } else {
            let start = self.selected_range.start;
            self.move_to(start, cx);
        }
    }

    pub(super) fn on_move_right(
        &mut self,
        _: &MoveRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            self.move_to(next, cx);
        } else {
            let end = self.selected_range.end;
            self.move_to(end, cx);
        }
    }

    pub(super) fn on_move_up(&mut self, _: &MoveUp, _window: &mut Window, cx: &mut Context<Self>) {
        match self.vertical(-1) {
            Some(offset) => {
                let goal = self.goal_x;
                self.move_to(offset, cx);
                self.goal_x = goal;
            }
            None => self.move_to(0, cx),
        }
    }

    pub(super) fn on_move_down(
        &mut self,
        _: &MoveDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.vertical(1) {
            Some(offset) => {
                let goal = self.goal_x;
                self.move_to(offset, cx);
                self.goal_x = goal;
            }
            None => {
                let end = self.content.len();
                self.move_to(end, cx);
            }
        }
    }

    pub(super) fn on_move_word_left(
        &mut self,
        _: &MoveWordLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = word_start_before(&self.content, self.cursor_offset());
        self.move_to(offset, cx);
    }

    pub(super) fn on_move_word_right(
        &mut self,
        _: &MoveWordRight,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = word_end_after(&self.content, self.cursor_offset());
        self.move_to(offset, cx);
    }

    pub(super) fn on_move_to_line_start(
        &mut self,
        _: &MoveToLineStart,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = paragraph_start(&self.content, self.cursor_offset());
        self.move_to(offset, cx);
    }

    pub(super) fn on_move_to_line_end(
        &mut self,
        _: &MoveToLineEnd,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = paragraph_end(&self.content, self.cursor_offset());
        self.move_to(offset, cx);
    }

    pub(super) fn on_move_to_start(
        &mut self,
        _: &MoveToStart,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        self.move_to(0, cx);
    }

    pub(super) fn on_move_to_end(
        &mut self,
        _: &MoveToEnd,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let end = self.content.len();
        self.move_to(end, cx);
    }

    pub(super) fn on_select_left(
        &mut self,
        _: &SelectLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = self.previous_boundary(self.cursor_offset());
        self.select_to(offset, cx);
    }

    pub(super) fn on_select_right(
        &mut self,
        _: &SelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = self.next_boundary(self.cursor_offset());
        self.select_to(offset, cx);
    }

    pub(super) fn on_select_up(
        &mut self,
        _: &SelectUp,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.vertical(-1).unwrap_or(0);
        let goal = self.goal_x;
        self.select_to(offset, cx);
        self.goal_x = goal;
    }

    pub(super) fn on_select_down(
        &mut self,
        _: &SelectDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.vertical(1).unwrap_or(self.content.len());
        let goal = self.goal_x;
        self.select_to(offset, cx);
        self.goal_x = goal;
    }

    pub(super) fn on_select_word_left(
        &mut self,
        _: &SelectWordLeft,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = word_start_before(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    pub(super) fn on_select_word_right(
        &mut self,
        _: &SelectWordRight,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = word_end_after(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    pub(super) fn on_select_to_line_start(
        &mut self,
        _: &SelectToLineStart,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = paragraph_start(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    pub(super) fn on_select_to_line_end(
        &mut self,
        _: &SelectToLineEnd,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let offset = paragraph_end(&self.content, self.cursor_offset());
        self.select_to(offset, cx);
    }

    pub(super) fn on_select_to_start(
        &mut self,
        _: &SelectToStart,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        self.select_to(0, cx);
    }

    pub(super) fn on_select_to_end(
        &mut self,
        _: &SelectToEnd,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goal_x = None;
        let end = self.content.len();
        self.select_to(end, cx);
    }

    pub(super) fn on_select_all(
        &mut self,
        _: &SelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_all_text(cx);
    }

    pub(super) fn on_copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let text = self.content[self.selected_range.clone()].to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub(super) fn on_cut(&mut self, _: &Cut, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() || self.disabled {
            return;
        }
        let text = self.content[self.selected_range.clone()].to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.replace(None, "", UndoGroup::Discrete, cx);
    }

    pub(super) fn on_paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.replace(None, &text, UndoGroup::Discrete, cx);
    }

    pub(super) fn on_undo(&mut self, _: &Undo, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore(snapshot, cx);
    }

    pub(super) fn on_redo(&mut self, _: &Redo, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore(snapshot, cx);
    }

    pub(super) fn on_confirm(&mut self, _: &Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        // Return commits the composition, so it is the IME's while one is open.
        if self.is_composing() {
            cx.propagate();
            return;
        }
        if self.multi_line {
            self.replace(None, "\n", UndoGroup::Discrete, cx);
        } else {
            cx.emit(TextInputEvent::Confirmed);
        }
    }

    pub(super) fn on_cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        // Escape abandons the composition, not the field.
        if self.is_composing() {
            cx.propagate();
            return;
        }
        cx.emit(TextInputEvent::Cancelled);
    }

    pub(super) fn on_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    // -- Pointer --------------------------------------------------------------

    pub(super) fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.text_bounds else {
            return 0;
        };
        let layout = self.layout.borrow();
        let Some(layout) = layout.as_ref() else {
            return 0;
        };
        let row_index = if position.y <= bounds.top() {
            0
        } else {
            ((f32::from(position.y - bounds.top()) / f32::from(self.line_height)) as usize)
                .min(layout.row_count().saturating_sub(1))
        };
        let align_offset = self.align_offset(layout, row_index, bounds.size.width);
        let local = point(
            position.x - bounds.left() - align_offset + self.scroll_x,
            self.line_height * row_index as f32,
        );
        layout.offset_for_point(local, self.line_height)
    }

    pub(super) fn align_offset(&self, layout: &InputLayout, row: usize, width: Pixels) -> Pixels {
        let Some(row) = layout.rows.get(row) else {
            return px(0.);
        };
        match self.align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (width - row.width) / 2.,
            TextAlign::Right => width - row.width,
        }
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let offset = self.offset_for_position(event.position);
        self.goal_x = None;

        // `click_count` is gpui's own, so double- and triple-click come free
        // and use the platform's interval.
        match event.click_count {
            0 | 1 => {
                if event.modifiers.shift {
                    self.select_to(offset, cx);
                    let anchor = self.selected_range.clone();
                    self.drag = Some((DragGranularity::Character, anchor));
                } else {
                    self.move_to(offset, cx);
                    self.drag = Some((DragGranularity::Character, offset..offset));
                }
            }
            2 => {
                let range = word_at(&self.content, offset);
                self.selected_range = range.clone();
                self.selection_reversed = false;
                self.drag = Some((DragGranularity::Word, range));
                cx.notify();
            }
            _ => {
                let range =
                    paragraph_start(&self.content, offset)..paragraph_end(&self.content, offset);
                self.selected_range = range.clone();
                self.selection_reversed = false;
                self.drag = Some((DragGranularity::Line, range));
                cx.notify();
            }
        }
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((granularity, anchor)) = self.drag.clone() else {
            return;
        };
        let offset = self.offset_for_position(event.position);
        let extent = match granularity {
            DragGranularity::Character => offset..offset,
            DragGranularity::Word => word_at(&self.content, offset),
            DragGranularity::Line => {
                paragraph_start(&self.content, offset)..paragraph_end(&self.content, offset)
            }
        };
        let start = anchor.start.min(extent.start);
        let end = anchor.end.max(extent.end);
        self.selection_reversed = extent.start < anchor.start;
        self.selected_range = start..end;
        cx.notify();
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.drag = None;
    }

    // -- utf-16 conversion, which is the protocol's unit ----------------------

    pub(super) fn offset_from_utf16(&self, offset: usize) -> usize {
        offset_from_utf16(&self.content, offset)
    }

    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        offset_to_utf16(&self.content, offset)
    }

    pub(super) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(super) fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl Focusable for TextInputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}
