//! The dropdown/select menu, with the Kobalte behaviour contract the Solid
//! app gets for free from `KSelect` / `KDropdownMenu`:
//!
//! - opens at an anchor, dismisses on click-away **and** on Escape,
//! - Arrow Down / Arrow Up move a highlight, wrapping at both ends,
//! - Home / End jump to the ends,
//! - Enter (or Space) commits the highlighted item,
//! - the currently-selected item carries a check mark,
//! - disabled rows are shown dimmed and every key steps over them.
//!
//! The state machine is a plain struct so it can be tested without a window;
//! [`Menu`] is the element that draws it.

use std::{cell::Cell, rc::Rc};

use gpui::{
    Anchor, App, Bounds, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, Point, RenderOnce, Role, SharedString, Size,
    StatefulInteractiveElement, Styled, Window, div, point, prelude::FluentBuilder, px, svg,
};

use studio_theme::{METRICS, Shadow, TextStyle, Theme, shadow_for, text_style};

use super::interact::{States, dim_disabled};

/// A menu's least width and greatest height (Oleafly's dropdown `min-w-40` and select
/// `max-h-96`).
const MIN_W: f32 = 160.;
const MAX_H: f32 = 384.;
/// A row: its height, the check column, the row radius and its padding and gap (Oleafly's
/// `px-2 gap-2 rounded-md` menu row).
const ROW_H: f32 = 32.;
const CHECK: f32 = 12.;
const ROW_RADIUS: f32 = METRICS.radius.item;
const ROW_PAD_X: f32 = METRICS.space_2;
const ROW_GAP: f32 = METRICS.space_2;
/// The gap kept between a menu, its trigger and the window edge.
const GAP: f32 = 4.;
const MARGIN: f32 = 12.;

pub(crate) type OpenHandler = Box<dyn Fn(&Bounds<Pixels>, &mut Window, &mut App) + 'static>;

/// One row: a label, whether it is the value currently in force, and whether
/// it can be chosen at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: SharedString,
    pub checked: bool,
    pub disabled: bool,
    /// The row's element id, for automation that addresses rows by name.
    /// Without one the row is `{menu id}-item-{index}`.
    pub id: Option<SharedString>,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, checked: bool) -> Self {
        Self {
            label: label.into(),
            checked,
            disabled: false,
            id: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// What a keystroke did to an open menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// The highlight moved; repaint.
    Moved,
    /// Commit this index and close.
    Commit(usize),
    /// Close without committing.
    Dismiss,
    /// Not ours -- let it through.
    Ignored,
}

/// An open menu: where it is anchored, how many rows it has, and which one the
/// keyboard is on.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    pub origin: Point<Pixels>,
    pub trigger_bounds: Option<Bounds<Pixels>>,
    pub len: usize,
    pub highlighted: Option<usize>,
    /// Whether the highlight is *drawn*. A menu opened by pointer shows the
    /// check mark and follows the mouse, exactly as a real `NSMenu` does; the
    /// keyboard highlight appears the moment an arrow key is used. Enter
    /// straight after opening still commits the current value, because
    /// `highlighted` is seeded either way.
    pub highlight_visible: bool,
    /// Rows that are shown but cannot be highlighted or committed.
    pub disabled: Vec<bool>,
}

impl MenuState {
    /// Kobalte opens a `Select` with the *selected* item highlighted, so the
    /// first Arrow Down steps off the current value rather than jumping to the
    /// top of the list.
    pub fn new(origin: Point<Pixels>, items: &[MenuItem]) -> Self {
        let mut state = Self {
            origin,
            trigger_bounds: None,
            len: items.len(),
            highlighted: items.iter().position(|item| item.checked),
            highlight_visible: false,
            disabled: items.iter().map(|item| item.disabled).collect(),
        };
        state.leave_disabled_row();
        state
    }

    pub fn anchored(trigger_bounds: Bounds<Pixels>, items: &[MenuItem]) -> Self {
        Self {
            trigger_bounds: Some(trigger_bounds),
            ..Self::new(trigger_bounds.bottom_left(), items)
        }
    }

