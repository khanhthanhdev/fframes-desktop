//! Text helpers over the theme's text styles, for the editor palette. Their text can be
//! selected and copied, except a section title's: it heads collapsible, clickable rows.

use gpui::{Div, ParentElement, SharedString, Styled, div};

use studio_theme::{TextStyle, Theme, text_style};

use super::selectable;

/// `Label` style text in the primary text colour.
pub fn label(theme: &Theme, text: impl Into<SharedString>) -> Div {
    text_style(div(), TextStyle::Label)
        .text_color(theme.colors.text_1)
        .child(selectable(text))
}

/// `Body` style text in the secondary text colour.
pub fn muted(theme: &Theme, text: impl Into<SharedString>) -> Div {
    text_style(div(), TextStyle::Body)
        .text_color(theme.colors.text_2)
        .child(selectable(text))
}

/// `Heading` style text in the primary text colour.
pub fn heading(theme: &Theme, text: impl Into<SharedString>) -> Div {
    text_style(div(), TextStyle::Heading)
        .text_color(theme.colors.text_1)
        .child(selectable(text))
}
