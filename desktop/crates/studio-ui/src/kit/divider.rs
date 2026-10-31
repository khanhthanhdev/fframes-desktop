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
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // A hairline (0.5 px) but never thinner than one device pixel. The renderer rounds a
        // box's edges to device pixels, so at 100 % a box 0.5 px thick rounds to nothing and
        // is not drawn, while a box one device pixel thick always is.
        let thickness = px(METRICS.size.divider.max(1. / window.scale_factor()));
        let rule = div().flex_none().bg(self.theme.colors.line);
        if self.vertical {
            rule.w(thickness).h(px(VERTICAL_H))
        } else {
            rule.h(thickness).w_full()
        }
    }
}
