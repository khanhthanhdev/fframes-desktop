//! A modal dialog over its window, Oleafly's `ModalShell`: the window dims under the
//! `scrim`, and the dialog is a card centred on it, `min(1040, 94 %)` wide and `min(900,
//! 88 %)` tall, rounded behind a hairline with the preview shadow. A press on the scrim
//! dismisses it; presses inside the card stay in it, and nothing under the scrim answers
//! the pointer. Tab and Shift+Tab cycle through the dialog's own tab stops and wrap at its
//! ends, so the keyboard never reaches the window under the scrim either.

use std::rc::Rc;

use gpui::{
    App, Div, ElementId, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Pixels, Size, Stateful, Styled, Window, div, px,
};
use studio_theme::{METRICS, Shadow, Theme, shadow_for};

use super::focus_within;

/// Oleafly's settings dialog, `w-[min(1040px,94vw)] h-[min(900px,88vh)]`.
pub const MODAL_MAX_W: f32 = 1040.;
pub const MODAL_MAX_H: f32 = 900.;
const MODAL_W_SHARE: f32 = 0.94;
const MODAL_H_SHARE: f32 = 0.88;
/// The card's corners (`rounded-xl`); content inside rounds its own outer corners to it.
pub const MODAL_RADIUS: f32 = 12.;

/// The dialog's size in a window of `viewport`.
pub fn modal_size(viewport: Size<Pixels>) -> (f32, f32) {
    let (w, h) = (f32::from(viewport.width), f32::from(viewport.height));
    (
        MODAL_MAX_W.min(w * MODAL_W_SHARE),
        MODAL_MAX_H.min(h * MODAL_H_SHARE),
    )
}

/// The dialog's card at `width` × `height`, without the scrim: for a gallery or a fixture.
pub fn modal_card(theme: &Theme, width: f32, height: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .rounded(px(MODAL_RADIUS))
        .border(px(METRICS.size.divider))
        .border_color(theme.colors.line)
        .bg(theme.colors.sidebar)
        .shadow(shadow_for(theme.appearance, Shadow::Preview).into_vec())
}

/// `content` in the dialog's card over the whole of its parent, sized for `viewport`. Make
/// it the last child of the window's root so it paints over everything else. `focus` is the
/// dialog's own focus, tracked inside `content` around all its controls: focus it on open,
/// and Tab keeps to the stops inside it.
pub fn modal(
    theme: &Theme,
    id: impl Into<ElementId>,
    viewport: Size<Pixels>,
    focus: &FocusHandle,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    content: impl IntoElement,
) -> Stateful<Div> {
    let (width, height) = modal_size(viewport);
    let dismiss = Rc::new(on_dismiss);
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.colors.scrim)
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| dismiss(window, cx))
        .capture_key_down({
            let focus = focus.clone();
            move |event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                let mods = keystroke.modifiers;
                if keystroke.key == "tab" && !mods.control && !mods.alt && !mods.platform {
                    focus_within(&focus, mods.shift, window, cx);
                    cx.stop_propagation();
                }
            }
        })
        .child(
            modal_card(theme, width, height)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(content),
        )
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    #[test]
    fn the_dialog_keeps_oleaflys_maximum_and_shrinks_with_the_window() {
        assert_eq!(modal_size(size(px(1920.), px(1080.))), (1040., 900.));
        let (w, h) = modal_size(size(px(800.), px(600.)));
        assert!(
            (w - 752.).abs() < 0.01 && (h - 528.).abs() < 0.01,
            "{w} × {h}"
        );
    }

    /// A window with two stops of its own and, over them, a modal with two more. Its root
    /// moves the focus on Tab through every stop of the window, as the shell's does.
    struct Host {
        under: [gpui::FocusHandle; 2],
        dialog: gpui::FocusHandle,
        inside: [gpui::FocusHandle; 2],
    }

    impl gpui::Render for Host {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let stop = |id: &'static str, handle: &gpui::FocusHandle| {
                div().id(id).track_focus(handle).size_4()
            };
            div()
                .relative()
                .size_full()
                .on_key_down(|event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "tab" {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                    }
                })
                .child(stop("a", &self.under[0]))
                .child(stop("b", &self.under[1]))
                .child(modal(
                    &theme,
                    "dialog",
                    window.viewport_size(),
                    &self.dialog,
                    |_, _| {},
                    div()
                        .id("dialog-content")
                        .track_focus(&self.dialog)
                        .child(stop("x", &self.inside[0]))
                        .child(stop("y", &self.inside[1])),
                ))
        }
    }

    #[gpui::test]
    fn tab_keeps_to_the_dialog_and_wraps_at_its_ends(cx: &mut gpui::TestAppContext) {
        use crate::kit::test_support::press;
        let (host, cx) = cx.add_window_view(|_, cx| {
            let stop = |cx: &mut gpui::Context<Host>| cx.focus_handle().tab_stop(true);
            Host {
                under: [stop(cx), stop(cx)],
                dialog: cx.focus_handle(),
                inside: [stop(cx), stop(cx)],
            }
        });
        let (dialog, inside) =
            host.read_with(cx, |host, _| (host.dialog.clone(), host.inside.clone()));
        cx.update(|window, cx| window.focus(&dialog, cx));
        cx.run_until_parked();
        let focused = |cx: &mut gpui::VisualTestContext| cx.update(|window, cx| window.focused(cx));
        let mut seen = Vec::new();
        for _ in 0..5 {
            press(cx, "tab");
            seen.push(focused(cx));
        }
        let x = Some(inside[0].clone());
        let y = Some(inside[1].clone());
        // Never the window's own stops: x, y, then round to x again.
        assert_eq!(
            seen,
            [x.clone(), y.clone(), x.clone(), y.clone(), x.clone()]
        );
        press(cx, "shift-tab");
        assert_eq!(
            focused(cx),
            y,
            "Shift+Tab from the first stop wraps to the last"
        );
    }
}
