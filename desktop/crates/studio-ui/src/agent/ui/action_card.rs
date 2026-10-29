//! Everything that waits for the user (permission requests, questions, the outside-file
//! prompt, held and refused messages) and the transcript's errors, as one card: the kit
//! `Notice`'s tone icon and surface with room for content and the actions on the right.

use gpui::{
    AnyElement, App, ClipboardItem, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use crate::kit::{
    Button, ButtonSize, Card, Emphasis, Tone, icon, notice_surface, selectable, tone_color,
    tone_icon,
};

/// `Warning` when the agent is blocked on the user, `Info` when it only informs, `Danger`
/// for errors. Title `BodyStrong`, body `Small` `text_2` that wraps and never truncates,
/// then any content (radio cards, a field), then the actions right-aligned.
#[derive(IntoElement)]
pub struct ActionCard {
    theme: Theme,
    id: ElementId,
    tone: Tone,
    title: SharedString,
    body: Option<SharedString>,
    content: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    copy: Option<SharedString>,
}

impl ActionCard {
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        tone: Tone,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            tone,
            title: title.into(),
            body: None,
            content: Vec::new(),
            actions: Vec::new(),
            copy: None,
        }
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// An element under the body: the options of a question, a text field.
    pub fn child(mut self, element: impl IntoElement) -> Self {
        self.content.push(element.into_any_element());
        self
    }

    /// A button in the right-aligned row at the bottom; the primary one goes last.
    pub fn action(mut self, button: impl IntoElement) -> Self {
        self.actions.push(button.into_any_element());
        self
    }

    /// A Copy button in the title row that puts `text` on the clipboard (errors).
    pub fn copy(mut self, text: impl Into<SharedString>) -> Self {
        self.copy = Some(text.into());
        self
    }
}

impl RenderOnce for ActionCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ActionCard {
            theme,
            id,
            tone,
            title,
            body,
            content,
            actions,
            copy,
        } = self;
        let colors = theme.colors;
        let accent = tone_color(&theme, tone);
        let copy = copy.map(|text| {
            Button::new(
                &theme,
                ElementId::from(SharedString::from(format!("{id}-copy"))),
            )
            .emphasis(Emphasis::Ghost)
            .size(ButtonSize::Sm)
            .label("Copy")
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
            })
        });
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_3))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(METRICS.space_2))
                    .child(
                        div()
                            .h(px(TextStyle::BodyStrong.line_height()))
                            .flex()
                            .items_center()
                            .child(icon(tone_icon(tone), METRICS.size.icon_sm, accent)),
                    )
                    .child(
                        text_style(div().flex_1().min_w_0(), TextStyle::BodyStrong)
                            .text_color(colors.text_1)
                            .child(selectable(title)),
                    )
                    .children(copy),
            )
            .children(body.map(|body| {
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_2)
                    .child(selectable(body))
            }))
            .children(content)
            .children((!actions.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(METRICS.space_2))
                    .children(actions)
            }));
        Card::raised(&theme)
            .id(id)
            .role(Role::Alert)
            .flex()
            .w_full()
            .overflow_hidden()
            .bg(notice_surface(&theme, tone))
            .child(column)
    }
}
