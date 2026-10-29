//! The element: per-surface constructors, layout, painting and mouse input.

use super::*;

// -- The element --------------------------------------------------------------

/// The chrome around the text: fill, border, radius, icon, padding.
#[derive(IntoElement)]
pub struct TextInput {
    state: Entity<TextInputState>,
    id: ElementId,
    icon: Option<SharedString>,
    height: Option<Pixels>,
    padding_x: Pixels,
    padding_y: Pixels,
    radius: Pixels,
    gap: Pixels,
    text_size: Pixels,
    line_height: Option<Pixels>,
    weight: FontWeight,
    align: TextAlign,
    bg: Option<Hsla>,
    border: Option<Hsla>,
    focus_border: Option<Hsla>,
    text: Hsla,
    muted: Hsla,
    icon_color: Hsla,
    caret_color: Hsla,
    selection_color: Hsla,
    flex: bool,
    full_width: bool,
    fixed_width: Option<Pixels>,
    fit_content: bool,
}

/// The field's horizontal padding and the gap before its leading icon.
const PAD_X: f32 = 8.;
const ICON_GAP: f32 = 4.;

impl TextInput {
    /// A borderless `ctl` field, `input_h` high, that shows an `accent` border while
    /// focused; `text_1` text and caret, `text_3` placeholder, `selection` highlight.
    pub fn new(theme: &Theme, id: impl Into<ElementId>, state: &Entity<TextInputState>) -> Self {
        let colors = theme.colors;
        Self {
            state: state.clone(),
            id: id.into(),
            icon: None,
            height: Some(px(METRICS.size.input_h)),
            padding_x: px(PAD_X),
            padding_y: px(0.),
            radius: px(METRICS.radius.control),
            gap: px(METRICS.space_1 / 2.),
            text_size: px(TextStyle::Body.size()),
            line_height: None,
            weight: FontWeight::NORMAL,
            align: TextAlign::Left,
            bg: Some(Hsla::from(colors.ctl)),
            border: None,
            focus_border: Some(Hsla::from(colors.focus)),
            text: Hsla::from(colors.text_1),
            muted: Hsla::from(colors.text_3),
            icon_color: Hsla::from(colors.text_3),
            caret_color: Hsla::from(colors.text_1),
            selection_color: Hsla::from(colors.selection),
            flex: false,
            full_width: false,
            fixed_width: None,
            fit_content: false,
        }
    }

    /// A search field: a leading magnifier, and the field takes the row's free width.
    pub fn search(mut self) -> Self {
        self.icon = Some(SharedString::from("icons/search.svg"));
        self.gap = px(ICON_GAP);
        self.flex = true;
        self
    }
    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = size;
        self
    }

    pub fn line_height(mut self, height: Pixels) -> Self {
        self.line_height = Some(height);
        self
    }

    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    pub fn padding_x(mut self, padding: Pixels) -> Self {
        self.padding_x = padding;
        self
    }

    pub fn padding_y(mut self, padding: Pixels) -> Self {
        self.padding_y = padding;
        self
    }

    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = radius;
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.fixed_width = Some(width);
        self
    }

    pub fn flex(mut self, flex: bool) -> Self {
        self.flex = flex;
        self
    }

    /// Size to the text rather than to the parent.
    ///
    /// `NameEditor` is an `<input>` laid over a hidden `<span>` whose only job
    /// is to measure the value (`Header.tsx:284-290, 325-329`), so the field is
    /// exactly as wide as its text and the literal `.cap` hugs it. The field
    /// here paints its own glyphs, so it can measure itself and the span is not
    /// needed -- but the *sizing* has to be the span's, not the parent's.
    pub fn fit_content(mut self) -> Self {
        self.fit_content = true;
        self.full_width = false;
        self
    }
}

// -- Rendering ----------------------------------------------------------------

/// Bind one action to a method on the state entity.
///
/// `RenderOnce::render` is handed an `&mut App` rather than a `Context<V>`, so
/// `cx.listener(..)` is not available here; the handler closes over the state
/// entity instead, which is what makes the component window-agnostic.
macro_rules! input_actions {
    ($element:expr, $state:expr, $( $action:ty => $method:ident ),* $(,)?) => {{
        let element = $element;
        $(
            let element = element.on_action({
                let state = $state.clone();
                move |action: &$action, window: &mut Window, cx: &mut App| {
                    state.update(cx, |this, cx| this.$method(action, window, cx));
                }
            });
        )*
        element
    }};
}

impl RenderOnce for TextInput {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.clone();
        let (focus, multi_line, disabled) = {
            let read = state.read(cx);
            (read.focus.clone(), read.multi_line, read.disabled)
        };
        let focused = focus.is_focused(window);
        // `leading-normal` on a `text-xs` input is 1.35 in this font stack;
        // the callers that care (the teleprompter) pass their own.
        let line_height = self
            .line_height
            .unwrap_or(px((f32::from(self.text_size) * 1.35).round()));

