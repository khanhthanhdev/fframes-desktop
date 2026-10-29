//! `Toast` and `ToastHost`: a transient confirmation ("Copied", "Saved") that goes away by
//! itself.
//!
//! A toast is a pill on the `card` surface with a `line` border, `control` corners and the
//! `Pop` shadow: `Small` text in `text_1` and an optional action. It disappears
//! [`TOAST_DURATION`] (3 s) after it was shown, and is kept while the pointer is over it (the
//! 3 s start again when the pointer leaves). Persistent messages and errors are `Notice`s,
//! never toasts.
//!
//! One [`ToastHost`] per window holds the toast on show; the window draws it bottom-centre of
//! the player and drives the timer:
//!
//! ```ignore
//! // Showing one:
//! self.toasts.show("Copied", Instant::now());
//! self.schedule_toast(cx);
//!
//! fn schedule_toast(&self, cx: &mut Context<Self>) {
//!     if let Some(wait) = self.toasts.remaining(Instant::now()) {
//!         cx.spawn(async move |this, cx| {
//!             cx.background_executor().timer(wait).await;
//!             this.update(cx, |view, cx| {
//!                 if view.toasts.tick(Instant::now()) {
//!                     cx.notify();
//!                 } else {
//!                     view.schedule_toast(cx); // shown again, or hovered meanwhile
//!                 }
//!             })
//!         })
//!         .detach();
//!     }
//! }
//!
//! // Rendering it:
//! toasts.current().map(|toast| {
//!     Toast::new(&theme, "toast", toast.message.clone()).on_hover(cx.listener(
//!         |view, hovered: &bool, _, cx| {
//!             view.toasts.set_hovered(*hovered, Instant::now());
//!             view.schedule_toast(cx);
//!         },
//!     ))
//! })
//! ```

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use studio_theme::{METRICS, Shadow, TextStyle, Theme, shadow_for, text_style};

/// How long a toast stays after it was shown or the pointer left it.
pub const TOAST_DURATION: Duration = Duration::from_secs(3);

/// How long a toast shown (or last left by the pointer) at `since` has left at `now`: `None`
/// while it is hovered (it stays), zero once it has expired.
pub fn toast_remaining(since: Instant, now: Instant, hovered: bool) -> Option<Duration> {
    (!hovered).then(|| TOAST_DURATION.saturating_sub(now.saturating_duration_since(since)))
}

type HoverHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Toast {
    id: ElementId,
    theme: Theme,
    message: SharedString,
    action: Option<AnyElement>,
    on_hover: Option<HoverHandler>,
}

impl Toast {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, message: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            message: message.into(),
            action: None,
            on_hover: None,
        }
    }

    /// One action after the message, usually a small ghost button ("Undo", "Show").
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    /// Runs when the pointer enters (`true`) or leaves (`false`) the toast; pass it to
    /// [`ToastHost::set_hovered`].
    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        div()
            .id(self.id)
            .role(Role::Status)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_2))
            .min_h(px(METRICS.size.control_h))
            .px(px(METRICS.space_3))
            .py(px(METRICS.space_1))
            .rounded(px(METRICS.radius.control))
            .border_1()
            .border_color(colors.line)
            .bg(colors.card)
            .shadow(shadow_for(self.theme.appearance, Shadow::Pop).into_vec())
            .when_some(self.on_hover, |this, handler| {
                this.on_hover(move |hovered, window, cx| handler(hovered, window, cx))
            })
            .child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_1)
                    .child(self.message),
            )
            .children(self.action)
    }
}

/// The toast a window shows now.
#[derive(Debug, Clone, PartialEq)]
pub struct HostedToast {
    /// Distinct for every [`ToastHost::show`], so a timer can tell its toast was replaced.
    pub id: u64,
    pub message: SharedString,
    since: Instant,
    hovered: bool,
}

/// One window's toast: at most one at a time; a new one replaces the old.
#[derive(Debug, Default)]
pub struct ToastHost {
    toast: Option<HostedToast>,
    shown: u64,
}

impl ToastHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows `message` from `now`, replacing any toast; returns its id.
    pub fn show(&mut self, message: impl Into<SharedString>, now: Instant) -> u64 {
        self.shown += 1;
        self.toast = Some(HostedToast {
            id: self.shown,
            message: message.into(),
            since: now,
            hovered: false,
        });
        self.shown
    }

    pub fn current(&self) -> Option<&HostedToast> {
        self.toast.as_ref()
    }

    /// The pointer entered or left the toast. Leaving starts the full duration again.
    pub fn set_hovered(&mut self, hovered: bool, now: Instant) {
        if let Some(toast) = &mut self.toast {
            if toast.hovered && !hovered {
                toast.since = now;
            }
            toast.hovered = hovered;
        }
    }

    pub fn dismiss(&mut self) {
        self.toast = None;
    }

    /// How long until the toast expires: `None` with no toast or while it is hovered.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let toast = self.toast.as_ref()?;
        toast_remaining(toast.since, now, toast.hovered)
    }

    /// Removes the toast if it has expired at `now`; whether it did.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.remaining(now) == Some(Duration::ZERO) {
            self.toast = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_lasts_three_seconds_unless_hovered() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert_eq!(toast_remaining(start, start, false), Some(TOAST_DURATION));
        assert_eq!(
            toast_remaining(start, at(1000), false),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            toast_remaining(start, at(3000), false),
            Some(Duration::ZERO)
        );
        assert_eq!(
            toast_remaining(start, at(9000), false),
            Some(Duration::ZERO)
        );
        assert_eq!(toast_remaining(start, at(9000), true), None);
        // A clock read before `since` does not underflow.
        assert_eq!(toast_remaining(at(10), start, false), Some(TOAST_DURATION));
    }

    #[test]
    fn the_host_expires_its_toast_and_keeps_it_while_hovered() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut host = ToastHost::new();
        assert_eq!(host.remaining(start), None);
        assert!(!host.tick(at(5000)));

        let first = host.show("Copied", start);
        assert!(!host.tick(at(2999)));
        host.set_hovered(true, at(2500));
        assert!(!host.tick(at(10_000)), "kept while hovered");
        host.set_hovered(false, at(10_000));
        assert_eq!(host.remaining(at(10_000)), Some(TOAST_DURATION));
        assert!(!host.tick(at(12_999)));
        assert!(host.tick(at(13_000)));
        assert!(host.current().is_none());

        let second = host.show("Saved", at(20_000));
        assert_ne!(first, second);
        let third = host.show("Exported", at(21_000));
        assert_eq!(host.current().map(|toast| toast.id), Some(third));
        assert!(!host.tick(at(23_500)), "the replacement has its own 3 s");
        host.dismiss();
        assert!(host.current().is_none());
    }
}
