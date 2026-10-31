//! Badge and status dot, coloured by a [`Tone`].

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, Rgba, SharedString, Styled, Window, div, px,
};

use studio_theme::{METRICS, TextStyle, Theme, mix, text_style};

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

/// Soft background for `tone`: the control fill for neutral, `info_bg` (Oleafly's
/// `bg-primary/10`) for info, the status surfaces otherwise.
pub fn tone_surface(theme: &Theme, tone: Tone) -> Rgba {
    match tone {
        Tone::Neutral => theme.colors.ctl,
        Tone::Info => theme.colors.info_bg,
        Tone::Success => theme.colors.success_bg,
        Tone::Warning => theme.colors.warning_bg,
        Tone::Danger => theme.colors.danger_bg,
    }
}

/// How far a dark info badge's text moves from the accent towards `text_1`. The accent alone
/// reads at about 3:1 on its dark wash; a lighter blue `info` text role in the theme would
/// replace this.
const DARK_INFO_TEXT_LIFT: f32 = 0.4;

/// A badge's fill and text: neutral is `text_2` on `ctl`; a tone is its colour on its soft
/// surface, the info blue lifted in the dark appearance so 12 px text stays legible.
fn badge_colors(theme: &Theme, tone: Tone) -> (Rgba, Rgba) {
    let colors = theme.colors;
    match tone {
        Tone::Neutral => (colors.ctl, colors.text_2),
        Tone::Info if theme.is_dark() => (
            tone_surface(theme, tone),
            mix(colors.accent, colors.text_1, DARK_INFO_TEXT_LIFT),
        ),
        tone => (tone_surface(theme, tone), tone_color(theme, tone)),
    }
}

/// A badge's padding (Oleafly's `px-2 py-0.5`) and a status dot's diameter.
const BADGE_PAD_X: f32 = METRICS.space_2;
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
        let (fill, text) = badge_colors(&theme, self.tone);
        // A soft tinted pill: `Label` type, fully round, no border.
        text_style(div(), TextStyle::Label)
            .flex_none()
            .whitespace_nowrap()
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
            assert_eq!(tone_surface(&theme, Tone::Info), theme.colors.info_bg);
            assert_eq!(tone_surface(&theme, Tone::Success), theme.colors.success_bg);
            assert_eq!(tone_surface(&theme, Tone::Warning), theme.colors.warning_bg);
            assert_eq!(tone_surface(&theme, Tone::Danger), theme.colors.danger_bg);
        }
    }

    #[test]
    fn info_badge_text_is_the_accent_in_light_and_lighter_in_dark() {
        let light = Theme::new(Appearance::Light);
        assert_eq!(badge_colors(&light, Tone::Info).1, light.colors.accent);
        let dark = Theme::new(Appearance::Dark);
        let (_, text) = badge_colors(&dark, Tone::Info);
        let sum = |c: Rgba| c.r + c.g + c.b;
        assert!(sum(text) > sum(dark.colors.accent));
        assert_eq!(
            badge_colors(&dark, Tone::Success),
            (dark.colors.success_bg, dark.colors.success)
        );
    }
}
