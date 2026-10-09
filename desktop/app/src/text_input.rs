use crate::design_system;
use crate::design_system::colors::{ACCENT, BORDER, MUTED, PANEL};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InteractiveElement,
    IntoElement, LayoutId, PaintQuad, ParentElement, Pixels, Render, ShapedLine, SharedString,
    Style, Styled, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, px,
    relative, rgb, rgba, size,
};
use std::ops::Range;

/// Key context shared by every input built on [`TextInput`].
pub const KEY_CONTEXT: &str = "TextInput";

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
    ]
);

/// Binds the editing keys of [`TextInput`] (cursor movement, deletion, selection and
/// clipboard). Call once per application before the first input renders; without it an
/// input still receives composed text (IME and plain typing) but not Backspace, arrows or
/// clipboard shortcuts.
pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("secondary-a", SelectAll, context),
        KeyBinding::new("secondary-c", Copy, context),
        KeyBinding::new("secondary-x", Cut, context),
        KeyBinding::new("secondary-v", Paste, context),
    ]);
}

/// Safely converts a UTF-16 range relative to `text` into a UTF-8 byte range inside `text`.
pub fn utf16_range_to_utf8_in_slice(text: &str, range_utf16: &Range<usize>) -> Range<usize> {
    let mut utf16_count = 0;
    let mut start_byte = None;
    let mut end_byte = None;

    for (byte_idx, ch) in text.char_indices() {
        if utf16_count == range_utf16.start {
            start_byte = Some(byte_idx);
        }
        if utf16_count == range_utf16.end {
            end_byte = Some(byte_idx);
            break;
        }
        utf16_count += ch.len_utf16();
    }

    if start_byte.is_none() {
        start_byte = Some(text.len());
    }
    if end_byte.is_none() {
        end_byte = Some(text.len());
    }

    let s = start_byte.unwrap();
    let e = end_byte.unwrap();
    s.min(e)..e
}

