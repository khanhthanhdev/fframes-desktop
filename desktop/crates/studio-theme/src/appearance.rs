//! The user's UI preferences (light/dark, motion) and how a window resolves them.

use gpui::{App, Context, Global, Window};

use crate::{AppTheme, Appearance, MotionPreference, Theme};

/// The app-wide UI preferences, loaded from the app settings at start. One global holds
/// both so a settings change repaints once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiPrefs {
    pub theme: AppTheme,
    pub motion: MotionPreference,
}

impl Global for UiPrefs {}

/// Installs the theme preference the app settings hold, keeping the motion preference.
/// Phase 1 replaces this with [`init_prefs`].
pub fn init(cx: &mut App, preference: AppTheme) {
    let prefs = UiPrefs {
        theme: preference,
        ..current_prefs(cx)
    };
    init_prefs(cx, prefs);
}

/// Installs the preferences the app settings hold.
pub fn init_prefs(cx: &mut App, prefs: UiPrefs) {
    apply_motion(prefs.motion, cx);
    cx.set_global(prefs);
}

/// The preferences in force; the defaults (`System`, `System`) until [`init_prefs`] ran.
pub fn current_prefs(cx: &App) -> UiPrefs {
    cx.try_global::<UiPrefs>().copied().unwrap_or_default()
}

/// The theme preference in force; `System` until [`init`] ran.
pub fn current_preference(cx: &App) -> AppTheme {
    current_prefs(cx).theme
}

/// The motion preference in force; `System` until [`init_prefs`] ran.
pub fn current_motion(cx: &App) -> MotionPreference {
    current_prefs(cx).motion
}

/// Changes both preferences and repaints every window. Persisting them
/// (`AppSettings::save`) is the caller's job.
pub fn set_prefs(prefs: UiPrefs, cx: &mut App) {
    init_prefs(cx, prefs);
    cx.refresh_windows();
}

/// Changes the theme preference and repaints every window. Persisting it is the
/// caller's job.
pub fn set_preference(preference: AppTheme, cx: &mut App) {
    let prefs = UiPrefs {
        theme: preference,
        ..current_prefs(cx)
    };
    set_prefs(prefs, cx);
}

/// Changes the motion preference and repaints every window. Persisting it is the
/// caller's job.
pub fn set_motion(motion: MotionPreference, cx: &mut App) {
    let prefs = UiPrefs {
        motion,
        ..current_prefs(cx)
    };
    set_prefs(prefs, cx);
}

/// Whether animations should snap: `Reduced` always, `Full` never, and `System` when the
/// operating system asks for it. GPUI reports no OS setting, so `system_reduced` is
/// `false` for every caller today and `System` means full motion.
pub fn resolve_reduced_motion(preference: MotionPreference, system_reduced: bool) -> bool {
    match preference {
        MotionPreference::System => system_reduced,
        MotionPreference::Reduced => true,
        MotionPreference::Full => false,
    }
}

/// Whether the studio should reduce motion now. When true every animation snaps, a
/// spinner shows a static ring, and the streaming veil is off. Phase 1's
/// `reduced_motion(cx)`; named apart from the context-free [`crate::reduced_motion`]
/// until that one's callers move.
pub fn prefers_reduced_motion(cx: &App) -> bool {
    resolve_reduced_motion(current_motion(cx), false)
}

/// Mirrors the preference into GPUI's own flag, so `with_animation` and animated images
/// honour it too.
fn apply_motion(motion: MotionPreference, cx: &mut App) {
    cx.set_reduce_motion(resolve_reduced_motion(motion, false));
}
/// Light and Dark force that appearance; System follows the window, which
/// follows the operating system.
pub fn resolve_appearance(preference: AppTheme, system: Appearance) -> Appearance {
    match preference {
        AppTheme::System => system,
        AppTheme::Light => Appearance::Light,
        AppTheme::Dark => Appearance::Dark,
    }
}

impl Theme {
    /// The theme `window` draws with now: the preference, or the window's own
    /// appearance under `System`.
    pub fn for_window(window: &Window, cx: &App) -> Self {
        let system = Appearance::from_window(window.appearance());
        Self::new(resolve_appearance(current_preference(cx), system))
    }

