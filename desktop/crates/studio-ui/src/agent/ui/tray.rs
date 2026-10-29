//! The tray above the composer: the agent's todo list and the queued messages, on a `card_2`
//! sheet that tucks under the composer's top edge.

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Role, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
    px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::ROW_H;
use crate::kit::{Handler, Icon, focusable, icon, on_activate, reveal_actions};

/// The height of a tray's header row.
const TRAY_HEADER_H: f32 = 32.;
/// An open tray taller than this scrolls.
pub const TRAY_MAX_H: f32 = 220.;
/// A long item wraps up to this many lines, then ends in an ellipsis.
const ITEM_MAX_LINES: usize = 3;

/// The sheet the trays sit on: `card_2` behind a `line` border, round at the top only, inset
/// from the composer's sides and running under its top edge.
pub fn tray_frame(theme: &Theme) -> Div {
    let colors = theme.colors;
    div()
        .mx(px(METRICS.space_2))
        .mb(px(-METRICS.space_2))
        .pb(px(METRICS.space_2))
        .flex()
        .flex_col()
        .rounded_t(px(METRICS.radius.card))
        .border_1()
        .border_color(colors.line)
        .bg(colors.card_2)
        .overflow_hidden()
}

/// A tray's header: an icon, the title, a count in `text_3`, the current item while closed,
/// and the chevron. Clicking (or Enter, Space) opens and closes the tray.
#[derive(IntoElement)]
pub struct TrayHeader {
    theme: Theme,
    id: ElementId,
    icon: Icon,
    icon_color: Option<gpui::Rgba>,
    title: SharedString,
    count: Option<SharedString>,
    current: Option<(SharedString, gpui::Rgba)>,
    open: bool,
    on_toggle: Option<Handler>,
    selectors: Option<(&'static str, &'static str)>,
}

impl TrayHeader {
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        icon: Icon,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            icon,
            icon_color: None,
            title: title.into(),
            count: None,
            current: None,
            open: true,
            on_toggle: None,
            selectors: None,
        }
    }

    /// The icon in a status colour (the todo list's check once everything is done).
    pub fn icon_color(mut self, color: gpui::Rgba) -> Self {
        self.icon_color = Some(color);
        self
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// The item named while the tray is closed, in its colour.
    pub fn current(mut self, text: impl Into<SharedString>, color: gpui::Rgba) -> Self {
        self.current = Some((text.into(), color));
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(handler));
        self
    }

    /// Test selectors for the header and its count.
    pub fn selectors(mut self, header: &'static str, count: &'static str) -> Self {
        self.selectors = Some((header, count));
        self
    }
}

impl RenderOnce for TrayHeader {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let TrayHeader {
            theme,
            id,
            icon: glyph,
            icon_color,
            title,
            count,
            current,
            open,
            on_toggle,
            selectors,
        } = self;
        let colors = theme.colors;
        let interactive = on_toggle.is_some();
        let row = focusable(div().id(id), &theme, !interactive);
        let row = match selectors {
            Some((header, _)) => row.debug_selector(move || header.to_owned()),
            None => row,
        };
        let count_selector = selectors.map(|(_, count)| count);
        on_activate(row, false, on_toggle)
            .role(Role::Button)
            .aria_expanded(open)
            .aria_label(title.clone())
            .w_full()
            .flex_none()
            .h(px(TRAY_HEADER_H))
            .px(px(METRICS.space_2))
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .when(interactive, |row| {
                row.cursor_pointer()
                    .hover(move |style| style.bg(colors.ctl_hover))
            })
            .child(icon(
                glyph,
                METRICS.size.icon_sm,
                icon_color.unwrap_or(colors.text_2),
            ))
            .child(
                text_style(
                    div()
                        .flex_none()
                        .flex()
                        .items_baseline()
                        .gap(px(METRICS.space_1)),
                    TextStyle::Label,
                )
                .child(div().text_color(colors.text_1).child(title))
                .children(count.map(|count| {
                    let count_el = div().text_color(colors.text_3).child(count);
                    match count_selector {
                        Some(name) => count_el.debug_selector(move || name.to_owned()),
                        None => count_el,
                    }
                })),
            )
            .child(div().flex_1().min_w_0().when(!open, |slot| {
                slot.children(current.map(|(text, color)| {
                    text_style(div().truncate(), TextStyle::Small)
                        .text_color(color)
                        .child(text)
                }))
            }))
            .child(icon(
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                METRICS.size.icon_sm,
                colors.text_3,
            ))
    }
}