        let body = TextBody {
            layout: state.read(cx).shared_layout(),
            state: state.clone(),
            fit_content: self.fit_content,
            text_size: self.text_size,
            line_height,
            weight: self.weight,
            align: self.align,
            color: self.text,
            placeholder_color: self.muted,
            caret_color: self.caret_color,
            selection_color: self.selection_color,
            multi_line,
            focused,
        };

        let border = if focused {
            self.focus_border.or(self.border)
        } else {
            self.border
        };

        let element = div()
            .id(self.id.clone())
            .key_context(KEY_CONTEXT)
            .track_focus(&focus)
            .relative()
            .flex()
            .flex_row()
            .when(multi_line, |this| this.items_start())
            .when(!multi_line, |this| this.items_center())
            .gap(self.gap)
            .when(self.flex, |this| this.flex_1().min_w_0())
            .when(self.full_width, |this| this.w_full())
            .when(self.fit_content, |this| this.min_w_0())
            .when_some(self.fixed_width, |this, width| this.w(width))
            .when_some(self.height, |this, height| this.h(height))
            .px(self.padding_x)
            .py(self.padding_y)
            .rounded(self.radius)
            .overflow_hidden()
            .when_some(self.bg, |this, bg| this.bg(bg))
            .when_some(border, |this, border| this.border_1().border_color(border))
            .when(!disabled, |this| this.cursor(gpui::CursorStyle::IBeam))
            .text_size(self.text_size)
            .text_color(self.text)
            .children(self.icon.map(|icon| {
                svg()
                    .path(icon)
                    .size(px(TextStyle::Label.size()))
                    .flex_shrink_0()
                    .text_color(self.icon_color)
            }))
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    state.update(cx, |this, cx| this.on_mouse_down(event, window, cx));
                }
            });

        let element = input_actions!(
            element,
            state,
            Backspace => on_backspace,
            DeleteForward => on_delete_forward,
            DeleteWordLeft => on_delete_word_left,
            DeleteWordRight => on_delete_word_right,
            DeleteToLineStart => on_delete_to_line_start,
            DeleteToLineEnd => on_delete_to_line_end,
            MoveLeft => on_move_left,
            MoveRight => on_move_right,
            MoveUp => on_move_up,
            MoveDown => on_move_down,
            MoveWordLeft => on_move_word_left,
            MoveWordRight => on_move_word_right,
            MoveToLineStart => on_move_to_line_start,
            MoveToLineEnd => on_move_to_line_end,
            MoveToStart => on_move_to_start,
            MoveToEnd => on_move_to_end,
            SelectLeft => on_select_left,
            SelectRight => on_select_right,
            SelectUp => on_select_up,
            SelectDown => on_select_down,
            SelectWordLeft => on_select_word_left,
            SelectWordRight => on_select_word_right,
            SelectToLineStart => on_select_to_line_start,
            SelectToLineEnd => on_select_to_line_end,
            SelectToStart => on_select_to_start,
            SelectToEnd => on_select_to_end,
            SelectAll => on_select_all,
            Copy => on_copy,
            Cut => on_cut,
            Paste => on_paste,
            Undo => on_undo,
            Redo => on_redo,
            Confirm => on_confirm,
            Cancel => on_cancel,
            ShowCharacterPalette => on_character_palette,
        );

        element.child(div().flex_1().min_w_0().child(body))
    }
}

#[derive(Clone)]
struct TextBody {
    state: Entity<TextInputState>,
    layout: Rc<RefCell<Option<InputLayout>>>,
    fit_content: bool,
    text_size: Pixels,
    line_height: Pixels,
    weight: FontWeight,
    align: TextAlign,
    color: Hsla,
    placeholder_color: Hsla,
    caret_color: Hsla,
    selection_color: Hsla,
    multi_line: bool,
    focused: bool,
}

