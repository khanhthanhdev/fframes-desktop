//! The two voices of the transcript: the user's bubble on the right and the agent's flush
//! answer on the left, each followed by a hover lane with the time and Copy.

use gpui::{
    AnyElement, App, ClipboardItem, Div, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled, Window, div, prelude::FluentBuilder, px, relative,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::{CARD_RADIUS, HOVER_LANE_H};
use crate::kit::{Button, ButtonSize, Emphasis, Handler, focusable, on_activate, selectable};

/// A longer user message shows this many lines until the reader asks for the rest.
pub const USER_MESSAGE_MAX_LINES: usize = 6;
/// The widest a user bubble grows, as a share of the row.
const BUBBLE_MAX_W: f32 = 0.85;
/// About how many characters fit on one line of a bubble in the narrowest sidebar.
const CHARS_PER_LINE: usize = 44;

/// How many lines `text` takes in a bubble, estimated from its length: hard line breaks
/// plus wrapping at the narrowest sidebar's width.
pub fn estimated_lines(text: &str) -> usize {
    text.lines()
        .map(|line| line.chars().count().div_ceil(CHARS_PER_LINE).max(1))
        .sum()
}

/// The lane under a message: its time and a Copy button, shown while the pointer is over the
/// message. Its height is always reserved, so hovering never moves the transcript.
fn hover_lane(
    theme: &Theme,
    group: SharedString,
    meta: Option<SharedString>,
    copy: Option<(ElementId, SharedString)>,
    end: bool,
) -> Div {
    let colors = theme.colors;
    div()
        .h(px(HOVER_LANE_H))
        .flex()
        .items_center()
        .gap(px(METRICS.space_2))
        .when(end, |lane| lane.justify_end())
        .invisible()
        .group_hover(group, |style| style.visible())
        .children(meta.map(|meta| {
            text_style(div(), TextStyle::Small)
                .text_color(colors.text_3)
                .child(meta)
        }))
        .children(copy.map(|(id, text)| {
            Button::new(theme, id)
                .emphasis(Emphasis::Ghost)
                .size(ButtonSize::Sm)
                .label("Copy")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
                })
        }))
}

/// The user's message: a `ctl` bubble on the right, at most 85 % of the row wide, with an
/// optional caption above ("Your answer", "Follow-up") and a hover lane below.
#[derive(IntoElement)]
pub struct UserMessage {
    theme: Theme,
    id: u64,
    text: SharedString,
    caption: Option<SharedString>,
    meta: Option<SharedString>,
    expanded: bool,
    on_toggle: Option<Handler>,
}

impl UserMessage {
    /// `id` is the transcript row's id; it keys the text selection and the buttons.
    pub fn new(theme: &Theme, id: u64, text: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            id,
            text: text.into(),
            caption: None,
            meta: None,
            expanded: false,
            on_toggle: None,
        }
    }

    /// `Small` `text_3` above the bubble: where the message came from.
    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    /// The time shown in the hover lane.
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// Whether a long message shows all its lines, and what Show more or Show less does.
    pub fn expanded(
        mut self,
        expanded: bool,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.expanded = expanded;
        self.on_toggle = Some(std::rc::Rc::new(on_toggle));
        self
    }
}

impl RenderOnce for UserMessage {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let UserMessage {
            theme,
            id,
            text,
            caption,
            meta,
            expanded,
            on_toggle,
        } = self;
        let colors = theme.colors;
        let long = estimated_lines(&text) > USER_MESSAGE_MAX_LINES;
        let clamped = long && !expanded;
        let group = SharedString::from(format!("agent-user-{id}"));
        let toggle = on_toggle.filter(|_| long).map(|handler| {
            let link = focusable(div().id(("agent-row-more", id as usize)), &theme, false);
            text_style(on_activate(link, false, Some(handler)), TextStyle::Small)
                .cursor_pointer()
                .text_color(colors.text_2)
                .hover(move |style| style.text_color(colors.text_1))
                .child(if expanded { "Show less" } else { "Show more" })
        });
        div()
            .group(group.clone())
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(METRICS.space_1))
            .children(caption.map(|caption| {
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_3)
                    .child(selectable(caption))
            }))
            .child(
                div()
                    .max_w(relative(BUBBLE_MAX_W))
                    .rounded(CARD_RADIUS)
                    .bg(colors.ctl)
                    .px(px(METRICS.space_3))
                    .py(px(METRICS.space_2))
                    .child(
                        text_style(div(), TextStyle::Body)
                            .text_color(colors.text_1)
                            .when(clamped, |text| text.line_clamp(USER_MESSAGE_MAX_LINES))
                            .child(selectable(text.clone()).id(("agent-row-text", id))),
                    ),
            )
            .children(toggle)
            .child(hover_lane(
                &theme,
                group,
                meta,
                Some((("agent-row-copy", id as usize).into(), text)),
                true,
            ))
    }
}

/// The agent's answer: flush with the panel inset, no frame, then the hover lane.
#[derive(IntoElement)]
pub struct AssistantMessage {
    theme: Theme,
    id: u64,
    body: AnyElement,
    copy: Option<SharedString>,
    meta: Option<SharedString>,
}

impl AssistantMessage {
    /// `body` is the rendered answer (a `MarkdownRow`).
    pub fn new(theme: &Theme, id: u64, body: impl IntoElement) -> Self {
        Self {
            theme: *theme,
            id,
            body: body.into_any_element(),
            copy: None,
            meta: None,
        }
    }

    /// The raw text Copy puts on the clipboard; without it the lane has no Copy.
    pub fn copy(mut self, text: impl Into<SharedString>) -> Self {
        self.copy = Some(text.into());
        self
    }

    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }
}

impl RenderOnce for AssistantMessage {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let group = SharedString::from(format!("agent-answer-{}", self.id));
        let copy = self
            .copy
            .map(|text| (("agent-row-copy", self.id as usize).into(), text));
        div()
            .group(group.clone())
            .w_full()
            .flex()
            .flex_col()
            .child(self.body)
            .child(hover_lane(&self.theme, group, self.meta, copy, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_count_as_several() {
        assert_eq!(estimated_lines("one"), 1);
        assert_eq!(estimated_lines("one\ntwo\n\nfour"), 4);
        assert_eq!(estimated_lines(&"x".repeat(CHARS_PER_LINE * 2 + 1)), 3);
        assert!(estimated_lines(&"line\n".repeat(7)) > USER_MESSAGE_MAX_LINES);
    }
}