/// How a tray row's text reads: done is struck through in `text_3`, current `text_1`
/// medium, pending `text_2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayRowState {
    Done,
    Current,
    Pending,
}

/// One row of a tray: a status mark, the text (up to three lines), a note and an action
/// shown on hover in space that is always reserved.
#[derive(IntoElement)]
pub struct TrayRow {
    theme: Theme,
    id: ElementId,
    text: SharedString,
    state: TrayRowState,
    mark: Option<AnyElement>,
    note: Option<(SharedString, gpui::Rgba)>,
    action: Option<AnyElement>,
    selector: Option<String>,
}

impl TrayRow {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            text: text.into(),
            state: TrayRowState::Pending,
            mark: None,
            note: None,
            action: None,
            selector: None,
        }
    }

    pub fn state(mut self, state: TrayRowState) -> Self {
        self.state = state;
        self
    }

    /// The status mark before the text.
    pub fn mark(mut self, mark: impl IntoElement) -> Self {
        self.mark = Some(mark.into_any_element());
        self
    }

    /// A `Small` line under the text in its colour ("will be refused").
    pub fn note(mut self, note: impl Into<SharedString>, color: gpui::Rgba) -> Self {
        self.note = Some((note.into(), color));
        self
    }

    /// A control shown while the row is hovered (remove).
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    pub fn selector(mut self, selector: String) -> Self {
        self.selector = Some(selector);
        self
    }
}

impl RenderOnce for TrayRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let TrayRow {
            theme,
            id,
            text,
            state,
            mark,
            note,
            action,
            selector,
        } = self;
        let colors = theme.colors;
        let group = SharedString::from(format!("tray-row-{id}"));
        let color = match state {
            TrayRowState::Done => colors.text_3,
            TrayRowState::Current => colors.text_1,
            TrayRowState::Pending => colors.text_2,
        };
        let mut body =
            text_style(div().line_clamp(ITEM_MAX_LINES), TextStyle::Label).text_color(color);
        if state == TrayRowState::Done {
            body = body.line_through();
        }
        let row = div().id(id);
        let row = match selector {
            Some(name) => row.debug_selector(move || name),
            None => row,
        };
        row.group(group.clone())
            .min_h(px(ROW_H))
            .px(px(METRICS.space_2))
            .py(px(METRICS.space_1))
            .flex()
            .items_start()
            .gap(px(METRICS.space_2))
            .child(
                div()
                    .w(px(METRICS.size.icon_sm))
                    .h(px(TextStyle::Label.line_height()))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(mark),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(body.child(text))
                    .children(note.map(|(note, color)| {
                        text_style(div(), TextStyle::Small)
                            .text_color(color)
                            .child(note)
                    })),
            )
            .children(action.map(|action| {
                reveal_actions(
                    div().id(SharedString::from(format!("{group}-action"))),
                    group,
                )
                .flex_none()
                .child(action)
            }))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};
    use crate::kit::{IconButton, IconButtonSize};

    struct QueuedRow(Log<&'static str>);

    impl Render for QueuedRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::dark();
            let removed = self.0.clone();
            div().child(
                TrayRow::new(&theme, "queued", "A queued brief").action(
                    IconButton::new(&theme, "remove", Icon::X.path())
                        .size(IconButtonSize::Sm)
                        .label("Remove")
                        .on_click(move |_, _, _| removed.borrow_mut().push("removed")),
                ),
            )
        }
    }

    #[gpui::test]
    fn an_unhovered_queue_action_is_reachable_and_activatable(cx: &mut TestAppContext) {
        let removed = log();
        let shared = removed.clone();
        let (_, cx) = cx.add_window_view(|_, _| QueuedRow(shared));
        tab(cx);
        press(cx, "enter");
        assert_eq!(*removed.borrow(), ["removed"]);
    }
}
