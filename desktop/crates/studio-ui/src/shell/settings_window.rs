//! The settings window, a top-level window of its own as in Cap. Its root view only draws
//! the window's chrome; the pages are the shell's, drawn through the shell like
//! [`AgentSetupPage`](crate::agent::AgentSetupPage) draws the panel's.

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, Styled, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowHandle, WindowKind, WindowOptions, div, point,
    prelude::FluentBuilder, px, size,
};
use studio_theme::{METRICS, Theme};

use super::StudioShell;
use crate::kit::{SETTINGS_NAV_PAD, windows_caption_controls};

pub(super) struct SettingsWindow {
    shell: Entity<StudioShell>,
    focus: FocusHandle,
    _shell_changed: gpui::Subscription,
    _panel_changed: gpui::Subscription,
}

impl SettingsWindow {
    /// Opens the window for `shell` and focuses it.
    pub(super) fn open(shell: Entity<StudioShell>, cx: &mut App) -> Option<WindowHandle<Self>> {
        // Cap's settings window: 782 × 775, never narrower than 780 × 560.
        let layout = METRICS.layout;
        let bounds = Bounds::centered(None, size(px(layout.settings_w), px(layout.settings_h)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // The pages draw the title bar, as in the main window: the caption
                    // buttons on Windows, the traffic lights over the nav column on macOS.
                    titlebar: Some(TitlebarOptions {
                        title: Some("fframes Studio Settings".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(
                            px(layout.traffic_light_inset),
                            px(layout.traffic_light_inset),
                        )),
                    }),
                    app_owns_titlebar_drag: true,
                    // A normal window: a floating one hides whenever the app deactivates.
                    kind: WindowKind::Normal,
                    window_min_size: Some(size(
                        px(layout.settings_min_w),
                        px(layout.settings_min_h),
                    )),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        studio_theme::bind_window(window, cx);
                        Self::new(shell, cx)
                    })
                },
            )
            .ok()?;
        // Focused once the window exists, not from inside its builder.
        let _ = handle.update(cx, |this, window, cx| window.focus(&this.focus, cx));
        Some(handle)
    }

    fn new(shell: Entity<StudioShell>, cx: &mut Context<Self>) -> Self {
        // Closing the window (its close button, Ctrl/Cmd+W) releases this view: the shell
        // forgets the window then.
        let weak = shell.downgrade();
        cx.on_release(move |_, cx| {
            let _ = weak.update(cx, |shell, cx| {
                shell.settings_window = None;
                cx.notify();
            });
        })
        .detach();
        // The Agent page's Review reads the panel's snapshot, which changes without the shell.
        let panel = shell.read(cx).panel.clone();
        Self {
            _shell_changed: cx.observe(&shell, |_, _, cx| cx.notify()),
            _panel_changed: cx.observe(&panel, |_, _, cx| cx.notify()),
            shell,
            focus: cx.focus_handle(),
        }
    }
}

/// Brings `window` to the front.
pub(super) fn activate(window: AnyWindowHandle, cx: &mut App) {
    let _ = window.update(cx, |_, window, _| window.activate_window());
}

/// Closes `window`.
pub(super) fn close(window: AnyWindowHandle, cx: &mut App) {
    let _ = window.update(cx, |_, window, _| window.remove_window());
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let windows = cfg!(target_os = "windows");
        let frame = self.shell.update(cx, |shell, cx| {
            // The pages paint with this window's theme. It goes back at once: the main
            // window's render repaints its cached sections only when its own refresh sees
            // the theme change.
            let main = std::mem::replace(&mut shell.theme, theme);
            let frame = shell.render_settings(nav_header(), cx);
            shell.theme = main;
            frame
        });
        div()
            .id("settings-window")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                let keystroke = &event.keystroke;
                if keystroke.modifiers.secondary() && keystroke.key == "w" {
                    window.remove_window();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.window)
            .text_color(theme.colors.text_1)
            .when(windows, |root| {
                root.child(
                    div()
                        .flex()
                        .flex_none()
                        .justify_end()
                        .w_full()
                        .h(px(METRICS.layout.windows_title_h))
                        .bg(theme.colors.window)
                        .border_b_1()
                        .border_color(theme.colors.line)
                        .window_control_area(WindowControlArea::Drag)
                        .child(windows_caption_controls(
                            &theme,
                            window.is_window_active(),
                            window.is_maximized(),
                            true,
                            true,
                        )),
                )
            })
            .child(div().flex().flex_1().min_h_0().child(frame))
    }
}

/// The strip above the nav rows that the traffic lights sit in on macOS, a drag area like
/// the main window's header. Windows has its title strip instead, Linux the system's.
fn nav_header() -> Option<gpui::AnyElement> {
    cfg!(target_os = "macos").then(|| {
        div()
            .flex_none()
            .w_full()
            // The nav column's padding brings the first row down to the header's height.
            .h(px(METRICS.layout.header_h - SETTINGS_NAV_PAD))
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .into_any_element()
    })
}
