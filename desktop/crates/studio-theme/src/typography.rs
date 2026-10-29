//! Text styles: size, weight and line height.

use gpui::{FontWeight, Styled, px};

/// System UI font on every platform; no bundled font.
pub const UI_FONT: &str = ".SystemUIFont";

/// The monospace family code is set in, per platform's stock font.
pub const MONO_FAMILY: &str = if cfg!(target_os = "windows") {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStyle {
    /// 10, medium: badges and key chips.
    Caption,
    /// 11: inline values.
    Small,
    /// 12, medium: labels, segmented items, tooltips, menus.
    Label,
    /// 13: body, buttons, fields.
    Body,
    /// 13, semibold.
    BodyStrong,
    /// 14, medium: card titles.
    Title,
    /// 16, semibold: the one size fframes adds, for the empty-state title.
    Heading,
    /// 18, semibold: the start screen title and markdown h1.
    Display,
    /// 12 on 18, [`MONO_FAMILY`]: code blocks, inline code, diffs.
    Code,
    /// 11 on 16, [`MONO_FAMILY`]: diff gutters and hunk headers.
    CodeSmall,
}

impl TextStyle {
    pub const fn size(self) -> f32 {
        match self {
            Self::Caption => 10.,
            Self::Small | Self::CodeSmall => 11.,
            Self::Label | Self::Code => 12.,
            Self::Body | Self::BodyStrong => 13.,
            Self::Title => 14.,
            Self::Heading => 16.,
            Self::Display => 18.,
        }
    }

    pub fn weight(self) -> FontWeight {
        match self {
            Self::Caption | Self::Label | Self::Title => FontWeight::MEDIUM,
            Self::Small | Self::Body | Self::Code | Self::CodeSmall => FontWeight::NORMAL,
            Self::BodyStrong | Self::Heading | Self::Display => FontWeight::SEMIBOLD,
        }
    }

    pub const fn line_height(self) -> f32 {
        match self {
            Self::Caption => 14.,
            Self::Small | Self::Label | Self::CodeSmall => 16.,
            Self::Body | Self::BodyStrong | Self::Code => 18.,
            Self::Title => 20.,
            Self::Heading => 22.,
            Self::Display => 24.,
        }
    }

    /// The family a style sets, if it is not the inherited UI font.
    pub const fn family(self) -> Option<&'static str> {
        match self {
            Self::Code | Self::CodeSmall => Some(MONO_FAMILY),
            _ => None,
        }
    }
}

/// Applies size, weight and line height of `style`, and its family for the code styles.
pub fn text_style<E: Styled>(element: E, style: TextStyle) -> E {
    let element = element
        .text_size(px(style.size()))
        .font_weight(style.weight())
        .line_height(px(style.line_height()));
    match style.family() {
        Some(family) => element.font_family(family),
        None => element,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_styles_match_cap_sizes() {
        let sizes = [
            TextStyle::Caption,
            TextStyle::Small,
            TextStyle::Label,
            TextStyle::Body,
            TextStyle::BodyStrong,
            TextStyle::Title,
            TextStyle::Heading,
        ]
        .map(TextStyle::size);
        assert_eq!(sizes, [10., 11., 12., 13., 13., 14., 16.]);
    }

    #[test]
    fn code_and_display_styles() {
        let code = TextStyle::Code;
        assert_eq!((code.size(), code.line_height()), (12., 18.));
        assert_eq!(code.weight(), FontWeight::NORMAL);
        assert_eq!(code.family(), Some(MONO_FAMILY));
        let small = TextStyle::CodeSmall;
        assert_eq!((small.size(), small.line_height()), (11., 16.));
        assert_eq!(small.family(), Some(MONO_FAMILY));
        let display = TextStyle::Display;
        assert_eq!(display.size(), 18.);
        assert_eq!(display.weight(), FontWeight::SEMIBOLD);
        assert_eq!(display.family(), None);
        assert_eq!(TextStyle::Body.family(), None);
        assert!(!MONO_FAMILY.is_empty() && MONO_FAMILY != UI_FONT);
    }
}
