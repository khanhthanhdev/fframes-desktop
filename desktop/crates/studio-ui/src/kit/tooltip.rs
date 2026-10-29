//! `Tooltip` -- `apps/desktop/src/components/Tooltip.tsx`, a Kobalte `Tooltip`
//! with an OS-aware kbd-chip renderer built in.
//!
//! gpui already owns the hover/dismiss half of the contract: `.tooltip(..)` on
//! any interactive element shows a view after `tooltip_show_delay` and hides it
//! on mouse-leave. What it does not own is the *look*, or the kbd chips, which
//! is what this is. `openDelay={200}` is the Solid default and the default
//! here.
//!
//! Not reproduced: the forced-open override the Solid `Slider` uses to pin a
//! value tooltip to the thumb mid-drag, and `getAnchorRect`. gpui's tooltip is
//! pointer-anchored and hover-driven only, so a slider that wants a live value
//! readout draws one itself -- which is what `GradientEditor` does anyway with
//! its redundant numeric readout beside the slider.

use std::time::Duration;

use gpui::{
    App, AppContext, Context, IntoElement, ParentElement, Render, SharedString, Styled, Window,
    div, px,
};

use studio_theme::{METRICS, MOTION, Shadow, TextStyle, Theme, shadow_for, text_style};

use crate::kit::kbd::{KbdChip, kbd_symbol};

/// `openDelay={200}`.
pub const TOOLTIP_SHOW_DELAY: Duration = MOTION.tooltip_delay;

/// The bubble's paddings and least width.
const PAD_X: f32 = 6.;
const PAD_Y: f32 = 4.;
const MIN_W: f32 = 24.;

/// A tooltip body: `tooltip_text` on `tooltip_bg`, the inverse of the window, with
/// optional key chips. Build it with [`Tooltip::view`] and hand that to `.tooltip(..)`.
pub struct Tooltip {
    label: SharedString,
    keys: Vec<SharedString>,
    theme: Theme,
}

impl Tooltip {
    pub fn new(theme: &Theme, label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            keys: Vec::new(),
            theme: *theme,
        }
    }

    /// One chip per key, in order.
    pub fn keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.keys = keys
            .into_iter()
            .map(|key| kbd_symbol(key.as_ref()))
            .collect();
        self
    }

    /// Build the view `.tooltip(..)` wants.
    pub fn view(self, cx: &mut App) -> gpui::AnyView {
        cx.new(|_| self).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        let keys = self.keys.clone();

        text_style(div(), TextStyle::Label)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(PAD_X))
            .min_w(px(MIN_W))
            .px(px(PAD_X))
            .py(px(PAD_Y))
            .rounded(px(METRICS.radius.item))
            .bg(colors.tooltip_bg)
            .text_color(colors.tooltip_text)
            .shadow(shadow_for(theme.appearance, Shadow::Pop).into_vec())
            .child(self.label.clone())
            .children(
                keys.into_iter()
                    .map(|key| KbdChip::new(&theme, key.as_ref()).in_tooltip()),
            )
    }
}
