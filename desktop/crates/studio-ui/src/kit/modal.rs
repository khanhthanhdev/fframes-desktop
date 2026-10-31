//! A modal dialog over its window, Oleafly's `ModalShell`: the window dims under the
//! `scrim`, and the dialog is a card centred on it, `min(1040, 94 %)` wide and `min(900,
//! 88 %)` tall, rounded behind a hairline with the preview shadow. A press on the scrim
//! dismisses it; presses inside the card stay in it, and nothing under the scrim answers
//! the pointer.

use std::rc::Rc;

use gpui::{
    App, Div, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Size,
    Stateful, Styled, Window, div, px,
};
use studio_theme::{METRICS, Shadow, Theme, shadow_for};

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
/// it the last child of the window's root so it paints over everything else.
pub fn modal(
    theme: &Theme,
    id: impl Into<ElementId>,
    viewport: Size<Pixels>,
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
}
