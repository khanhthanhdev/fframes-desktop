//! `EmptyState`: what a list, panel or stage shows when it has nothing to show.
//!
//! Two sizes, after Oleafly's compact and default empty states. Inline (the default) sits in
//! panels and tabs: a `Label` title in `text_1` and a `Small` body in `text_2`, left-aligned.
//! [`EmptyState::centered`] fills the stage, the start screen and the timeline: a `Heading`
//! (Oleafly's `EmptyTitle`) in `text_1` and a `Body` in `text_2`, centred. Either takes one
//! action, usually a button.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::selectable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyStateSize {
    Inline,
    Centered,
}

impl EmptyStateSize {
    /// The title's and body's text styles.
    pub(crate) fn styles(self) -> (TextStyle, TextStyle) {
        match self {
            Self::Inline => (TextStyle::Label, TextStyle::Small),
            Self::Centered => (TextStyle::Heading, TextStyle::Body),
        }
    }
}

#[derive(IntoElement)]
pub struct EmptyState {
    theme: Theme,
    size: EmptyStateSize,
    title: SharedString,
    body: Option<SharedString>,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(theme: &Theme, title: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            size: EmptyStateSize::Inline,
            title: title.into(),
            body: None,
            action: None,
        }
    }

    /// The stage, start screen and timeline size: larger type, centred in its parent.
    pub fn centered(mut self) -> Self {
        self.size = EmptyStateSize::Centered;
        self
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The one thing to do about it, usually a button.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        let centered = self.size == EmptyStateSize::Centered;
        let (title_style, body_style) = self.size.styles();
        let (title_color, body_color) = (colors.text_1, colors.text_2);

        let text = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(
                text_style(div(), title_style)
                    .text_color(title_color)
                    .child(selectable(self.title)),
            )
            .children(self.body.map(|body| {
                text_style(div(), body_style)
                    .text_color(body_color)
                    .child(selectable(body))
            }));

        let column = div().flex().flex_col().gap(px(if centered {
            METRICS.space_4
        } else {
            METRICS.space_2
        }));
        if centered {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    column
                        .items_center()
                        .max_w_full()
                        .text_center()
                        .p(px(METRICS.space_6))
                        .child(text.items_center().max_w_full())
                        .children(self.action),
                )
        } else {
            div().w_full().child(
                column
                    .items_start()
                    .py(px(METRICS.space_2))
                    .child(text)
                    .children(self.action),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_is_small_and_centered_is_large() {
        assert_eq!(
            EmptyStateSize::Inline.styles(),
            (TextStyle::Label, TextStyle::Small)
        );
        assert_eq!(
            EmptyStateSize::Centered.styles(),
            (TextStyle::Heading, TextStyle::Body)
        );
        let theme = Theme::new(studio_theme::Appearance::Light);
        assert_eq!(EmptyState::new(&theme, "x").size, EmptyStateSize::Inline);
        assert_eq!(
            EmptyState::new(&theme, "x").centered().size,
            EmptyStateSize::Centered
        );
    }
}
