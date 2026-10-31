//! The settings frame, Oleafly's `SettingsModal`: a 208 px navigation column (`w-52 p-3`)
//! headed by its title, beside the page, whose 48 px header (`h-12 px-5`) holds the selected
//! page's title (`BodyStrong`) over a hairline. Both sit on the same muted fill (Studio's
//! `sidebar`), apart by a hairline, so the setting rows' cards stand out on it.
//!
//! Nav rows are Oleafly's `NavItem`: a 16 px icon and a `Body` title in `text_2`; the
//! selected row on the document fill behind a hairline, in `text_1` at the medium weight.
//! Every row is a tab stop with the
//! focus ring, and Enter or Space opens it, like every other control. The frame can start
//! with a Back row, for a frame shown in place of another view rather than in its own window.
//!
//! As in Oleafly's settings, a search field can sit above the rows ([`SettingsFrame::search`]),
//! the rows scroll on their own, and a footer of small icon links
//! ([`SettingsFooterLink`]) stays pinned to the bottom of the column.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, Rgba, Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window,
    canvas, div, point, prelude::FluentBuilder, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::interact::{
    BoundsHandler, Handler, RevealFocused, RevealedBounds, bounds_probe, focusable,
};
use super::{Emphasis, Icon, IconButton, IconButtonSize, icon};

/// Oleafly's settings nav, `w-52`.
pub const SETTINGS_NAV_W: f32 = 208.;
/// The nav column's padding (`p-3`); a window's title strip above it is the header less this.
pub const SETTINGS_NAV_PAD: f32 = METRICS.space_3;
/// A nav row: `py-2` around the 20 px `Body` line, inside a 1 px hairline.
const NAV_ROW_H: f32 = 38.;
/// The nav title's line, so its centre meets the page header's (12 px padding + 24 / 2).
const NAV_TITLE_H: f32 = 24.;
/// The page's padding (`p-5`) and the header's side padding (`px-5`).
const PAGE_PAD: f32 = METRICS.space_4 + METRICS.space_1;
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

/// A small icon link in the footer pinned under the nav rows (Oleafly's `SettingsFooter`).
pub struct SettingsFooterLink {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Icon,
    pub open: Handler,
}