pub struct TextInput {
    pub focus_handle: FocusHandle,
    pub content: SharedString,
    pub placeholder: SharedString,
    pub selected_range: Range<usize>,
    pub selection_reversed: bool,
    pub marked_range: Option<Range<usize>>,
    pub last_layout: Option<ShapedLine>,
    pub last_bounds: Option<Bounds<Pixels>>,
    /// Hide the composition status line unless a composition is in progress.
    pub compact: bool,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            content: "".into(),
            placeholder: "Type composed text here...".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            compact: false,
        }
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.marked_range = None;
        cx.notify();
    }

    /// An IME composition is in progress: Enter/Tab/Escape belong to the IME, not the form.
    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }
    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn marked_text(&self) -> Option<&str> {
        self.marked_range
            .as_ref()
            .and_then(|r| self.content.get(r.clone()))
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        let start = self.offset_to_utf16(range.start);
        let end = self.offset_to_utf16(range.end);
        start..end
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        let start = self.offset_from_utf16(range_utf16.start);
        let end = self.offset_from_utf16(range_utf16.end);
        start..end
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        for (idx, ch) in self.content.char_indices() {
            if idx >= offset {
                break;
            }
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn offset_from_utf16(&self, utf16_offset: usize) -> usize {
        let mut current_utf16 = 0;
        for (idx, ch) in self.content.char_indices() {
            if current_utf16 >= utf16_offset {
                return idx;
            }
            current_utf16 += ch.len_utf16();
        }
        self.content.len()
    }

    pub fn previous_boundary(&self, offset: usize) -> usize {
        let mut prev = 0;
        for (idx, _) in self.content.char_indices() {
            if idx >= offset {
                return prev;
            }
            prev = idx;
        }
        prev
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        for (idx, _) in self.content.char_indices() {
            if idx > offset {
                return idx;
            }
        }
        self.content.len()
    }

    pub fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            self.selected_range = prev..prev;
        } else {
            let start = self.selected_range.start;
            self.selected_range = start..start;
        }
        cx.notify();
    }

    pub fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            self.selected_range = next..next;
        } else {
            let end = self.selected_range.end;
            self.selected_range = end..end;
        }
        cx.notify();
    }

    pub fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let prev = self.previous_boundary(self.cursor_offset());
        self.selected_range = prev..self.selected_range.end;
        cx.notify();
    }

    pub fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.next_boundary(self.cursor_offset());
        self.selected_range = self.selected_range.start..next;
        cx.notify();
    }

    pub fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        cx.notify();
    }

    pub fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..0;
        cx.notify();
    }

    pub fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.content.len();
        self.selected_range = len..len;
        cx.notify();
    }

    pub fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            self.selected_range = prev..self.cursor_offset();
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    pub fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            self.selected_range = self.cursor_offset()..next;
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    pub fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let text = self.content[self.selected_range.clone()].to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    pub fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let text = self.content[self.selected_range.clone()].to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn paste_action(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            self.paste(item, window, cx);
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        self.content.get(range).map(|s| s.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len());

        let mut next_content = String::with_capacity(self.content.len() + new_text.len());
        next_content.push_str(&self.content[..start]);
        next_content.push_str(new_text);
        next_content.push_str(&self.content[end..]);

        self.content = next_content.into();
        let new_pos = start + new_text.len();
        self.selected_range = new_pos..new_pos;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len());

        let mut next_content = String::with_capacity(self.content.len() + new_text.len());
        next_content.push_str(&self.content[..start]);
        next_content.push_str(new_text);
        next_content.push_str(&self.content[end..]);

        self.content = next_content.into();

        if !new_text.is_empty() {
            self.marked_range = Some(start..start + new_text.len());
        } else {
            self.marked_range = None;
        }

        self.selected_range = if let Some(new_sel) = new_selected_range_utf16 {
            let relative = utf16_range_to_utf8_in_slice(new_text, &new_sel);
            (start + relative.start)..(start + relative.end)
        } else {
            let pos = start + new_text.len();
            pos..pos
        };

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start_x = last_layout.x_for_index(range.start);
        let end_x = last_layout.x_for_index(range.end);

        Some(Bounds::from_corners(
            point(bounds.left() + start_x, bounds.top()),
            point(bounds.left() + end_x, bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line_point = bounds.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_idx = last_layout.index_for_x(line_point.x)?;
        Some(self.offset_to_utf16(utf8_idx))
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_focused = self.focus_handle.is_focused(window);

        let composition_info = if let Some(marked) = &self.marked_range {
            format!("Composing (marked): {:?}", &self.content[marked.clone()])
        } else {
            "Composition: idle".to_string()
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("TextInput")
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste_action))
            .p(px(design_system::tokens().metrics.space_2))
            .border_1()
            .border_color(if is_focused { rgb(ACCENT) } else { rgb(BORDER) })
            .bg(rgb(PANEL))
            .rounded(px(design_system::tokens().metrics.radius_md))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(design_system::tokens().metrics.space_1))
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .child(TextElement { input: cx.entity() }),
            )
            .children((!self.compact || self.marked_range.is_some()).then(|| {
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(composition_info)
            }))
    }
}

pub struct TextElement {
    pub input: Entity<TextInput>,
}

pub struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (
                input.placeholder.clone(),
                gpui::Hsla::from(rgb(design_system::tokens().palette.muted)),
            )
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end.saturating_sub(marked_range.start),
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len().saturating_sub(marked_range.end),
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor_quad) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    rgb(design_system::tokens().palette.accent),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(design_system::tokens().palette.selection_overlay),
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            cursor: cursor_quad,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        if let Some(line) = prepaint.line.take() {
            let _ = line.paint(
                bounds.origin,
                window.line_height(),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );

            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }

            self.input.update(cx, |input, _cx| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utf16_range_to_utf8_in_slice_ascii_and_multibyte() {
        let text = "🔥 hello";
        // '🔥' is 4 bytes in UTF-8, 2 code units in UTF-16.
        // ' ' is 1 byte / 1 UTF-16 unit.
        // "hello" starts at UTF-16 offset 3, UTF-8 offset 5.
        let range_utf16 = 3..8;
        let byte_range = utf16_range_to_utf8_in_slice(text, &range_utf16);
        assert_eq!(byte_range, 5..10);
        assert_eq!(&text[byte_range], "hello");

        // Emoji itself: UTF-16 0..2 -> UTF-8 0..4
        let emoji_range = utf16_range_to_utf8_in_slice(text, &(0..2));
        assert_eq!(emoji_range, 0..4);
        assert_eq!(&text[emoji_range], "🔥");
    }
}
