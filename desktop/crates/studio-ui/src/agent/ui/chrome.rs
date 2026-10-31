//! The panel's frame around the conversation: the header with the tabs, the one-line status,
//! the working trailer under the last row, the jump-to-latest pill and the empty states.

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Role, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_theme::{METRICS, Shadow, TextStyle, Theme, shadow_for, text_style};

use super::ROW_H;
use super::metrics::{header, home, status};
use crate::kit::{Handler, Spinner, StatusDot, Tone, focusable, on_activate, selectable};

/// A centred empty state: `title` in its style and `text_1`, one `Small` line in `text_2`,
/// both wrapping in a narrow sidebar.
pub fn panel_empty(
    theme: &Theme,
    style: TextStyle,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
) -> Div {
    let colors = theme.colors;
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(METRICS.space_1))
        .text_center()
        .child(
            text_style(div(), style)
                .text_color(colors.text_1)
                .child(selectable(title.into())),
        )
        .child(
            text_style(div(), TextStyle::Small)
                .text_color(colors.text_2)
                .child(selectable(body.into())),
        )
}

/// The empty transcript's home: a centred column with a `Display` heading and one `Small` line
/// in `text_2` under it; a caller adds suggestions or an action as further children, 20 px apart.
pub fn panel_home(
    theme: &Theme,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
) -> Div {
    let colors = theme.colors;
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(home::GAP))
        .py(px(home::PAD_Y))
        .child(
            div()
                .w_full()
                .max_w(px(home::MAX_W))
                .flex()
                .flex_col()
                .gap(px(home::TITLE_GAP))
                .text_center()
                .child(
                    text_style(div(), TextStyle::Display)
                        .text_color(colors.text_1)
                        .child(selectable(title.into())),
                )
                .child(
                    text_style(div(), TextStyle::Small)
                        .text_color(colors.text_2)
                        .child(selectable(body.into())),
                ),
        )
}

/// The 36 px header row: the tabs on the left, the panel's 28 px icon buttons on the right,
/// a `line` under it.
#[derive(IntoElement)]
pub struct PanelHeader {
    theme: Theme,
    tabs: AnyElement,
    actions: Vec<AnyElement>,
}

impl PanelHeader {
    pub fn new(theme: &Theme, tabs: impl IntoElement) -> Self {
        Self {
            theme: *theme,
            tabs: tabs.into_any_element(),
            actions: Vec::new(),
        }
    }

    pub fn action(mut self, button: impl IntoElement) -> Self {
        self.actions.push(button.into_any_element());
        self
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .h(px(header::H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(header::GAP))
            .px(px(header::PAD_X))
            .border_b_1()
            .border_color(self.theme.colors.line)
            .child(div().min_w_0().child(self.tabs))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(header::ACTION_GAP))
                    .children(self.actions),
            )
    }
}

/// One 24 px line: a status dot, what the agent is doing in `Small` `text_2`, and the
/// policy badge and buttons on the right.
#[derive(IntoElement)]
pub struct StatusLine {
    tone: Tone,
    summary: SharedString,
    trailing: Vec<AnyElement>,
    theme: Theme,
}

impl StatusLine {
    pub fn new(theme: &Theme, tone: Tone, summary: impl Into<SharedString>) -> Self {
        Self {
            tone,
            summary: summary.into(),
            trailing: Vec::new(),
            theme: *theme,
        }
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }
}

impl RenderOnce for StatusLine {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .id("agent-status-line")
            .min_h(px(ROW_H))
            .flex()
            .items_center()
            .gap(px(status::GAP))
            .child(StatusDot::new(self.tone))
            .child(
                text_style(div().flex_1().min_w_0().truncate(), TextStyle::Small)
                    .text_color(self.theme.colors.text_2)
                    .child(selectable(self.summary)),
            )
            .children(self.trailing)
    }
}

/// `12s`, `3m 05s`, `1h 02m`.
pub fn elapsed_label(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// Under the last row while the agent works: a spinner, what it is doing and for how long.
#[derive(IntoElement)]
pub struct WorkingTrailer {
    theme: Theme,
    label: SharedString,
    elapsed: Option<SharedString>,
}

impl WorkingTrailer {
    pub fn new(theme: &Theme, label: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            label: label.into(),
            elapsed: None,
        }
    }

    pub fn elapsed(mut self, elapsed: impl Into<SharedString>) -> Self {
        self.elapsed = Some(elapsed.into());
        self
    }
}

impl RenderOnce for WorkingTrailer {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        text_style(div(), TextStyle::Small)
            .id("agent-working")
            .h(px(ROW_H))
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(Spinner::new(&self.theme, "agent-working-spinner"))
            .child(div().text_color(colors.text_2).child(self.label))
            .children(
                self.elapsed
                    .map(|elapsed| div().text_color(colors.text_3).child(elapsed)),
            )
    }
}

/// The pill that floats above the dock while the reader is away from the newest rows.
#[derive(IntoElement)]
pub struct JumpPill {
    theme: Theme,
    id: ElementId,
    newer: usize,
    on_click: Option<Handler>,
}

impl JumpPill {
    /// `newer` counts rows that are not laid out; it shows as `· N new`.
    pub fn new(theme: &Theme, id: impl Into<ElementId>, newer: usize) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            newer,
            on_click: None,
        }
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for JumpPill {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        let label = if self.newer > 0 {
            format!("\u{2193} Jump to latest \u{b7} {} new", self.newer)
        } else {
            "\u{2193} Jump to latest".to_owned()
        };
        let pill = focusable(div().id(self.id), &self.theme, false);
        text_style(on_activate(pill, false, self.on_click), TextStyle::Small)
            .role(Role::Button)
            .aria_label(SharedString::from(label.clone()))
            .h(px(METRICS.size.control_h))
            .px(px(METRICS.space_3))
            .flex()
            .items_center()
            .rounded_full()
            .border_1()
            .border_color(colors.line)
            .bg(colors.card)
            .shadow(shadow_for(self.theme.appearance, Shadow::Pop).into_vec())
            .text_color(colors.text_1)
            .cursor_pointer()
            .hover(move |style| style.bg(colors.ctl_hover))
            .child(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_reads_short() {
        assert_eq!(elapsed_label(12), "12s");
        assert_eq!(elapsed_label(185), "3m 05s");
        assert_eq!(elapsed_label(3720), "1h 02m");
    }
}
