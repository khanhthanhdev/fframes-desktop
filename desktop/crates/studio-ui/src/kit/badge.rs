//! Badge and status dot, coloured by a [`Tone`].

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, Rgba, SharedString, Styled, Window, div, px,
};

use studio_theme::{TextStyle, Theme, text_style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

/// Solid colour that represents `tone` (dot fill, notice bar, icon).
pub fn tone_color(theme: &Theme, tone: Tone) -> Rgba {
    match tone {
        Tone::Neutral => theme.colors.text_3,
        Tone::Info => theme.colors.accent,
        Tone::Success => theme.colors.success,
        Tone::Warning => theme.colors.warning,
        Tone::Danger => theme.colors.danger,
    }
}

/// Soft background for `tone`; neutral and info use the control fill.
pub fn tone_surface(theme: &Theme, tone: Tone) -> Rgba {
    match tone {
        Tone::Neutral | Tone::Info => theme.colors.ctl,
        Tone::Success => theme.colors.success_bg,
        Tone::Warning => theme.colors.warning_bg,
        Tone::Danger => theme.colors.danger_bg,
    }
}

/// A badge's padding and a status dot's diameter.
const BADGE_PAD_X: f32 = 6.;
const BADGE_PAD_Y: f32 = 2.;
const DOT: f32 = 6.;

#[derive(IntoElement)]
pub struct Badge {
    text: SharedString,
    tone: Tone,
}

impl Badge {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Neutral,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let (fill, text) = match self.tone {
            Tone::Neutral => (theme.colors.ctl, theme.colors.text_2),
            tone => (tone_surface(&theme, tone), tone_color(&theme, tone)),
        };
        text_style(div(), TextStyle::Caption)
            .flex_none()
            .px(px(BADGE_PAD_X))
            .py(px(BADGE_PAD_Y))
            .rounded_full()
            .bg(fill)
            .text_color(text)
            .child(self.text)
    }
}

#[derive(IntoElement)]
pub struct StatusDot {
    tone: Tone,
}

impl StatusDot {
    pub fn new(tone: Tone) -> Self {
        Self { tone }
    }
}

impl RenderOnce for StatusDot {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        div()
            .size(px(DOT))
            .flex_none()
            .rounded_full()
            .bg(tone_color(&theme, self.tone))
    }
}

#[cfg(test)]
mod tests {
    use studio_theme::Appearance;

    use super::*;

    #[test]
    fn tones_map_to_the_theme() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let theme = Theme::new(appearance);
            assert_eq!(tone_color(&theme, Tone::Neutral), theme.colors.text_3);
            assert_eq!(tone_color(&theme, Tone::Info), theme.colors.accent);
            assert_eq!(tone_color(&theme, Tone::Success), theme.colors.success);
            assert_eq!(tone_color(&theme, Tone::Warning), theme.colors.warning);
            assert_eq!(tone_color(&theme, Tone::Danger), theme.colors.danger);
            assert_eq!(tone_surface(&theme, Tone::Neutral), theme.colors.ctl);
            assert_eq!(tone_surface(&theme, Tone::Info), theme.colors.ctl);
            assert_eq!(tone_surface(&theme, Tone::Success), theme.colors.success_bg);
            assert_eq!(tone_surface(&theme, Tone::Warning), theme.colors.warning_bg);
            assert_eq!(tone_surface(&theme, Tone::Danger), theme.colors.danger_bg);
        }
    }
}