    /// The index to paint highlighted, if any.
    pub fn visible_highlight(&self) -> Option<usize> {
        self.highlighted.filter(|_| self.highlight_visible)
    }

    pub fn is_disabled(&self, index: usize) -> bool {
        self.disabled.get(index).copied().unwrap_or(false)
    }

    /// Availability that changes while the menu is open. Missing entries are
    /// enabled; a highlight left on a disabled row moves to the next enabled
    /// one.
    pub fn set_disabled(&mut self, disabled: Vec<bool>) {
        self.disabled = (0..self.len)
            .map(|index| disabled.get(index).copied().unwrap_or(false))
            .collect();
        self.leave_disabled_row();
    }

    /// Shows the highlight on `index` -- what a [`Menu::on_highlight`] handler
    /// applies. Out-of-range and disabled rows are refused.
    pub fn set_highlighted(&mut self, index: usize) {
        if index < self.len && !self.is_disabled(index) {
            self.highlighted = Some(index);
            self.highlight_visible = true;
        }
    }

    fn leave_disabled_row(&mut self) {
        if let Some(index) = self.highlighted
            && self.is_disabled(index)
        {
            self.highlighted = self.step_enabled(index, 1);
        }
    }

    /// The enabled row `delta` (1 or -1) steps from `from`, wrapping. `None`
    /// when every row is disabled.
    fn step_enabled(&self, from: usize, delta: isize) -> Option<usize> {
        let len = self.len as isize;
        (1..=len)
            .map(|n| (from as isize + delta * n).rem_euclid(len) as usize)
            .find(|index| !self.is_disabled(*index))
    }

    pub fn on_key(&mut self, key: &str) -> MenuKey {
        if self.len == 0 {
            return match key {
                "escape" => MenuKey::Dismiss,
                _ => MenuKey::Ignored,
            };
        }
        match key {
            "down" | "up" | "home" | "end" => match self.move_target(key) {
                Some(index) => {
                    self.highlighted = Some(index);
                    self.highlight_visible = true;
                    MenuKey::Moved
                }
                None => MenuKey::Ignored,
            },
            "enter" | "space" => match self.highlighted.filter(|index| !self.is_disabled(*index)) {
                Some(index) => MenuKey::Commit(index),
                None => MenuKey::Ignored,
            },
            "escape" => MenuKey::Dismiss,
            _ => MenuKey::Ignored,
        }
    }

    /// Down and Up wrap, Home and End take the first and last enabled row;
    /// all of them step over disabled rows.
    fn move_target(&self, key: &str) -> Option<usize> {
        let last = self.len - 1;
        match key {
            "down" => self.step_enabled(self.highlighted.unwrap_or(last), 1),
            "up" => self.step_enabled(self.highlighted.unwrap_or(0), -1),
            "home" => self.step_enabled(last, 1),
            _ => self.step_enabled(0, -1),
        }
    }
}

/// As with [`crate::kit::SegmentedControl`], the index arrives by reference so
/// `cx.listener` can build the handler directly.
type SelectHandler = Box<dyn Fn(&usize, &mut Window, &mut App) + 'static>;
type DismissHandler = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Menu {
    id: ElementId,
    items: Vec<MenuItem>,
    origin: Point<Pixels>,
    trigger_bounds: Option<Bounds<Pixels>>,
    state: MenuState,
    focus: Option<FocusHandle>,
    min_width: Pixels,
    max_height: Pixels,
    radius: Pixels,
    shadow: Vec<gpui::BoxShadow>,
    bg: Hsla,
    border: Hsla,
    hover: Hsla,
    text: Hsla,
    on_highlight: Option<SelectHandler>,
    on_select: Option<SelectHandler>,
    on_dismiss: Option<DismissHandler>,
}

