//! `Divider`: a 1 px rule in `line`, across the parent or 16 px tall between toolbar groups.
//! Replaces `surface::divider`, `border_t_1` separators and the hand-built 1×16 rules in the
//! header and transport.

use gpui::{App, IntoElement, RenderOnce, Styled, Window, div, px};

use studio_theme::{METRICS, Theme};

/// How tall a vertical divider stands between toolbar groups.
const VERTICAL_H: f32 = 16.;

#[derive(IntoElement)]
pub struct Divider {
    theme: Theme,
    vertical: bool,
}

impl Divider {
    /// A full-width horizontal rule.
    pub fn horizontal(theme: &Theme) -> Self {
        Self {
            theme: *theme,
            vertical: false,
        }
    }

    /// A 16 px tall vertical rule.
    pub fn vertical(theme: &Theme) -> Self {
        Self {
            theme: *theme,
            vertical: true,
        }
    }
}

impl RenderOnce for Divider {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let rule = div().flex_none().bg(self.theme.colors.line);
        if self.vertical {
            rule.w(px(METRICS.size.divider)).h(px(VERTICAL_H))
        } else {
            rule.h(px(METRICS.size.divider)).w_full()
        }
    }
}