    /// Replaces `self` with the window's current theme; whether it changed.
    pub fn refresh(&mut self, window: &Window, cx: &App) -> bool {
        let next = Self::for_window(window, cx);
        if next.appearance != self.appearance {
            *self = next;
            true
        } else {
            false
        }
    }
}

/// Repaints the view when the operating system flips between light and dark,
/// so a `System` preference follows it without waiting for another event.
pub fn bind_window<T: 'static>(window: &mut Window, cx: &mut Context<T>) {
    cx.observe_window_appearance(window, |_, _, cx| cx.notify())
        .detach();
}

#[cfg(test)]
mod tests {
    use gpui::{Empty, Render, TestAppContext};

    use super::*;

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            Empty
        }
    }

    fn appearance_in_window(cx: &mut TestAppContext, preference: Option<AppTheme>) -> Appearance {
        if let Some(preference) = preference {
            cx.update(|cx| init(cx, preference));
        }
        let window = cx.add_window(|_, _| Probe);
        window
            .update(cx, |_, window, cx| Theme::for_window(window, cx).appearance)
            .unwrap()
    }

    #[gpui::test]
    fn a_dark_preference_overrides_a_light_window(cx: &mut TestAppContext) {
        // The test platform's windows always report a light appearance.
        assert_eq!(appearance_in_window(cx, None), Appearance::Light);
        assert_eq!(
            appearance_in_window(cx, Some(AppTheme::Dark)),
            Appearance::Dark
        );
        assert_eq!(
            appearance_in_window(cx, Some(AppTheme::System)),
            Appearance::Light
        );
    }

    #[gpui::test]
    fn changing_the_preference_is_seen_by_the_next_render(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Probe);
        let mut theme = window
            .update(cx, |_, window, cx| Theme::for_window(window, cx))
            .unwrap();
        cx.update(|cx| set_preference(AppTheme::Dark, cx));
        let changed = window
            .update(cx, |_, window, cx| theme.refresh(window, cx))
            .unwrap();
        assert!(changed);
        assert!(theme.is_dark());
        assert_eq!(cx.update(|cx| current_preference(cx)), AppTheme::Dark);
    }

    #[test]
    fn motion_preference_resolves() {
        for system in [false, true] {
            assert_eq!(
                resolve_reduced_motion(MotionPreference::System, system),
                system
            );
            assert!(resolve_reduced_motion(MotionPreference::Reduced, system));
            assert!(!resolve_reduced_motion(MotionPreference::Full, system));
        }
    }

    #[gpui::test]
    fn prefs_default_to_system_and_full_motion(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert_eq!(current_prefs(cx), UiPrefs::default());
            assert_eq!(current_motion(cx), MotionPreference::System);
            assert!(!prefers_reduced_motion(cx));
        });
    }

    #[gpui::test]
    fn the_motion_and_theme_preferences_change_independently(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init_prefs(
                cx,
                UiPrefs {
                    theme: AppTheme::Dark,
                    motion: MotionPreference::Reduced,
                },
            );
            assert!(prefers_reduced_motion(cx));
            assert!(cx.reduce_motion(), "GPUI's own flag follows");

            set_preference(AppTheme::Light, cx);
            assert_eq!(current_motion(cx), MotionPreference::Reduced);
            assert_eq!(current_preference(cx), AppTheme::Light);

            set_motion(MotionPreference::Full, cx);
            assert!(!prefers_reduced_motion(cx));
            assert!(!cx.reduce_motion());
            assert_eq!(current_preference(cx), AppTheme::Light);

            init(cx, AppTheme::Dark);
            assert_eq!(
                current_prefs(cx),
                UiPrefs {
                    theme: AppTheme::Dark,
                    motion: MotionPreference::Full,
                }
            );

            set_prefs(
                UiPrefs {
                    theme: AppTheme::System,
                    motion: MotionPreference::Reduced,
                },
                cx,
            );
            assert!(cx.reduce_motion());
            assert_eq!(current_preference(cx), AppTheme::System);
        });
    }
}
