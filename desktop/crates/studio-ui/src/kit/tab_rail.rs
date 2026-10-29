//! The tab rail: `ConfigSidebar`'s six icon-only triggers under a
//! `KTabs.Indicator` box (`ConfigSidebar.tsx:593-692`).
//!
//! **This is not the settings sidebar's shape.** The usage matrix lists `KTabs`
//! in the editor and the screenshot editor only; the settings window's sidebar
//! is a vertical nav *list* (icon plus label rows, `.cap-settings-nav`), and
//! `settings.tsx` renders it with `<For>` over `settingsItems`, not with tabs.
//! They were checked against each other and kept apart deliberately -- merging
//! them would mean inventing a shared component neither app has.
//!
//! The indicator does not slide (no transform in this gpui rev); the selected
//! item paints the `size-9 rounded-lg bg-gray-3` box itself, which is what the
//! editor window has always done.
//!
//! Each tab is exactly one element -- the pill. A wrapper carrying the hit box,
//! the tooltip anchor or a focus ring would paint its own square corners around
//! the pill's radius, which is what the first pass shipped.

use gpui::{
    App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use studio_theme::{METRICS, Theme};

use super::interact::{Nav, States, dim_disabled, focusable, nav_key, step_enabled};

#[derive(Debug, Clone)]
pub struct TabRailItem {
    pub icon: SharedString,
    /// The tab's name. It is not drawn -- the rail is icon-only -- so it is
    /// carried as the tooltip instead.
    pub label: SharedString,
    pub selected: bool,
    pub disabled: bool,
    /// The tab's element id; `{rail id}-{index}` when `None`.
    pub id: Option<SharedString>,
    /// A small accent dot on the tab: something behind it waits for the user.
    pub dot: bool,
}

impl TabRailItem {
    pub fn new(
        icon: impl Into<SharedString>,
        label: impl Into<SharedString>,
        selected: bool,
        disabled: bool,
    ) -> Self {
        Self {
            icon: icon.into(),
            label: label.into(),
            selected,
            disabled,
            id: None,
            dot: false,
        }
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn dot(mut self, dot: bool) -> Self {
        self.dot = dot;
        self
    }
}

/// Diameter of a tab's attention dot.
const DOT: f32 = 6.;

/// The tab a Left or Right key moves to: one step, no wrap, skipping disabled
/// tabs.
pub fn tab_target(from: usize, key: Nav, items: &[TabRailItem]) -> Option<usize> {
    let delta = match key {
        Nav::Left => -1,
        Nav::Right => 1,
        _ => return None,
    };
    step_enabled(from, delta, items.len(), |i| items[i].disabled)
}

/// The index arrives by reference, the same shape the other index-dispatching
/// components use, so `cx.listener` builds the handler directly.
type TabHandler = Box<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// A tab's box and the rail's side padding.
const TAB_W: f32 = 40.;
const TAB_H: f32 = 30.;
const RAIL_PAD_X: f32 = 10.;
/// Where the attention dot sits in the tab's corner.
const DOT_TOP: f32 = 4.;
const DOT_RIGHT: f32 = 8.;

/// The sidebar's icon tabs: a bar on `card` under a `line` hairline, `tab_bar_h` high,
/// the selected tab on `ctl_active`.
#[derive(IntoElement)]
pub struct TabRail {
    id: ElementId,
    items: Vec<TabRailItem>,
    theme: Theme,
    on_select: Option<TabHandler>,
}

impl TabRail {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, items: Vec<TabRailItem>) -> Self {
        Self {
            id: id.into(),
            items,
            theme: *theme,
            on_select: None,
        }
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TabRail {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let TabRail {
            id,
            items,
            theme,
            on_select,
        } = self;
        let colors = theme.colors;
        let states = States::of(&theme);
        let selected_icon = Hsla::from(colors.text_1);
        let idle_icon = Hsla::from(colors.text_2);
        let prefix: SharedString = match &id {
            ElementId::Name(name) => name.clone(),
            other => SharedString::from(format!("{other:?}")),
        };
        let handler: Option<std::rc::Rc<TabHandler>> = on_select.map(std::rc::Rc::new);
        let keyed = std::rc::Rc::new(items.clone());

        div()
            .id(id)
            .role(Role::TabList)
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(METRICS.layout.tab_bar_h))
            .flex_none()
            .overflow_hidden()
            // Inside the card's 1 px border.
            .rounded_t(px(METRICS.radius.card - METRICS.size.divider))
            .justify_around()
            .px(px(RAIL_PAD_X))
            .border_b_1()
            .border_color(colors.line)
            .bg(colors.card)
            .children(items.into_iter().enumerate().map(|(index, item)| {
                let handler = handler.clone();
                let keyed = keyed.clone();
                let selected = item.selected;
                let disabled = item.disabled;
                let label = item.label.clone();
                let item_id = item
                    .id
                    .clone()
                    .unwrap_or_else(|| SharedString::from(format!("{prefix}-{index}")));

                let tab = focusable(div().id(item_id), &theme, disabled)
                    .role(Role::Tab)
                    .aria_selected(selected)
                    .aria_label(item.label.clone())
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .h(px(TAB_H))
                    .w(px(TAB_W))
                    .rounded(px(METRICS.radius.tab));
                dim_disabled(tab, disabled)
                    .when(selected, |this| this.bg(states.selected))
                    .when(!selected && !disabled, |this| {
                        this.cursor_pointer()
                            .hover(move |style| style.bg(states.hover))
                    })
                    .tooltip_show_delay(crate::kit::TOOLTIP_SHOW_DELAY)
                    .tooltip(move |_, cx| crate::kit::Tooltip::new(&theme, label.clone()).view(cx))
                    .child(
                        svg()
                            .path(item.icon)
                            .size(px(METRICS.size.icon_md))
                            .text_color(if selected { selected_icon } else { idle_icon }),
                    )
                    .when(item.dot, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(DOT_TOP))
                                .right(px(DOT_RIGHT))
                                .size(px(DOT))
                                .rounded_full()
                                .bg(colors.accent),
                        )
                    })
                    .when_some(handler.filter(|_| !disabled), |this, handler| {
                        let keys = handler.clone();
                        this.on_click(move |_, window, cx| handler(&index, window, cx))
                            .on_key_down(move |event, window, cx| {
                                let Some(key @ (Nav::Left | Nav::Right)) =
                                    nav_key(event.keystroke.key.as_str())
                                else {
                                    return;
                                };
                                // Every enabled item is a tab stop, so the key belongs to the
                                // focused one, which need not be the selected one.
                                if let Some(target) = tab_target(index, key, &keyed) {
                                    keys(&target, window, cx);
                                    if key == Nav::Left {
                                        window.focus_prev(cx);
                                    } else {
                                        window.focus_next(cx);
                                    }
                                    cx.stop_propagation();
                                }
                            })
                    })
            }))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::interact::Nav;
    use crate::kit::test_support::{Log, log, press, tab};

    fn items(disabled: &[bool]) -> Vec<TabRailItem> {
        disabled
            .iter()
            .enumerate()
            .map(|(i, d)| TabRailItem::new("icons/x.svg", format!("t{i}"), i == 0, *d))
            .collect()
    }

    #[test]
    fn keys_move_selection_without_wrapping() {
        let t = items(&[false, false, false]);
        assert_eq!(tab_target(1, Nav::Right, &t), Some(2));
        assert_eq!(tab_target(1, Nav::Left, &t), Some(0));
        assert_eq!(tab_target(2, Nav::Right, &t), None);
        assert_eq!(tab_target(0, Nav::Left, &t), None);
        assert_eq!(tab_target(0, Nav::Down, &t), None);
        assert_eq!(
            tab_target(0, Nav::Right, &items(&[false, true, false])),
            Some(2)
        );
    }

    struct Rail {
        selected: usize,
        picks: Log<usize>,
    }

    impl Render for Rail {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let picks = self.picks.clone();
            let items = ["Project", "Style", "Agent"]
                .into_iter()
                .enumerate()
                .map(|(i, label)| TabRailItem::new("icons/x.svg", label, i == self.selected, false))
                .collect();
            TabRail::new(&theme, "rail", items).on_select(cx.listener(
                move |this, index: &usize, _, cx| {
                    picks.borrow_mut().push(*index);
                    this.selected = *index;
                    cx.notify();
                },
            ))
        }
    }

    #[gpui::test]
    fn arrows_step_from_the_focused_tab_not_the_selected_one(cx: &mut TestAppContext) {
        let picks = log();
        let shared = picks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Rail {
            selected: 2,
            picks: shared,
        });
        tab(cx);
        press(cx, "right");
        press(cx, "right");
        assert_eq!(*picks.borrow(), [1, 2]);
    }

    #[gpui::test]
    fn arrows_move_between_tabs(cx: &mut TestAppContext) {
        let picks = log();
        let shared = picks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Rail {
            selected: 0,
            picks: shared,
        });
        tab(cx);
        press(cx, "right");
        press(cx, "right");
        press(cx, "right");
        press(cx, "left");
        assert_eq!(*picks.borrow(), [1, 2, 1]);
    }
}