impl SettingsFooterLink {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: Icon,
        open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon,
            open: Rc::new(open),
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
    search: Option<AnyElement>,
    empty: Option<SharedString>,
    footer: Vec<SettingsFooterLink>,
    title: Option<SharedString>,
    heading: Option<SharedString>,
    header_end: Option<AnyElement>,
    radius: f32,
    scroll: Option<ScrollHandle>,
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
            search: None,
            empty: None,
            footer: Vec::new(),
            title: None,
            heading: None,
            header_end: None,
            radius: 0.,
            scroll: None,
            content: Vec::new(),
        }
    }

    /// The content header's title, when it is not the selected row's (no row is shown).
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The nav column's own title ("Settings"), level with the page's title.
    pub fn heading(mut self, heading: impl Into<SharedString>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    /// An element at the end of the page's header, such as a window's caption buttons.
    pub fn header_end(mut self, element: Option<AnyElement>) -> Self {
        self.header_end = element;
        self
    }

    /// Rounds the frame's outer corners to `radius`, for a frame inside a rounded card: gpui
    /// clips to rectangles, so each column rounds its own fill.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// The search field at the top of the nav column, above the rows.
    pub fn search(mut self, field: impl IntoElement) -> Self {
        self.search = Some(field.into_any_element());
        self
    }

    /// The line shown in place of the rows when there are none (a search matched nothing).
    pub fn empty(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = Some(text.into());
        self
    }

    /// Icon links pinned to the bottom of the nav column, apart by hairlines.
    pub fn footer(mut self, links: Vec<SettingsFooterLink>) -> Self {
        self.footer = links;
        self
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

    /// Tracks the page's scrolling with `handle`, so a deep link can bring one item into view
    /// (see [`reveal_probe`]) and a control focused from the keyboard scrolls into view.
    pub fn scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// The page's sections, 32 px apart.
    pub fn children(mut self, content: impl IntoIterator<Item = AnyElement>) -> Self {
        self.content.extend(content);
        self
    }
}

/// An invisible layer over one item of a settings page that scrolls the page (whose
/// [`SettingsFrame::scroll`] handle is `handle`) until the item is in view, then calls
/// `done` with the app. For a deep link to that item: the item is `relative` and holds the
/// layer only while the link is pending, and `done` ends it.
pub fn reveal_probe(handle: ScrollHandle, done: impl Fn(&mut App) + 'static) -> AnyElement {
    canvas(
        move |target, window, cx| {
            let view = handle.bounds();
            // The page has not been laid out yet.
            if view.size.height <= px(0.) {
                return;
            }
            let offset = handle.offset();
            let wanted = reveal_offset(view, target, offset.y, handle.max_offset().y, px(PAGE_PAD));
            if wanted != offset.y {
                handle.set_offset(point(offset.x, wanted));
                // The offset shows from the next frame on.
                window.request_animation_frame();
            }
            done(cx);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// The [RevealFocused] listener of a container whose scrolling is handle: when a control
/// inside it takes the focus from the keyboard, scrolls until the control is in view with
/// margin to spare.
pub fn reveal_focused(
    handle: ScrollHandle,
    margin: f32,
) -> impl Fn(&RevealFocused, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        let Some(target) = cx
            .try_global::<RevealedBounds>()
            .and_then(|revealed| revealed.0)
        else {
            return;
        };
        cx.set_global(RevealedBounds(None));
        let view = handle.bounds();
        if view.size.height <= px(0.) {
            return;
        }
        let offset = handle.offset();
        let wanted = reveal_offset(view, target, offset.y, handle.max_offset().y, px(margin));
        if wanted != offset.y {
            handle.set_offset(point(offset.x, wanted));
            window.refresh();
        }
    }
}

/// The vertical scroll offset (zero or below) that shows `target`, as painted at `offset`,
/// inside `view` with `margin` to spare: unchanged when it is in view already, otherwise its
/// top `margin` below the view's top, within the page's `max` scroll.
fn reveal_offset(
    view: Bounds<Pixels>,
    target: Bounds<Pixels>,
    offset: Pixels,
    max: Pixels,
    margin: Pixels,
) -> Pixels {
    if target.top() >= view.top() + margin && target.bottom() <= view.bottom() - margin {
        return offset;
    }
    // Where the target's top sits in the page, unscrolled.
    let top = target.top() - view.top() - offset;
    (margin - top).clamp(-max, px(0.))
}

/// The content header's height (Oleafly's settings `h-12`).
const CONTENT_HEADER_H: f32 = 48.;
/// The document fill's alpha under the pointer (`hover:bg-background/60`).
const NAV_HOVER_ALPHA: f32 = 0.6;

/// A navigation row, Oleafly's `NavItem`: selected, the document fill behind a `line`
/// hairline with `text_1` at the medium weight; otherwise a transparent hairline (so
/// selecting does not shift the row) and `text_2`, turning `text_1` on a translucent document
/// fill under the pointer.
fn nav_row(
    theme: &Theme,
    id: SharedString,
    label: SharedString,
    row_icon: Icon,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let colors = theme.colors;
    let (text, border) = if selected {
        (colors.text_1, colors.line)
    } else {
        (
            colors.text_2,
            Rgba {
                a: 0.,
                ..colors.line
            },
        )
    };
    let hover = Rgba {
        a: NAV_HOVER_ALPHA,
        ..colors.window
    };
    let row = focusable(div().id(id), theme, false)
        .role(Role::Tab)
        .aria_selected(selected)
        .aria_label(label.clone())
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .h(px(NAV_ROW_H))
        .px(px(METRICS.space_2 + METRICS.space_1 / 2.))
        .gap(px(METRICS.space_2 + METRICS.space_1 / 2.))
        .rounded(px(METRICS.radius.control))
        .border(px(METRICS.size.divider))
        .border_color(border)
        .text_color(text)
        .cursor_pointer();
    // The icon is an `svg`, which does not inherit the text colour, so it keeps the row's
    // resting colour under the pointer.
    let style = if selected {
        TextStyle::BodyMedium
    } else {
        TextStyle::Body
    };
    text_style(row, style)
        .when(selected, |row| row.bg(colors.window))
        .when(!selected, |row| {
            row.hover(move |style| style.bg(hover).text_color(colors.text_1))
        })
        .child(icon(row_icon, METRICS.size.icon_md, text).flex_shrink_0())
        .child(div().flex_1().min_w_0().truncate().child(label))
}
/// The footer's link buttons and the hairline between two of them (`h-4 w-px`).
const FOOTER_RULE_H: f32 = METRICS.size.icon_md;

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
            search,
            empty,
            footer,
            title,
            heading,
            header_end,
            radius,
            scroll,
            content,
        } = self;
        let heading = heading.map(|heading| {
            text_style(div(), TextStyle::BodyStrong)
                .flex()
                .flex_none()
                .items_center()
                .h(px(NAV_TITLE_H))
                .px(px(METRICS.space_2))
                .mb(px(METRICS.space_2))
                .text_color(theme.colors.text_1)
                .child(heading)
        });
        let colors = theme.colors;

        // The page's title is the selected row's, unless the caller names it.
        let title = title.or_else(|| {
            items
                .iter()
                .find(|item| item.selected)
                .map(|item| item.label.clone())
        });
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
        let no_rows = items.is_empty();
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
        let empty = empty.filter(|_| no_rows).map(|text| {
            text_style(div(), TextStyle::Small)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(METRICS.space_2))
                .px(px(METRICS.space_2))
                .py(px(METRICS.space_8))
                .text_color(colors.text_2)
                .child(icon(Icon::Search, METRICS.size.icon_lg, colors.text_3))
                .child(text)
        });
        let link_count = footer.len();
        let footer = (link_count > 0).then(|| {
            div()
                .id("settings-footer")
                .flex()
                .flex_none()
                .items_center()
                .justify_between()
                .mt(px(METRICS.space_2))
                .pt(px(METRICS.space_3))
                .border_t(px(METRICS.size.divider))
                .border_color(colors.line)
                .children(footer.into_iter().enumerate().flat_map(|(index, link)| {
                    let rule = (index > 0).then(|| {
                        div()
                            // A border, which gpui draws at least one device pixel wide.
                            .flex_none()
                            .h(px(FOOTER_RULE_H))
                            .border_l(px(METRICS.size.divider))
                            .border_color(colors.line)
                            .into_any_element()
                    });
                    let open = link.open;
                    let button = IconButton::new(&theme, link.id, link.icon.path())
                        .emphasis(Emphasis::Ghost)
                        .size(IconButtonSize::Md)
                        .label(link.label.clone())
                        .tooltip(link.label)
                        .on_click(move |_, window, cx| open(window, cx))
                        .into_any_element();
                    rule.into_iter().chain([button])
                }))
        });

        div()
            .id(id)
            .size_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .text_color(colors.text_1)
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
                    .rounded_tl(px(radius))
                    .rounded_bl(px(radius))
                    .bg(colors.sidebar)
                    .children(nav_header)
                    .children(heading)
                    .children(
                        search.map(|field| div().flex_none().mb(px(METRICS.space_2)).child(field)),
                    )
                    .child(
                        div()
                            .id("settings-nav-rows")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .gap(px(NAV_ROW_GAP))
                            .overflow_y_scroll()
                            .children(back)
                            .children(rows)
                            .children(empty),
                    )
                    .children(footer),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    // Oleafly's page sits on the nav's muted fill; its setting cards stand
                    // out on it.
                    .rounded_tr(px(radius))
                    .rounded_br(px(radius))
                    .bg(colors.sidebar)
                    .border_l(px(METRICS.size.divider))
                    .border_color(colors.line)
                    .when(title.is_some() || header_end.is_some(), |page| {
                        let has_end = header_end.is_some();
                        page.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .h(px(CONTENT_HEADER_H))
                                .pl(px(PAGE_PAD))
                                .when(!has_end, |header| header.pr(px(PAGE_PAD)))
                                .border_b(px(METRICS.size.divider))
                                .border_color(colors.line)
                                .child(
                                    text_style(div(), TextStyle::BodyStrong)
                                        .flex()
                                        .items_center()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .text_color(colors.text_1)
                                        .children(
                                            title.map(|title| {
                                                div().min_w_0().truncate().child(title)
                                            }),
                                        ),
                                )
                                .children(header_end.map(|end| {
                                    div()
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .h_full()
                                        .pr(px(PAGE_PAD / 2.))
                                        .child(end)
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("settings-page")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .when_some(scroll, |page, handle| {
                                page.track_scroll(&handle)
                                    .on_action(reveal_focused(handle, PAGE_PAD))
                            })
                            .p(px(PAGE_PAD))
                            .gap(px(PAGE_PAD))
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

    /// A settings page far taller than the window, its rows inside `focus`.
    struct LongPage {
        focus: gpui::FocusHandle,
        scroll: ScrollHandle,
    }

    impl Render for LongPage {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let rows = (0..100usize).map(|i| {
                crate::kit::Button::new(&theme, ("row", i))
                    .label(format!("Row {i}"))
                    .into_any_element()
            });
            div().size_full().track_focus(&self.focus).child(
                SettingsFrame::new(&theme, "frame", Vec::new())
                    .scroll(&self.scroll)
                    .children(rows),
            )
        }
    }

    #[gpui::test]
    fn a_control_tab_reaches_below_the_fold_scrolls_into_view(cx: &mut TestAppContext) {
        let (page, cx) = cx.add_window_view(|_, cx| LongPage {
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        });
        let (focus, scroll) =
            page.read_with(cx, |page, _| (page.focus.clone(), page.scroll.clone()));
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.run_until_parked();
        assert_eq!(scroll.offset().y, px(0.));
        for _ in 0..100 {
            cx.update(|window, cx| crate::kit::focus_within(&focus, false, window, cx));
            cx.run_until_parked();
        }
        // The last row is in view: the page scrolled to its end.
        assert!(scroll.max_offset().y > px(0.), "the page scrolls");
        assert_eq!(scroll.offset().y, -scroll.max_offset().y);
    }

    #[test]
    fn a_revealed_item_scrolls_into_view_only_when_it_is_out_of_view() {
        use gpui::{point, size};
        // A 400 px page at the top of the window that scrolls 1000 px; a 100 px item.
        let view = Bounds::new(point(px(0.), px(0.)), size(px(600.), px(400.)));
        let item = |top: f32| Bounds::new(point(px(0.), px(top)), size(px(600.), px(100.)));
        let margin = px(20.);
        let max = px(1000.);
        // In view: the page stays where it is.
        assert_eq!(
            reveal_offset(view, item(100.), px(-50.), max, margin),
            px(-50.)
        );
        // Below the view (painted at 700 while scrolled 50): its top comes 20 px under the
        // view's top, so it is 750 into the page.
        assert_eq!(
            reveal_offset(view, item(700.), px(-50.), max, margin),
            px(-730.)
        );
        // Above the view: scrolled back up, but never past the page's top.
        assert_eq!(
            reveal_offset(view, item(-40.), px(-50.), max, margin),
            px(0.)
        );
        // Near the page's end: no further than it scrolls.
        assert_eq!(
            reveal_offset(view, item(1350.), px(0.), max, margin),
            px(-1000.)
        );
    }
}
