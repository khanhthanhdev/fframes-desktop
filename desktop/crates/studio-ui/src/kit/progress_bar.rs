//! `Progress::linear`: a determinate bar, `progress_h` (4) high with pill ends, an `accent`
//! fill on a `ctl` track. [`Progress::on_dark`] is the present-mode variant: white on white
//! at 20 %. The ring is [`CircularProgress`](super::CircularProgress).

use gpui::{App, Hsla, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px, relative};

use studio_theme::{METRICS, Theme};

/// White at this alpha is the track on dark, present-mode surfaces.
const ON_DARK_TRACK_ALPHA: f32 = 0.2;

#[derive(IntoElement)]
pub struct Progress {
    theme: Theme,
    fraction: f32,
    on_dark: bool,
}

impl Progress {
    /// A bar `fraction` full, clamped to `0..=1` (NaN shows empty).
    pub fn linear(theme: &Theme, fraction: f32) -> Self {
        Self {
            theme: *theme,
            fraction: clamp_fraction(fraction),
            on_dark: false,
        }
    }

    /// White on a 20 % white track, for the present-mode strip over video.
    pub fn on_dark(mut self) -> Self {
        self.on_dark = true;
        self
    }

    /// The track and fill colours.
    fn colors(&self) -> (Hsla, Hsla) {
        let colors = self.theme.colors;
        if self.on_dark {
            let white = Hsla::from(colors.on_accent);
            (white.opacity(ON_DARK_TRACK_ALPHA), white)
        } else {
            (colors.ctl.into(), colors.accent.into())
        }
    }
}

fn clamp_fraction(fraction: f32) -> f32 {
    if fraction.is_nan() {
        0.
    } else {
        fraction.clamp(0., 1.)
    }
}

impl RenderOnce for Progress {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (track, fill) = self.colors();
        let height = px(METRICS.size.progress_h);
        div()
            .w_full()
            .h(height)
            .flex_none()
            .rounded(px(METRICS.radius.pill))
            .overflow_hidden()
            .bg(track)
            .child(
                div()
                    .h(height)
                    .w(relative(self.fraction))
                    .rounded(px(METRICS.radius.pill))
                    .bg(fill),
            )
    }
}

#[cfg(test)]
mod tests {
    use studio_theme::Appearance;

    use super::*;

    #[test]
    fn fractions_are_clamped() {
        let theme = Theme::new(Appearance::Light);
        let fraction = |value: f32| Progress::linear(&theme, value).fraction;
        assert_eq!(fraction(-0.5), 0.);
        assert_eq!(fraction(0.25), 0.25);
        assert_eq!(fraction(1.5), 1.);
        assert_eq!(fraction(f32::NAN), 0.);
        assert_eq!(fraction(f32::INFINITY), 1.);
    }

    #[test]
    fn on_dark_uses_white_on_a_faint_white_track() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let theme = Theme::new(appearance);
            let (track, fill) = Progress::linear(&theme, 0.5).colors();
            assert_eq!(track, Hsla::from(theme.colors.ctl));
            assert_eq!(fill, Hsla::from(theme.colors.accent));
            let (track, fill) = Progress::linear(&theme, 0.5).on_dark().colors();
            assert_eq!(fill, Hsla::from(theme.colors.on_accent));
            assert!((track.a - ON_DARK_TRACK_ALPHA * fill.a).abs() < 1e-6);
        }
    }
}
