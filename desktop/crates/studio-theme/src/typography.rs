//! Text styles: size, weight and line height.

use gpui::{FontWeight, Styled, px};

/// Bundled Geist, registered before Studio opens its first window.
pub const UI_FONT: &str = "Geist";

/// Bundled Geist Mono for code, diffs and numeric readouts.
pub const MONO_FAMILY: &str = "Geist Mono";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStyle {
    /// 10, medium: badges and key chips.
    Caption,
    /// 12: secondary text and inline values.
    Small,
    /// 12, medium: labels, segmented items, tooltips, menus.
    Label,
    /// 14: body, buttons, fields.
    Body,
    /// 14, semibold.
    BodyStrong,
    /// 14, medium: setting-row labels and the selected settings nav row.
    BodyMedium,
    /// 12, semibold: section headings, set in capitals by the caller (Oleafly's
    /// uppercase `SectionHeading`; gpui has no letter spacing).
    Overline,
    /// 16, medium: card titles.
    Title,
    /// 18, semibold: dialog and empty-state titles.
    Heading,
    /// 24, semibold: the start screen title and markdown h1.
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
            Self::CodeSmall => 11.,
            Self::Small | Self::Label | Self::Overline | Self::Code => 12.,
            Self::Body | Self::BodyStrong | Self::BodyMedium => 14.,
            Self::Title => 16.,
            Self::Heading => 18.,
            Self::Display => 24.,
        }
    }

    pub fn weight(self) -> FontWeight {
        match self {
            Self::Caption | Self::Label | Self::BodyMedium | Self::Title => FontWeight::MEDIUM,
            Self::Small | Self::Body | Self::Code | Self::CodeSmall => FontWeight::NORMAL,
            Self::BodyStrong | Self::Overline | Self::Heading | Self::Display => {
                FontWeight::SEMIBOLD
            }
        }
    }

    pub const fn line_height(self) -> f32 {
        match self {
            Self::Caption => 14.,
            Self::Small | Self::Label | Self::Overline | Self::CodeSmall => 16.,
            Self::Code => 18.,
            Self::Body | Self::BodyStrong | Self::BodyMedium => 20.,
            Self::Title => 24.,
            Self::Heading => 24.,
            Self::Display => 32.,
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
    fn text_styles_match_studio_sizes() {
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
        assert_eq!(sizes, [10., 12., 12., 14., 14., 16., 18.]);
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
        assert_eq!(display.size(), 24.);
        assert_eq!(display.weight(), FontWeight::SEMIBOLD);
        assert_eq!(display.family(), None);
        assert_eq!(TextStyle::Body.family(), None);
        assert!(!MONO_FAMILY.is_empty() && MONO_FAMILY != UI_FONT);
    }
}