struct BodyPrepaint {
    placeholder: Option<gpui::ShapedLine>,
    selection: Vec<gpui::PaintQuad>,
    caret: Option<gpui::PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextBody {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TextBody {
    /// Shape the content if the cached layout is stale, and hand back the size
    /// it occupies. The only place `shape_text` is called.
    pub(super) fn ensure_layout(
        &self,
        wrap_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        let mut font = window.text_style().font();
        font.weight = self.weight;

        let (text, marked) = {
            let read = self.state.read(cx);
            (read.content.clone(), read.marked_range.clone())
        };
        let key = LayoutKey {
            text: text.clone(),
            wrap_width,
            font_size: self.text_size,
            line_height: self.line_height,
            weight: self.weight,
            marked: marked.clone(),
            color: self.color,
        };

        if let Some(layout) = self.layout.borrow().as_ref()
            && layout.key == key
        {
            return layout_size(layout, self.line_height);
        }

        // The composing run is underlined, which is the whole visual contract
        // of marked text.
        let base = TextRun {
            len: text.len(),
            font: font.clone(),
            color: self.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs: Vec<TextRun> = match marked.as_ref() {
            Some(marked) if marked.end <= text.len() && marked.start <= marked.end => vec![
                TextRun {
                    len: marked.start,
                    ..base.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(self.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..base.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..base
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            _ => vec![base],
        };

        let lines = window
            .text_system()
            .shape_text(text, self.text_size, &runs, wrap_width, None)
            .unwrap_or_default();

        let mut paragraph_starts = Vec::with_capacity(lines.len());
        let mut paragraph_rows = Vec::with_capacity(lines.len());
        let mut rows = Vec::new();
        let mut offset = 0usize;
        for (index, line) in lines.iter().enumerate() {
            paragraph_starts.push(offset);
            paragraph_rows.push(rows.len());
            rows_for_paragraph(line, self.line_height, index, offset, &mut rows);
            // +1 for the `\n` that split this paragraph from the next.
            offset += line.text.len() + 1;
        }
        if rows.is_empty() {
            paragraph_starts.push(0);
            paragraph_rows.push(0);
            rows.push(Row {
                range: 0..0,
                paragraph: 0,
                local_start: 0,
                width: px(0.),
            });
        }

        let layout = InputLayout {
            lines,
            paragraph_starts,
            paragraph_rows,
            rows,
            key,
        };
        let size = layout_size(&layout, self.line_height);
        *self.layout.borrow_mut() = Some(layout);
        self.state.update(cx, |state, _| {
            state.line_height = self.line_height;
            state.align = self.align;
        });
        size
    }
}

pub(super) fn layout_size(layout: &InputLayout, line_height: Pixels) -> Size<Pixels> {
    let width = layout
        .rows
        .iter()
        .map(|row| row.width)
        .fold(px(0.), |a: Pixels, b| if b > a { b } else { a });
    size(width, line_height * layout.rows.len() as f32)
}

impl Element for TextBody {
    type RequestLayoutState = ();
    type PrepaintState = BodyPrepaint;

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

        if !self.multi_line && !self.fit_content {
            // One row, always: the height is the line box and the width is
            // whatever the flex parent gives.
            style.size.width = relative(1.).into();
            style.size.height = self.line_height.into();
            return (window.request_layout(style, [], cx), ());
        }

        // Both remaining shapes have to measure. A wrapping field's *height* is
        // a function of its width, and a fit-to-content field's *width* is a
        // function of its text -- and taffy only offers either during
        // measurement, which is the shape gpui's own text element uses
        // (`elements/text.rs:647`).
        if !self.fit_content {
            style.size.width = relative(1.).into();
        }
        let body = self.clone();
        let fit_content = self.fit_content;
        let line_height = self.line_height;
        let layout_id = window.request_measured_layout(
            style,
            move |known_dimensions, available_space, window, cx| {
                if fit_content {
                    let size = body.ensure_layout(None, window, cx);
                    // One pixel of slack so the caret at the very end is inside
                    // the element rather than on its edge.
                    return gpui::size(size.width + px(1.), line_height);
                }
                let wrap_width = known_dimensions.width.or(match available_space.width {
                    gpui::AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let size = body.ensure_layout(wrap_width, window, cx);
                gpui::size(wrap_width.unwrap_or(size.width), size.height)
            },
        );
        (layout_id, ())
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
        let wrap_width = (self.multi_line && !self.fit_content).then_some(bounds.size.width);
        self.ensure_layout(wrap_width, window, cx);

        let placeholder = {
            let read = self.state.read(cx);
            read.content.is_empty().then(|| read.placeholder.clone())
        }
        .filter(|placeholder| !placeholder.is_empty())
        .map(|placeholder| {
            let mut font = window.text_style().font();
            font.weight = self.weight;
            let run = TextRun {
                len: placeholder.len(),
                font,
                color: self.placeholder_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(placeholder, self.text_size, &[run], None)
        });

        self.state.update(cx, |state, _| {
            state.text_bounds = Some(bounds);
            state.line_height = self.line_height;
            state.align = self.align;
        });

        let layout = self.layout.borrow();
        let state = self.state.read(cx);
        let Some(layout) = layout.as_ref() else {
            return BodyPrepaint {
                placeholder,
                selection: Vec::new(),
                caret: None,
                scroll_x: px(0.),
            };
        };

        // A single-line field scrolls horizontally to keep the caret in view;
        // a wrapping one never scrolls sideways at all.
        let caret_offset = state.cursor_offset();
        let caret_row = layout.row_for_offset(caret_offset);
        let caret_x = state.align_offset(layout, caret_row, bounds.size.width)
            + layout.x_for_offset(caret_offset, self.line_height);
        // An unfocused single-line field shows its start, the way an `<input>`
        // that has lost focus does -- otherwise a long value that was last
        // edited at its end reads as a fragment of itself.
        let scroll_x = if self.multi_line || !self.focused {
            px(0.)
        } else {
            let content_width = layout.rows.first().map(|row| row.width).unwrap_or(px(0.));
            let max_scroll = (content_width - bounds.size.width).max(px(0.));
            let mut scroll = state.scroll_x.min(max_scroll).max(px(0.));
            if caret_x - scroll < px(0.) {
                scroll = caret_x;
            } else if caret_x - scroll > bounds.size.width - px(1.) {
                scroll = caret_x - bounds.size.width + px(1.);
            }
            scroll.min(max_scroll).max(px(0.))
        };

        // The selection wash, one quad per visual row it crosses.
        let selected = state.selected_range.clone();
        let mut selection = Vec::new();
        if !selected.is_empty() {
            for (index, row) in layout.rows.iter().enumerate() {
                let start = selected.start.max(row.range.start);
                let end = selected.end.min(row.range.end);
                if start > end {
                    continue;
                }
                if start == end
                    && !(selected.start <= row.range.start && selected.end > row.range.end)
                {
                    continue;
                }
                let align = state.align_offset(layout, index, bounds.size.width);
                let left =
                    bounds.left() + align + layout.x_for_offset(start, self.line_height) - scroll_x;
                // A selection that runs past this row's end paints to the row's
                // full width, which is what shows a selected newline.
                let right = if selected.end > row.range.end {
                    bounds.left() + align + row.width - scroll_x
                } else {
                    bounds.left() + align + layout.x_for_offset(end, self.line_height) - scroll_x
                };
                let top = bounds.top() + self.line_height * index as f32;
                selection.push(fill(
                    Bounds::from_corners(
                        point(left, top),
                        point(right.max(left + px(1.)), top + self.line_height),
                    ),
                    self.selection_color,
                ));
            }
        }

        let caret = (self.focused && selected.is_empty()).then(|| {
            let top = bounds.top() + self.line_height * caret_row as f32;
            fill(
                Bounds::new(
                    point(bounds.left() + caret_x - scroll_x, top),
                    gpui::size(px(1.), self.line_height),
                ),
                self.caret_color,
            )
        });

        BodyPrepaint {
            placeholder,
            selection,
            caret,
            scroll_x,
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
        let focus = self.state.read(cx).focus.clone();
        // The IME seam. Only installed for the focused field --
        // `Window::handle_input` checks that itself (`window.rs:4769`), so every
        // field can call it unconditionally.
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );
        self.state.update(cx, |state, _| {
            state.scroll_x = prepaint.scroll_x;
        });

        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }

        if let Some(placeholder) = prepaint.placeholder.take() {
            let x = match self.align {
                TextAlign::Left => bounds.left(),
                TextAlign::Center => bounds.left() + (bounds.size.width - placeholder.width()) / 2.,
                TextAlign::Right => bounds.left() + bounds.size.width - placeholder.width(),
            };
            placeholder
                .paint(
                    point(x, bounds.top()),
                    self.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        } else {
            let mut origin = point(bounds.left() - prepaint.scroll_x, bounds.top());
            // `WrappedLine` is not `Clone`, so the layout is borrowed for the
            // whole paint rather than lifted out of the cell one line at a
            // time -- which is exactly why it lives in an `Rc<RefCell<..>>`
            // and not inside the entity.
            let layout = self.layout.borrow();
            if let Some(layout) = layout.as_ref() {
                for line in layout.lines.iter() {
                    let height = line.size(self.line_height).height;
                    line.paint(
                        origin,
                        self.line_height,
                        self.align,
                        Some(bounds),
                        window,
                        cx,
                    )
                    .ok();
                    origin.y += height;
                }
            }
        }

        if let Some(caret) = prepaint.caret.take() {
            window.paint_quad(caret);
        }

        // Drag-select keeps tracking outside the field's own bounds, which a
        // listener on the element cannot do -- the same reason `kit::Slider`
        // needs its drag layer.
        if self.state.read(cx).drag.is_some() {
            let state = self.state.clone();
            window.on_mouse_event({
                let state = state.clone();
                move |event: &MouseMoveEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase == gpui::DispatchPhase::Bubble {
                        state.update(cx, |this, cx| this.on_mouse_move(event, window, cx));
                    }
                }
            });
            window.on_mouse_event(
                move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase == gpui::DispatchPhase::Bubble {
                        state.update(cx, |this, cx| this.on_mouse_up(event, window, cx));
                    }
                },
            );
        }
    }
}
