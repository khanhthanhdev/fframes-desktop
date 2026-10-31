//! The column of a workflow page shown in place of the editor (Create, Export): centred, at
//! most `METRICS.layout.workflow_max_w` wide (Oleafly's `max-w-2xl` dialog), so its rows stay
//! readable in a wide window, under a title and the line that explains it.

use gpui::{Div, ParentElement, Styled, div, px};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::selectable;

/// The centred column of a workflow page.
pub fn workflow_column() -> Div {
    // A definite width that narrower windows shrink: with `w_full` and `max_w` wrapped
    // text is measured at the unclamped width.
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(METRICS.layout.workflow_max_w))
        .max_w_full()
        .mx_auto()
}

/// A workflow page's title (`Heading`) and the line under it (`Body`, `text_2`).
pub fn workflow_title(theme: &Theme, title: &'static str, text: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(METRICS.space_1))
        .child(
            text_style(div(), TextStyle::Heading)
                .text_color(theme.colors.text_1)
                .child(title),
        )
        .child(
            text_style(div(), TextStyle::Body)
                .text_color(theme.colors.text_2)
                .child(selectable(text)),
        )
}
