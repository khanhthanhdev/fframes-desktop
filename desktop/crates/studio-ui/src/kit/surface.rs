//! Surfaces: the card, the hairline and the popover.

use gpui::{Div, Styled, div, px};

use studio_theme::{METRICS, Shadow, Theme, shadow_for};

/// A card: `card` behind a `line` hairline at the card radius.
pub struct Card;

impl Card {
    /// A panel on the window: the card fill and the card shadow.
    pub fn raised(theme: &Theme) -> Div {
        div()
            .rounded(px(METRICS.radius.card))
            .border_1()
            .border_color(theme.colors.line)
            .bg(theme.colors.card)
            .shadow(shadow_for(theme.appearance, Shadow::Card).into_vec())
    }

    /// A surface nested inside a card: `card_2`, no shadow.
    pub fn nested(theme: &Theme) -> Div {
        div()
            .rounded(px(METRICS.radius.card))
            .border_1()
            .border_color(theme.colors.line)
            .bg(theme.colors.card_2)
    }
}

/// A 1 px horizontal rule in the `line` colour.
pub fn divider(theme: &Theme) -> Div {
    div()
        .h(px(METRICS.size.divider))
        .w_full()
        .flex_none()
        .bg(theme.colors.line)
}

/// An anchored floating panel, not a modal: `card` behind a `line` hairline with the
/// popover shadow. Positioning stays with the caller.
pub struct Popover;

impl Popover {
    // A styled `Div` the caller positions and fills, like `Card::raised`.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(theme: &Theme) -> Div {
        div()
            .absolute()
            .flex()
            .flex_col()
            .rounded(px(METRICS.radius.popover))
            .border_1()
            .border_color(theme.colors.line)
            .bg(theme.colors.card)
            .shadow(shadow_for(theme.appearance, Shadow::Pop).into_vec())
    }
}
