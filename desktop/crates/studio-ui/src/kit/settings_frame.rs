//! The settings frame, after Cap's settings window (`settings_window.rs`): a 227 px nav
//! column on `window` beside a scrolling page on `card` that holds the page's sections.
//!
//! The nav rows look like [`ListRow`](super::ListRow)s: `list_row_h` high with a 16 px
//! `text_2` icon and a `Label` title, `ctl_hover` under the pointer and `ctl_active` when
//! selected. Cap's nav rows answer the mouse only. Here every row is a tab stop with the
//! focus ring, and Enter or Space opens it, like every other control. The frame can start
//! with a Back row, for a frame shown in place of another view rather than in its own window.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::interact::{BoundsHandler, Handler, bounds_probe, focusable};
use super::list_row::row_look;
use super::{Icon, icon};

/// Cap's `SIDEBAR_WIDTH`.
pub const SETTINGS_NAV_W: f32 = 227.;
/// The nav column's padding; a window's title strip above it is the header less this.
pub const SETTINGS_NAV_PAD: f32 = METRICS.space_2;
/// The space between nav rows, as in a `List`.
const NAV_ROW_GAP: f32 = METRICS.space_1 / 2.;

/// One page in the nav column.
#[derive(Debug, Clone)]
pub struct SettingsNavItem {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Icon,
    pub selected: bool,
}

impl SettingsNavItem {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: Icon,
        selected: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon,
            selected,
        }
    }
}

type SelectHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;
/// Receives a nav row's index and its bounds on every prepaint.
type ItemBoundsHandler = Rc<dyn Fn(usize, gpui::Bounds<gpui::Pixels>, &mut App)>;

#[derive(IntoElement)]
pub struct SettingsFrame {
    id: ElementId,
    theme: Theme,
    items: Vec<SettingsNavItem>,
    on_select: Option<SelectHandler>,
    back: Option<Handler>,
    back_bounds: Option<BoundsHandler>,
    item_bounds: Option<ItemBoundsHandler>,
    nav_header: Option<AnyElement>,
    content: Vec<AnyElement>,
}

impl SettingsFrame {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, items: Vec<SettingsNavItem>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            items,
            on_select: None,
            back: None,
            back_bounds: None,
            item_bounds: None,
            nav_header: None,
            content: Vec::new(),
        }
    }

    /// An element above the nav rows, such as the strip a window's title bar buttons sit in.
    pub fn nav_header(mut self, header: Option<AnyElement>) -> Self {
        self.nav_header = header;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Adds the Back row (`settings-back`) at the top of the nav column.
    pub fn on_back(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.back = Some(Rc::new(handler));
        self
    }

    /// Reports where the Back row was painted.
    pub fn on_back_bounds(
        mut self,
        report: impl Fn(gpui::Bounds<gpui::Pixels>, &mut App) + 'static,
    ) -> Self {
        self.back_bounds = Some(Rc::new(report));
        self
    }

    /// Reports where each nav row was painted, by its index.
    pub fn on_item_bounds(
        mut self,
        report: impl Fn(usize, gpui::Bounds<gpui::Pixels>, &mut App) + 'static,
    ) -> Self {
        self.item_bounds = Some(Rc::new(report));
        self
    }

    /// The page's sections, 32 px apart.
    pub fn children(mut self, content: impl IntoIterator<Item = AnyElement>) -> Self {
        self.content.extend(content);
        self
    }
}

/// A nav row in the look of a `ListRow`: `list_row_h` high, a 16 px `text_2` icon and a
/// `Label` title; `ctl_active` when selected, `ctl_hover` under the pointer otherwise.
fn nav_row(
    theme: &Theme,
    id: SharedString,
    label: SharedString,
    row_icon: Icon,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let look = row_look(theme, selected, false, false);
    let row = focusable(div().id(id), theme, false)
        .role(Role::Tab)
        .aria_selected(selected)
        .aria_label(label.clone())
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .h(px(METRICS.size.list_row_h))
        .px(px(METRICS.space_2))
        .gap(px(METRICS.space_2))
        .rounded(px(METRICS.radius.item))
        .text_color(look.title)
        .cursor_pointer();
    text_style(row, TextStyle::Label)
        .when_some(look.fill, |row, fill| row.bg(fill))
        .when_some(look.hover, |row, hover| {
            row.hover(move |style| style.bg(hover))
        })
        .child(icon(row_icon, METRICS.size.icon_md, theme.colors.text_2).flex_shrink_0())
        .child(div().flex_1().min_w_0().truncate().child(label))
}
impl RenderOnce for SettingsFrame {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let SettingsFrame {
            id,
            theme,
            items,
            on_select,
            back,
            back_bounds,
            item_bounds,
            nav_header,
            content,
        } = self;

        let back = back.map(|handler| {
            nav_row(
                &theme,
                "settings-back".into(),
                "Back".into(),
                Icon::ArrowLeft,
                false,
            )
            .role(Role::Button)
            .children(back_bounds.map(bounds_probe))
            .on_click(move |_, window, cx| handler(window, cx))
        });
        let rows = items.into_iter().enumerate().map(|(index, item)| {
            let handler = on_select.clone();
            let report = item_bounds.clone();
            nav_row(&theme, item.id, item.label, item.icon, item.selected)
                .children(report.map(|report| {
                    bounds_probe(Rc::new(move |bounds, cx: &mut App| {
                        report(index, bounds, cx)
                    }))
                }))
                .when_some(handler, |row, handler| {
                    row.on_click(move |_, window, cx| handler(&index, window, cx))
                })
        });

        div()
            .id(id)
            .size_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .text_color(theme.colors.text_1)
            .child(
                div()
                    .id("settings-nav")
                    .role(Role::TabList)
                    .flex()
                    .flex_col()
                    .flex_none()
                    .h_full()
                    .w(px(SETTINGS_NAV_W))
                    .p(px(SETTINGS_NAV_PAD))
                    .gap(px(NAV_ROW_GAP))
                    .overflow_y_scroll()
                    .bg(theme.colors.window)
                    .children(nav_header)
                    .children(back)
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .bg(theme.colors.card)
                    .border_l_1()
                    .border_color(theme.colors.line)
                    .child(
                        div()
                            .id("settings-page")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .pt(px(METRICS.space_6))
                            .px(px(METRICS.space_6))
                            .pb(px(METRICS.space_8))
                            .gap(px(METRICS.space_8))
                            .children(content),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Frame {
        selected: usize,
        picks: Log<&'static str>,
    }

    impl Render for Frame {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let items = ["Appearance", "Setup"]
                .into_iter()
                .enumerate()
                .map(|(i, label)| {
                    SettingsNavItem::new(label, label, Icon::Palette, i == self.selected)
                })
                .collect();
            let picks = self.picks.clone();
            let back = self.picks.clone();
            SettingsFrame::new(&theme, "frame", items)
                .on_select(cx.listener(move |this, index: &usize, _, cx| {
                    picks.borrow_mut().push(["Appearance", "Setup"][*index]);
                    this.selected = *index;
                    cx.notify();
                }))
                .on_back(move |_, _| back.borrow_mut().push("back"))
        }
    }

    #[gpui::test]
    fn back_and_every_page_are_tab_stops_that_activate_with_the_keyboard(cx: &mut TestAppContext) {
        let picks = log();
        let shared = picks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Frame {
            selected: 0,
            picks: shared,
        });
        tab(cx);
        press(cx, "enter");
        tab(cx);
        tab(cx);
        press(cx, "space");
        assert_eq!(*picks.borrow(), ["back", "Setup"]);
    }
}
