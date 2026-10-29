//! Text the reader can select with the mouse and copy: a drag selects a range, a double
//! click a word, a triple click the whole block, and Ctrl+C (Cmd+C on macOS) copies it.
//!
//! The app has one selection at a time, kept in a global. A press anywhere else ends it,
//! and so does any key but the copy key in a focused text field (typing, Ctrl+A, arrows),
//! so it never outlives a later selection in a field. [`CopySelection`] is bound without a
//! context, which gpui ranks above a focused field's own copy binding: with nothing
//! selected here it passes the key on, and the field copies its selection.
//! A selection never spans two text blocks.
//!
//! Sections of the shell are cached views, which repaint only when notified: a change of
//! selection notifies the view that painted the old owner and the view of the new one.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId, EntityId,
    GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    SharedString, StyledText, TextLayout, Window, actions,
};

actions!(selectable_text, [CopySelection]);

/// The app's one text selection.
#[derive(Default)]
struct Selection {
    owner: Option<GlobalElementId>,
    /// The view that painted the owner, notified when the selection moves away from it.
    view: Option<EntityId>,
    text: SharedString,
    anchor: usize,
    head: usize,
    dragging: bool,
}

impl gpui::Global for Selection {}

impl Selection {
    fn range(&self) -> Range<usize> {
        clamp_range(
            &self.text,
            self.anchor.min(self.head)..self.anchor.max(self.head),
        )
    }
}

/// The key [`CopySelection`] is bound to.
const COPY_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-c"
} else {
    "ctrl-c"
};

/// Binds the copy key and handles it. Called once at start.
pub fn bind_selectable_text_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(COPY_KEY, CopySelection, None)]);
    cx.on_action(|_: &CopySelection, cx| match selected_text(cx) {
        Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
        // The next binding of the key (a focused text field's copy) handles it.
        None => cx.propagate(),
    });
    // Any other key in a focused text field (typing, Ctrl+A, arrows) ends the selection, so
    // the copy key copies the field's selection again. An observer sees keys a binding
    // handled too, which a key listener would not.
    cx.observe_keystrokes(|event, _, cx| {
        let in_field = event
            .context_stack
            .iter()
            .any(|context| context.contains(super::text_input::KEY_CONTEXT));
        if in_field && !is_copy_key(&event.keystroke) {
            clear_selection(cx);
        }
    })
    .detach();
}

/// The selected text, if any is selected.
pub fn selected_text(cx: &App) -> Option<String> {
    let selection = cx.try_global::<Selection>()?;
    selection.owner.as_ref()?;
    let range = selection.range();
    (!range.is_empty()).then(|| selection.text[range].to_owned())
}

/// Selectable text that inherits the surrounding text style, like a string child does.
/// Its id is the text itself; [`SelectableText::id`] gives a stable one to text that
/// changes while it is read (a streaming message).
pub fn selectable(text: impl Into<SharedString>) -> SelectableText {
    let text = text.into();
    SelectableText {
        id: ElementId::Name(text.clone()),
        text,
        layout: TextLayout::default(),
        hitbox: HitboxBehavior::Normal,
    }
}

pub struct SelectableText {
    id: ElementId,
    text: SharedString,
    layout: TextLayout,
    hitbox: HitboxBehavior,
}

