//! Colours and fonts of rendered markdown, derived from the resolved studio theme.

use gpui::{Hsla, SharedString};
use studio_theme::{MONO_FAMILY, TextStyle, Theme};

/// The text style of a heading of `level` (1 to 6) on the body's scale: h1 `Display`, h2
/// `Heading`, h3 `Title`, the rest `BodyStrong`.
pub fn heading_style(level: u8) -> TextStyle {
    match level {
        1 => TextStyle::Display,
        2 => TextStyle::Heading,
        3 => TextStyle::Title,
        _ => TextStyle::BodyStrong,
    }
}

/// The tokens markdown draws with.
#[derive(Clone, Debug)]
pub struct MdStyle {
    pub theme: Theme,
    pub text: Hsla,
    pub muted: Hsla,
    pub link: Hsla,
    pub code_bg: Hsla,
    pub code_border: Hsla,
    pub rule: Hsla,
    pub quote_bar: Hsla,
    pub selection: Hsla,
    pub mono: SharedString,
}

impl MdStyle {
    pub fn new(theme: &Theme) -> Self {
        let palette = theme.colors;
        Self {
            theme: *theme,
            text: palette.text_1.into(),
            muted: palette.text_2.into(),
            link: palette.accent.into(),
            code_bg: palette.card_2.into(),
            code_border: palette.line.into(),
            rule: palette.line.into(),
            quote_bar: palette.line_strong.into(),
            selection: palette.selection.into(),
            mono: SharedString::from(MONO_FAMILY),
        }
    }
}
