//! `ListRow` and `List`: the one row every list in the studio is built from (files, recent
//! projects, slides, presets, overrides, history, the queue).
//!
//! A row is a single line `list_row_h` high, or two lines `list_row_2_h` high when it has a
//! [`ListRow::meta`] line. Its slots, left to right: `leading` (an icon or any element), the
//! title (and meta under it), `trailing` (a badge, a status dot) and `actions`, which are laid
//! out all the time (so the row does not reflow) and shown while the pointer is over the row,
//! the row is selected or keyboard focus is inside them.
//!
//! A row is a tab stop when it has an [`ListRow::on_click`]; Enter and Space activate it.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::icon::{Icon, icon};
use super::interact::{Handler, focusable, on_activate, reveal_actions, with_tooltip};

/// The space between the rows of a [`List`].
const LIST_GAP: f32 = 2.;

/// How a row looks in its state: its fill, the title's colour, and whether actions show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RowLook {
    pub fill: Option<Hsla>,
    pub hover: Option<Hsla>,
    pub title: Hsla,
    pub actions_always: bool,
    pub opacity: f32,
}

/// The look of a row: selected rows are `ctl_active`, others turn `ctl_hover` under the
/// pointer; a disabled row has no hover and is dimmed; a muted row's title is `text_3`.
pub(crate) fn row_look(theme: &Theme, selected: bool, disabled: bool, muted: bool) -> RowLook {
    let colors = theme.colors;
    RowLook {
        fill: selected.then(|| colors.ctl_active.into()),
        hover: (!selected && !disabled).then(|| colors.ctl_hover.into()),
        title: if muted { colors.text_3 } else { colors.text_1 }.into(),
        actions_always: selected && !disabled,
        opacity: if disabled {
            METRICS.opacity.disabled
        } else {
            1.
        },
    }
}

#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    theme: Theme,
    leading: Option<AnyElement>,
    icon: Option<Icon>,
    title: Option<SharedString>,
    meta: Option<SharedString>,
    trailing: Option<AnyElement>,
    actions: Vec<AnyElement>,
    selected: bool,
    disabled: bool,
    muted: bool,
    tooltip: Option<SharedString>,
    label: Option<SharedString>,
    on_click: Option<Handler>,
}

impl ListRow {
    pub fn new(theme: &Theme, id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            leading: None,
            icon: None,
            title: None,
            meta: None,
            trailing: None,
            actions: Vec::new(),
            selected: false,
            disabled: false,
            muted: false,
            tooltip: None,
            label: None,
            on_click: None,
        }
    }

    /// Shown on hover after the standard delay: the full name of a truncated title.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// An element before the title: a thumbnail, a status glyph.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading = Some(element.into_any_element());
        self
    }

    /// A 16 px icon before the title, in `text_2` (`text_3` when muted).
    pub fn icon(mut self, glyph: Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// The row's name: `Label` in `text_1`, or `text_3` when [`ListRow::muted`].
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A second line under the title (`Small`, `text_3`); makes the row `list_row_2_h` high.
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// An element at the end of the row that always shows: a badge, a status dot.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    /// A control shown while the row is hovered or selected. Its space is reserved, so the
    /// row does not reflow. Clicking it does not activate the row.
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A row that shows a default or inherited value: the title in `text_3`.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Runs on click, Enter and Space; makes the row a tab stop.
    /// The row's accessible name, for a screen reader to read in place of its parts.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ListRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ListRow {
            id,
            theme,
            leading,
            icon: glyph,
            title,
            meta,
            trailing,
            actions,
            selected,
            disabled,
            muted,
            tooltip,
            label,
            on_click,
        } = self;
        let colors = theme.colors;
        let look = row_look(&theme, selected, disabled, muted);
        let group = SharedString::from(format!("list-row-{id}"));
        let height = if meta.is_some() {
            METRICS.size.list_row_2_h
        } else {
            METRICS.size.list_row_h
        };
        let interactive = on_click.is_some();
        let icon_color = if muted { colors.text_3 } else { colors.text_2 };
        let leading = leading.or_else(|| {
            glyph.map(|glyph| icon(glyph, METRICS.size.icon_md, icon_color).into_any_element())
        });

        let row = div().id(id);
        let row = if interactive {
            focusable(row, &theme, disabled)
        } else {
            row
        };
        let row = on_activate(row, disabled, on_click);
        let row = with_tooltip(row, &theme, tooltip);

        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .children(title.map(|title| {
                text_style(div(), TextStyle::Label)
                    .text_color(look.title)
                    .truncate()
                    .child(title)
            }))
            .children(meta.map(|meta| {
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_3)
                    .truncate()
                    .child(meta)
            }));

        let actions = (!actions.is_empty()).then(|| {
            div()
                .id(SharedString::from(format!("{group}-actions")))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(METRICS.space_1))
                // An action's click is the action's, not the row's.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when(!look.actions_always, |this| {
                    if disabled {
                        this.opacity(0.)
                    } else {
                        reveal_actions(this, group.clone())
                    }
                })
                .children(actions)
        });

        row.group(group.clone())
            .role(Role::ListItem)
            .aria_selected(selected)
            .when_some(label, |this, label| this.aria_label(label))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_2))
            .h(px(height))
            .w_full()
            .px(px(METRICS.space_2))
            .rounded(px(METRICS.radius.item))
            .when_some(look.fill, |this, fill| this.bg(fill))
            .when_some(look.hover, |this, hover| {
                this.hover(move |style| style.bg(hover))
            })
            .when(interactive && !disabled, |this| this.cursor_pointer())
            .opacity(look.opacity)
            .children(leading.map(|leading| div().flex().flex_none().items_center().child(leading)))
            .child(text)
            .children(
                trailing.map(|trailing| div().flex().flex_none().items_center().child(trailing)),
            )
            .children(actions)
    }
}

