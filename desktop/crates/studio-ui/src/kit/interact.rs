//! Behaviour shared by every interactive component: the tab stop, the focus
//! ring, keyboard intents and the disabled look.
//!
//! gpui turns Enter and Space on a focused element into its click listener
//! (`ClickEvent::Keyboard`), so a component that is a tab stop and has an
//! `on_click` is keyboard-activatable with nothing more. Components whose keys
//! mean more than "activate" (arrows in a segmented control, a slider or a
//! menu) read them through [`nav_key`].

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Div, FocusHandle, Global, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, actions, canvas, point, px,
};

use studio_theme::{METRICS, Theme};

use super::{TOOLTIP_SHOW_DELAY, Tooltip};

/// What a hand-built control runs when it is activated.
pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Receives an element's painted bounds, in window coordinates, on every prepaint.
pub type BoundsHandler = Rc<dyn Fn(Bounds<Pixels>, &mut App)>;

/// Width of the focus ring in pixels.
pub const FOCUS_RING_PX: f32 = METRICS.size.focus_ring;
/// Opacity of a disabled control.
pub const DISABLED_OPACITY: f32 = METRICS.opacity.disabled;

/// The fills every interactive component shares: hover is `ctl_hover`, pressed and
/// selected are `ctl_active`, a control at rest that has a fill is `ctl`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct States {
    pub rest: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub selected: Hsla,
}

impl States {
    pub fn of(theme: &Theme) -> Self {
        let colors = theme.colors;
        Self {
            rest: colors.ctl.into(),
            hover: colors.ctl_hover.into(),
            pressed: colors.ctl_active.into(),
            selected: colors.ctl_active.into(),
        }
    }
}

/// The disabled look: the whole element at [`DISABLED_OPACITY`].
pub fn dim_disabled<E: Styled>(element: E, disabled: bool) -> E {
    if disabled {
        element.opacity(DISABLED_OPACITY)
    } else {
        element
    }
}

/// Hides row actions until hover or descendant focus, without removing their tab stops.
pub fn reveal_actions(element: Stateful<Div>, group: SharedString) -> Stateful<Div> {
    element
        .focusable()
        .opacity(0.)
        .in_focus(|style| style.opacity(1.))
        .group_hover(group, |style| style.opacity(1.))
}

/// What a key press means to a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Activate,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Escape,
}

/// Maps a gpui key name to a navigation intent.
pub fn nav_key(key: &str) -> Option<Nav> {
    Some(match key {
        "enter" | "space" => Nav::Activate,
        "left" => Nav::Left,
        "right" => Nav::Right,
        "up" => Nav::Up,
        "down" => Nav::Down,
        "home" => Nav::Home,
        "end" => Nav::End,
        "escape" => Nav::Escape,
        _ => return None,
    })
}

/// Whether a key press activates a control: Enter or Space on an enabled one.
pub fn activates(disabled: bool, key: &str) -> bool {
    !disabled && nav_key(key) == Some(Nav::Activate)
}

/// The 2 px `focus` ring painted outside the element while it has keyboard focus.
pub fn focus_ring(theme: &Theme) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: theme.colors.focus.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(FOCUS_RING_PX),
        inset: false,
    }]
}

actions!(
    kit,
    [
        /// Sent to the control that just took the focus from the keyboard: it reports where it
        /// was painted, and the nearest scrolling container that listens brings it into view
        /// (see [ocus_within] and kit::reveal_focused).
        RevealFocused
    ]
);

/// Where the control that answered [RevealFocused] was painted, for its container.
#[derive(Default)]
pub(super) struct RevealedBounds(pub(super) Option<Bounds<Pixels>>);

impl Global for RevealedBounds {}

/// The most tab stops [ocus_within] steps over looking for one inside its container.
const MAX_STEPS: usize = 4096;

/// Tab stop and focus ring. A disabled element is skipped by Tab. The control answers
/// [RevealFocused] with the bounds it was last painted at.
pub fn focusable(element: Stateful<Div>, theme: &Theme, disabled: bool) -> Stateful<Div> {
    let ring = focus_ring(theme);
    let painted = Rc::new(Cell::new(None));
    let probe = painted.clone();
    element
        .tab_index(0)
        .tab_stop(!disabled)
        .focus_visible(move |style| style.shadow(ring.clone()))
        .on_action(move |_: &RevealFocused, _, cx| {
            if let Some(bounds) = painted.get() {
                cx.set_global(RevealedBounds(Some(bounds)));
                cx.propagate();
            }
        })
        .child(bounds_probe(Rc::new(move |bounds, _| {
            probe.set(Some(bounds))
        })))
}

