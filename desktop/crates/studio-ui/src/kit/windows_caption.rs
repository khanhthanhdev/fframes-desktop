//! The Windows caption buttons (minimize, maximize or restore, close), after Cap's
//! `windows_caption_controls`.
//!
//! Cap clicks them through its own Win32 helpers. Here each button is a native window
//! control area instead: Windows answers the press itself (minimize, maximize and restore,
//! `WM_CLOSE`), shows Snap Layouts over maximize on Windows 11, and nothing changes window
//! state from inside a gpui update. Control areas are hit-tested from the last painted
//! frame and a cached view does not replay them, so the row must be painted uncached. No
//! button handles the mouse itself: a listener that stopped the press would keep Windows
//! from seeing it.

use gpui::{
    Div, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, WindowControlArea,
    div, prelude::FluentBuilder, px, svg,
};
use studio_theme::Theme;

use super::Icon;

/// Width of one caption button.
pub const CAPTION_BUTTON_W: f32 = 46.;

pub fn windows_caption_controls(
    theme: &Theme,
    active: bool,
    maximized: bool,
    minimizable: bool,
    resizable: bool,
) -> Div {
    let colors = theme.colors;
    let (hover, pressed) = (colors.caption_hover, colors.caption_pressed);
    let (close_hover, close_pressed) = (colors.caption_close, colors.caption_close_pressed);
    let close_glyph = colors.on_accent;
    let button = |id: &'static str, glyph: Icon, height: f32, area, enabled: bool| {
        let foreground = Theme::with_alpha(
            colors.caption_glyph,
            if active && enabled { 0.8 } else { 0.4 },
        );
        let close = id == "caption-close";
        div()
            .id(id)
            .group(id)
            .w(px(CAPTION_BUTTON_W))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .window_control_area(area)
            .when(enabled, |button| {
                button
                    .hover(move |style| style.bg(if close { close_hover } else { hover }))
                    .active(move |style| style.bg(if close { close_pressed } else { pressed }))
            })
            .child(
                svg()
                    .path(glyph.path())
                    .id("caption-glyph")
                    .w(px(10.))
                    .h(px(height))
                    .text_color(foreground)
                    .when(close, |glyph| {
                        glyph
                            .group_hover(id, move |style| style.text_color(close_glyph))
                            .group_active(id, move |style| style.text_color(close_glyph))
                    }),
            )
    };

    div()
        .flex()
        .h_full()
        .flex_shrink_0()
        .occlude()
        .child(button(
            "caption-minimize",
            Icon::CaptionMinimize,
            1.,
            WindowControlArea::Min,
            minimizable,
        ))
        .when(resizable, |row| {
            row.child(button(
                "caption-maximize",
                if maximized {
                    Icon::CaptionRestore
                } else {
                    Icon::CaptionMaximize
                },
                if maximized { 11. } else { 10. },
                WindowControlArea::Max,
                true,
            ))
        })
        .child(button(
            "caption-close",
            Icon::CaptionClose,
            10.,
            WindowControlArea::Close,
            true,
        ))
}