/// A column of [`ListRow`]s, 2 px apart, or the [`List::empty`] element when it has none.
#[derive(IntoElement, Default)]
pub struct List {
    rows: Vec<AnyElement>,
    empty: Option<AnyElement>,
}

impl List {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn child(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }

    pub fn children(mut self, rows: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.rows
            .extend(rows.into_iter().map(IntoElement::into_any_element));
        self
    }

    /// Shown instead of the rows when there are none, usually an inline `EmptyState`.
    pub fn empty(mut self, element: impl IntoElement) -> Self {
        self.empty = Some(element.into_any_element());
        self
    }
}

impl RenderOnce for List {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let list = div().flex().flex_col().w_full();
        if self.rows.is_empty() {
            list.children(self.empty)
        } else {
            list.gap(px(LIST_GAP)).children(self.rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};
    use studio_theme::Appearance;

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    #[test]
    fn selection_hover_and_muting_pick_their_roles() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let theme = Theme::new(appearance);
            let c = theme.colors;
            let plain = row_look(&theme, false, false, false);
            assert_eq!(plain.fill, None);
            assert_eq!(plain.hover, Some(c.ctl_hover.into()));
            assert_eq!(plain.title, Hsla::from(c.text_1));
            assert!(!plain.actions_always);
            assert_eq!(plain.opacity, 1.);

            let selected = row_look(&theme, true, false, false);
            assert_eq!(selected.fill, Some(c.ctl_active.into()));
            assert_eq!(selected.hover, None);
            assert!(selected.actions_always);

            let disabled = row_look(&theme, false, true, false);
            assert_eq!(disabled.hover, None);
            assert_eq!(disabled.opacity, METRICS.opacity.disabled);

            assert_eq!(
                row_look(&theme, false, false, true).title,
                Hsla::from(c.text_3)
            );
        }
    }

    /// Three rows: a disabled one, a plain one and a two-line selected one.
    struct Rows {
        hits: Log<&'static str>,
        selected: usize,
    }

    impl Render for Rows {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let entity = cx.entity();
            let row = |id: &'static str, index: usize| {
                let hits = self.hits.clone();
                let entity = entity.clone();
                ListRow::new(&theme, id)
                    .title(id)
                    .selected(self.selected == index)
                    .on_click(move |_, cx| {
                        hits.borrow_mut().push(id);
                        entity.update(cx, |rows, cx| {
                            rows.selected = index;
                            cx.notify();
                        });
                    })
            };
            let first = row("off", 0).disabled(true);
            let second = row("one", 1);
            let third = row("two", 2).meta("2 lines");
            div().w(px(240.)).child(
                List::new()
                    .child(div().debug_selector(|| "off".into()).child(first))
                    .child(div().debug_selector(|| "one".into()).child(second))
                    .child(div().debug_selector(|| "two".into()).child(third)),
            )
        }
    }

    #[gpui::test]
    fn rows_activate_from_the_keyboard_and_skip_disabled_ones(cx: &mut TestAppContext) {
        let hits = log();
        let shared = hits.clone();
        let (view, cx) = cx.add_window_view(|_, _| Rows {
            hits: shared,
            selected: 2,
        });
        tab(cx);
        press(cx, "enter");
        tab(cx);
        press(cx, "space");
        assert_eq!(*hits.borrow(), ["one", "two"]);
        assert_eq!(view.read_with(cx, |rows, _| rows.selected), 2);
        tab(cx);
        press(cx, "enter");
        assert_eq!(view.read_with(cx, |rows, _| rows.selected), 1);
    }

    #[gpui::test]
    fn rows_are_one_or_two_lines_high_and_two_apart(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Rows {
            hits: log(),
            selected: 0,
        });
        cx.run_until_parked();
        let one = cx.debug_bounds("one").expect("row");
        let two = cx.debug_bounds("two").expect("row");
        assert_eq!(f32::from(one.size.height), METRICS.size.list_row_h);
        assert_eq!(f32::from(two.size.height), METRICS.size.list_row_2_h);
        assert_eq!(f32::from(two.top() - one.bottom()), LIST_GAP);
    }

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            List::new().empty(div().debug_selector(|| "empty".into()).h(px(10.)))
        }
    }

    #[gpui::test]
    fn an_empty_list_shows_its_empty_state(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Empty);
        cx.run_until_parked();
        assert!(cx.debug_bounds("empty").is_some());
    }
}
