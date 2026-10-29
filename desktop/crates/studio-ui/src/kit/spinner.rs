//! `Spinner`: a 12 px `accent` glyph turning once per `MOTION.spin_period`, for work in
//! progress ("Thinking…", running tasks). Under reduced motion it is a still ring instead.

use std::f32::consts::TAU;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, IntoElement, RenderOnce, Styled,
    Transformation, Window, div, px, radians,
};

use studio_theme::{MOTION, Theme, prefers_reduced_motion};

use super::icon::{Icon, icon};

/// The spinner's side.
const SPINNER_SIZE: f32 = 12.;

/// What a spinner draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerKind {
    /// The turning glyph.
    Spinning,
    /// A still ring, under reduced motion.
    Static,
}

impl SpinnerKind {
    /// What a spinner draws with the user's motion preference in force.
    pub fn current(cx: &App) -> Self {
        Self::for_reduced_motion(prefers_reduced_motion(cx))
    }

    pub fn for_reduced_motion(reduced: bool) -> Self {
        if reduced {
            Self::Static
        } else {
            Self::Spinning
        }
    }
}

#[derive(IntoElement)]
pub struct Spinner {
    id: ElementId,
    theme: Theme,
}

impl Spinner {
    /// `id` keys the animation; spinners on one screen need distinct ids.
    pub fn new(theme: &Theme, id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
        }
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let accent = self.theme.colors.accent;
        let element: AnyElement = match SpinnerKind::current(cx) {
            SpinnerKind::Spinning => icon(Icon::RefreshCw, SPINNER_SIZE, accent)
                .with_animation(
                    self.id,
                    Animation::new(MOTION.spin_period).repeat(),
                    |glyph, t| glyph.with_transformation(Transformation::rotate(radians(t * TAU))),
                )
                .into_any_element(),
            SpinnerKind::Static => div()
                .size(px(SPINNER_SIZE))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(accent)
                .into_any_element(),
        };
        element
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};
    use studio_theme::MotionPreference;

    use super::*;

    #[test]
    fn reduced_motion_draws_a_still_ring() {
        assert_eq!(SpinnerKind::for_reduced_motion(true), SpinnerKind::Static);
        assert_eq!(
            SpinnerKind::for_reduced_motion(false),
            SpinnerKind::Spinning
        );
    }

    struct Spinning;

    impl Render for Spinning {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            Spinner::new(&Theme::for_window(window, cx), "spinner")
        }
    }

    #[gpui::test]
    fn the_spinner_follows_the_motion_preference(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Spinning);
        cx.update(|_, cx| {
            assert_eq!(SpinnerKind::current(cx), SpinnerKind::Spinning);
            studio_theme::set_motion(MotionPreference::Reduced, cx);
            assert_eq!(SpinnerKind::current(cx), SpinnerKind::Static);
        });
        // The still ring renders without asking for animation frames.
        cx.run_until_parked();
        cx.update(|_, cx| studio_theme::set_motion(MotionPreference::Full, cx));
        cx.run_until_parked();
    }
}