impl SelectableText {
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// For text over a window drag area (the Windows title bar): the system takes presses
    /// there to move the window, so this text blocks the area under it and is selected
    /// instead. The rest of the area still moves the window.
    pub fn over_drag_area(mut self) -> Self {
        self.hitbox = HitboxBehavior::BlockMouse;
        self
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = StyledText;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, StyledText) {
        let mut styled = StyledText::new(self.text.clone());
        if let Some(selection) = cx.try_global::<Selection>()
            && selection.owner.as_ref() == id
        {
            let range = clamp_range(&self.text, selection.range());
            if !range.is_empty() {
                styled = styled.with_highlights([(
                    range,
                    HighlightStyle {
                        background_color: Some(Hsla::from(
                            studio_theme::Theme::for_window(window, cx).colors.selection,
                        )),
                        ..Default::default()
                    },
                )]);
            }
        }
        self.layout = styled.layout().clone();
        let (layout_id, ()) = styled.request_layout(None, inspector_id, window, cx);
        (layout_id, styled)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        styled: &mut StyledText,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        styled.prepaint(None, inspector_id, bounds, &mut (), window, cx);
        window.insert_hitbox(bounds, self.hitbox)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        styled: &mut StyledText,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        styled.paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        let Some(id) = id.cloned() else {
            return;
        };
        let view = window.current_view();

        // A press anywhere else ends this element's selection; the press that starts a new
        // one on other text runs later, in the bubble phase.
        window.on_mouse_event({
            let id = id.clone();
            let hitbox = hitbox.clone();
            move |_: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && !hitbox.is_hovered(window) && owns(&id, cx) {
                    clear_selection(cx);
                }
            }
        });
        window.on_mouse_event({
            let id = id.clone();
            let hitbox = hitbox.clone();
            let layout = self.layout.clone();
            let text = self.text.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let index = index_at(&layout, &text, event.position);
                let range = match event.click_count {
                    0 | 1 => index..index,
                    2 => word_at(&text, index),
                    _ => 0..text.len(),
                };
                let previous = cx.try_global::<Selection>().and_then(|s| s.view);
                cx.set_global(Selection {
                    owner: Some(id.clone()),
                    view: Some(view),
                    text: text.clone(),
                    anchor: range.start,
                    head: range.end,
                    dragging: event.click_count <= 1,
                });
                if let Some(previous) = previous.filter(|v| *v != view) {
                    cx.notify(previous);
                }
                cx.notify(view);
            }
        });
        window.on_mouse_event({
            let id = id.clone();
            let layout = self.layout.clone();
            let text = self.text.clone();
            move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble || !owns(&id, cx) {
                    return;
                }
                let index = index_at(&layout, &text, event.position);
                let selection = cx.global_mut::<Selection>();
                if selection.dragging && selection.head != index {
                    selection.head = index;
                    cx.notify(view);
                }
            }
        });
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && owns(&id, cx) {
                cx.global_mut::<Selection>().dragging = false;
            }
        });
    }
}

fn owns(id: &GlobalElementId, cx: &App) -> bool {
    cx.try_global::<Selection>()
        .is_some_and(|s| s.owner.as_ref() == Some(id))
}

/// Ends the app's text selection, if there is one.
fn clear_selection(cx: &mut App) {
    let Some(selection) = cx.try_global::<Selection>() else {
        return;
    };
    if selection.owner.is_none() {
        return;
    }
    let view = selection.view;
    cx.set_global(Selection::default());
    if let Some(view) = view {
        cx.notify(view);
    }
}

/// Whether `keystroke` is the key [`CopySelection`] is bound to.
fn is_copy_key(keystroke: &gpui::Keystroke) -> bool {
    gpui::Keystroke::parse(COPY_KEY)
        .is_ok_and(|copy| copy.key == keystroke.key && copy.modifiers == keystroke.modifiers)
}

/// The byte index nearest to `position`, on a character boundary.
fn index_at(layout: &TextLayout, text: &str, position: Point<Pixels>) -> usize {
    let index = layout
        .index_for_position(position)
        .unwrap_or_else(|nearest| nearest);
    floor_boundary(text, index)
}

/// `range` inside `text` and on character boundaries: the text may have changed since the
/// selection was made.
fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    floor_boundary(text, range.start)..floor_boundary(text, range.end)
}

fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The word around `index`; a character that is not part of a word on its own.
fn word_at(text: &str, index: usize) -> Range<usize> {
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let index = floor_boundary(text, index);
    let Some(ch) = text[index..].chars().next() else {
        return index..index;
    };
    if !is_word(ch) {
        return index..index + ch.len_utf8();
    }
    let start = text[..index]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(index, |(i, _)| i);
    let end = text[index..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| index + i);
    start..end
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext, Context, Entity, InteractiveElement, Modifiers, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, div, point, px,
    };

    use super::*;
    use crate::kit::text_input::{TextInput, TextInputState};
    use studio_theme::Theme;

    #[test]
    fn a_double_click_selects_the_word_under_it() {
        let text = "copy héllo_wörld, now";
        assert_eq!(&text[word_at(text, 7)], "héllo_wörld");
        assert_eq!(&text[word_at(text, 0)], "copy");
        assert_eq!(&text[word_at(text, 4)], " ");
        assert_eq!(word_at(text, text.len()), text.len()..text.len());
    }

    #[test]
    fn a_selection_is_clamped_to_text_that_shrank() {
        let text = "héllo";
        assert_eq!(clamp_range(text, 0..2), 0..1);
        assert_eq!(clamp_range(text, 1..40), 1..text.len());
    }

    struct Page {
        field: Entity<TextInputState>,
    }

    impl Render for Page {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            div()
                .flex()
                .flex_col()
                .w(px(400.))
                .child(TextInput::new(&theme, "field", &self.field))
                .child(
                    div()
                        .debug_selector(|| "selectable-line".into())
                        .child(selectable("selected words here")),
                )
        }
    }

    fn page(cx: &mut TestAppContext) -> (Entity<Page>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::kit::bind_text_input_keys(cx);
            bind_selectable_text_keys(cx);
        });
        let (page, cx) = cx.add_window_view(|window, cx| Page {
            field: cx.new(|cx| TextInputState::single_line(window, cx)),
        });
        cx.run_until_parked();
        (page, cx)
    }

    #[gpui::test]
    fn ctrl_c_copies_the_dragged_text_and_otherwise_the_focused_field(cx: &mut TestAppContext) {
        let (page, cx) = page(cx);
        let copy = |cx: &mut VisualTestContext| {
            cx.simulate_keystrokes(if cfg!(target_os = "macos") {
                "cmd-c"
            } else {
                "ctrl-c"
            });
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
        };
        // Text selected in a focused field copies as before while no text is selected.
        cx.update(|window, cx| {
            let field = page.read(cx).field.clone();
            field.update(cx, |field, cx| {
                field.set_text("prompt text", cx);
                field.focus_and_select_all(window, cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(copy(cx).as_deref(), Some("prompt text"));
        let line = cx
            .debug_bounds("selectable-line")
            .expect("the selectable line was laid out");
        let (y, from, to) = (line.center().y, line.left() + px(1.), line.right() - px(1.));
        cx.simulate_mouse_down(point(from, y), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(point(to, y), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(point(to, y), MouseButton::Left, Modifiers::none());
        assert_eq!(
            cx.update(|_, cx| selected_text(cx)).as_deref(),
            Some("selected words here")
        );
        assert_eq!(copy(cx).as_deref(), Some("selected words here"));
        // Selecting in the still focused field takes the copy key back.
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        });
        assert_eq!(copy(cx).as_deref(), Some("prompt text"));
        let select_line = |cx: &mut VisualTestContext| {
            cx.simulate_mouse_down(point(from, y), MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(point(to, y), MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_up(point(to, y), MouseButton::Left, Modifiers::none());
            assert!(cx.update(|_, cx| selected_text(cx)).is_some());
        };
        // Typing in the field ends it too.
        select_line(cx);
        cx.simulate_keystrokes("x");
        assert_eq!(cx.update(|_, cx| selected_text(cx)), None);
        select_line(cx);
        // A press anywhere else ends the selection.
        let below = point(from, line.bottom() + px(20.));
        cx.simulate_click(below, Modifiers::none());
        assert_eq!(cx.update(|_, cx| selected_text(cx)), None);
    }
}