impl Menu {
    /// A menu panel: `popover` behind a `line` hairline with the popover shadow, rows that
    /// highlight on the quiet `ctl` fill (Oleafly's `focus:bg-accent`).
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        items: Vec<MenuItem>,
        state: &MenuState,
    ) -> Self {
        let colors = theme.colors;
        Self {
            id: id.into(),
            items,
            origin: state.origin,
            trigger_bounds: state.trigger_bounds,
            state: state.clone(),
            focus: None,
            min_width: px(MIN_W),
            max_height: px(MAX_H),
            radius: px(METRICS.radius.popover),
            shadow: shadow_for(theme.appearance, Shadow::Pop).into_vec(),
            bg: colors.popover.into(),
            border: colors.line.into(),
            hover: States::of(theme).rest,
            text: colors.text_1.into(),
            on_highlight: None,
            on_select: None,
            on_dismiss: None,
        }
    }
    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = width;
        self
    }

    pub fn trigger(
        element: gpui::Stateful<gpui::Div>,
        handler: impl Fn(&Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let bounds = Rc::new(Cell::new(None));
        let measured_bounds = bounds.clone();
        element
            .tab_index(0)
            .relative()
            .on_click(move |_, window, cx| {
                if let Some(bounds) = bounds.get() {
                    handler(&bounds, window, cx);
                }
            })
            .child(
                gpui::canvas(
                    move |bounds, _, _| measured_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }

    /// Click-away, and Escape while the menu has focus.
    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Routes the arrow, Home, End, Enter, Space and Escape keys to this menu
    /// while `focus` is focused. The caller focuses it when the menu opens.
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// The row an arrow, Home or End key moved to; apply it with
    /// [`MenuState::set_highlighted`].
    pub fn on_highlight(
        mut self,
        handler: impl Fn(&usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight = Some(Box::new(handler));
        self
    }
}

fn anchored_placement(
    trigger: Bounds<Pixels>,
    item_count: usize,
    max_height: Pixels,
    viewport: Size<Pixels>,
) -> (Point<Pixels>, Anchor, Pixels) {
    let gap = px(GAP);
    let margin = px(MARGIN);
    let below = (viewport.height - margin - trigger.bottom() - gap).max(px(0.));
    let above = (trigger.top() - gap - margin).max(px(0.));
    let height = (px(item_count as f32 * ROW_H + 10.)).min(max_height);
    if height <= below || below >= above {
        (
            trigger.bottom_left() + point(px(0.), gap),
            Anchor::TopLeft,
            max_height.min(below),
        )
    } else {
        (
            trigger.origin - point(px(0.), gap),
            Anchor::BottomLeft,
            max_height.min(above),
        )
    }
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Menu {
            id,
            items,
            origin,
            trigger_bounds,
            state,
            focus,
            min_width,
            max_height,
            radius,
            shadow,
            bg,
            border,
            hover,
            text,
            on_highlight,
            on_select,
            on_dismiss,
        } = self;
        let highlighted = state.visible_highlight();

        let prefix: SharedString = match &id {
            ElementId::Name(name) => name.clone(),
            other => SharedString::from(format!("{other:?}")),
        };
        let handler: Option<Rc<SelectHandler>> = on_select.map(Rc::new);
        let disabled_rows = state.disabled.clone();
        let keys = {
            let on_select = handler.clone();
            let on_dismiss = on_dismiss.clone();
            move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                let mut next = state.clone();
                match next.on_key(event.keystroke.key.as_str()) {
                    MenuKey::Moved => {
                        if let (Some(handler), Some(index)) = (&on_highlight, next.highlighted) {
                            handler(&index, window, cx);
                        }
                    }
                    MenuKey::Commit(index) => {
                        if let Some(handler) = &on_select {
                            handler(&index, window, cx);
                        }
                    }
                    MenuKey::Dismiss => {
                        if let Some(handler) = &on_dismiss {
                            handler(window, cx);
                        }
                    }
                    MenuKey::Ignored => return,
                }
                cx.stop_propagation();
            }
        };
        let viewport = window.viewport_size();
        let max_width = (viewport.width - px(2. * MARGIN)).max(px(0.));
        let max_height = max_height.min((viewport.height - px(2. * MARGIN)).max(px(0.)));
        let min_width = trigger_bounds.map_or(min_width, |bounds| min_width.max(bounds.size.width));
        let (origin, anchor, max_height) = trigger_bounds
            .map_or((origin, Anchor::TopLeft, max_height), |bounds| {
                anchored_placement(bounds, items.len(), max_height, viewport)
            });

        // Deferred and placed in window coordinates, so a menu hosted inside a
        // clipped card still paints above everything and its click-away layer
        // still covers the whole window.
        let layer = div()
            .relative()
            .w(viewport.width)
            .h(viewport.height)
            .child(
                // Click-away dismiss, the way a native menu closes.
                div()
                    .id(SharedString::from(format!("{prefix}-backdrop")))
                    .occlude()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .when_some(on_dismiss, |this, handler| {
                        this.on_click(move |_, window, cx| handler(window, cx))
                    }),
            )
            .child(
                gpui::anchored()
                    .position(origin)
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(MARGIN))
                    .child(
                        div()
                            .id(id)
                            .role(Role::Menu)
                            .occlude()
                            .when_some(focus, |this, focus| this.track_focus(&focus))
                            .on_key_down(keys)
                            .flex()
                            .flex_col()
                            .min_w(min_width.min(max_width))
                            .max_w(max_width)
                            .max_h(max_height)
                            .overflow_y_scroll()
                            .p(px(METRICS.space_1))
                            .rounded(radius)
                            .shadow(shadow)
                            .border(px(METRICS.size.divider))
                            .border_color(border)
                            .bg(bg)
                            .text_color(text)
                            .map(|this| text_style(this, TextStyle::Body))
                            .children(items.into_iter().enumerate().map(|(index, item)| {
                                let handler = handler.clone();
                                let disabled = disabled_rows.get(index).copied().unwrap_or(false);
                                let row = div()
                                    .id(item.id.clone().unwrap_or_else(|| {
                                        SharedString::from(format!("{prefix}-item-{index}"))
                                    }))
                                    .role(Role::MenuItem)
                                    .aria_label(item.label.clone())
                                    .flex()
                                    .flex_row()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap(px(ROW_GAP))
                                    .h(px(ROW_H))
                                    .px(px(ROW_PAD_X))
                                    .rounded(px(ROW_RADIUS));
                                dim_disabled(row, disabled)
                                    .when(!disabled && highlighted == Some(index), |this| {
                                        this.bg(hover)
                                    })
                                    .when(!disabled, |this| {
                                        this.hover(move |style| style.bg(hover))
                                    })
                                    .child(div().w(px(CHECK)).flex_shrink_0().children(
                                        item.checked.then(|| {
                                            svg()
                                                .path("icons/check.svg")
                                                .size(px(CHECK))
                                                .text_color(text)
                                        }),
                                    ))
                                    .child(div().flex_1().min_w_0().truncate().child(item.label))
                                    .when_some(handler.filter(|_| !disabled), |this, handler| {
                                        this.on_click(move |_, window, cx| {
                                            handler(&index, window, cx)
                                        })
                                    })
                            })),
                    ),
            );
        gpui::deferred(
            gpui::anchored()
                .position_mode(gpui::AnchoredPositionMode::Window)
                .position(point(px(0.), px(0.)))
                .child(layer),
        )
        .with_priority(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::test_support::{Log, log, press};
    use gpui::{Context, FocusHandle, Render, TestAppContext, point, size};

    /// A menu rendered inside a small clipped box, the way a card hosts it.
    struct Clipped {
        dismissed: Log<()>,
        picked: Log<usize>,
    }

    impl Render for Clipped {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let items = vec![MenuItem::new("One", false), MenuItem::new("Two", false)];
            let state = MenuState::new(point(px(40.), px(200.)), &items);
            let (dismissed, picked) = (self.dismissed.clone(), self.picked.clone());
            div().size_full().child(
                div()
                    .relative()
                    .w(px(100.))
                    .h(px(30.))
                    .overflow_hidden()
                    .child(
                        Menu::new(&theme, "clipped", items, &state)
                            .on_dismiss(move |_, _| dismissed.borrow_mut().push(()))
                            .on_select(move |index, _, _| picked.borrow_mut().push(*index)),
                    ),
            )
        }
    }

    #[gpui::test]
    fn a_menu_inside_a_clipped_box_still_covers_the_window(cx: &mut TestAppContext) {
        let (dismissed, picked) = (log(), log());
        let shared = (dismissed.clone(), picked.clone());
        let (_, cx) = cx.add_window_view(|_, _| Clipped {
            dismissed: shared.0,
            picked: shared.1,
        });
        cx.run_until_parked();
        // The first row, well outside the 100x30 host.
        cx.simulate_click(point(px(60.), px(217.)), gpui::Modifiers::none());
        assert_eq!(*picked.borrow(), [0]);
        cx.simulate_click(point(px(600.), px(500.)), gpui::Modifiers::none());
        assert_eq!(dismissed.borrow().len(), 1);
    }

    fn with_disabled(flags: &[bool], highlighted: Option<usize>) -> MenuState {
        let items: Vec<MenuItem> = flags
            .iter()
            .enumerate()
            .map(|(index, disabled)| {
                MenuItem::new(format!("item {index}"), false).disabled(*disabled)
            })
            .collect();
        let mut menu = MenuState::new(point(px(0.), px(0.)), &items);
        menu.highlighted = highlighted;
        menu
    }

    #[test]
    fn disabled_rows_are_skipped_by_every_key() {
        let flags = [true, false, true, false, true];
        let after = |highlighted: usize, key: &str| {
            let mut menu = with_disabled(&flags, Some(highlighted));
            menu.on_key(key);
            menu.highlighted
        };
        assert_eq!(after(1, "down"), Some(3));
        assert_eq!(after(3, "down"), Some(1));
        assert_eq!(after(1, "up"), Some(3));
        assert_eq!(after(3, "home"), Some(1));
        assert_eq!(after(1, "end"), Some(3));
    }

    #[test]
    fn availability_can_change_while_the_menu_is_open() {
        let mut menu = with_disabled(&[false; 3], Some(0));
        assert_eq!(menu.on_key("enter"), MenuKey::Commit(0));
        menu.set_disabled(vec![true, false, false]);
        assert!(menu.is_disabled(0));
        assert_eq!(menu.highlighted, Some(1));
        menu.set_disabled(vec![]);
        assert!(!menu.is_disabled(0));
    }

    #[test]
    fn disabled_rows_cannot_be_highlighted_or_committed() {
        let mut menu = with_disabled(&[true, false, true], Some(1));
        menu.set_highlighted(2);
        assert_eq!(menu.highlighted, Some(1));
        menu.highlighted = Some(0);
        assert_eq!(menu.on_key("enter"), MenuKey::Ignored);
        menu.highlighted = Some(1);
        assert_eq!(menu.on_key("enter"), MenuKey::Commit(1));
    }

    #[test]
    fn a_menu_of_only_disabled_rows_ignores_navigation_and_commit() {
        let mut menu = with_disabled(&[true, true], None);
        for key in ["down", "up", "home", "end", "enter"] {
            assert_eq!(menu.on_key(key), MenuKey::Ignored, "{key}");
        }
        assert_eq!(menu.on_key("escape"), MenuKey::Dismiss);
    }

    #[test]
    fn a_menu_opened_on_a_disabled_value_highlights_the_next_enabled_row() {
        let items = [
            MenuItem::new("a", false),
            MenuItem::new("b", true).disabled(true),
            MenuItem::new("c", false),
        ];
        assert_eq!(
            MenuState::new(point(px(0.), px(0.)), &items).highlighted,
            Some(2)
        );
    }

    #[test]
    fn a_select_menu_stays_below_when_neither_side_fits_better() {
        let bounds = Bounds::new(point(px(100.), px(20.)), size(px(160.), px(30.)));
        let (_, anchor, _) = anchored_placement(bounds, 40, px(320.), size(px(800.), px(300.)));
        assert_eq!(anchor, Anchor::TopLeft);
    }

    struct Popup {
        state: MenuState,
        focus: FocusHandle,
        events: Log<String>,
    }

    fn rows() -> Vec<MenuItem> {
        vec![
            MenuItem::new("Copy", true).id("copy"),
            MenuItem::new("Paste", false).disabled(true),
            MenuItem::new("Delete", false),
        ]
    }

    impl Render for Popup {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let (highlight, select, dismiss) = (
                self.events.clone(),
                self.events.clone(),
                self.events.clone(),
            );
            Menu::new(&theme, "popup", rows(), &self.state)
                .focus(&self.focus)
                .on_highlight(cx.listener(move |this, index: &usize, _, cx| {
                    highlight.borrow_mut().push(format!("highlight {index}"));
                    this.state.set_highlighted(*index);
                    cx.notify();
                }))
                .on_select(move |index, _, _| select.borrow_mut().push(format!("select {index}")))
                .on_dismiss(move |_, _| dismiss.borrow_mut().push("dismiss".into()))
        }
    }

    #[gpui::test]
    fn a_focused_menu_routes_its_keys(cx: &mut TestAppContext) {
        let events = log();
        let shared = events.clone();
        let (view, cx) = cx.add_window_view(|_, cx| Popup {
            state: MenuState::new(point(px(0.), px(0.)), &rows()),
            focus: cx.focus_handle(),
            events: shared,
        });
        cx.update(|window, cx| {
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
        });
        press(cx, "down");
        press(cx, "down");
        press(cx, "enter");
        press(cx, "escape");
        assert_eq!(
            *events.borrow(),
            ["highlight 2", "highlight 0", "select 0", "dismiss"]
        );
        assert_eq!(
            view.read_with(cx, |popup, _| popup.state.visible_highlight()),
            Some(0)
        );
    }

    fn items(checked: Option<usize>) -> Vec<MenuItem> {
        (0..4)
            .map(|index| MenuItem::new(format!("item {index}"), checked == Some(index)))
            .collect()
    }

    fn state(checked: Option<usize>) -> MenuState {
        MenuState::new(point(px(0.), px(0.)), &items(checked))
    }

    #[test]
    fn a_select_menu_uses_the_trigger_bounds_and_current_value() {
        let bounds = Bounds::new(point(px(100.), px(80.)), gpui::size(px(160.), px(36.)));
        let menu = MenuState::anchored(bounds, &items(Some(2)));
        assert_eq!(menu.trigger_bounds, Some(bounds));
        assert_eq!(menu.origin, bounds.bottom_left());
        assert_eq!(menu.highlighted, Some(2));
    }

    #[test]
    fn a_select_menu_opens_below_the_button_when_it_fits() {
        let bounds = Bounds::new(point(px(100.), px(80.)), gpui::size(px(160.), px(36.)));
        let (origin, anchor, height) =
            anchored_placement(bounds, 4, px(320.), gpui::size(px(800.), px(600.)));
        assert_eq!(origin, point(px(100.), px(120.)));
        assert_eq!(anchor, Anchor::TopLeft);
        assert_eq!(height, px(320.));
    }

    #[test]
    fn a_select_menu_flips_above_a_button_near_the_bottom() {
        let bounds = Bounds::new(point(px(100.), px(540.)), gpui::size(px(160.), px(36.)));
        let (origin, anchor, height) =
            anchored_placement(bounds, 4, px(320.), gpui::size(px(800.), px(600.)));
        assert_eq!(origin, point(px(100.), px(536.)));
        assert_eq!(anchor, Anchor::BottomLeft);
        assert_eq!(height, px(320.));
    }

    #[test]
    fn a_long_select_menu_is_limited_to_the_larger_side_of_the_button() {
        let bounds = Bounds::new(point(px(100.), px(220.)), gpui::size(px(160.), px(36.)));
        let (origin, anchor, height) =
            anchored_placement(bounds, 40, px(320.), gpui::size(px(800.), px(420.)));
        assert_eq!(origin, point(px(100.), px(216.)));
        assert_eq!(anchor, Anchor::BottomLeft);
        assert_eq!(height, px(204.));
    }

    #[test]
    fn a_menu_opens_on_the_value_it_currently_holds() {
        assert_eq!(state(Some(2)).highlighted, Some(2));
        assert_eq!(state(None).highlighted, None);
    }

    /// A menu opened by pointer paints no highlight -- the check mark is what
    /// marks the current value, as in a real `NSMenu`. The first arrow key
    /// turns the highlight on.
    #[test]
    fn the_highlight_is_not_painted_until_the_keyboard_is_used() {
        let mut menu = state(Some(2));
        assert_eq!(menu.visible_highlight(), None);
        menu.on_key("down");
        assert_eq!(menu.visible_highlight(), Some(3));
    }

    /// ...but Enter straight after opening still commits the current value,
    /// because the seed is there whether or not it is drawn.
    #[test]
    fn enter_before_any_arrow_commits_the_current_value() {
        let mut menu = state(Some(2));
        assert_eq!(menu.on_key("enter"), MenuKey::Commit(2));
    }

    #[test]
    fn arrows_walk_the_list_and_wrap_at_both_ends() {
        let mut menu = state(None);
        assert_eq!(menu.on_key("down"), MenuKey::Moved);
        assert_eq!(menu.highlighted, Some(0));
        for expected in [1, 2, 3] {
            menu.on_key("down");
            assert_eq!(menu.highlighted, Some(expected));
        }
        menu.on_key("down");
        assert_eq!(menu.highlighted, Some(0), "past the end wraps to the top");
        menu.on_key("up");
        assert_eq!(
            menu.highlighted,
            Some(3),
            "before the start wraps to the end"
        );
    }

    #[test]
    fn an_unopened_highlight_takes_the_end_on_arrow_up() {
        let mut menu = state(None);
        assert_eq!(menu.on_key("up"), MenuKey::Moved);
        assert_eq!(menu.highlighted, Some(3));
    }

    #[test]
    fn home_and_end_jump() {
        let mut menu = state(Some(1));
        menu.on_key("end");
        assert_eq!(menu.highlighted, Some(3));
        menu.on_key("home");
        assert_eq!(menu.highlighted, Some(0));
    }

    #[test]
    fn enter_commits_the_highlighted_row_and_escape_dismisses() {
        let mut menu = state(Some(2));
        assert_eq!(menu.on_key("enter"), MenuKey::Commit(2));
        assert_eq!(menu.on_key("escape"), MenuKey::Dismiss);
    }

    #[test]
    fn enter_with_nothing_highlighted_commits_nothing() {
        let mut menu = state(None);
        assert_eq!(menu.on_key("enter"), MenuKey::Ignored);
    }

    #[test]
    fn an_empty_menu_still_dismisses_but_never_commits() {
        let mut menu = MenuState::new(point(px(0.), px(0.)), &[]);
        assert_eq!(menu.on_key("down"), MenuKey::Ignored);
        assert_eq!(menu.on_key("enter"), MenuKey::Ignored);
        assert_eq!(menu.on_key("escape"), MenuKey::Dismiss);
    }

    #[test]
    fn keys_the_menu_does_not_own_pass_through() {
        let mut menu = state(Some(0));
        assert_eq!(menu.on_key("a"), MenuKey::Ignored);
        assert_eq!(menu.on_key("tab"), MenuKey::Ignored);
        assert_eq!(menu.highlighted, Some(0));
    }
}