/// Moves the focus to the next tab stop inside container (the previous one when
/// `backwards`), wrapping at its ends, and asks the control to come into view
/// ([RevealFocused]): Tab and Shift+Tab inside a modal or a popover, so they never reach
/// what lies under it. Returns `false`, the focus unchanged, when container holds no stop.
pub fn focus_within(
    container: &FocusHandle,
    backwards: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let start = window.focused(cx);
    for _ in 0..MAX_STEPS {
        if backwards {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        let now = window.focused(cx);
        if now.as_ref() != Some(container) && container.contains_focused(window, cx) {
            if let Some(now) = now {
                now.dispatch_action(&RevealFocused, window, cx);
            }
            return true;
        }
        if now == start {
            break;
        }
    }
    if let Some(start) = start {
        window.focus(&start, cx);
    }
    false
}

/// Runs `handler` on click, unless `disabled`. On a [`focusable`] element gpui
/// turns Enter and Space into that click, so the row is keyboard-activatable too.
pub fn on_activate(
    element: Stateful<Div>,
    disabled: bool,
    handler: Option<Handler>,
) -> Stateful<Div> {
    match handler.filter(|_| !disabled) {
        Some(handler) => element.on_click(move |_, window, cx| handler(window, cx)),
        None => element,
    }
}

/// Shows `text` as a tooltip after the standard delay.
pub fn with_tooltip(
    element: Stateful<Div>,
    theme: &Theme,
    text: Option<SharedString>,
) -> Stateful<Div> {
    let Some(text) = text else {
        return element;
    };
    let theme = *theme;
    element
        .tooltip_show_delay(TOOLTIP_SHOW_DELAY)
        .tooltip(move |_, cx| Tooltip::new(&theme, text.clone()).view(cx))
}

/// An absolute, full-size canvas that reports its parent's bounds. The parent
/// must be positioned (`relative()`), or the canvas measures the nearest
/// positioned ancestor instead.
pub fn bounds_probe(report: BoundsHandler) -> AnyElement {
    canvas(move |bounds, _, cx| report(bounds, cx), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
}

/// Index after moving `delta` steps from `current` over `len` items without
/// wrapping, skipping entries for which `skip` is true. `None` when nothing
/// enabled lies in that direction.
pub fn step_enabled(
    current: usize,
    delta: isize,
    len: usize,
    skip: impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut index = current as isize;
    loop {
        index += delta;
        if index < 0 || index >= len as isize {
            return None;
        }
        if !skip(index as usize) {
            return Some(index as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_space_activate() {
        assert_eq!(nav_key("enter"), Some(Nav::Activate));
        assert_eq!(nav_key("space"), Some(Nav::Activate));
        assert_eq!(nav_key("a"), None);
        assert_eq!(nav_key("escape"), Some(Nav::Escape));
    }

    #[test]
    fn disabled_controls_ignore_keys() {
        assert!(activates(false, "enter"));
        assert!(activates(false, "space"));
        assert!(!activates(true, "enter"));
        assert!(!activates(true, "space"));
        assert!(!activates(false, "tab"));
    }

    #[test]
    fn focus_ring_is_two_pixel_accent() {
        for theme in [Theme::light(), Theme::dark()] {
            let ring = focus_ring(&theme);
            assert_eq!(ring.len(), 1);
            assert_eq!(f32::from(ring[0].spread_radius), 2.);
            assert_eq!(ring[0].color, gpui::Hsla::from(theme.colors.focus));
        }
    }

    #[test]
    fn step_enabled_skips_and_stops_at_ends() {
        let skip = |i: usize| i == 1;
        assert_eq!(step_enabled(0, 1, 3, skip), Some(2));
        assert_eq!(step_enabled(2, -1, 3, skip), Some(0));
        assert_eq!(step_enabled(2, 1, 3, skip), None);
        assert_eq!(step_enabled(0, -1, 3, skip), None);
    }

    mod in_a_window {
        use std::rc::Rc;

        use gpui::{Context, IntoElement, ParentElement, Render, TestAppContext, Window, div};

        use super::super::*;
        use crate::kit::test_support::{Log, log, press, tab};

        /// Two hand-built rows, the first disabled, each logging its activations.
        struct Rows {
            hits: Log<&'static str>,
        }

        impl Render for Rows {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let theme = Theme::for_window(window, cx);
                let hit = |name: &'static str| -> Handler {
                    let hits = self.hits.clone();
                    Rc::new(move |_: &mut Window, _: &mut gpui::App| hits.borrow_mut().push(name))
                };
                let row = |id: &'static str, disabled: bool| {
                    let element = focusable(div().id(id), &theme, disabled);
                    on_activate(element, disabled, Some(hit(id)))
                };
                div().child(row("off", true)).child(row("on", false))
            }
        }

        #[gpui::test]
        fn a_hand_built_row_activates_from_the_keyboard_unless_disabled(cx: &mut TestAppContext) {
            let hits = log();
            let shared = hits.clone();
            let (_, cx) = cx.add_window_view(|_, _| Rows { hits: shared });
            tab(cx);
            press(cx, "enter");
            press(cx, "space");
            assert_eq!(*hits.borrow(), ["on", "on"]);
        }
    }
}
