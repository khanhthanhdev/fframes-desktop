//! The two voices of the transcript: the user's accent bubble on the right and the agent's
//! lightly framed answer across the row, each followed by a footer with the time and Copy.

use std::time::Duration;

use gpui::{
    AnyElement, App, ClipboardItem, Div, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled, Window, div, prelude::FluentBuilder, px, relative,
};
use studio_theme::{TextStyle, Theme, text_style};

use super::CARD_RADIUS;
use super::metrics::message as m;
use crate::kit::{
    Emphasis, Handler, Icon, IconButton, IconButtonSize, focusable, on_activate, reveal_actions,
    selectable,
};

/// A longer user message shows this many lines until the reader asks for the rest.
pub const USER_MESSAGE_MAX_LINES: usize = 6;
/// About how many characters fit on one line of a bubble in the narrowest sidebar.
const CHARS_PER_LINE: usize = 44;

/// How many lines `text` takes in a bubble, estimated from its length: hard line breaks
/// plus wrapping at the narrowest sidebar's width.
pub fn estimated_lines(text: &str) -> usize {
    text.lines()
        .map(|line| line.chars().count().div_ceil(CHARS_PER_LINE).max(1))
        .sum()
}

/// The fill and hairline of the agent's answer: the document fill and `line` at 60 % in
/// light, the muted fill and no visible line in dark.
fn answer_frame(theme: &Theme) -> (gpui::Hsla, gpui::Hsla) {
    let colors = theme.colors;
    if theme.is_dark() {
        (colors.card_2.into(), Theme::with_alpha(colors.card_2, 0.))
    } else {
        (
            colors.window.into(),
            Theme::with_alpha(colors.line, m::FRAME_LINE_ALPHA),
        )
    }
}

/// Copy in a message's footer: a 14 px glyph that shows while the pointer is over the message
/// or the button has the keyboard focus, and turns into a check for a moment after copying.
fn copy_action(
    theme: &Theme,
    group: SharedString,
    id: ElementId,
    text: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let copied = window.use_keyed_state(ElementId::from((id.clone(), "copied")), cx, |_, _| false);
    let done = *copied.read(cx);
    let button = IconButton::new(
        theme,
        id.clone(),
        if done { Icon::Check } else { Icon::Copy }.path(),
    )
    .size(IconButtonSize::Sm)
    .emphasis(Emphasis::Ghost)
    .label(if done { "Copied" } else { "Copy" })
    .tooltip("Copy")
    .on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        copied.update(cx, |copied, cx| {
            *copied = true;
            cx.notify();
        });
        let copied = copied.clone();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(m::COPIED_MS))
                .await;
            copied.update(cx, |copied, cx| {
                *copied = false;
                cx.notify();
            });
        })
        .detach();
    });
    let wrap = div().id(ElementId::from((id, "reveal")));
    // After a copy the check stays in view until it turns back.
    if done {
        wrap.child(button)
    } else {
        reveal_actions(wrap, group).child(button)
    }
}

/// The footer under a message: its time in `Caption` `text_3`, then Copy. Its height is
/// always reserved, so hovering or focusing never moves the transcript.
fn footer(
    theme: &Theme,
    group: SharedString,
    meta: Option<SharedString>,
    copy: Option<(ElementId, SharedString)>,
    end: bool,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let colors = theme.colors;
    div()
        .h(px(m::FOOTER_H))
        .flex()
        .items_center()
        .gap(px(m::FOOTER_GAP))
        .when(end, |footer| footer.justify_end())
        .children(meta.map(|meta| {
            text_style(div(), TextStyle::Caption)
                .text_color(colors.text_3)
                .child(meta)
        }))
        .children(copy.map(|(id, text)| copy_action(theme, group, id, text, window, cx)))
}

/// The user's message: an `accent` bubble with `on_accent` text on the right, at most 85 % of
/// the row wide, with an optional caption above ("Your answer", "Follow-up") and a footer
/// below.
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

    /// The time shown in the footer.
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
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
            .gap(px(m::FOOTER_GAP))
            .children(caption.map(|caption| {
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_3)
                    .child(selectable(caption))
            }))
            .child(
                div()
                    .max_w(relative(m::BUBBLE_MAX_W))
                    .rounded(CARD_RADIUS)
                    .bg(colors.accent)
                    .px(px(m::PAD_X))
                    .py(px(m::PAD_Y))
                    .child(
                        text_style(div(), TextStyle::Body)
                            .text_color(colors.on_accent)
                            .when(clamped, |text| text.line_clamp(USER_MESSAGE_MAX_LINES))
                            .child(selectable(text.clone()).id(("agent-row-text", id))),
                    ),
            )
            .children(toggle)
            .child(footer(
                &theme,
                group,
                meta,
                Some((("agent-row-copy", id as usize).into(), text)),
                true,
                window,
                cx,
            ))
    }
}

/// The agent's answer across the row in a light frame (see [`answer_frame`]), then the
/// footer.
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

    /// The raw text Copy puts on the clipboard; without it the footer has no Copy.
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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let group = SharedString::from(format!("agent-answer-{}", self.id));
        let copy = self
            .copy
            .map(|text| (("agent-row-copy", self.id as usize).into(), text));
        let (fill, line) = answer_frame(&self.theme);
        div()
            .group(group.clone())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(m::FOOTER_GAP))
            .child(
                div()
                    .w_full()
                    .rounded(CARD_RADIUS)
                    .border_1()
                    .border_color(line)
                    .bg(fill)
                    .px(px(m::PAD_X))
                    .py(px(m::PAD_Y))
                    .child(self.body),
            )
            .child(footer(
                &self.theme,
                group,
                self.meta,
                copy,
                false,
                window,
                cx,
            ))
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

    #[test]
    fn the_answer_frame_shows_a_line_in_light_only() {
        let (light_fill, light_line) = answer_frame(&Theme::light());
        assert_eq!(light_fill, Theme::light().colors.window.into());
        assert!(light_line.a > 0.);
        let (dark_fill, dark_line) = answer_frame(&Theme::dark());
        assert_eq!(dark_fill, Theme::dark().colors.card_2.into());
        assert_eq!(dark_line.a, 0.);
    }
}
