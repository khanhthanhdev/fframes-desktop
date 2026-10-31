//! `PictureCards`: exclusive choices shown as pictures, Oleafly's appearance cards (its dock
//! placement and background pattern pickers): a row of equal cards, each a picture over an
//! icon and a `Label` name, behind a `line` hairline; the selected card outlined in `accent`
//! on the info wash. [`appearance_picture`] draws the picture of a theme choice.
//!
//! Every card is a tab stop; Enter or Space selects it. Card `i` is `{id}-{i}`.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Div, ElementId, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use studio_theme::{Appearance, METRICS, TextStyle, Theme, text_style};

use super::badge::{Tone, tone_surface};
use super::interact::{States, bounds_probe, focusable};
use super::{Icon, icon};

/// One choice: its name, the icon before the name and its picture.
pub struct PictureCard {
    pub label: SharedString,
    pub icon: Option<Icon>,
    pub picture: AnyElement,
}

impl PictureCard {
    pub fn new(label: impl Into<SharedString>, picture: impl IntoElement) -> Self {
        Self {
            label: label.into(),
            icon: None,
            picture: picture.into_any_element(),
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}

type SelectHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;
type OptionBoundsHandler = Rc<dyn Fn(usize, Bounds<Pixels>, &mut App)>;

#[derive(IntoElement)]
pub struct PictureCards {
    id: ElementId,
    theme: Theme,
    items: Vec<PictureCard>,
    selected: Option<usize>,
    on_select: Option<SelectHandler>,
    on_option_bounds: Option<OptionBoundsHandler>,
}

/// The picture's height inside a card (Oleafly's `h-20`).
const PICTURE_H: f32 = 76.;

impl PictureCards {
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        items: Vec<PictureCard>,
        selected: Option<usize>,
    ) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            items,
            selected,
            on_select: None,
            on_option_bounds: None,
        }
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Reports each card's index and bounds, for controls that are measured.
    pub fn on_option_bounds(
        mut self,
        report: impl Fn(usize, Bounds<Pixels>, &mut App) + 'static,
    ) -> Self {
        self.on_option_bounds = Some(Rc::new(report));
        self
    }
}

impl RenderOnce for PictureCards {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let PictureCards {
            id,
            theme,
            items,
            selected,
            on_select,
            on_option_bounds,
        } = self;
        let colors = theme.colors;
        let states = States::of(&theme);
        let prefix: SharedString = match &id {
            ElementId::Name(name) => name.clone(),
            other => SharedString::from(format!("{other:?}")),
        };
        div()
            .id(id)
            .role(Role::RadioGroup)
            .flex()
            .w_full()
            .gap(px(METRICS.space_3))
            .children(items.into_iter().enumerate().map(|(index, item)| {
                let checked = selected == Some(index);
                let text = if checked {
                    colors.text_1
                } else {
                    colors.text_2
                };
                focusable(
                    div().id(SharedString::from(format!("{prefix}-{index}"))),
                    &theme,
                    false,
                )
                .role(Role::RadioButton)
                .aria_selected(checked)
                .aria_label(item.label.clone())
                .relative()
                .children(on_option_bounds.clone().map(|report| {
                    bounds_probe(Rc::new(move |bounds, cx: &mut App| {
                        report(index, bounds, cx)
                    }))
                }))
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(METRICS.space_2))
                .p(px(METRICS.space_3))
                .rounded(px(METRICS.radius.card))
                .border(px(METRICS.size.divider))
                .border_color(if checked { colors.accent } else { colors.line })
                .bg(colors.card)
                .when(checked, |this| this.bg(tone_surface(&theme, Tone::Info)))
                .when(!checked, |this| {
                    this.cursor_pointer()
                        .hover(move |style| style.bg(states.hover))
                })
                .when_some(on_select.clone(), |this, handler| {
                    this.on_click(move |_, window, cx| handler(&index, window, cx))
                })
                .child(
                    div()
                        .h(px(PICTURE_H))
                        .w_full()
                        .overflow_hidden()
                        .rounded(px(METRICS.radius.item))
                        .child(item.picture),
                )
                .child(
                    text_style(div(), TextStyle::Label)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(METRICS.space_2))
                        .text_color(text)
                        .children(
                            item.icon
                                .map(|glyph| icon(glyph, METRICS.size.icon_sm, text)),
                        )
                        .child(item.label),
                )
            }))
    }
}

/// The picture of a theme choice: a small window, its sidebar and two lines of text and an
/// accent button, drawn in the light or dark palette; `None` (follow the system) is the
/// light window with its right half dark.
pub fn appearance_picture(appearance: Option<Appearance>) -> Div {
    match appearance {
        Some(appearance) => mini_window(&Theme::new(appearance)),
        None => {
            let half = |appearance, right: bool| {
                div().relative().flex_1().h_full().overflow_hidden().child(
                    // The whole window, shifted so this half shows its own side.
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .w(gpui::relative(2.))
                        .when(right, |this| this.right_0())
                        .when(!right, |this| this.left_0())
                        .child(mini_window(&Theme::new(appearance))),
                )
            };
            div()
                .flex()
                .size_full()
                .child(half(Appearance::Light, false))
                .child(half(Appearance::Dark, true))
        }
    }
}

/// The parts of the small window: a sidebar, a title line, a body line, a button.
const MINI_PAD: f32 = 8.;
const MINI_SIDEBAR_W: f32 = 22.;
const MINI_LINE_H: f32 = 5.;
const MINI_BUTTON_W: f32 = 24.;
const MINI_BUTTON_H: f32 = 8.;
const MINI_RADIUS: f32 = 2.;

fn mini_window(theme: &Theme) -> Div {
    let colors = theme.colors;
    let line = |width: f32, color| {
        div()
            .h(px(MINI_LINE_H))
            .w(gpui::relative(width))
            .rounded(px(MINI_RADIUS))
            .bg(color)
    };
    div()
        .size_full()
        .flex()
        .bg(colors.window)
        .border(px(METRICS.size.divider))
        .border_color(colors.line)
        .rounded(px(METRICS.radius.item))
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .w(px(MINI_SIDEBAR_W))
                .h_full()
                .bg(colors.sidebar)
                .border_r(px(METRICS.size.divider))
                .border_color(colors.line),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(MINI_LINE_H))
                .p(px(MINI_PAD))
                .child(line(0.6, colors.text_1))
                .child(line(0.85, colors.ctl_active))
                .child(line(0.7, colors.ctl_active))
                .child(
                    div()
                        .mt_auto()
                        .h(px(MINI_BUTTON_H))
                        .w(px(MINI_BUTTON_W))
                        .rounded(px(MINI_RADIUS))
                        .bg(colors.accent),
                ),
        )
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Picker(Log<usize>);

    impl Render for Picker {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let picks = self.0.clone();
            PictureCards::new(
                &theme,
                "theme",
                vec![
                    PictureCard::new("Light", appearance_picture(Some(Appearance::Light))),
                    PictureCard::new("Dark", appearance_picture(Some(Appearance::Dark))),
                ],
                Some(0),
            )
            .on_select(move |index, _, _| picks.borrow_mut().push(*index))
        }
    }

    #[gpui::test]
    fn every_card_is_a_tab_stop_that_space_selects(cx: &mut TestAppContext) {
        let picks = log();
        let shared = picks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Picker(shared));
        tab(cx);
        tab(cx);
        press(cx, "space");
        assert_eq!(*picks.borrow(), [1]);
    }
}
